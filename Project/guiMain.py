#!/usr/bin/env python3
"""
PKMS Graph Browser (Tkinter version, Python 3.13+ compatible)
- Black background, white outlines
- Click folder to expand/collapse; click file to show metadata
 - Pan: Left-drag empty space | Zoom: Ctrl + Wheel or +/-
- Drag nodes: Left-drag anywhere on a node (continuous, canvas-level)
- Edges clipped to box borders
- Scale-aware box collisions + neighbor "push" (domino effect)
- Force-directed auto-layout toggle with 'F'
- Alt+Space (or Ctrl+Space) toggles a search bar at the top. Type a directory path and press Enter to jump there.
- NEW: F11 toggles fullscreen. In the search bar, type "res 1920x1080" (or "r"/"resolution") to change window resolution.
"""

from __future__ import annotations
import json
import os
import stat
import math
import time
import re
import shutil
from pathlib import Path
import tkinter as tk
from tkinter import filedialog, colorchooser, messagebox, simpledialog

try:
    from PIL import Image, ImageTk
except ImportError:
    Image = None
    ImageTk = None

DEFAULT_BACKGROUND = "#000000"
DEFAULT_TEXT = "#FFFFFF"
DEFAULT_NODE = "#FFFFFF"
DEFAULT_RESOLUTION_INDEX = 2
DEFAULT_ROOT_PATH = Path.home().resolve()
DEFAULT_SHOW_HIDDEN = False
SETTINGS_FILE = Path(__file__).with_name("user_settings.json")

BG = DEFAULT_BACKGROUND
FG = DEFAULT_TEXT
NODE_W = 170      # screen-space
NODE_H = 48       # screen-space
RADIUS = 12
LINE_W = 2
MAX_CHILDREN = 120

# Spacing for children layout
BASE_RADIUS = 220
RADIUS_PER_CHILD = 7

# Collision settings (screen-space; converted to world-space on the fly)
COLLISION_MARGIN = 8
COLLISION_ITER = 20

# Force-directed layout parameters
FORCE_STEP_MS = 12
K_REPULSE = 15000.0   # repulsive constant
K_SPRING = 0.035      # attractive spring constant along edges
SPRING_REST = 260.0   # desired edge length
DAMPING = 0.78
MAX_STEP = 48.0       # clamp per-step movement in world units
DOUBLE_CLICK_MAX_INTERVAL = 0.25  # seconds; tighten double-click window for rename
IMAGE_EXTENSIONS = {".png", ".jpg", ".jpeg", ".bmp", ".gif", ".tif", ".tiff", ".webp"}
IMAGE_PREVIEW_MAX_WIDTH = 960
IMAGE_PREVIEW_MAX_HEIGHT = 720


def readable_size(num_bytes: int) -> str:
    step = 1024.0
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if num_bytes < step:
            return f"{num_bytes:.0f} {unit}" if unit == "B" else f"{num_bytes:.1f} {unit}"
        num_bytes /= step
    return f"{num_bytes:.1f} PB"


def clip_to_rect_edge(xc: float, yc: float, xt: float, yt: float, w: float, h: float) -> tuple[float, float]:
    dx = xt - xc
    dy = yt - yc
    if dx == 0 and dy == 0:
        return (xc, yc)
    hx = w / 2.0
    hy = h / 2.0
    sx = float('inf') if dx == 0 else abs(hx / dx)
    sy = float('inf') if dy == 0 else abs(hy / dy)
    s = min(sx, sy)
    return (xc + dx * s, yc + dy * s)


class Node:
    _next_tag_id = 0

    def __init__(self, app: "App", label: str, path: Path, is_dir: bool, x: float, y: float,
                 parent: "Node|None"=None, virtual: bool=False):
        self.app = app
        self.label = label
        self.path = path
        self.is_dir = is_dir
        self.x = x  # world-space
        self.y = y  # world-space
        self.parent = parent
        self.expanded = False
        self.children: list[Node] = []
        self.tag = f"node_{Node._next_tag_id}"
        Node._next_tag_id += 1
        self.virtual = virtual
        # velocity for force simulation
        self.vx = 0.0
        self.vy = 0.0

    @property
    def bbox(self):
        return (self.x - self.app.box_w_world/2, self.y - self.app.box_h_world/2,
                self.x + self.app.box_w_world/2, self.y + self.app.box_h_world/2)

    def expand(self):
        if self.expanded:
            return
        self.expanded = True
        labels = []
        try:
            if self.is_dir:
                entries = list(self.path.iterdir())
                if not self.app.show_hidden:
                    entries = [p for p in entries if not self.app._is_hidden_path(p)]
                entries.sort(key=lambda p: (not p.is_dir(), p.name.lower()))
                entries = entries[:MAX_CHILDREN]
                labels = [(p.name, p, p.is_dir(), False) for p in entries]
            else:
                if not self.app or not self.app._should_generate_file_metadata(self):
                    labels = []
                else:
                    p = self.path
                    st = p.stat()
                    typ = p.suffix.lower()[1:] if p.suffix else "file"
                    labels = [
                        (f"Type: {typ}", p, False, True),
                        (f"Size: {readable_size(st.st_size)}", p, False, True),
                        (f"Modified: {time.strftime('%Y-%m-%d %H:%M', time.localtime(st.st_mtime))}", p, False, True),
                    ]
        except Exception as e:
            labels = [(f"Error: {e}", self.path, False, True)]

        n = max(1, len(labels))
        radius = max(BASE_RADIUS, 90 + n * RADIUS_PER_CHILD)
        angle0 = -90.0
        self.children.clear()
        for i, (label, path, is_dir, virtual) in enumerate(labels):
            angle = angle0 + (360.0 * i / n)
            rad = math.radians(angle)
            cx = self.x + radius * math.cos(rad)
            cy = self.y + radius * math.sin(rad)
            child = Node(self.app, label, path, is_dir, cx, cy, parent=self, virtual=virtual)
            self.children.append(child)

    def collapse(self):
        if not self.expanded:
            return
        self.expanded = False
        for ch in self.children:
            ch.collapse()
        self.children.clear()


