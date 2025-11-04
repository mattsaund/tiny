#!/usr/bin/env python3
"""
PKMS Graph Browser (Tkinter version, Python 3.13+ compatible)
- Black background, white outlines
- Click folder to expand/collapse; click file to show metadata
- Pan: Right-drag | Zoom: Ctrl + Wheel or +/-
- Drag nodes: Left-drag anywhere on a node (continuous, canvas-level)
- Edges clipped to box borders
- Scale-aware box collisions + neighbor "push" (domino effect)
- Force-directed auto-layout toggle with 'F'
- Alt+Space (or Ctrl+Space) toggles a search bar at the top. Type a directory path and press Enter to jump there.
- NEW: F11 toggles fullscreen. In the search bar, type "res 1920x1080" (or "r"/"resolution") to change window resolution.
"""

from __future__ import annotations
import os
import math
import time
import re
from pathlib import Path
import tkinter as tk
from tkinter import filedialog

BG = "#000000"
FG = "#FFFFFF"
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
FORCE_STEP_MS = 20
K_REPULSE = 8000.0    # repulsive constant
K_SPRING = 0.02       # attractive spring constant along edges
SPRING_REST = 260.0   # desired edge length
DAMPING = 0.85
MAX_STEP = 35.0       # clamp per-step movement in world units


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

    def __init__(self, app: "App", label: str, path: Path, is_dir: bool, x: float, y: float, parent: "Node|None"=None):
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
                entries.sort(key=lambda p: (not p.is_dir(), p.name.lower()))
                entries = entries[:MAX_CHILDREN]
                labels = [(p.name, p, p.is_dir()) for p in entries]
            else:
                p = self.path
                st = p.stat()
                typ = p.suffix.lower()[1:] if p.suffix else "file"
                labels = [
                    (f"Type: {typ}", p, False),
                    (f"Size: {readable_size(st.st_size)}", p, False),
                    (f"Modified: {time.strftime('%Y-%m-%d %H:%M', time.localtime(st.st_mtime))}", p, False),
                ]
        except Exception as e:
            labels = [(f"Error: {e}", self.path, False)]

        n = max(1, len(labels))
        radius = max(BASE_RADIUS, 90 + n * RADIUS_PER_CHILD)
        angle0 = -90.0
        self.children.clear()
        for i, (label, path, is_dir) in enumerate(labels):
            angle = angle0 + (360.0 * i / n)
            rad = math.radians(angle)
            cx = self.x + radius * math.cos(rad)
            cy = self.y + radius * math.sin(rad)
            child = Node(self.app, label, path, is_dir, cx, cy, parent=self)
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
        self.root.title("PKMS Graph Browser (Tkinter)")
        self.root.configure(bg=BG)

        # --- UI: top bar + canvas ---
        self.topbar = tk.Frame(self.root, bg=BG)
        self.topbar.pack(side="top", fill="x")
        self.canvas = tk.Canvas(self.root, bg=BG, highlightthickness=0)
        self.canvas.pack(side="top", fill="both", expand=True)

        # Search bar (hidden by default)
        self.search_var = tk.StringVar()
        self.search_frame = tk.Frame(self.topbar, bg=BG)
        self.search_entry = tk.Entry(self.search_frame, textvariable=self.search_var, bg="#111111", fg="#FFFFFF",
                                     insertbackground="#FFFFFF", relief="flat", font=("Segoe UI", 11))
        self.search_entry.pack(side="left", fill="x", expand=True, padx=8, pady=6)
        self.search_entry.bind("<Return>", self._on_search_enter)
        self.search_entry.bind("<Escape>", lambda e: self.toggle_searchbar(hide_only=True))
        self.search_hint = tk.Label(self.search_frame, text="Type a path or 'res 1920x1080' and press Enter",
                                    bg=BG, fg="#888888", font=("Segoe UI", 10))
        self.search_hint.pack(side="left", padx=8)
        self.search_shown = False  # start hidden

        # View transform
        self.scale_factor = 1.0
        self.offset_x = 0.0
        self.offset_y = 0.0

        # Fullscreen state
        self.fullscreen = False

        # Box size in world-space (derived)
        self._update_world_box_size()

        # Drag state
        self._panning = False
        self._drag_start = None           # for panning
        self._drag_node: Node | None = None
        self._last_drag_screen = (0, 0)   # incremental dragging reference

        # Force simulation
        self.force_running = False

        self.root.bind("<Configure>", self._on_resize)
        # Pan with right mouse
        self.canvas.bind("<ButtonPress-3>", self._pan_start)
        self.canvas.bind("<B3-Motion>", self._pan_move)
        self.canvas.bind("<ButtonRelease-3>", self._pan_end)
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
        self.root.bind("<KeyPress-F>", lambda e: self.toggle_force())
        self.root.bind("<KeyPress-f>", lambda e: self.toggle_force())
        # F11 fullscreen toggle
        self.root.bind("<F11>", lambda e: self.toggle_fullscreen())

        # Alt+Space toggle searchbar (Windows may reserve Alt+Space; Ctrl+Space as fallback)
        self.root.bind_all("<Alt-Key-space>", lambda e: self.toggle_searchbar())
        self.root.bind_all("<Control-Key-space>", lambda e: self.toggle_searchbar())

        self.root_node: Node | None = None
        if root_path is None:
            root_path = Path.home()
        self.load_root(root_path)

    # ---------- Search bar ----------
    def toggle_searchbar(self, hide_only: bool = False):
        if hide_only or self.search_shown:
            self.search_frame.pack_forget()
            self.search_shown = False
            self.canvas.focus_set()
        else:
            self.search_var.set(str(self.root_node.path) if self.root_node else "")
            self.search_frame.pack(side="top", fill="x")
            self.search_shown = True
            self.search_entry.focus_set()
            self.search_entry.select_range(0, 'end')

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
        # Ctrl must be held for zoom on Windows; on X11 we accept Button-4/5 without ctrl
        if hasattr(evt, "delta"):
            if (evt.state & 0x4) == 0:  # Control bit on Windows
                return
            delta = 1 if evt.delta > 0 else -1
        else:
            delta = 1 if evt.num == 4 else -1
        factor = 1.15 if delta > 0 else 1/1.15
        self.zoom(factor, (evt.x, evt.y))

    def _pan_start(self, evt):
        self._panning = True
        self._drag_start = (evt.x, evt.y)

    def _pan_move(self, evt):
        if not self._panning or not self._drag_start:
            return
        x0, y0 = self._drag_start
        dx = evt.x - x0
        dy = evt.y - y0
        self._drag_start = (evt.x, evt.y)
        self.offset_x += dx
        self.offset_y += dy
        self.redraw_all()

    def _pan_end(self, _evt):
        self._panning = False
        self._drag_start = None

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

    # ---------- Collision resolution ----------
    def _resolve_global_collisions(self, anchor: Node | None = None):
        nodes = self._all_nodes()
        hw = self.box_w_world / 2.0
        hh = self.box_h_world / 2.0
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

    # ---------- Node dragging (canvas-level continuous with neighbor push) ----------
    def _bind_node_events(self, node: Node):
        self.canvas.tag_bind(node.tag, "<Button-1>", lambda e, n=node: self._node_press(e, n))
        self.canvas.tag_bind(node.tag, "<Double-Button-1>", lambda e, n=node: self._toggle_and_redraw(n))

    def _node_press(self, evt, node: Node):
        self._drag_node = node
        self._last_drag_screen = (evt.x, evt.y)
        self.force_running = False
        self.canvas.bind("<B1-Motion>", self._node_drag_global)
        self.canvas.bind("<ButtonRelease-1>", self._node_release_global)

    def _node_drag_global(self, evt):
        if not self._drag_node:
            return
        sx0, sy0 = self._last_drag_screen
        sx1, sy1 = evt.x, evt.y
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
        self._drag_node = None
        self.canvas.unbind("<B1-Motion>")
        self.canvas.unbind("<ButtonRelease-1>")

    # ---------- Force-directed layout ----------
    def toggle_force(self):
        self.force_running = not self.force_running
        if self.force_running:
            self._force_step()

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
        self.canvas.delete("all")
        self.scale_factor = 1.0
        self._update_world_box_size()
        self.offset_x = self.canvas.winfo_width()/2
        self.offset_y = self.canvas.winfo_height()/2
        self.root_node = Node(self, str(path), path, True, 0, 0, None)
        self.root_node.expand()
        self.redraw_all()

    # ---------- Drawing ----------
    def _draw_node_recursive(self, node: Node):
        sx, sy = self.world_to_screen(node.x, node.y)
        for ch in node.children:
            cx, cy = self.world_to_screen(ch.x, ch.y)
            ex0, ey0 = clip_to_rect_edge(sx, sy, cx, cy, NODE_W, NODE_H)
            ex1, ey1 = clip_to_rect_edge(cx, cy, sx, sy, NODE_W, NODE_H)
            self.canvas.create_line(ex0, ey0, ex1, ey1, fill=FG, width=1.5)

        x0, y0, x1, y1 = sx - NODE_W/2, sy - NODE_H/2, sx + NODE_W/2, sy + NODE_H/2
        r = RADIUS
        tags = (node.tag, "node")
        self.canvas.create_arc(x0, y0, x0+2*r, y0+2*r, start=90, extent=90, style="arc", outline=FG, width=LINE_W, tags=tags)
        self.canvas.create_arc(x1-2*r, y0, x1, y0+2*r, start=0, extent=90, style="arc", outline=FG, width=LINE_W, tags=tags)
        self.canvas.create_arc(x1-2*r, y1-2*r, x1, y1, start=270, extent=90, style="arc", outline=FG, width=LINE_W, tags=tags)
        self.canvas.create_arc(x0, y1-2*r, x0+2*r, y1, start=180, extent=90, style="arc", outline=FG, width=LINE_W, tags=tags)
        self.canvas.create_line(x0+r, y0, x1-r, y0, fill=FG, width=LINE_W, tags=tags)
        self.canvas.create_line(x1, y0+r, x1, y1-r, fill=FG, width=LINE_W, tags=tags)
        self.canvas.create_line(x0+r, y1, x1-r, y1, fill=FG, width=LINE_W, tags=tags)
        self.canvas.create_line(x0, y0+r, x0, y1-r, fill=FG, width=LINE_W, tags=tags)

        icon_x0 = x0 + 10
        icon_y0 = (y0 + y1)/2 - 8
        self.canvas.create_rectangle(icon_x0, icon_y0, icon_x0+16, icon_y0+16, outline=FG, width=LINE_W, tags=tags)
        if node.is_dir:
            self.canvas.create_rectangle(icon_x0+2, icon_y0-6, icon_x0+14, icon_y0, outline=FG, width=LINE_W, tags=tags)

        text = node.label if len(node.label) <= 26 else node.label[:24] + "…"
        self.canvas.create_text(x0+34, (y0+y1)/2, text=text, fill=FG, anchor="w", font=("Segoe UI", 10), tags=tags)

        self._bind_node_events(node)

        if node.expanded:
            for ch in node.children:
                self._draw_node_recursive(ch)

    def redraw_all(self):
        self.canvas.delete("all")
        if self.root_node:
            self._draw_node_recursive(self.root_node)

    def _toggle_and_redraw(self, node: Node):
        if node.expanded:
            node.collapse()
        else:
            node.expand()
        self.redraw_all()

    def run(self):
        self.root.geometry("1280x840")
        self.root.mainloop()


if __name__ == "__main__":
    App().run()