class App:
    def __init__(self, root_path: Path | None = None):
        self.root = tk.Tk()
        self.root.title("TinyPKM")
        self.background_color = BG
        self.text_color = FG
        self.node_color = DEFAULT_NODE
        self.root.configure(bg=self.background_color)

        # --- UI: top bar + canvas ---
        self.topbar = tk.Frame(self.root, bg=self.background_color)
        self.topbar.pack(side="top", fill="x")
        self.canvas = tk.Canvas(self.root, bg=self.background_color, highlightthickness=0)
        self.canvas.pack(side="top", fill="both", expand=True)

        # Search bar (hidden by default)
        self.search_var = tk.StringVar()
        self.search_frame = tk.Frame(self.topbar, bg=self.background_color)
        self.search_entry = tk.Entry(self.search_frame, textvariable=self.search_var, bg="#111111", fg="#FFFFFF",
                                     insertbackground="#FFFFFF", relief="flat", font=("Segoe UI", 11))
        self.settings_icon = tk.Label(self.search_frame, text="\u2699", bg=self.background_color, fg=self.text_color,
                                      font=("Segoe UI", 16), cursor="hand2")
        self.settings_icon.pack(side="left", padx=(8, 6), pady=6)
        self.settings_icon.bind("<Button-1>", lambda e: self.open_settings())
        self.search_entry.pack(side="left", fill="x", expand=True, padx=8, pady=6)
        self.search_entry.bind("<Return>", self._on_search_enter)
        self.search_entry.bind("<Escape>", lambda e: self.toggle_searchbar(hide_only=True))
        self.search_hint = tk.Label(self.search_frame, text="Type a path or 'res 1920x1080' and press Enter",
                                    bg=self.background_color, fg="#888888", font=("Segoe UI", 10))
        self.search_hint.pack(side="left", padx=8)
        self.search_shown = False  # start hidden
        self._search_open_via_hover = False
        self.settings_window: tk.Frame | None = None
        self._settings_buttons: dict[str, tk.Button] = {}
        self._settings_active: str | None = None
        self._settings_header: tk.Frame | None = None
        self._settings_title: tk.Label | None = None
        self._settings_close: tk.Button | None = None
        self._settings_nav: tk.Frame | None = None
        self.settings_content: tk.Frame | None = None
        self._settings_canvas: tk.Canvas | None = None
        self._settings_scrollbar: tk.Scrollbar | None = None
        self._settings_content_wrapper: tk.Frame | None = None
        self._settings_canvas_window = None
        self._settings_drag_offset: tuple[int, int] | None = None
        self._keybinds_info = [
            ("Alt + Space", "Toggle search bar"),
            ("Ctrl + Space", "Toggle search bar"),
            ("Enter (search)", "Execute search or command"),
            ("Escape (search)", "Hide search bar"),
            ("F11", "Toggle fullscreen"),
            ("R / r", "Reload directory"),
            ("Plus (+)", "Zoom in"),
            ("Minus (-) / Underscore (_)", "Zoom out"),
            ("Ctrl + Mouse Wheel", "Zoom on cursor"),
            ("Left Mouse Drag (empty canvas)", "Pan canvas"),
            ("Left Drag on node", "Drag node position"),
            ("Double-click node", "Expand or collapse node"),
        ]

        # View transform
        self.scale_factor = 1.0
        self.offset_x = 0.0
        self.offset_y = 0.0

        # Fullscreen state
        self.fullscreen = False
        self.display_mode = tk.StringVar(value="windowed")

        # Box size in world-space (derived)
        self._update_world_box_size()

        # Drag state
        self._panning = False
        self._drag_start = None           # for panning
        self._drag_node: Node | None = None
        self._last_drag_screen = (0, 0)   # incremental dragging reference
        self._node_dragging = False
        self._suppress_click = False
        self._pan_moved = False
        self._last_click_node: Node | None = None
        self._last_click_time = 0.0
        self._expand_animations: dict[Node, dict[str, object]] = {}
        self._image_panel_frame: tk.Frame | None = None
        self._image_panel_canvas: tk.Canvas | None = None
        self._image_panel_title: tk.Label | None = None
        self._image_panel_path: Path | None = None
        self._image_panel_photo: tk.PhotoImage | None = None
        self._image_panel_base_image = None
        self._image_panel_zoom = 1.0
        self._image_panel_image_id: int | None = None
        self._image_panel_offset = (0.0, 0.0)
        self._image_panel_drag_offset: tuple[int, int] | None = None
        self._image_panel_position: tuple[int, int] | None = None
        self._image_panel_size: tuple[int, int] | None = (480, 360)
        self._image_panel_pan_start: tuple[int, int] | None = None
        self._image_panel_resize_start: tuple[int, int, int, int] | None = None
        self._image_panel_resizing = False
        self._image_panel_resize_from_canvas = False
        self._image_panel_auto_size = False

        # Force simulation
        self.force_running = False
        self._resolution_presets = [
            ("1280 x 720", (1280, 720)),
            ("1600 x 900", (1600, 900)),
            ("1920 x 1080", (1920, 1080)),
            ("2560 x 1440", (2560, 1440)),
            ("3200 x 1800", (3200, 1800)),
            ("3840 x 2160", (3840, 2160)),
        ]
        self._resolution_index = tk.IntVar(value=DEFAULT_RESOLUTION_INDEX)  # default 1920x1080
        self._resolution_label_var = tk.StringVar(value=self._resolution_presets[self._resolution_index.get()][0])
        self._color_vars: dict[str, tuple[tk.IntVar, tk.IntVar, tk.IntVar]] = {}
        self._color_preview: dict[str, tk.Label] = {}
        self._color_trace_tokens: dict[tuple[str, int], str] = {}
        for key, hex_color in (("background", self.background_color),
                               ("text", self.text_color),
                               ("node", self.node_color)):
            r, g, b = self._hex_to_rgb(hex_color)
            self._color_vars[key] = (
                tk.IntVar(value=r),
                tk.IntVar(value=g),
                tk.IntVar(value=b),
            )

        self._settings_file = SETTINGS_FILE
        self.default_root_path = DEFAULT_ROOT_PATH
        self.show_hidden = DEFAULT_SHOW_HIDDEN
        self._default_root_var = tk.StringVar(value=str(self.default_root_path))
        self._show_hidden_var = tk.BooleanVar(value=self.show_hidden)
        self._size_cache: dict[tuple[Path, bool], tuple[int, int]] = {}
        self._pending_display_mode = "windowed"
        self._pending_resolution_index = self._resolution_index.get()
        self._collision_job: str | None = None
        self._load_user_settings()
        self._apply_window_preferences()

        self.root.bind("<Configure>", self._on_resize)
        # Pan with left mouse (background drag)
        self.canvas.bind("<ButtonPress-1>", self._pan_start)
        # Right-click for context menu actions
        self.canvas.bind("<ButtonPress-3>", self._context_click)
        # Wheel zoom (Ctrl + Wheel)
        self.canvas.bind("<MouseWheel>", self._on_wheel)  # Windows
        self.canvas.bind("<Button-4>", self._on_wheel)    # Linux scroll up
        self.canvas.bind("<Button-5>", self._on_wheel)    # Linux scroll down
        self.root.bind("<KeyPress-plus>", lambda e: self.zoom(1.15, self._center()))
        self.root.bind("<KeyPress-minus>", lambda e: self.zoom(1/1.15, self._center()))
        self.root.bind("<KeyPress-underscore>", lambda e: self.zoom(1/1.15, self._center()))
        # Removed 'O' shortcut for choose_dir as requested
        self.root.bind("<KeyPress-R>", lambda e: self.reload())
        self.root.bind("<KeyPress-r>", lambda e: self.reload())
        # F11 fullscreen toggle
        self.root.bind("<F11>", lambda e: self.toggle_fullscreen())

        # Alt+Space toggle searchbar (Windows may reserve Alt+Space; Ctrl+Space as fallback)
        self.root.bind_all("<Alt-Key-space>", lambda e: self.toggle_searchbar())
        self.root.bind_all("<Control-Key-space>", lambda e: self.toggle_searchbar())

        # Show search bar when hovering near the top edge
        self.root.bind_all("<Motion>", self._on_global_mouse_move)

        self.root_node: Node | None = None
        if root_path is None:
            root_path = self.default_root_path
        else:
            root_path = Path(root_path)
            if root_path.exists():
                self.default_root_path = root_path
                self._default_root_var.set(str(self.default_root_path))
        self.load_root(root_path)
        self._start_force_simulation()

    # ---------- Settings ----------
    def _load_user_settings(self):
        try:
            with open(self._settings_file, "r", encoding="utf-8") as fh:
                data = json.load(fh)
        except FileNotFoundError:
            data = {}
        except (json.JSONDecodeError, OSError) as exc:
            print(f"Warning: could not load settings ({exc})")
            data = {}

        path_str = data.get("default_root_path")
        if path_str:
            candidate = Path(path_str).expanduser()
            if candidate.exists():
                try:
                    candidate = candidate.resolve()
                except OSError:
                    pass
                self.default_root_path = candidate
        self._default_root_var.set(str(self.default_root_path))
        show_hidden = data.get("show_hidden")
        if isinstance(show_hidden, bool):
            self.show_hidden = show_hidden
        self._show_hidden_var.set(self.show_hidden)
        res_idx = data.get("resolution_index")
        if isinstance(res_idx, int):
            res_idx = max(0, min(len(self._resolution_presets) - 1, res_idx))
            self._pending_resolution_index = res_idx
        else:
            self._pending_resolution_index = self._resolution_index.get()
        mode = data.get("display_mode")
        if mode in ("windowed", "fullscreen"):
            self._pending_display_mode = mode
        else:
            self._pending_display_mode = "windowed"
        self._resolution_index.set(self._pending_resolution_index)
        self._resolution_label_var.set(self._resolution_presets[self._pending_resolution_index][0])
        self.display_mode.set(self._pending_display_mode)

    def _save_user_settings(self):
        payload = {
            "default_root_path": str(self.default_root_path),
            "show_hidden": bool(self.show_hidden),
            "resolution_index": int(self._resolution_index.get()),
            "display_mode": self.display_mode.get(),
        }
        try:
            with open(self._settings_file, "w", encoding="utf-8") as fh:
                json.dump(payload, fh, indent=2)
        except OSError as exc:
            print(f"Warning: could not save settings ({exc})")
        self._pending_resolution_index = self._resolution_index.get()
        self._pending_display_mode = self.display_mode.get()

    def _apply_window_preferences(self):
        idx = max(0, min(len(self._resolution_presets) - 1, int(self._pending_resolution_index)))
        self._resolution_index.set(idx)
        self._resolution_label_var.set(self._resolution_presets[idx][0])
        desired_mode = self._pending_display_mode
        self.display_mode.set(desired_mode)
        width, height = self._resolution_presets[idx][1]

        if desired_mode == "fullscreen":
            if not self.fullscreen:
                self.toggle_fullscreen()
        else:
            if self.fullscreen:
                self.toggle_fullscreen()
            self.root.geometry(f"{width}x{height}")
            self.root.update_idletasks()

    def _is_hidden_path(self, path: Path) -> bool:
        name = path.name
        if name.startswith(".") and name not in (".", ".."):
            return True
        if os.name == "nt":
            try:
                attrs = os.stat(path, follow_symlinks=False).st_file_attributes
                hidden_flag = getattr(stat, "FILE_ATTRIBUTE_HIDDEN", 0x02)
                return bool(attrs & hidden_flag)
            except (OSError, AttributeError):
                return False
        return False

    def _should_generate_file_metadata(self, node: Node) -> bool:
        return not self._is_image_file(node)

    def open_settings(self):
        self.display_mode.set("fullscreen" if self.fullscreen else "windowed")
        if self.settings_window and self.settings_window.winfo_exists():
            self.settings_window.lift()
            return
        self._sync_resolution_index()

        self.root.update_idletasks()
        width, height = 420, 320
        root_w = max(width, self.root.winfo_width())
        root_h = max(height, self.root.winfo_height())
        x = max(20, (root_w - width) // 2)
        y = max(20, (root_h - height) // 2)

        win = tk.Frame(self.root, bg=self.background_color, bd=1, relief="ridge",
                       highlightthickness=1, highlightbackground="#333333")
        win.place(x=x, y=y, width=width, height=height)
        win.pack_propagate(False)
        win.grid_propagate(False)
        self.settings_window = win
        win.lift()

        header = tk.Frame(win, bg=self.background_color, cursor="fleur")
        header.pack(side="top", fill="x")
        title = tk.Label(header, text="Settings", bg=self.background_color, fg=self.text_color, font=("Segoe UI", 14, "bold"))
        title.pack(side="left", padx=12, pady=10)
        close_btn = tk.Button(header, text="X", command=self._close_settings, bg="#111111", fg=self.text_color,
                              relief="flat", font=("Segoe UI", 10, "bold"), highlightthickness=0,
                              activebackground="#222222", activeforeground=self.text_color, padx=8, pady=2)
        close_btn.pack(side="right", padx=12, pady=10)
        self._settings_header = header
        self._settings_title = title
        self._settings_close = close_btn
        header.bind("<ButtonPress-1>", self._start_settings_drag)
        header.bind("<B1-Motion>", self._drag_settings_window)
        header.bind("<ButtonRelease-1>", self._stop_settings_drag)
        title.bind("<ButtonPress-1>", self._start_settings_drag)
        title.bind("<B1-Motion>", self._drag_settings_window)
        title.bind("<ButtonRelease-1>", self._stop_settings_drag)

        nav = tk.Frame(win, bg=self.background_color)
        nav.pack(side="top", fill="x", pady=(0, 6))
        self._settings_nav = nav
        self._settings_buttons = {}
        for key, label in (("keybinds", "Keybinds"), ("keywords", "Keywords"), ("config", "Config")):
            btn = tk.Button(nav, text=label, command=lambda k=key: self._show_settings_page(k),
                            bg="#111111", fg=self.text_color, relief="flat", font=("Segoe UI", 10),
                            activebackground="#222222", activeforeground=self.text_color, padx=10, pady=6)
            btn.pack(side="left", padx=6, pady=6)
            self._settings_buttons[key] = btn

        content_wrapper = tk.Frame(win, bg=self.background_color)
        content_wrapper.pack(side="top", fill="both", expand=True)
        canvas = tk.Canvas(content_wrapper, bg=self.background_color, highlightthickness=0)
        scrollbar = tk.Scrollbar(content_wrapper, orient="vertical", command=canvas.yview)
        canvas.configure(yscrollcommand=scrollbar.set)
        canvas.pack(side="left", fill="both", expand=True)
        scrollbar.pack(side="right", fill="y")

        self.settings_content = tk.Frame(canvas, bg=self.background_color, padx=16, pady=12)
        content_window = canvas.create_window((0, 0), window=self.settings_content, anchor="nw")

        def _on_frame_configure(_event):
            canvas.configure(scrollregion=canvas.bbox("all"))

        def _on_canvas_configure(event):
            canvas.itemconfigure(content_window, width=event.width)

        self.settings_content.bind("<Configure>", _on_frame_configure)
        self.settings_content.bind("<MouseWheel>", self._on_settings_mousewheel)
        self.settings_content.bind("<Button-4>", lambda e: self._on_settings_mousewheel(e, delta=120))
        self.settings_content.bind("<Button-5>", lambda e: self._on_settings_mousewheel(e, delta=-120))
        canvas.bind("<Configure>", _on_canvas_configure)
        canvas.bind("<MouseWheel>", self._on_settings_mousewheel)
        canvas.bind("<Button-4>", lambda e: self._on_settings_mousewheel(e, delta=120))
        canvas.bind("<Button-5>", lambda e: self._on_settings_mousewheel(e, delta=-120))

        self._settings_canvas = canvas
        self._settings_scrollbar = scrollbar
        self._settings_content_wrapper = content_wrapper
        self._settings_canvas_window = content_window
        self._color_preview = {}

        self._show_settings_page("keybinds")

    def _close_settings(self):
        if self.settings_window and self.settings_window.winfo_exists():
            self.settings_window.destroy()
        self.settings_window = None
        self._settings_buttons = {}
        self._settings_active = None
        self._settings_header = None
        self._settings_title = None
        self._settings_close = None
        self._settings_nav = None
        self.settings_content = None
        self._color_preview = {}
        self._settings_canvas = None
        self._settings_scrollbar = None
        self._settings_content_wrapper = None
        self._settings_canvas_window = None
        self._settings_drag_offset = None

    def _start_settings_drag(self, event):
        if not self.settings_window or not self.settings_window.winfo_exists():
            return
        frame = self.settings_window
        frame.lift()
        self.root.update_idletasks()
        self._settings_drag_offset = (
            event.x_root - frame.winfo_rootx(),
            event.y_root - frame.winfo_rooty(),
        )

    def _drag_settings_window(self, event):
        if not self.settings_window or not self.settings_window.winfo_exists():
            return
        if self._settings_drag_offset is None:
            return
        frame = self.settings_window
        offset_x, offset_y = self._settings_drag_offset
        self.root.update_idletasks()
        root_x = self.root.winfo_rootx()
        root_y = self.root.winfo_rooty()
        root_w = max(1, self.root.winfo_width())
        root_h = max(1, self.root.winfo_height())
        new_screen_x = event.x_root - offset_x
        new_screen_y = event.y_root - offset_y
        max_x = root_x + root_w - frame.winfo_width()
        max_y = root_y + root_h - frame.winfo_height()
        new_screen_x = max(root_x, min(new_screen_x, max_x))
        new_screen_y = max(root_y, min(new_screen_y, max_y))
        frame.place_configure(x=new_screen_x - root_x, y=new_screen_y - root_y)

    def _stop_settings_drag(self, _event):
        self._settings_drag_offset = None

    def _show_settings_page(self, page: str):
        if not self.settings_window or not self.settings_window.winfo_exists():
            return

        if self._settings_active == page:
            return

        for child in self.settings_content.winfo_children():
            child.destroy()

        for key, btn in self._settings_buttons.items():
            btn.configure(bg="#111111")

        if page == "keybinds":
            self._render_keybinds_page()
        elif page == "keywords":
            self._render_placeholder_page("Keyword shortcuts and search tags coming soon.")
        elif page == "config":
            self._render_config_page()

        if page in self._settings_buttons:
            self._settings_buttons[page].configure(bg="#222222")

        self._settings_active = page

    def _render_keybinds_page(self):
        heading = tk.Label(self.settings_content, text="Keybinds", bg=self.background_color, fg=self.text_color,
                           font=("Segoe UI", 12, "bold"))
        heading.pack(anchor="w", pady=(0, 8))

        for combo, desc in self._keybinds_info:
            row = tk.Frame(self.settings_content, bg=self.background_color)
            row.pack(anchor="w", fill="x", pady=2)
            combo_lbl = tk.Label(row, text=combo, bg=self.background_color, fg=self.text_color, font=("Segoe UI", 10, "bold"))
            combo_lbl.pack(side="left")
            desc_lbl = tk.Label(row, text=desc, bg=self.background_color, fg="#BBBBBB", font=("Segoe UI", 10))
            desc_lbl.pack(side="left", padx=12)

    def _render_config_page(self):
        path_frame = tk.Frame(self.settings_content, bg=self.background_color)
        path_frame.pack(anchor="w", fill="x", pady=(0, 12))
        path_label = tk.Label(path_frame, text="Default Root Path", bg=self.background_color,
                              fg=self.text_color, font=("Segoe UI", 11, "bold"))
        path_label.pack(anchor="w")
        path_entry_row = tk.Frame(path_frame, bg=self.background_color)
        path_entry_row.pack(anchor="w", fill="x", pady=(6, 0))
        path_entry = tk.Entry(path_entry_row, textvariable=self._default_root_var,
                              bg="#111111", fg=self.text_color, insertbackground=self.text_color,
                              relief="flat", font=("Segoe UI", 10))
        path_entry.pack(side="left", fill="x", expand=True)
        browse_btn = tk.Button(path_entry_row, text="Browse...", command=self._browse_default_root,
                               bg="#111111", fg=self.text_color, relief="flat",
                               font=("Segoe UI", 10), activebackground="#222222",
                               activeforeground=self.text_color, padx=10, pady=4)
        browse_btn.pack(side="left", padx=(8, 0))

        hidden_frame = tk.Frame(self.settings_content, bg=self.background_color)
        hidden_frame.pack(anchor="w", fill="x", pady=(0, 12))
        hidden_label = tk.Label(hidden_frame, text="Hidden Files", bg=self.background_color,
                                fg=self.text_color, font=("Segoe UI", 11, "bold"))
        hidden_label.pack(anchor="w")
        hidden_toggle = tk.Checkbutton(hidden_frame, text="Show dot-prefixed files (e.g. .git, .ini)",
                                       variable=self._show_hidden_var, onvalue=True, offvalue=False,
                                       bg=self.background_color, fg=self.text_color,
                                       activebackground="#222222", activeforeground=self.text_color,
                                       selectcolor="#222222", font=("Segoe UI", 10))
        hidden_toggle.pack(anchor="w", pady=(6, 0))

        mode_frame = tk.Frame(self.settings_content, bg=self.background_color)
        mode_frame.pack(anchor="w", fill="x", pady=(0, 12))
        mode_label = tk.Label(mode_frame, text="Display Mode", bg=self.background_color, fg=self.text_color,
                              font=("Segoe UI", 11, "bold"))
        mode_label.pack(anchor="w")
        buttons = tk.Frame(mode_frame, bg=self.background_color)
        buttons.pack(anchor="w", pady=(6, 0))
        for label, value in (("Windowed", "windowed"), ("Fullscreen", "fullscreen")):
            rb = tk.Radiobutton(buttons, text=label, variable=self.display_mode, value=value,
                                bg=self.background_color, fg=self.text_color, selectcolor="#222222",
                                font=("Segoe UI", 10), activebackground="#222222", activeforeground=self.text_color,
                                highlightthickness=0)
            rb.pack(side="left", padx=(0, 12))

        res_frame = tk.Frame(self.settings_content, bg=self.background_color)
        res_frame.pack(anchor="w", fill="x", pady=(0, 12))
        res_label = tk.Label(res_frame, text="Resolution", bg=self.background_color, fg=self.text_color,
                             font=("Segoe UI", 11, "bold"))
        res_label.pack(anchor="w")
        slider = tk.Scale(res_frame, from_=0, to=len(self._resolution_presets)-1,
                          orient="horizontal", showvalue=False, variable=self._resolution_index,
                          command=self._on_resolution_change, bg=self.background_color, fg=self.text_color,
                          highlightthickness=0, troughcolor="#222222", length=240)
        slider.pack(anchor="w", pady=(6, 0))
        current_label = tk.Label(res_frame, textvariable=self._resolution_label_var,
                                 bg=self.background_color, fg="#BBBBBB", font=("Segoe UI", 10))
        current_label.pack(anchor="w", pady=(4, 0))

        colors_frame = tk.Frame(self.settings_content, bg=self.background_color)
        colors_frame.pack(anchor="w", fill="x", pady=(8, 12))
        colors_label = tk.Label(colors_frame, text="Colors (RGB 0-255)", bg=self.background_color, fg=self.text_color,
                                font=("Segoe UI", 11, "bold"))
        colors_label.pack(anchor="w")

        for key, title in (("node", "File Nodes"), ("text", "Text"), ("background", "Background")):
            row = tk.Frame(colors_frame, bg=self.background_color)
            row.pack(anchor="w", fill="x", pady=6)
            name = tk.Label(row, text=title, bg=self.background_color, fg=self.text_color, font=("Segoe UI", 10, "bold"))
            name.pack(side="left", padx=(0, 8))
            preview = tk.Label(row, width=3, height=1, relief="ridge", bd=2)
            preview.pack(side="left", padx=(0, 10))
            self._color_preview[key] = preview
            r_var, g_var, b_var = self._color_vars[key]
            for var, channel in zip((r_var, g_var, b_var), ("R", "G", "B")):
                spin = tk.Spinbox(row, from_=0, to=255, width=4, textvariable=var,
                                  validate="key",
                                  validatecommand=(self.root.register(self._validate_rgb_entry), "%P"),
                                  command=lambda k=key: self._update_color_preview(k))
                spin.pack(side="left", padx=(0, 4))
                spin.configure(font=("Segoe UI", 10))
            for idx, var in enumerate((r_var, g_var, b_var)):
                token = self._color_trace_tokens.get((key, idx))
                if token:
                    try:
                        var.trace_remove("write", token)
                    except tk.TclError:
                        pass
                new_token = var.trace_add("write", lambda *_args, k=key: self._update_color_preview(k))
                self._color_trace_tokens[(key, idx)] = new_token
            pick = tk.Button(row, text="Pick", command=lambda k=key: self._choose_color(k),
                             bg="#111111", fg=self.text_color, relief="flat", font=("Segoe UI", 10),
                             activebackground="#222222", activeforeground=self.text_color, padx=6, pady=2)
            pick.pack(side="left", padx=(4, 0))
            self._update_color_preview(key)

        actions = tk.Frame(self.settings_content, bg=self.background_color)
        actions.pack(anchor="e", pady=(6, 0))
        reset_btn = tk.Button(actions, text="Reset to Defaults", command=self.reset_config_defaults,
                              bg="#111111", fg=self.text_color, relief="flat",
                              font=("Segoe UI", 10, "bold"), activebackground="#222222",
                              activeforeground=self.text_color, padx=10, pady=4)
        reset_btn.pack(side="left", padx=(0, 8))
        apply_btn = tk.Button(actions, text="Apply", command=self.apply_config,
                              bg="#1f1f1f", fg=self.text_color, relief="flat",
                              font=("Segoe UI", 11, "bold"), activebackground="#333333",
                              activeforeground=self.text_color, padx=12, pady=6)
        apply_btn.pack(side="left")

    def _browse_default_root(self):
        current = self._default_root_var.get()
        init_dir = current if current and Path(current).expanduser().exists() else str(Path.home())
        selected = filedialog.askdirectory(initialdir=init_dir, title="Choose default root")
        if selected:
            self._default_root_var.set(selected)

    def _render_placeholder_page(self, message: str):
        note = tk.Label(self.settings_content, text=message, bg=self.background_color, fg="#BBBBBB",
                        font=("Segoe UI", 10), justify="left", wraplength=360)
        note.pack(anchor="w")

    def _on_settings_mousewheel(self, event, delta: int | None = None):
        if not self._settings_canvas:
            return
        raw_delta = delta if delta is not None else getattr(event, "delta", 0)
        if raw_delta == 0:
            return
        magnitude = int(max(1, abs(raw_delta) / 120))
        direction = -1 if raw_delta > 0 else 1
        self._settings_canvas.yview_scroll(direction * magnitude, "units")

    def _validate_rgb_entry(self, value: str) -> bool:
        if value == "":
            return True
        if not value.isdigit():
            return False
        return 0 <= int(value) <= 255

    def _on_resolution_change(self, value: str):
        try:
            idx = int(float(value))
        except (TypeError, ValueError):
            return
        idx = max(0, min(len(self._resolution_presets) - 1, idx))
        self._resolution_index.set(idx)
        self._resolution_label_var.set(self._resolution_presets[idx][0])

    def _choose_color(self, key: str):
        r_var, g_var, b_var = self._color_vars[key]
        initial = self._rgb_to_hex(r_var.get(), g_var.get(), b_var.get())
        color = colorchooser.askcolor(color=initial, parent=self.settings_window)
        if color and color[0]:
            r, g, b = (int(round(c)) for c in color[0])
            r_var.set(max(0, min(255, r)))
            g_var.set(max(0, min(255, g)))
            b_var.set(max(0, min(255, b)))
            self._update_color_preview(key)

    def _update_color_preview(self, key: str):
        if key not in self._color_preview:
            return
        r_var, g_var, b_var = self._color_vars[key]
        try:
            r = max(0, min(255, int(r_var.get())))
            g = max(0, min(255, int(g_var.get())))
            b = max(0, min(255, int(b_var.get())))
        except (TypeError, ValueError):
            return
        hex_color = self._rgb_to_hex(r, g, b)
        preview = self._color_preview[key]
        if key == "background":
            preview.configure(bg=hex_color, fg=self.text_color)
        else:
            preview.configure(bg=hex_color, fg=self.background_color)

    def apply_config(self):
        raw_path = self._default_root_var.get().strip()
        if not raw_path:
            messagebox.showerror("Invalid Path", "Please enter a directory for the default root.")
            self._default_root_var.set(str(self.default_root_path))
            return
        candidate = Path(raw_path).expanduser()
        if not candidate.exists():
            messagebox.showerror("Invalid Path", f"The directory '{raw_path}' does not exist.")
            self._default_root_var.set(str(self.default_root_path))
            return
        try:
            candidate_resolved = candidate.resolve()
        except OSError:
            candidate_resolved = candidate
        try:
            current_root_resolved = self.default_root_path.resolve()
        except OSError:
            current_root_resolved = self.default_root_path
        path_changed = candidate_resolved != current_root_resolved
        show_hidden_selected = bool(self._show_hidden_var.get())
        hidden_changed = show_hidden_selected != self.show_hidden
        self.default_root_path = candidate_resolved
        self._default_root_var.set(str(self.default_root_path))
        self.show_hidden = show_hidden_selected

        mode = self.display_mode.get()
        target_fullscreen = (mode == "fullscreen")
        if target_fullscreen != self.fullscreen:
            self.toggle_fullscreen()

        prev_idx = max(0, min(len(self._resolution_presets) - 1, int(self._pending_resolution_index)))
        idx = max(0, min(len(self._resolution_presets) - 1, self._resolution_index.get()))
        res_changed = idx != prev_idx
        width, height = self._resolution_presets[idx][1]
        if not self.fullscreen:
            self.root.geometry(f"{width}x{height}")
            if res_changed:
                self.root.update_idletasks()
                self.canvas.update_idletasks()
                self.offset_x = self.canvas.winfo_width()/2
                self.offset_y = self.canvas.winfo_height()/2

        node_hex = self._get_color_from_vars("node")
        text_hex = self._get_color_from_vars("text")
        bg_hex = self._get_color_from_vars("background")

        self.node_color = node_hex
        self.text_color = text_hex
        self.background_color = bg_hex
        self._apply_theme()
        self._save_user_settings()
        if path_changed or hidden_changed:
            self.load_root(self.default_root_path)
        elif res_changed and not self.fullscreen:
            self.redraw_all()

    def reset_config_defaults(self):
        self.display_mode.set("windowed")
        self._resolution_index.set(DEFAULT_RESOLUTION_INDEX)
        self._resolution_label_var.set(self._resolution_presets[DEFAULT_RESOLUTION_INDEX][0])

        defaults = {
            "background": DEFAULT_BACKGROUND,
            "text": DEFAULT_TEXT,
            "node": DEFAULT_NODE,
        }
        for key, hex_color in defaults.items():
            r, g, b = self._hex_to_rgb(hex_color)
            r_var, g_var, b_var = self._color_vars[key]
            r_var.set(r)
            g_var.set(g)
            b_var.set(b)

        self._default_root_var.set(str(DEFAULT_ROOT_PATH))
        self._show_hidden_var.set(DEFAULT_SHOW_HIDDEN)

        self.apply_config()

    def _sync_resolution_index(self):
        try:
            current_w = int(self.root.winfo_width())
            current_h = int(self.root.winfo_height())
        except Exception:
            current_w, current_h = 0, 0
        if current_w <= 1 or current_h <= 1:
            return
        best_idx = self._resolution_index.get()
        best_score = float("inf")
        for idx, (_label, (w, h)) in enumerate(self._resolution_presets):
            score = abs(w - current_w) + abs(h - current_h)
            if score < best_score:
                best_idx = idx
                best_score = score
        self._resolution_index.set(best_idx)
        self._resolution_label_var.set(self._resolution_presets[best_idx][0])

    def _get_color_from_vars(self, key: str) -> str:
        r_var, g_var, b_var = self._color_vars[key]
        try:
            r = max(0, min(255, int(r_var.get())))
            g = max(0, min(255, int(g_var.get())))
            b = max(0, min(255, int(b_var.get())))
        except (TypeError, ValueError):
            r, g, b = 0, 0, 0
        return self._rgb_to_hex(r, g, b)

    def _apply_theme(self):
        global BG, FG
        BG = self.background_color
        FG = self.text_color
        self.root.configure(bg=self.background_color)
        self.topbar.configure(bg=self.background_color)
        self.canvas.configure(bg=self.background_color)
        self.search_frame.configure(bg=self.background_color)
        self.search_hint.configure(bg=self.background_color)
        self.settings_icon.configure(bg=self.background_color, fg=self.text_color)
        if self._settings_content_wrapper:
            self._settings_content_wrapper.configure(bg=self.background_color)
        if self._settings_canvas:
            self._settings_canvas.configure(bg=self.background_color, highlightthickness=0)
        if self._settings_scrollbar:
            try:
                self._settings_scrollbar.configure(bg=self.background_color, activebackground="#333333",
                                                   troughcolor="#222222", highlightthickness=0)
            except tk.TclError:
                pass
        if self.settings_window and self.settings_window.winfo_exists():
            self.settings_window.configure(bg=self.background_color)
            if hasattr(self, "_settings_header"):
                self._settings_header.configure(bg=self.background_color)
            if hasattr(self, "_settings_title"):
                self._settings_title.configure(bg=self.background_color, fg=self.text_color)
            if hasattr(self, "_settings_close"):
                self._settings_close.configure(bg="#111111", fg=self.text_color,
                                               activeforeground=self.text_color)
            if hasattr(self, "_settings_nav"):
                self._settings_nav.configure(bg=self.background_color)
            for btn in self._settings_buttons.values():
                btn.configure(fg=self.text_color,
                              activeforeground=self.text_color)
            self.settings_content.configure(bg=self.background_color)
            for child in self.settings_content.winfo_children():
                self._update_child_theme(child)
            for key in self._color_preview:
                self._update_color_preview(key)
        self.redraw_all()

    def _update_child_theme(self, widget):
        if isinstance(widget, tk.Frame):
            widget.configure(bg=self.background_color)
        if isinstance(widget, tk.Label):
            if widget in self._color_preview.values():
                return
            current_fg = widget.cget("fg")
            if current_fg in (FG, self.text_color, "#FFFFFF"):
                widget.configure(fg=self.text_color)
            widget.configure(bg=self.background_color)
        if isinstance(widget, tk.Button):
            widget.configure(fg=self.text_color, activeforeground=self.text_color)
        if isinstance(widget, tk.Radiobutton):
            widget.configure(bg=self.background_color, fg=self.text_color,
                             selectcolor="#222222", activebackground="#222222",
                             activeforeground=self.text_color)
        if isinstance(widget, tk.Checkbutton):
            widget.configure(bg=self.background_color, fg=self.text_color,
                             selectcolor="#222222", activebackground="#222222",
                             activeforeground=self.text_color)
        if isinstance(widget, tk.Scale):
            widget.configure(bg=self.background_color, fg=self.text_color,
                             troughcolor="#222222", highlightthickness=0)
        if isinstance(widget, tk.Spinbox):
            widget.configure(bg="#111111", fg=self.text_color, insertbackground=self.text_color)
        if isinstance(widget, tk.Entry):
            widget.configure(bg="#111111", fg=self.text_color, insertbackground=self.text_color,
                             relief="flat")
        for child in widget.winfo_children():
            self._update_child_theme(child)

    def _rgb_to_hex(self, r: int, g: int, b: int) -> str:
        return f"#{max(0, min(255, r)):02x}{max(0, min(255, g)):02x}{max(0, min(255, b)):02x}"

    def _hex_to_rgb(self, value: str) -> tuple[int, int, int]:
        value = value.lstrip("#")
        if len(value) != 6:
            return (0, 0, 0)
        r = int(value[0:2], 16)
        g = int(value[2:4], 16)
        b = int(value[4:6], 16)
        return r, g, b

    # ---------- Search bar ----------
    def toggle_searchbar(self, hide_only: bool = False, from_hover: bool = False):
        if hide_only or self.search_shown:
            self.search_frame.pack_forget()
            self.search_shown = False
            self.canvas.focus_set()
            self._search_open_via_hover = False
        else:
            self.search_var.set("")
            self.search_frame.pack(side="top", fill="x")
            self.search_shown = True
            self.search_entry.focus_set()
            self.search_entry.select_range(0, 'end')
            self._search_open_via_hover = from_hover

    def _parse_resolution(self, text: str):
        # Accept "1920x1080", "1920 1080", "1920,1080"
        m = re.search(r'(\d+)\s*[xX,\s]\s*(\d+)', text)
        if not m:
            return None
        w = int(m.group(1))
        h = int(m.group(2))
        if w <= 0 or h <= 0:
            return None
        return w, h

    def _on_search_enter(self, _evt):
        text = self.search_var.get().strip()
        if not text:
            self.toggle_searchbar(hide_only=True)
            return

        # Command mode: res/r/resolution <WxH>
        parts = text.split(None, 1)
        cmd = parts[0].lower()
        if cmd in ("res", "r", "resolution"):
            if len(parts) == 1:
                self._flash_error()
                return
            wh = self._parse_resolution(parts[1])
            if not wh:
                self._flash_error()
                return
            w, h = wh
            self.root.geometry(f"{w}x{h}")
            self.toggle_searchbar(hide_only=True)
            return

        # Path mode
        path_str = os.path.expandvars(os.path.expanduser(text))
        p = Path(path_str)
        if p.exists() and p.is_dir():
            self.load_root(p)
            self.toggle_searchbar(hide_only=True)
        else:
            self._flash_error()

    def _flash_error(self):
        orig = self.search_entry['bg']
        self.search_entry.configure(bg="#441111")
        self.root.after(350, lambda: self.search_entry.configure(bg=orig))

    # ---------- Fullscreen ----------
    def toggle_fullscreen(self):
        self.fullscreen = not self.fullscreen
        self.display_mode.set("fullscreen" if self.fullscreen else "windowed")
        try:
            self.root.attributes("-fullscreen", self.fullscreen)
        except Exception:
            # Fallback geometry trick
            if self.fullscreen:
                self._saved_geom = self.root.geometry()
                self.root.state('zoomed')
            else:
                self.root.state('normal')
                if hasattr(self, "_saved_geom"):
                    self.root.geometry(self._saved_geom)

    # ---------- Transform helpers ----------
    def world_to_screen(self, x, y):
        return (x * self.scale_factor + self.offset_x, y * self.scale_factor + self.offset_y)

    def screen_to_world(self, x, y):
        return ((x - self.offset_x) / self.scale_factor, (y - self.offset_y) / self.scale_factor)

    def _center(self):
        w = self.canvas.winfo_width()
        h = self.canvas.winfo_height()
        return (w/2, h/2)

    def _update_world_box_size(self):
        self.box_w_world = NODE_W / max(self.scale_factor, 1e-6)
        self.box_h_world = NODE_H / max(self.scale_factor, 1e-6)
        self.margin_world = COLLISION_MARGIN / max(self.scale_factor, 1e-6)

    # ---------- Pan & Zoom ----------
    def zoom(self, factor: float, anchor_screen_xy: tuple[float, float]):
        ax, ay = anchor_screen_xy
        axw, ayw = self.screen_to_world(ax, ay)
        self.scale_factor *= factor
        self.offset_x = ax - axw * self.scale_factor
        self.offset_y = ay - ayw * self.scale_factor
        self._update_world_box_size()
        self.redraw_all()

    def _on_wheel(self, evt):
        if hasattr(evt, "delta"):
            delta = evt.delta
            if delta == 0:
                return
        else:
            delta = 120 if evt.num == 4 else -120
        factor = 1.15 if delta > 0 else 1/1.15
        self.zoom(factor, (evt.x, evt.y))

    def _pan_start(self, evt):
        if self._drag_node is not None:
            return "break"
        if self._node_at_canvas(evt.x, evt.y):
            return
        self._panning = True
        self._drag_start = (evt.x, evt.y)
        self._pan_moved = False
        self.canvas.bind("<B1-Motion>", self._pan_move)
        self.canvas.bind("<ButtonRelease-1>", self._pan_end)

    def _pan_move(self, evt):
        if not self._panning or not self._drag_start:
            return
        x0, y0 = self._drag_start
        dx = evt.x - x0
        dy = evt.y - y0
        if not self._pan_moved:
            if abs(dx) < 3 and abs(dy) < 3:
                return
            self._pan_moved = True
        self._drag_start = (evt.x, evt.y)
        self.offset_x += dx
        self.offset_y += dy
        self.redraw_all()

    def _pan_end(self, _evt):
        if not self._panning:
            return
        self._panning = False
        self._drag_start = None
        self._pan_moved = False
        self.canvas.unbind("<B1-Motion>")
        self.canvas.unbind("<ButtonRelease-1>")

    def _context_click(self, evt):
        target = self._node_at_canvas(evt.x, evt.y)
        self._show_context_menu(evt.x_root, evt.y_root, target)

    # ---------- Node utilities ----------
    def _all_nodes(self) -> list[Node]:
        out = []
        def walk(n: Node):
            out.append(n)
            for ch in n.children:
                walk(ch)
        if self.root_node:
            walk(self.root_node)
        return out

    def _edges(self):
        pairs = []
        def walk(n: Node):
            for ch in n.children:
                pairs.append((n, ch))
                walk(ch)
        if self.root_node:
            walk(self.root_node)
        return pairs

    def _find_node_by_tag(self, tag: str) -> Node | None:
        if not self.root_node:
            return None
        stack = [self.root_node]
        while stack:
            node = stack.pop()
            if node.tag == tag:
                return node
            stack.extend(node.children)
        return None

    def _node_at_canvas(self, sx: float, sy: float) -> Node | None:
        hits = self.canvas.find_overlapping(sx, sy, sx, sy)
        for item in hits:
            tags = self.canvas.gettags(item)
            for tag in tags:
                if tag.startswith("node_"):
                    node = self._find_node_by_tag(tag)
                    if node:
                        return node
        return None

    # ---------- Image helpers ----------
    def _is_image_file(self, node: Node) -> bool:
        if node.virtual or node.is_dir:
            return False
        suffix = node.path.suffix.lower()
        return suffix in IMAGE_EXTENSIONS

    def _load_image_for_preview(self, path: Path) -> tuple[object | None, tk.PhotoImage | None]:
        if not path.exists():
            messagebox.showerror("Preview Failed", "Image no longer exists on disk.")
            return (None, None)
        if Image and ImageTk:
            try:
                img = Image.open(path)
                img.load()
                base = img.copy()
                img.close()
                return (base, None)
            except Exception as exc:
                messagebox.showerror("Preview Failed", f"Could not load image: {exc}")
                return (None, None)
        try:
            photo = tk.PhotoImage(file=str(path))
            return (None, photo)
        except Exception as exc:
            messagebox.showerror("Preview Failed", f"Could not load image: {exc}")
            return (None, None)

    def _ensure_image_panel(self):
        if self._image_panel_frame and self._image_panel_frame.winfo_exists():
            return self._image_panel_frame
        frame = tk.Frame(self.root, bg=self.background_color, bd=1, relief="ridge",
                         highlightbackground="#333333", highlightthickness=1)
        frame.place(x=120, y=120, width=320, height=240)
        frame.pack_propagate(False)
        header = tk.Frame(frame, bg=self.background_color, cursor="fleur")
        header.pack(side="top", fill="x")
        title = tk.Label(header, text="Preview", bg=self.background_color, fg=self.text_color,
                         font=("Segoe UI", 11, "bold"))
        title.pack(side="left", padx=10, pady=6)
        close_btn = tk.Button(header, text="X", command=lambda: self._hide_image_panel(),
                              bg="#111111", fg=self.text_color, relief="flat",
                              font=("Segoe UI", 9, "bold"), highlightthickness=0,
                              activebackground="#222222", activeforeground=self.text_color, padx=6, pady=2)
        close_btn.pack(side="right", padx=10, pady=6)
        body = tk.Frame(frame, bg=self.background_color)
        body.pack(side="top", fill="both", expand=True)
        canvas = tk.Canvas(body, bg=self.background_color, highlightthickness=0, bd=0)
        canvas.pack(side="top", fill="both", expand=True, padx=8, pady=(0, 8))
        canvas.bind("<Configure>", lambda _e: self._update_image_canvas_position())
        canvas.bind("<MouseWheel>", self._on_image_panel_wheel)
        canvas.bind("<Button-4>", lambda e: self._on_image_panel_wheel(e, delta=120))
        canvas.bind("<Button-5>", lambda e: self._on_image_panel_wheel(e, delta=-120))
        canvas.bind("<ButtonPress-1>", self._start_image_pan)
        canvas.bind("<B1-Motion>", self._drag_image_pan)
        canvas.bind("<ButtonRelease-1>", self._stop_image_pan)
        header.bind("<ButtonPress-1>", self._start_image_panel_drag)
        header.bind("<B1-Motion>", self._drag_image_panel)
        header.bind("<ButtonRelease-1>", self._stop_image_panel_drag)
        title.bind("<ButtonPress-1>", self._start_image_panel_drag)
        title.bind("<B1-Motion>", self._drag_image_panel)
        title.bind("<ButtonRelease-1>", self._stop_image_panel_drag)
        canvas.bind("<Motion>", self._update_image_canvas_cursor)
        canvas.bind("<Leave>", lambda _e: self._reset_image_canvas_cursor())
        self._image_panel_frame = frame
        self._image_panel_canvas = canvas
        self._image_panel_title = title
        return frame

    def _place_image_panel(self, width: int | None = None, height: int | None = None):
        frame = self._ensure_image_panel()
        cur_w, cur_h = self._image_panel_size or (360, 280)
        width = width or cur_w
        height = height or cur_h
        self._image_panel_size = (width, height)
        frame.config(width=width, height=height)
        if not self._image_panel_position or any(v is None for v in self._image_panel_position):
            root_w = max(self.root.winfo_width(), width + 80)
            x = max(24, root_w - width - 40)
            y = max(60, 80)
            self._image_panel_position = (x, y)
        x, y = self._image_panel_position
        frame.place(x=x, y=y, width=width, height=height)
        self._image_panel_position = (x, y)
        frame.lift()

    def _start_image_panel_drag(self, event):
        if not self._image_panel_frame:
            return
        self._image_panel_drag_offset = (event.x, event.y)
        self._image_panel_frame.lift()

    def _drag_image_panel(self, event):
        if not self._image_panel_frame or not self._image_panel_drag_offset:
            return
        dx = event.x - self._image_panel_drag_offset[0]
        dy = event.y - self._image_panel_drag_offset[1]
        x = self._image_panel_frame.winfo_x() + dx
        y = self._image_panel_frame.winfo_y() + dy
        self._image_panel_position = (x, y)
        self._image_panel_frame.place(x=x, y=y)

    def _stop_image_panel_drag(self, _event):
        self._image_panel_drag_offset = None

    def _start_image_panel_resize(self, event):
        if not self._image_panel_frame or not self._image_panel_size:
            return
        self._image_panel_resize_start = (
            event.x_root,
            event.y_root,
            self._image_panel_size[0],
            self._image_panel_size[1],
        )
        self._image_panel_resizing = True
        self._image_panel_resize_from_canvas = event.widget is self._image_panel_canvas
        if self._image_panel_canvas:
            self._image_panel_canvas.config(cursor="size_se")

    def _perform_image_panel_resize(self, event):
        if not self._image_panel_resize_start:
            return
        x0, y0, w0, h0 = self._image_panel_resize_start
        dx = event.x_root - x0
        dy = event.y_root - y0
        width = max(220, w0 + dx)
        height = max(200, h0 + dy)
        self._image_panel_size = (width, height)
        self._image_panel_auto_size = False
        self._place_image_panel(width, height)

    def _stop_image_panel_resize(self, _event):
        self._image_panel_resize_start = None
        self._image_panel_resizing = False
        self._image_panel_resize_from_canvas = False
        self._reset_image_canvas_cursor()

    def _start_image_pan(self, event):
        if not self._image_panel_canvas or not self._image_panel_photo:
            return
        if self._should_resize_canvas(event):
            self._start_image_panel_resize(event)
            return "break"
        self._image_panel_canvas.config(cursor="fleur")
        self._image_panel_pan_start = (event.x, event.y)

    def _drag_image_pan(self, event):
        if not self._image_panel_canvas or not self._image_panel_photo:
            return
        if self._image_panel_resizing:
            self._perform_image_panel_resize(event)
            return
        if not self._image_panel_pan_start:
            return
        sx, sy = self._image_panel_pan_start
        dx = event.x - sx
        dy = event.y - sy
        ox, oy = self._image_panel_offset
        self._image_panel_offset = (ox + dx, oy + dy)
        self._image_panel_pan_start = (event.x, event.y)
        self._update_image_canvas_position()

    def _stop_image_pan(self, _event):
        if self._image_panel_resizing:
            self._stop_image_panel_resize(_event)
            return
        if self._image_panel_canvas:
            self._image_panel_canvas.config(cursor="")
        self._image_panel_pan_start = None

    def _hide_image_panel(self, path: Path | None = None):
        if path is not None and self._image_panel_path and path != self._image_panel_path:
            return
        if self._image_panel_frame and self._image_panel_frame.winfo_exists():
            self._image_panel_frame.place_forget()
        self._reset_image_canvas()
        self._image_panel_photo = None
        self._image_panel_path = None
        self._image_panel_base_image = None
        self._image_panel_zoom = 1.0
        self._image_panel_offset = (0.0, 0.0)
        self._image_panel_pan_start = None
        self._image_panel_resizing = False
        self._image_panel_resize_from_canvas = False
        self._image_panel_auto_size = False

    def _compute_initial_image_zoom(self, img) -> float:
        try:
            w, h = img.size
        except Exception:
            return 1.0
        if w <= 0 or h <= 0:
            return 1.0
        self.root.update_idletasks()
        limit_w = max(320, int(self.root.winfo_width() * 0.5) or 320)
        limit_h = max(320, int(self.root.winfo_height() * 0.6) or 320)
        scale_w = limit_w / w
        scale_h = limit_h / h
        scale = min(scale_w, scale_h, 1.0)
        return max(scale, 0.1)

    def _update_image_canvas_position(self):
        if not self._image_panel_canvas or not self._image_panel_photo:
            return
        canvas = self._image_panel_canvas
        if self._image_panel_image_id is None:
            self._image_panel_image_id = canvas.create_image(0, 0, image=self._image_panel_photo, anchor="center")
        canvas.update_idletasks()
        width = max(1, canvas.winfo_width())
        height = max(1, canvas.winfo_height())
        cx = width / 2 + self._image_panel_offset[0]
        cy = height / 2 + self._image_panel_offset[1]
        canvas.coords(self._image_panel_image_id, cx, cy)

    def _reset_image_canvas(self):
        if self._image_panel_canvas and self._image_panel_image_id is not None:
            try:
                self._image_panel_canvas.delete(self._image_panel_image_id)
            except Exception:
                pass
        self._image_panel_image_id = None

    def _should_resize_canvas(self, event) -> bool:
        canvas = self._image_panel_canvas
        if not canvas:
            return False
        canvas.update_idletasks()
        width = canvas.winfo_width()
        height = canvas.winfo_height()
        if width <= 0 or height <= 0:
            return False
        margin = 14
        near_right = (width - event.x) <= margin
        near_bottom = (height - event.y) <= margin
        return near_right or near_bottom

    def _update_image_canvas_cursor(self, event):
        canvas = self._image_panel_canvas
        if not canvas:
            return
        if self._image_panel_resizing:
            canvas.config(cursor="size_se")
            return
        if self._should_resize_canvas(event):
            canvas.config(cursor="size_se")
        else:
            canvas.config(cursor="")

    def _reset_image_canvas_cursor(self):
        if self._image_panel_canvas and not self._image_panel_resizing:
            self._image_panel_canvas.config(cursor="")

    def _update_image_panel_photo(self):
        if not (self._image_panel_base_image and Image and ImageTk and self._image_panel_canvas):
            return
        img = self._image_panel_base_image
        zoom = self._image_panel_zoom
        width = max(1, int(img.width * zoom))
        height = max(1, int(img.height * zoom))
        try:
            resample = Image.LANCZOS
        except AttributeError:
            resample = Image.BICUBIC
        scaled = img.resize((width, height), resample)
        photo = ImageTk.PhotoImage(scaled)
        self._image_panel_photo = photo
        canvas = self._image_panel_canvas
        if self._image_panel_image_id is None:
            self._image_panel_image_id = canvas.create_image(0, 0, image=photo, anchor="center")
        else:
            canvas.itemconfigure(self._image_panel_image_id, image=photo)
        if self._image_panel_auto_size:
            panel_w = min(max(width + 48, 240), max(320, int(self.root.winfo_width() * 0.8)))
            panel_h = min(max(height + 92, 260), max(320, int(self.root.winfo_height() * 0.85)))
            self._image_panel_size = (panel_w, panel_h)
            self._image_panel_auto_size = False
        self._update_image_canvas_position()
        self._place_image_panel()

    def _on_image_panel_wheel(self, event, delta: int | None = None):
        if self._image_panel_base_image is None or not (Image and ImageTk):
            return
        if delta is None:
            delta = getattr(event, "delta", 0)
        if delta == 0:
            return "break"
        if delta < 0:
            factor = 1 / 1.15
        else:
            factor = 1.15
        new_zoom = self._image_panel_zoom * factor
        new_zoom = max(0.1, min(5.0, new_zoom))
        if abs(new_zoom - self._image_panel_zoom) < 1e-5:
            return "break"
        self._image_panel_zoom = new_zoom
        self._update_image_panel_photo()
        return "break"

    def _open_image_preview(self, node: Node):
        path = node.path
        if self._image_panel_path == path and self._image_panel_frame and self._image_panel_frame.winfo_ismapped():
            self._image_panel_frame.lift()
            return
        base_image, photo_direct = self._load_image_for_preview(path)
        if base_image is None and photo_direct is None:
            return
        self._ensure_image_panel()
        self._reset_image_canvas()
        if self._image_panel_title:
            self._image_panel_title.config(text=path.name or str(path))
        self._image_panel_offset = (0.0, 0.0)
        self._image_panel_pan_start = None
        self._image_panel_resizing = False
        self._image_panel_resize_start = None
        self._reset_image_canvas_cursor()
        self._image_panel_path = path
        if base_image is not None and Image and ImageTk:
            self._image_panel_base_image = base_image
            self._image_panel_zoom = self._compute_initial_image_zoom(base_image)
            self._image_panel_auto_size = True
            self._update_image_panel_photo()
        else:
            self._image_panel_base_image = None
            self._image_panel_zoom = 1.0
            photo = photo_direct
            if not photo:
                return
            if self._image_panel_canvas is None:
                self._ensure_image_panel()
            canvas = self._image_panel_canvas
            if canvas:
                if self._image_panel_image_id is None:
                    self._image_panel_image_id = canvas.create_image(0, 0, image=photo, anchor="center")
                else:
                    canvas.itemconfigure(self._image_panel_image_id, image=photo)
            self._image_panel_photo = photo
            if photo:
                self._image_panel_auto_size = True
                width = min(max(photo.width() + 48, 240), max(320, int(self.root.winfo_width() * 0.5) or 320))
                height = min(max(photo.height() + 92, 260), max(320, int(self.root.winfo_height() * 0.6) or 320))
                self._image_panel_size = (width, height)
                self._update_image_canvas_position()
                self._image_panel_auto_size = False
        self._place_image_panel()

    def _truncate_label(self, label: str, padding: int = 24) -> str:
        ellipsis = "..."
        try:
            max_chars = max(4, int((NODE_W - padding) // 7.0))
        except Exception:
            max_chars = 24
        if len(label) <= max_chars:
            return label
        return label[:max_chars - len(ellipsis)] + ellipsis

    def _format_node_size(self, node: Node) -> str | None:
        if node.virtual:
            return None
        try:
            if node.is_dir:
                count, total_size = self._get_dir_stats(node.path)
                item_text = "item" if count == 1 else "items"
                return f"{count} {item_text}, {readable_size(total_size)}"
            size = node.path.stat(follow_symlinks=False).st_size
            return readable_size(size)
        except Exception:
            return None

    def _get_dir_stats(self, path: Path) -> tuple[int, int]:
        key = (path, self.show_hidden)
        cached = self._size_cache.get(key)
        if cached is not None:
            return cached

        count = 0
        total_size = 0
        try:
            for entry in path.iterdir():
                if not self.show_hidden and self._is_hidden_path(entry):
                    continue
                count += 1
                try:
                    if entry.is_dir() and not entry.is_symlink():
                        _, child_size = self._get_dir_stats(entry)
                        total_size += child_size
                    else:
                        total_size += entry.stat(follow_symlinks=False).st_size
                except Exception:
                    continue
        except Exception:
            pass

        stats = (count, total_size)
        self._size_cache[key] = stats
        return stats

    def _schedule_collision_watch(self, delay: int = 220):
        if hasattr(self, "root"):
            if self._collision_job is not None:
                self.root.after_cancel(self._collision_job)
            self._collision_job = self.root.after(delay, self._collision_watch)

    def _collision_watch(self):
        self._collision_job = None
        if not self.root_node:
            return
        if self._drag_node is not None or self.force_running:
            self._schedule_collision_watch()
            return
        moved = self._resolve_global_collisions()
        if moved:
            self.redraw_all()
        self._schedule_collision_watch()

    def _start_force_simulation(self):
        if self.force_running:
            return
        self.force_running = True
        self._force_step()

    def _on_global_mouse_move(self, event):
        try:
            root_y = self.root.winfo_rooty()
        except Exception:
            return
        threshold = root_y + 8
        if event.y_root <= threshold:
            if not self.search_shown:
                self.toggle_searchbar(from_hover=True)
            return

        if self.search_shown and self._search_open_via_hover:
            try:
                bar_top = self.search_frame.winfo_rooty()
                bar_bottom = bar_top + self.search_frame.winfo_height()
            except Exception:
                bar_bottom = root_y + 42
            if event.y_root > bar_bottom + 6:
                self.toggle_searchbar(hide_only=True)

    # ---------- Collision resolution ----------
    def _resolve_global_collisions(self, anchor: Node | None = None) -> bool:
        nodes = self._all_nodes()
        hw = self.box_w_world / 2.0
        hh = self.box_h_world / 2.0
        moved_overall = False
        for _ in range(COLLISION_ITER):
            moved_any = False
            for i in range(len(nodes)):
                a = nodes[i]
                ax, ay = a.x, a.y
                for j in range(i+1, len(nodes)):
                    b = nodes[j]
                    dx = (hw + hw + self.margin_world) - abs(ax - b.x)
                    dy = (hh + hh + self.margin_world) - abs(ay - b.y)
                    if dx > 0 and dy > 0:
                        moved_any = True
                        if dx < dy:
                            push = dx
                            if anchor is a and anchor is not b:
                                b.x += push if b.x >= ax else -push
                            elif anchor is b and anchor is not a:
                                a.x += push if a.x >= b.x else -push
                            else:
                                half = push/2.0
                                a.x += half if a.x >= b.x else -half
                                b.x += half if b.x >= ax else -half
                        else:
                            push = dy
                            if anchor is a and anchor is not b:
                                b.y += push if b.y >= ay else -push
                            elif anchor is b and anchor is not a:
                                a.y += push if a.y >= b.y else -push
                            else:
                                half = push/2.0
                                a.y += half if a.y >= b.y else -half
                                b.y += half if b.y >= ay else -half
                ax, ay = a.x, a.y
            if not moved_any:
                break
            moved_overall = True
        return moved_overall
    # ---------- Node dragging (canvas-level continuous with neighbor push) ----------
    def _bind_node_events(self, node: Node):
        self.canvas.tag_bind(node.tag, "<Button-1>", lambda e, n=node: self._node_press(e, n))

    def _node_press(self, evt, node: Node):
        if self._drag_node is not None:
            return "break"
        self._drag_node = node
        self._last_drag_screen = (evt.x, evt.y)
        self.force_running = False
        self._node_dragging = False
        self._suppress_click = False
        self.canvas.bind("<B1-Motion>", self._node_drag_global)
        self.canvas.bind("<ButtonRelease-1>", self._node_release_global)
        return "break"

    def _node_drag_global(self, evt):
        if not self._drag_node:
            return
        sx0, sy0 = self._last_drag_screen
        sx1, sy1 = evt.x, evt.y
        if not self._node_dragging:
            if abs(sx1 - sx0) >= 2 or abs(sy1 - sy0) >= 2:
                self._node_dragging = True
        w0x, w0y = self.screen_to_world(sx0, sy0)
        w1x, w1y = self.screen_to_world(sx1, sy1)
        dx = w1x - w0x
        dy = w1y - w0y
        self._drag_node.x += dx
        self._drag_node.y += dy
        self._last_drag_screen = (sx1, sy1)
        self._resolve_global_collisions(anchor=self._drag_node)
        self.redraw_all()

    def _node_release_global(self, _evt):
        node = self._drag_node
        self._drag_node = None
        self.canvas.unbind("<B1-Motion>")
        self.canvas.unbind("<ButtonRelease-1>")
        if self._node_dragging:
            self._last_click_node = None
        if node and not self._node_dragging and not self._suppress_click:
            now = time.perf_counter()
            if self._last_click_node is node and (now - self._last_click_time) <= DOUBLE_CLICK_MAX_INTERVAL:
                self._last_click_node = None
                self._last_click_time = 0.0
                self._rename_node(None, node)
            else:
                self._last_click_node = node
                self._last_click_time = now
                self._handle_node_click(node)
        self._node_dragging = False
        self._suppress_click = False
        self._start_force_simulation()

    def _rename_node(self, _evt, node: Node):
        self._suppress_click = True
        self._last_click_node = None
        self._last_click_time = 0.0
        if node.virtual:
            return
        self._drag_node = None
        self._node_dragging = False
        try:
            if not node.path.exists():
                messagebox.showerror("Rename Failed", "The selected item no longer exists.")
                return
        except OSError:
            messagebox.showerror("Rename Failed", "Unable to access the selected item.")
            return

        initial = node.path.name
        new_name = simpledialog.askstring("Rename", "Enter a new name:", initialvalue=initial, parent=self.root)
        if new_name is None:
            return
        new_name = new_name.strip()
        if not new_name:
            messagebox.showerror("Rename Failed", "Name cannot be empty.")
            return
        if new_name in (".", ".."):
            messagebox.showerror("Rename Failed", "Invalid name.")
            return
        separators = {os.sep}
        if os.altsep:
            separators.add(os.altsep)
        if any(sep in new_name for sep in separators):
            messagebox.showerror("Rename Failed", "Name cannot contain path separators.")
            return

        old_path = node.path
        new_path = old_path.parent / new_name
        if new_path.exists():
            messagebox.showerror("Rename Failed", f"'{new_name}' already exists.")
            return
        try:
            old_path.rename(new_path)
        except Exception as exc:
            messagebox.showerror("Rename Failed", str(exc))
            return

        if self.default_root_path == old_path:
            self.default_root_path = new_path
            self._default_root_var.set(str(self.default_root_path))
        if self.root_node and self.root_node.path == old_path:
            root_target = new_path
        else:
            root_target = self.root_node.path if self.root_node else new_path

        self._save_user_settings()
        self.load_root(root_target)

    # ---------- Force-directed layout ----------
    def toggle_force(self):
        if self.force_running:
            self.force_running = False
        else:
            self._start_force_simulation()

    def _force_step(self):
        if not self.force_running:
            return
        nodes = self._all_nodes()
        if not nodes:
            return

        fx = {n: 0.0 for n in nodes}
        fy = {n: 0.0 for n in nodes}
        # repulsion
        for i in range(len(nodes)):
            a = nodes[i]
            for j in range(i+1, len(nodes)):
                b = nodes[j]
                dx = a.x - b.x
                dy = a.y - b.y
                dist2 = dx*dx + dy*dy + 1e-6
                force = K_REPULSE / dist2
                d = math.sqrt(dist2)
                ux = dx / d
                uy = dy / d
                fx[a] += force * ux
                fy[a] += force * uy
                fx[b] -= force * ux
                fy[b] -= force * uy

        # springs along edges
        for a, b in self._edges():
            dx = b.x - a.x
            dy = b.y - a.y
            d = math.sqrt(dx*dx + dy*dy) + 1e-6
            target = SPRING_REST
            force = K_SPRING * (d - target)
            ux = dx / d
            uy = dy / d
            fx[a] += force * ux
            fy[a] += force * uy
            fx[b] -= force * ux
            fy[b] -= force * uy

        # integrate
        for n in nodes:
            n.vx = (n.vx + fx[n]) * DAMPING
            n.vy = (n.vy + fy[n]) * DAMPING
            step_len = math.hypot(n.vx, n.vy)
            if step_len > MAX_STEP:
                scale = MAX_STEP / (step_len + 1e-6)
                n.vx *= scale
                n.vy *= scale
            n.x += n.vx
            n.y += n.vy

        self._resolve_global_collisions(anchor=None)
        self.redraw_all()
        self.root.after(FORCE_STEP_MS, self._force_step)

    # ---------- App wiring ----------
    def _on_resize(self, _evt):
        pass

    def choose_dir(self):
        init = str(self.root_node.path if self.root_node else Path.home())
        d = filedialog.askdirectory(initialdir=init, title="Choose root directory")
        if d:
            self.load_root(Path(d))

    def reload(self):
        if self.root_node:
            self.load_root(self.root_node.path)

    def load_root(self, path: Path):
        self._cancel_all_expand_animations()
        self._hide_image_panel()
        self._size_cache.clear()
        self.canvas.delete("all")
        self.scale_factor = 1.0
        self._update_world_box_size()
        self.root.update_idletasks()
        self.canvas.update_idletasks()
        canvas_w = self.canvas.winfo_width()
        canvas_h = self.canvas.winfo_height()
        if canvas_w <= 1 or canvas_h <= 1:
            canvas_w = 1280
            canvas_h = 840
        self.offset_x = canvas_w / 2
        self.offset_y = canvas_h / 2
        self.root_node = Node(self, str(path), path, True, 0, 0, None)
        self.root_node.expand()
        self._resolve_global_collisions(anchor=self.root_node)
        self.redraw_all()
        self._schedule_collision_watch()
        self._start_force_simulation()

    # ---------- Drawing ----------
    def _draw_node_recursive(self, node: Node):
        sx, sy = self.world_to_screen(node.x, node.y)
        for ch in node.children:
            cx, cy = self.world_to_screen(ch.x, ch.y)
            ex0, ey0 = clip_to_rect_edge(sx, sy, cx, cy, NODE_W, NODE_H)
            ex1, ey1 = clip_to_rect_edge(cx, cy, sx, sy, NODE_W, NODE_H)
            self.canvas.create_line(ex0, ey0, ex1, ey1, fill=self.node_color, width=1.5)

        x0, y0, x1, y1 = sx - NODE_W/2, sy - NODE_H/2, sx + NODE_W/2, sy + NODE_H/2
        r = RADIUS
        tags = (node.tag, "node")
        # Add an invisible-but-clickable hit box so the whole rectangle responds to events.
        self.canvas.create_rectangle(x0, y0, x1, y1, fill=self.background_color, outline="", width=0, tags=tags)
        self.canvas.create_arc(x0, y0, x0+2*r, y0+2*r, start=90, extent=90, style="arc", outline=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_arc(x1-2*r, y0, x1, y0+2*r, start=0, extent=90, style="arc", outline=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_arc(x1-2*r, y1-2*r, x1, y1, start=270, extent=90, style="arc", outline=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_arc(x0, y1-2*r, x0+2*r, y1, start=180, extent=90, style="arc", outline=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_line(x0+r, y0, x1-r, y0, fill=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_line(x1, y0+r, x1, y1-r, fill=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_line(x0+r, y1, x1-r, y1, fill=self.node_color, width=LINE_W, tags=tags)
        self.canvas.create_line(x0, y0+r, x0, y1-r, fill=self.node_color, width=LINE_W, tags=tags)

        icon_x0 = x0 + 10
        icon_y0 = (y0 + y1)/2 - 8
        self.canvas.create_rectangle(icon_x0, icon_y0, icon_x0+16, icon_y0+16, outline=self.node_color, width=LINE_W, tags=tags)
        if node.is_dir:
            self.canvas.create_rectangle(icon_x0+2, icon_y0-6, icon_x0+14, icon_y0, outline=self.node_color, width=LINE_W, tags=tags)

        text = self._truncate_label(node.label)
        self.canvas.create_text(x0+34, (y0+y1)/2 - 6, text=text, fill=self.text_color, anchor="w", font=("Segoe UI", 10), tags=tags)

        size_text = self._format_node_size(node)
        if size_text:
            self.canvas.create_text((x0+x1)/2, y1 - 8, text=size_text, fill=self.text_color,
                                    font=("Segoe UI", 9), tags=tags)

        self._bind_node_events(node)

        if node.expanded:
            for ch in node.children:
                self._draw_node_recursive(ch)

    def redraw_all(self):
        self.canvas.delete("all")
        if self.root_node:
            self._draw_node_recursive(self.root_node)

    def _expand_with_animation(self, node: Node, steps: int = 10):
        if node in self._expand_animations or node.expanded:
            return
        node.expand()
        targets: list[tuple[Node, float, float]] = []
        for ch in node.children:
            targets.append((ch, ch.x, ch.y))
            ch.x = node.x
            ch.y = node.y
        if not targets:
            self.redraw_all()
            self._resolve_global_collisions(anchor=node)
            self._start_force_simulation()
            return
        self._expand_animations[node] = {"targets": targets, "step": 0, "steps": steps, "job": None}
        self.force_running = False
        self.redraw_all()
        self._animate_expand_step(node)

    def _animate_expand_step(self, node: Node):
        anim = self._expand_animations.get(node)
        if not anim:
            return
        step = anim.get("step", 0)
        steps = anim.get("steps", 10)
        if step >= steps:
            for ch, tx, ty in anim.get("targets", []):
                ch.x = tx
                ch.y = ty
            job = anim.get("job")
            if job:
                try:
                    self.root.after_cancel(job)
                except Exception:
                    pass
            self._expand_animations.pop(node, None)
            self._resolve_global_collisions(anchor=node)
            self.redraw_all()
            self._start_force_simulation()
            return
        progress = (step + 1) / max(1, steps)
        for ch, tx, ty in anim.get("targets", []):
            ch.x = node.x + (tx - node.x) * progress
            ch.y = node.y + (ty - node.y) * progress
        anim["step"] = step + 1
        self.redraw_all()
        job = self.root.after(16, lambda n=node: self._animate_expand_step(n))
        anim["job"] = job

    def _cancel_expand_animation(self, node: Node):
        anim = self._expand_animations.pop(node, None)
        if not anim:
            return
        job = anim.get("job")
        if job:
            try:
                self.root.after_cancel(job)
            except Exception:
                pass
        for ch, tx, ty in anim.get("targets", []):
            ch.x = tx
            ch.y = ty

    def _cancel_all_expand_animations(self):
        for node in list(self._expand_animations.keys()):
            self._cancel_expand_animation(node)
        self._expand_animations.clear()

    def _handle_node_click(self, node: Node):
        if node.virtual:
            return
        if self._is_image_file(node):
            if not node.expanded:
                self._expand_with_animation(node)
                self._open_image_preview(node)
            else:
                self._cancel_expand_animation(node)
                node.collapse()
                self._hide_image_panel(node.path)
                self.redraw_all()
                self._start_force_simulation()
            return
        self._cancel_expand_animation(node)
        if not node.expanded:
            node.expand()
        else:
            node.collapse()
        self._resolve_global_collisions(anchor=node)
        self.redraw_all()
        self._start_force_simulation()

    # ---------- Context menu & file operations ----------
    def _show_context_menu(self, x_root: int, y_root: int, target: Node | None):
        menu = tk.Menu(self.root, tearoff=0)
        actionable_target = target if (target and not target.virtual) else None
        if actionable_target:
            menu.add_command(label="Delete", command=lambda n=actionable_target: self._delete_node(n))
        else:
            has_root = bool(self.root_node and getattr(self.root_node, "path", None))
            state = "normal" if has_root else "disabled"
            menu.add_command(label="New File", state=state, command=lambda: self._create_entry(is_dir=False))
            menu.add_command(label="New Folder", state=state, command=lambda: self._create_entry(is_dir=True))
        try:
            menu.tk_popup(x_root, y_root)
        finally:
            menu.grab_release()

    def _create_entry(self, is_dir: bool):
        base_dir = self.root_node.path if self.root_node else None
        if not base_dir:
            messagebox.showerror("Create Failed", "No active directory to create items in.")
            return
        prompt = "Enter new folder name:" if is_dir else "Enter new file name:"
        new_name = simpledialog.askstring("Create Item", prompt, parent=self.root)
        if new_name is None:
            return
        new_name = new_name.strip()
        if not new_name:
            messagebox.showerror("Create Failed", "Name cannot be empty.")
            return
        if new_name in (".", ".."):
            messagebox.showerror("Create Failed", "Invalid name.")
            return
        separators = {os.sep}
        if os.altsep:
            separators.add(os.altsep)
        if any(sep in new_name for sep in separators):
            messagebox.showerror("Create Failed", "Name cannot contain path separators.")
            return
        target_path = Path(base_dir) / new_name
        if target_path.exists():
            messagebox.showerror("Create Failed", f"'{new_name}' already exists.")
            return
        try:
            if is_dir:
                target_path.mkdir()
            else:
                target_path.touch(exist_ok=False)
        except Exception as exc:
            messagebox.showerror("Create Failed", str(exc))
            return
        self.reload()

    def _delete_node(self, node: Node):
        if node.virtual:
            return
        if node is self.root_node:
            messagebox.showwarning("Delete Blocked", "Cannot delete the root directory from this view.")
            return
        try:
            path = node.path
        except AttributeError:
            messagebox.showerror("Delete Failed", "Unknown target.")
            return
        if not path.exists():
            messagebox.showinfo("Delete", "Item already deleted; refreshing view.")
            self.reload()
            return
        label = f"'{path.name}'" if path.name else str(path)
        if not messagebox.askyesno("Delete", f"Delete {label}? This cannot be undone."):
            return
        try:
            if path.is_dir():
                shutil.rmtree(path)
            else:
                path.unlink()
        except Exception as exc:
            messagebox.showerror("Delete Failed", str(exc))
            return
        self.reload()

    def run(self):
        if not self.fullscreen:
            idx = max(0, min(len(self._resolution_presets) - 1, self._resolution_index.get()))
            width, height = self._resolution_presets[idx][1]
            self.root.geometry(f"{width}x{height}")
        self.root.mainloop()


if __name__ == "__main__":
    App().run()
