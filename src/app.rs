//! Application state and input handling.
//!
//! Rendering lives in `ui`; this module owns everything else. The two panes
//! share one selection: moving the tree cursor changes what the preview shows,
//! and focusing the preview starts editing that same file.
//!
//! Open buffers are kept in a map rather than reloaded per selection, so
//! arrowing away from a file with unsaved edits and back does not lose them.
//!
//! # The two axes of state
//!
//! Almost every question about "why did that key do that" is answered by two
//! independent pieces of state:
//!
//! - [`Focus`] — which pane the keyboard belongs to, `Tree` or `Editor`.
//! - [`Mode`] — what, if anything, is layered on top: a prompt, a confirmation,
//!   the search or command bar, the settings area, the help overlay, or
//!   `Normal` for none of them.
//!
//! [`App::on_key`] dispatches on `Mode` first and only falls through to `Focus`
//! when the mode is `Normal`. The map is checked before either, because it
//! takes the whole screen and every key with it.
//!
//! # `i j k l` are the arrow keys
//!
//! Everywhere a pane is being navigated rather than typed into, `i j k l` do
//! what the four arrows do — up, left, down, right, in the inverted-T anyone
//! who has held a keyboard sideways already knows. `I` and `K` go all the way,
//! the same as Shift with an arrow. They exist for keyboards with no cursor
//! cluster, and they are deliberately *not* the vim `h j k l`: `j` and `k`
//! cannot mean both left-down and down-up at once, and a layout that matches
//! the arrows is the one that needs no explaining.
//!
//! The bar, the prompts and the editor are exempt, because there a letter is a
//! letter being typed.
//!
//! A third flag, `read_mode`, decides whether a focused text file shows
//! rendered output or raw source. It is not part of `Mode` because it is a
//! property of *how you are looking at this file*, not of what has the
//! keyboard.
//!
//! # The mode take-and-replace pattern
//!
//! `on_key` does `std::mem::replace(&mut self.mode, Mode::Normal)` and hands
//! the owned mode to a handler. This is not a borrow-checker workaround so much
//! as a useful default: a handler that does nothing leaves `Normal` behind, so
//! **closing is the default and staying open is explicit**. Every handler that
//! wants its mode to persist has to put it back — which is why you see
//! `self.mode = Mode::Bar(b)` at the end of so many branches. Forget it and the
//! overlay closes; that is the intended failure direction.
//!
//! # Buffers outlive the selection
//!
//! `buffers` is keyed by path and is never cleared wholesale. Arrowing past a
//! file opens it; arrowing away leaves it open, with any unsaved edits intact.
//! Only three things remove entries: a delete (drops the file and anything
//! under it), a rename (moves the entry to the new key so edits follow the
//! file), and a refresh or replace (drops *clean* buffers so they re-read from
//! disk, keeps dirty ones).
//!
//! # Where drawing state lives
//!
//! `preview_len`, `last_edit_height` and `last_tree_height` are written by
//! `ui` during a draw and read here by the key handlers. They are how paging
//! and scroll clamping know the size of a window this module cannot see. On
//! the very first frame they hold their defaults, which is harmless — every
//! use is clamped.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Config, Markers, Palette};
use crate::editor::Editor;
use crate::graph;
use crate::highlight::Highlighter;
use crate::media;
use crate::project;
use crate::projectmap::{self, ProjectMap};
use crate::search::{self, Hit, HitKind};
use crate::tree::{Row, Tree};

/// Which pane has the keyboard. Combined with [`Mode`] and `read_mode`, this
/// determines how any given key is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Editor,
}

/// Which single-line prompt is open in the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// A file or a folder, decided by whether the typed name has an extension.
    New,
    Rename,
}

/// A one-line text prompt in the status bar, for naming and renaming.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    /// Shown before the field, e.g. `New file`.
    pub label: String,
    pub input: String,
    /// Character index into `input`, not a byte offset.
    pub cursor: usize,
    /// Directory the typed name is resolved against. Captured when the prompt
    /// opens, so moving the tree cursor afterwards cannot change the target.
    pub base: PathBuf,
}

/// The character that turns the bar from a search into a command line.
///
/// One bar does both jobs, and this is the whole of the switch: type
/// `README` and it searches, type `*copy README.md to notes` and it runs. The
/// cost is that a search cannot begin with a literal `*`, which is a small
/// price for never having to remember which of two bars you are in.
pub const COMMAND_SIGIL: char = '*';

/// State for the bar. Search results live here rather than on `App` because
/// they only exist while the bar is open, and closing it should discard them.
#[derive(Debug, Clone)]
pub struct Bar {
    pub input: String,
    /// Character index into `input`.
    pub cursor: usize,
    /// Re-run from scratch on every keystroke. See the module docs in `search`
    /// for why that is affordable.
    pub results: Vec<Hit>,
    /// Highlighted result. Moving it previews that hit without leaving the bar.
    pub selected: usize,
    pub scroll: usize,
    /// Set when a search found nothing, so the bar can say so.
    pub searched: bool,
}

impl Bar {
    fn new(input: String) -> Self {
        Self {
            cursor: input.chars().count(),
            input,
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            searched: false,
        }
    }

    /// Whether what has been typed so far is a command rather than a query.
    /// Re-read on every keystroke, so deleting the `*` turns the line back
    /// into a search and the results come straight back.
    pub fn is_command(&self) -> bool {
        self.input.starts_with(COMMAND_SIGIL)
    }

    /// The command itself, without the sigil. Empty for a search.
    pub fn command(&self) -> &str {
        self.input.strip_prefix(COMMAND_SIGIL).unwrap_or("")
    }
}

/// The in-program settings area. Rows come from `Config::settings_index`, so
/// this holds only the cursor and whatever is being typed.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Index into `Config::settings_index`.
    pub selected: usize,
    pub scroll: usize,
    /// Present while a value is being typed.
    pub editing: Option<String>,
    pub cursor: usize,
}

/// What a `y` will actually do. Every irreversible action in tiny goes through
/// one of these — there is no undo for a delete or a project-wide replace, so
/// the confirmation is the only guard.
#[derive(Debug, Clone)]
pub enum ConfirmKind {
    /// Remove a file, or a directory and everything under it.
    Delete(PathBuf),
    /// Quit, discarding unsaved buffers.
    QuitUnsaved,
    /// Rewrite every occurrence across the project.
    Replace { find: String, replace: String },
}

/// A pending yes/no question. The `message` is built at the point the action
/// is requested, so it can quote real counts — how many files, how many
/// occurrences — rather than a generic warning.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub kind: ConfirmKind,
    pub message: String,
}

/// What is layered over the normal two-pane view. Checked before [`Focus`] in
/// [`App::on_key`], so an open overlay owns the keyboard.
#[derive(Debug, Clone)]
pub enum Mode {
    /// No overlay; keys go to whichever pane has focus.
    Normal,
    Prompt(Prompt),
    Confirm(Confirm),
    /// The keymap, with a scroll offset for terminals too short for it.
    Help(usize),
    Bar(Bar),
    Settings(Settings),
}

/// How a text file wants to be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// Rendered markdown.
    Markdown,
    /// Wrapped prose — notes, licences, logs. Read first, edited on demand.
    Prose,
    /// Source. Straight into the editor, with line numbers.
    Code,
}

impl TextKind {
    /// Prose and markdown open to be read; code opens to be typed into.
    pub fn reads_first(self) -> bool {
        self != TextKind::Code
    }
}

/// Extensionless files that are prose by convention rather than by suffix.
///
/// Without this, `LICENSE` and `README` would be classified as code and open
/// straight into the editor with line numbers, which is not what anyone wants
/// from a licence file.
const PROSE_NAMES: &[&str] = &[
    "LICENCE",
    "LICENSE",
    "COPYING",
    "NOTICE",
    "AUTHORS",
    "CONTRIBUTORS",
    "CHANGELOG",
    "CHANGES",
    "README",
    "TODO",
];

/// Decide how a text file should open.
///
/// Checked in order: markdown first (it is usually also in the prose list but
/// has a renderer of its own), then the configured prose extensions, then
/// anything else with an extension is code. Extensionless files fall back to
/// [`PROSE_NAMES`], and are code otherwise — which is the right default for a
/// `Makefile` or a shell script.
pub fn text_kind(path: &Path, prose_exts: &[String]) -> TextKind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
    {
        // Markdown is checked first: it is also in the prose list by default,
        // but it has a renderer of its own.
        Some(e) if matches!(e.as_str(), "md" | "markdown" | "mdown" | "mkd") => TextKind::Markdown,
        Some(e) if prose_exts.iter().any(|p| p.eq_ignore_ascii_case(&e)) => TextKind::Prose,
        Some(_) => TextKind::Code,
        None => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            if PROSE_NAMES.contains(&name.as_str()) {
                TextKind::Prose
            } else {
                TextKind::Code
            }
        }
    }
}

/// What the preview pane is showing for the current selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// A text file with an open buffer. Notes and prose render when the tree
    /// has focus and show raw source while being edited.
    Buffer {
        path: PathBuf,
        kind: TextKind,
    },
    /// A picture or a video, drawn as half-blocks.
    Media {
        path: PathBuf,
        kind: media::Kind,
        size: u64,
    },
    Directory {
        path: PathBuf,
        entries: usize,
    },
    Binary {
        path: PathBuf,
        size: u64,
        kind: String,
    },
    Unreadable(String),
    Empty,
}

/// A drawn media preview, kept so it is not re-decoded on every keystroke.
///
/// Keyed on the pane size as well as the path, because the preview is rendered
/// to fit — a resized pane needs a fresh decode. Holds a `Result` so a failed
/// decode is cached too, and a broken file is not retried on every frame.
pub struct MediaCache {
    pub path: PathBuf,
    pub cols: usize,
    pub rows: usize,
    pub result: std::result::Result<media::Preview, String>,
}

/// Files larger than this are not loaded into the editor. Shown as a
/// "large file" preview instead — the undo model snapshots whole buffers, so
/// there is a real ceiling on what can be edited comfortably.
const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;

/// Everything mutable. One of these exists for the life of the program;
/// `ui::draw` reads it and `on_key` writes it.
pub struct App {
    pub tree: Tree,
    /// Flattened tree, rebuilt after every structural change. The cursor
    /// indexes into this.
    pub rows: Vec<Row>,
    /// Cursor position in `rows`. Kept valid by every mutation path — a stale
    /// index would silently preview the wrong file.
    pub selected: usize,
    /// First visible row. Owned here but written by `ui` once the pane height
    /// is known.
    pub tree_scroll: usize,
    pub focus: Focus,
    pub mode: Mode,
    /// What the right pane is showing for the current selection. Derived from
    /// the cursor by [`App::sync_preview`]; never set directly.
    pub preview: Preview,
    pub preview_scroll: usize,
    /// How many lines the preview produced at the current width. Written by
    /// `ui` during a draw, read here to clamp scrolling.
    pub preview_len: usize,
    /// Markdown opens rendered; `e` drops into raw editing.
    pub read_mode: bool,
    /// Open files, keyed by path. Outlives the selection so unsaved edits
    /// survive arrowing away and back — see the module docs.
    pub buffers: HashMap<PathBuf, Editor>,
    /// The live config. Mutated by `:set` and the settings area; persisted
    /// only on an explicit `Ctrl+S`.
    pub config: Config,
    /// Parsed styles. Rebuilt from `config.theme` by [`App::apply_config`], so
    /// a theme change repaints without a restart.
    pub palette: Palette,
    /// Rebuilt alongside the palette when `syntax_theme` changes.
    pub highlighter: Highlighter,
    pub media: Option<MediaCache>,
    /// The project map, while it is being looked at.
    pub project_map: Option<ProjectMap>,
    /// What `Ctrl+C` picked up, waiting for a `Ctrl+V`. A path, not a copy of
    /// the bytes: the file is read at paste time, so editing it in between
    /// pastes what is there now rather than a stale snapshot.
    pub clipboard: Option<PathBuf>,
    /// `Ctrl+B` folds the tree away to give the whole window to the file.
    /// Purely a view state — nothing about the tree itself changes, so
    /// bringing it back costs no reading.
    pub tree_hidden: bool,
    /// Which pane had the keyboard when the tree was folded away, so unfolding
    /// puts it back and the key is a true toggle.
    focus_before_hide: Focus,
    /// The message on the status line. Every action sets this — it is the
    /// program's only channel for telling the user what just happened.
    pub status: String,
    /// Checked by the event loop after each draw, so the final frame is shown
    /// before the program exits.
    pub should_quit: bool,
    /// Pane heights from the last draw, used for page-up/page-down. Defaults
    /// are used on the first frame, before anything has been drawn.
    pub last_edit_height: usize,
    pub last_tree_height: usize,
    /// The columns the tree pane covered in the last frame, as `[x0, x1)`, so
    /// a mouse wheel can tell which pane it is over. `None` when the tree was
    /// not drawn at all. Written by `ui` for the same reason the heights are.
    pub last_tree_cols: Option<(u16, u16)>,
}

impl App {
    /// Build the initial state from a resolved target and a loaded config.
    ///
    /// The startup warning, if any, is shown in place of the usual greeting —
    /// a broken config file should be the first thing you see, not something
    /// buried behind a "? for help".
    ///
    /// When the target named a file, the cursor is revealed onto it and the
    /// editor is focused straight away: that is the entire point of writing
    /// `tiny ~/code/main.py` rather than naming the folder.
    pub fn new(target: project::Target, config: Config, warning: Option<String>) -> Result<Self> {
        let root = target.root.clone();
        if !root.is_dir() {
            return Err(anyhow!("{} is not a directory", root.display()));
        }
        let (highlighter, theme_warning) = Highlighter::with_theme(&config.syntax_theme);
        let tree = Tree::new(root, config.show_hidden);
        let rows = tree.flatten();
        let palette = Palette::from_theme(&config.theme);

        let opening = if target.created {
            "new project — ? for help".to_string()
        } else {
            "? for help".to_string()
        };
        let mut app = Self {
            tree,
            rows,
            selected: 0,
            tree_scroll: 0,
            focus: Focus::Tree,
            mode: Mode::Normal,
            preview: Preview::Empty,
            preview_scroll: 0,
            preview_len: 0,
            read_mode: true,
            buffers: HashMap::new(),
            config,
            palette,
            highlighter,
            media: None,
            project_map: None,
            clipboard: None,
            tree_hidden: false,
            focus_before_hide: Focus::Tree,
            status: warning.or(theme_warning).unwrap_or(opening),
            should_quit: false,
            last_edit_height: 20,
            last_tree_height: 20,
            last_tree_cols: None,
        };
        app.sync_preview();

        // `tiny <file>` lands on that file with the editor already open, which
        // is the whole point of naming a file instead of a folder.
        //
        // A project tiny just scaffolded also arrives with a file — its
        // README — but only shows it. There is nothing to type into a page you
        // have not read yet, and leaving the keyboard on the tree keeps the
        // "new project" hint on the status line.
        if let Some(file) = target.file {
            app.reveal(&file);
            if !target.created && matches!(app.preview, Preview::Buffer { .. }) {
                app.focus_editor();
            }
        }
        Ok(app)
    }

    pub fn root(&self) -> &Path {
        self.tree.root_path()
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected_row().map(|r| r.path.as_path())
    }

    /// The buffer behind the current preview, if the preview is a text file.
    /// Returns `None` for directories, media and binaries, which is what makes
    /// `Ctrl+S` on a picture say "nothing to save" instead of panicking.
    pub fn active_buffer(&self) -> Option<&Editor> {
        match &self.preview {
            Preview::Buffer { path, .. } => self.buffers.get(path),
            _ => None,
        }
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut Editor> {
        match &self.preview {
            Preview::Buffer { path, .. } => {
                let path = path.clone();
                self.buffers.get_mut(&path)
            }
            _ => None,
        }
    }

    /// Every buffer with unsaved changes. Drives the confirm-on-quit prompt,
    /// which names them so you know what you are about to discard.
    pub fn dirty_buffers(&self) -> Vec<&Path> {
        self.buffers
            .values()
            .filter(|e| e.dirty)
            .map(|e| e.path.as_path())
            .collect()
    }

    pub fn is_dirty(&self, path: &Path) -> bool {
        self.buffers.get(path).is_some_and(|e| e.dirty)
    }

    /// Glyphs for an open folder, a closed folder, and a file.
    pub fn tree_markers(&self) -> (&'static str, &'static str, &'static str) {
        match self.config.markers {
            Markers::Arrows => ("▾ ", "▸ ", "  "),
            Markers::Ascii => ("- ", "+ ", "  "),
        }
    }

    /// Settings the link graph is built with, derived from the live config so
    /// `*set show_hidden true` changes the map too.
    pub fn graph_options(&self) -> graph::Options {
        graph::Options {
            ignore: self.config.search_ignore.clone(),
            show_hidden: self.config.show_hidden,
            prose_extensions: self.config.prose_extensions.clone(),
            max_ambiguity: self.config.graph_max_ambiguity,
        }
    }

    /// Build the map and hand the screen over to it.
    ///
    /// Built fresh every time rather than cached: it is the only way to be
    /// sure it reflects files edited since the last look, and there is no
    /// invalidation scheme that would be simpler than just rebuilding.
    /// Synchronous, so a large project pauses here — see `graph::build`.
    fn open_map(&mut self) {
        let root = self.root().to_path_buf();
        let options = self.graph_options();
        let view = ProjectMap::build(&root, &options);
        self.status = if view.graph.nodes.is_empty() {
            "nothing on the map yet".into()
        } else {
            format!("project map — {}", view.summary())
        };
        self.project_map = Some(view);
    }

    /// Move the cursor to a path and start editing it. Used when leaving the
    /// map with Enter. A path outside the project is reported rather than
    /// opened, since the tree has nowhere to put it.
    pub fn open_path(&mut self, path: &Path) {
        if self.reveal(path) {
            if matches!(self.preview, Preview::Buffer { .. }) {
                self.focus_editor();
            }
            self.status = format!("opened {}", display_name(path));
        } else {
            self.status = format!("{} is not in this project", display_name(path));
        }
    }

    fn search_opts(&self) -> search::Opts {
        search::Opts {
            max_results: self.config.max_search_results,
            ignore: self.config.search_ignore.clone(),
            show_hidden: self.config.show_hidden,
        }
    }

    // ---- selection & preview ---------------------------------------------

    /// Re-flatten the tree and keep the cursor on the same *file*, not the same
    /// index.
    ///
    /// This is the difference between a refresh feeling stable and feeling like
    /// the list jumped: rows shift whenever anything above them is added,
    /// removed, expanded or collapsed, so the path is remembered and looked up
    /// again afterwards. Only when the file is genuinely gone does the index
    /// get clamped instead.
    fn rebuild_rows(&mut self) {
        let want = self.selected_path().map(Path::to_path_buf);
        self.rows = self.tree.flatten();
        if let Some(want) = want {
            if let Some(i) = self.rows.iter().position(|r| r.path == want) {
                self.selected = i;
            } else {
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            }
        }
        self.sync_preview();
    }

    /// Open every folder on the way to `path` and put the cursor on it.
    ///
    /// Necessary because the tree loads lazily: a file three directories deep
    /// does not exist as a row until its parents have been expanded. Returns
    /// false when the path is outside the project or could not be found, and
    /// still syncs the preview either way so the pane never shows something
    /// stale.
    fn reveal(&mut self, path: &Path) -> bool {
        let root = self.tree.root_path().to_path_buf();
        let Ok(rel) = path.strip_prefix(&root) else {
            return false;
        };
        let mut cur = root;
        let parts: Vec<_> = rel.components().collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            cur.push(part);
            self.tree.expand(&cur);
        }
        self.rows = self.tree.flatten();
        match self.rows.iter().position(|r| r.path == path) {
            Some(i) => {
                self.selected = i;
                self.sync_preview();
                true
            }
            None => {
                self.sync_preview();
                false
            }
        }
    }

    /// Point the preview at whatever the cursor is now on, and reset its
    /// scroll.
    ///
    /// Called after every cursor move. This is the single seam between the two
    /// panes — the tree does not know about the preview and vice versa; they
    /// are coupled only here.
    fn sync_preview(&mut self) {
        self.preview_scroll = 0;
        let Some(row) = self.selected_row().cloned() else {
            self.preview = Preview::Empty;
            return;
        };
        if row.is_dir {
            if row.unreadable {
                self.preview = Preview::Unreadable(format!("cannot read {}", row.name));
                return;
            }
            let entries = fs::read_dir(&row.path).map(|d| d.count()).unwrap_or(0);
            self.preview = Preview::Directory {
                path: row.path,
                entries,
            };
            return;
        }
        self.preview = self.load_file_preview(&row.path);
    }

    /// Classify a file and, if it is text, open a buffer for it.
    ///
    /// Order matters. An already-open buffer wins over anything on disk, so a
    /// file with unsaved edits is never re-read out from under them. Then
    /// media, then the size ceiling, then a decode attempt — a file that is not
    /// valid UTF-8, or that contains a zero byte, is reported as binary rather
    /// than opened as mojibake.
    ///
    /// The `\0` check catches UTF-16 and similar, which decode as valid UTF-8
    /// often enough to slip past `from_utf8` alone.
    fn load_file_preview(&mut self, path: &Path) -> Preview {
        // A buffer already open, possibly with unsaved edits, wins over disk.
        if self.buffers.contains_key(path) {
            return Preview::Buffer {
                path: path.to_path_buf(),
                kind: text_kind(path, &self.config.prose_extensions),
            };
        }
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => return Preview::Unreadable(format!("{}: {e}", display_name(path))),
        };
        let size = meta.len();

        let kind = media::classify(path);
        if kind != media::Kind::Other && self.config.media_preview {
            return Preview::Media {
                path: path.to_path_buf(),
                kind,
                size,
            };
        }
        if size > MAX_EDIT_BYTES {
            return Preview::Binary {
                path: path.to_path_buf(),
                size,
                kind: "large file".into(),
            };
        }
        match fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => {
                    self.buffers.insert(
                        path.to_path_buf(),
                        Editor::from_str(path.to_path_buf(), &text),
                    );
                    Preview::Buffer {
                        path: path.to_path_buf(),
                        kind: text_kind(path, &self.config.prose_extensions),
                    }
                }
                _ => Preview::Binary {
                    path: path.to_path_buf(),
                    size,
                    kind: binary_kind(path).into(),
                },
            },
            Err(e) => Preview::Unreadable(format!("{}: {e}", display_name(path))),
        }
    }

    /// Decode a picture or video frame for the pane, reusing the last one when
    /// nothing about the request has changed.
    pub fn ensure_media(&mut self, path: &Path, kind: media::Kind, cols: usize, rows: usize) {
        let fresh = self
            .media
            .as_ref()
            .is_some_and(|c| c.path == path && c.cols == cols && c.rows == rows);
        if fresh {
            return;
        }
        let result = media::render(path, kind, cols, rows).map_err(|e| format!("{e:#}"));
        self.media = Some(MediaCache {
            path: path.to_path_buf(),
            cols,
            rows,
            result,
        });
    }

    /// Move the tree cursor by `delta` rows, clamped to the list. Only syncs
    /// the preview when the cursor actually moved, so holding a key at the end
    /// of the list does not re-read the same file repeatedly.
    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        if next != self.selected {
            self.selected = next;
            self.sync_preview();
        }
    }

    fn select_index(&mut self, i: usize) {
        let i = i.min(self.rows.len().saturating_sub(1));
        if i != self.selected {
            self.selected = i;
            self.sync_preview();
        }
    }

    // ---- key dispatch -----------------------------------------------------

    /// The single entry point for input. Dispatches in strict priority order:
    /// map, then mode, then focus.
    ///
    /// See the module docs for the take-and-replace pattern: the mode is moved
    /// out and `Normal` left in its place, so a handler that does nothing
    /// closes its overlay, and one that wants to stay open has to say so.
    pub fn on_key(&mut self, key: KeyEvent) {
        // The map takes the whole screen while it is open, and every key
        // with it.
        if self.project_map.is_some() {
            return self.on_map_key(key);
        }
        // Take the mode out so handlers can own it without fighting the borrow
        // checker, then put back whatever they leave behind.
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Help(scroll) => self.on_help_key(scroll, key),
            Mode::Prompt(p) => self.on_prompt_key(p, key),
            Mode::Confirm(c) => self.on_confirm_key(c, key),
            Mode::Bar(b) => self.on_bar_key(b, key),
            Mode::Settings(s) => self.on_settings_key(s, key),
            Mode::Normal => match self.focus {
                Focus::Tree => self.on_tree_key(key),
                Focus::Editor => self.on_editor_key(key),
            },
        }
    }

    /// Arrows scroll the keymap; anything else puts it away.
    fn on_help_key(&mut self, scroll: usize, key: KeyEvent) {
        let page = self.last_tree_height.max(1);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let next = match key.code {
            KeyCode::Up if shift => 0,
            KeyCode::Down if shift => usize::MAX,
            KeyCode::Char('I') => 0,
            KeyCode::Char('K') => usize::MAX,
            KeyCode::Up | KeyCode::Char('i') => scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('k') => scroll + 1,
            KeyCode::PageUp => scroll.saturating_sub(page),
            KeyCode::PageDown => scroll + page,
            KeyCode::Home => 0,
            KeyCode::End => usize::MAX,
            _ => return,
        };
        self.mode = Mode::Help(next);
    }

    /// Forward a key to the map and act on what it asks for. The view
    /// handles its own navigation; only opening a file and closing the map
    /// need application state.
    fn on_map_key(&mut self, key: KeyEvent) {
        let action = match self.project_map.as_mut() {
            Some(view) => view.on_key(key),
            None => return,
        };
        match action {
            projectmap::Action::None => {}
            projectmap::Action::Close => {
                self.project_map = None;
                self.status = "back to the tree".into();
                return;
            }
            projectmap::Action::Open(path) => {
                self.project_map = None;
                self.open_path(&path);
                return;
            }
        }
        // The counts move as filters change, so keep them in front of you.
        if let Some(view) = self.project_map.as_ref() {
            self.status = format!("project map — {}", view.summary());
        }
    }

    /// Keys for the tree pane. Single letters are free here — unlike the
    /// editor, nothing is being typed — which is why `n`, `r`, `d` and friends
    /// can be bare.
    fn on_tree_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.last_tree_height.saturating_sub(1).max(1) as isize;
        match key.code {
            KeyCode::Char('s') if ctrl => self.save_active(),
            KeyCode::Char('q') if ctrl => self.request_quit(),
            KeyCode::Char('f') if ctrl => self.open_bar(false),
            KeyCode::Char('p') if ctrl => self.open_bar(true),
            KeyCode::Char('c') if ctrl => self.copy_selection(),
            KeyCode::Char('v') if ctrl => self.paste_clipboard(),
            KeyCode::Char('b') if ctrl => self.toggle_tree_pane(),
            // Shift and an arrow goes all the way. `I` and `K` are the same
            // gesture for a keyboard with no arrows — see the note on `i j k l`
            // in the module docs.
            KeyCode::Up if shift => self.select_index(0),
            KeyCode::Down if shift => self.select_index(usize::MAX),
            KeyCode::Char('I') => self.select_index(0),
            KeyCode::Char('K') => self.select_index(usize::MAX),
            KeyCode::Up | KeyCode::Char('i') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('k') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::Home => self.select_index(0),
            KeyCode::End => self.select_index(usize::MAX),
            KeyCode::Right | KeyCode::Char('l') => self.activate(),
            KeyCode::Enter => self.toggle_or_open(),
            KeyCode::Left | KeyCode::Char('j') => self.collapse_or_parent(),
            KeyCode::Tab => self.focus_editor(),
            KeyCode::Char('/') => self.open_bar(false),
            KeyCode::Char(',') | KeyCode::F(2) => self.open_settings(),
            KeyCode::Char('m') => self.open_map(),
            KeyCode::Char('n') => self.begin_prompt(PromptKind::New),
            KeyCode::Char('r') => self.begin_prompt(PromptKind::Rename),
            KeyCode::Char('d') => self.begin_delete(),
            KeyCode::Char('.') => self.toggle_hidden(),
            KeyCode::F(5) => self.refresh(),
            KeyCode::Char('?') => self.mode = Mode::Help(0),
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            _ => {}
        }
    }

    /// Keys for the preview pane.
    ///
    /// The escape hatches come first and return early, so nothing that leaves
    /// the pane or acts on the file can be swallowed as text input. That is
    /// also why `Ctrl+P` exists as a second binding for the command bar: a
    /// bare `:` in the editor is a colon being typed.
    ///
    /// Below that the function forks on `read_mode`: reading scrolls, editing
    /// types.
    fn on_editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let tab_width = self.config.tab_width;
        let page = self.last_edit_height.saturating_sub(1).max(1);

        // Keys that leave the pane or act on the file come first, so they can
        // never be swallowed as text input.
        match key.code {
            KeyCode::Char('s') if ctrl => return self.save_active(),
            KeyCode::Char('q') if ctrl => return self.request_quit(),
            KeyCode::Char('f') if ctrl => return self.open_bar(false),
            // Ctrl+P reaches the command bar without leaving the editor, where
            // a bare `:` is just a character being typed.
            KeyCode::Char('p') if ctrl => return self.open_bar(true),
            KeyCode::Char('b') if ctrl => return self.toggle_tree_pane(),
            // Esc means "back to the tree" even when the tree is folded away,
            // so it brings the pane back rather than handing the keyboard to
            // something that is not on screen.
            KeyCode::Esc => {
                self.focus_tree();
                self.status = "back to tree".into();
                return;
            }
            _ => {}
        }

        if self.read_mode {
            return self.on_read_key(key, page);
        }

        let Some(ed) = self.active_buffer_mut() else {
            self.focus = Focus::Tree;
            return;
        };
        match key.code {
            KeyCode::Char('z') if ctrl => {
                if !ed.undo() {
                    self.status = "nothing to undo".into();
                }
            }
            KeyCode::Char('y') if ctrl => {
                if !ed.redo() {
                    self.status = "nothing to redo".into();
                }
            }
            KeyCode::Char('k') if ctrl => ed.delete_line(),
            KeyCode::Left if ctrl => ed.move_word_left(),
            KeyCode::Right if ctrl => ed.move_word_right(),
            KeyCode::Home if ctrl => ed.move_doc_start(),
            KeyCode::End if ctrl => ed.move_doc_end(),
            KeyCode::Left => ed.move_left(),
            KeyCode::Right => ed.move_right(),
            KeyCode::Up => ed.move_up(),
            KeyCode::Down => ed.move_down(),
            KeyCode::Home => ed.move_home(),
            KeyCode::End => ed.move_end(),
            KeyCode::PageUp => ed.page_up(page),
            KeyCode::PageDown => ed.page_down(page),
            KeyCode::Enter => ed.insert_newline(),
            KeyCode::Backspace => ed.backspace(),
            KeyCode::Delete => ed.delete_forward(),
            KeyCode::Tab => ed.insert_tab(tab_width),
            KeyCode::Char(c) if !ctrl => ed.insert_char(c),
            _ => {}
        }
    }

    /// Reading rendered markdown, or looking at a picture: arrows scroll.
    fn on_read_key(&mut self, key: KeyEvent, page: usize) {
        let max = self.preview_len.saturating_sub(1);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Up if shift => self.preview_scroll = 0,
            KeyCode::Down if shift => self.preview_scroll = max,
            KeyCode::Char('I') => self.preview_scroll = 0,
            KeyCode::Char('K') => self.preview_scroll = max,
            KeyCode::Char('e') => {
                if matches!(self.preview, Preview::Buffer { .. }) {
                    self.read_mode = false;
                    self.status = "editing — ^S save, Esc back".into();
                }
            }
            KeyCode::Char('/') => self.open_bar(false),
            KeyCode::Up | KeyCode::Char('i') => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('k') => {
                self.preview_scroll = (self.preview_scroll + 1).min(max)
            }
            KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(page),
            KeyCode::PageDown => self.preview_scroll = (self.preview_scroll + page).min(max),
            KeyCode::Home => self.preview_scroll = 0,
            KeyCode::End => self.preview_scroll = max,
            _ => {}
        }
    }

    // ---- the bar ----------------------------------------------------------

    /// Open the bar, discarding whatever was there before.
    ///
    /// `as_command` only decides what is already typed into it: the sigil, or
    /// nothing. After that the bar decides for itself, keystroke by keystroke,
    /// which of the two things it is.
    fn open_bar(&mut self, as_command: bool) {
        let input = if as_command {
            COMMAND_SIGIL.to_string()
        } else {
            String::new()
        };
        self.mode = Mode::Bar(Bar::new(input));
        self.status = if as_command {
            "command — Tab completes | Enter runs | Esc closes".into()
        } else {
            format!("search — or {COMMAND_SIGIL} for a command, like {COMMAND_SIGIL}copy a to b")
        };
    }

    /// Keys for the search and command bar.
    ///
    /// Two structural things to know when editing this:
    ///
    /// - Branches that end with `return` leave `self.mode` as the `Normal` that
    ///   `on_key` swapped in — that is how Esc and Enter close the bar. Every
    ///   other branch has to put the bar back.
    /// - Ctrl chords are handled up front and never fall through, so a chord
    ///   cannot leak its letter into the query.
    ///
    /// Text edits fall out of the `match` to the bottom, where a search bar
    /// re-runs its query and previews the top hit.
    fn on_bar_key(&mut self, mut b: Bar, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            // Ctrl+Q means quit everywhere, including here. No other chord is
            // allowed to fall through and type its letter into the query.
            if key.code == KeyCode::Char('q') {
                self.request_quit();
            } else {
                self.mode = Mode::Bar(b);
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.status = "closed".into();
                return;
            }
            KeyCode::Enter => {
                if b.is_command() {
                    self.run_command(b.command());
                } else if let Some(hit) = b.results.get(b.selected).cloned() {
                    self.jump_to(&hit);
                    self.status = display_name(&hit.path);
                } else {
                    self.status = "no matches".into();
                }
                return;
            }
            KeyCode::Up => {
                b.selected = b.selected.saturating_sub(1);
                self.preview_hit(&b);
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Down => {
                if !b.results.is_empty() {
                    b.selected = (b.selected + 1).min(b.results.len() - 1);
                }
                self.preview_hit(&b);
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Tab => {
                if b.is_command() {
                    complete_command(&mut b, self.tree.root_path(), self.config.show_hidden);
                }
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Backspace => {
                if b.cursor > 0 {
                    let s = char_byte(&b.input, b.cursor - 1);
                    let e = char_byte(&b.input, b.cursor);
                    b.input.replace_range(s..e, "");
                    b.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let n = b.input.chars().count();
                if b.cursor < n {
                    let s = char_byte(&b.input, b.cursor);
                    let e = char_byte(&b.input, b.cursor + 1);
                    b.input.replace_range(s..e, "");
                }
            }
            KeyCode::Left => b.cursor = b.cursor.saturating_sub(1),
            KeyCode::Right => {
                // At the end of the line the only thing to the right of the
                // cursor is the grey suggestion, so the arrow takes it up —
                // the same key, doing the same thing, to what is drawn there.
                // Anywhere else it moves along the text as it always did.
                if b.is_command() && b.cursor == b.input.chars().count() {
                    complete_command(&mut b, self.tree.root_path(), self.config.show_hidden);
                } else {
                    b.cursor = (b.cursor + 1).min(b.input.chars().count());
                }
            }
            KeyCode::Home => b.cursor = 0,
            KeyCode::End => b.cursor = b.input.chars().count(),
            KeyCode::Char(c) => {
                let byte = char_byte(&b.input, b.cursor);
                b.input.insert(byte, c);
                b.cursor += 1;
            }
            _ => {
                self.mode = Mode::Bar(b);
                return;
            }
        }

        // The line may have just become a command, or stopped being one.
        if b.is_command() {
            b.results.clear();
            b.searched = false;
            b.selected = 0;
        } else {
            self.run_search(&mut b);
            self.preview_hit(&b);
        }
        self.mode = Mode::Bar(b);
    }

    /// Re-run the query from scratch and reset the result cursor. Called on
    /// every keystroke — see `search`'s module docs for why that is fine.
    fn run_search(&mut self, b: &mut Bar) {
        let opts = self.search_opts();
        b.results = search::search(self.tree.root_path(), &b.input, &opts);
        b.searched = !b.input.trim().is_empty();
        b.selected = 0;
        b.scroll = 0;
    }

    /// Show the highlighted result in the preview pane without leaving the bar.
    fn preview_hit(&mut self, b: &Bar) {
        let Some(hit) = b.results.get(b.selected) else {
            return;
        };
        let hit = hit.clone();
        self.reveal(&hit.path);
        self.place_cursor_on(&hit);
    }

    /// Commit to a result: move to it and hand the keyboard to the preview.
    /// The Enter-key counterpart to [`App::preview_hit`].
    fn jump_to(&mut self, hit: &Hit) {
        self.reveal(&hit.path);
        self.place_cursor_on(hit);
        if matches!(self.preview, Preview::Buffer { .. }) {
            self.focus = Focus::Editor;
        }
    }

    /// Put the cursor on a hit, choosing rendered or raw view to suit it.
    ///
    /// A content hit forces raw source: the whole point is to see the matching
    /// line with the cursor on it, and rendered markdown has no line the hit
    /// corresponds to. A name hit leaves the file in whatever view it would
    /// normally open in.
    fn place_cursor_on(&mut self, hit: &Hit) {
        if hit.kind == HitKind::Content {
            // A match inside a note is easier to see as raw source with the
            // cursor sitting on it than as rendered prose.
            self.read_mode = false;
            let (line, col) = (hit.line, hit.col);
            if let Some(ed) = self.active_buffer_mut() {
                ed.goto(line, col);
            }
        } else {
            self.read_mode = matches!(self.preview, Preview::Buffer { kind, .. } if kind.reads_first())
                || matches!(self.preview, Preview::Media { .. });
        }
    }

    // ---- commands ---------------------------------------------------------

    /// Parse and run a `:` command.
    ///
    /// Adding one means adding an arm here and an entry in `complete_command`'s
    /// `COMMANDS` list, or it will work but never tab-complete. Handlers return
    /// `Result<String>`: a non-empty `Ok` becomes the status message, an empty
    /// one means the command already set its own, and an `Err` is shown with
    /// its full context chain.
    fn run_command(&mut self, line: &str) {
        let args = split_args(line);
        let Some(cmd) = args.first().map(String::as_str) else {
            self.status = "no command".into();
            return;
        };
        let rest = &args[1..];
        let result = match cmd {
            "set" => self.cmd_set(rest),
            "replace" | "sub" => self.cmd_replace(rest),
            "config" | "settings" => {
                self.open_settings();
                Ok(String::new())
            }
            "map" | "graph" | "web" => {
                self.open_map();
                Ok(String::new())
            }
            "w" | "write" | "save" => {
                self.save_active();
                Ok(String::new())
            }
            "q" | "quit" => {
                self.request_quit();
                Ok(String::new())
            }
            "wq" => {
                self.save_active();
                self.request_quit();
                Ok(String::new())
            }
            "help" => {
                self.mode = Mode::Help(0);
                Ok(String::new())
            }
            "reload" | "refresh" => {
                self.refresh();
                Ok(String::new())
            }
            "new" => self.cmd_new(rest, false),
            "mkdir" => self.cmd_new(rest, true),
            "delete" | "rm" => self.cmd_delete(rest),
            "copy" | "cp" => self.cmd_copy(rest),
            "line" | "go" => self.cmd_line(rest),
            // A bare number is a line number, the way `:42` is everywhere else.
            n if n.parse::<usize>().is_ok() => self.cmd_line(&args),
            other => Err(anyhow!("unknown command `{other}` — try :help")),
        };
        match result {
            Ok(msg) if !msg.is_empty() => self.status = msg,
            Ok(_) => {}
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// `:set key value`, or `:set key` to report the current value.
    ///
    /// Remaining arguments are rejoined with spaces, so a style spec like
    /// `:set theme.heading cyan bold` arrives intact.
    fn cmd_set(&mut self, args: &[String]) -> Result<String> {
        let key = args.first().ok_or_else(|| anyhow!(":set <key> <value>"))?;
        if args.len() < 2 {
            // With no value, report the current one rather than erroring.
            let value = self
                .config
                .get(key)
                .ok_or_else(|| anyhow!("unknown setting `{key}`"))?;
            return Ok(format!("{key} = {value}"));
        }
        let value = args[1..].join(" ");
        self.config.set(key, &value)?;
        self.apply_config();
        Ok(format!("{key} = {value}"))
    }

    /// `:replace find replace`. Counts first and asks before writing anything —
    /// this rewrites files on disk and cannot be undone.
    fn cmd_replace(&mut self, args: &[String]) -> Result<String> {
        if args.len() < 2 {
            return Err(anyhow!(
                ":replace <find> <replace> — quote strings containing spaces"
            ));
        }
        let find = args[0].clone();
        let replace = args[1].clone();
        let report = search::count(self.tree.root_path(), &find, &self.search_opts());
        if report.occurrences == 0 {
            return Ok(format!("`{find}` is not in this project"));
        }
        // Rewriting many files at once is worth a second look first.
        self.mode = Mode::Confirm(Confirm {
            message: format!(
                "Replace {} occurrence{} of `{find}` with `{replace}` across {} file{}?  (y/n)",
                report.occurrences,
                plural(report.occurrences),
                report.files,
                plural(report.files),
            ),
            kind: ConfirmKind::Replace { find, replace },
        });
        Ok(String::new())
    }

    fn cmd_new(&mut self, args: &[String], is_dir: bool) -> Result<String> {
        let name = args
            .first()
            .ok_or_else(|| anyhow!("give a name: :new notes/today.md"))?;
        let base = self.creation_base();
        self.create_entry(&base, name, is_dir)
    }

    /// `:copy <src> to <dst>` — `:cp` is the same command.
    ///
    /// The word `to` is the separator, and everything on each side of it is one
    /// path, so a name with spaces in it needs no quoting: `copy my notes to
    /// old work` does what it reads like. Two arguments with no `to` between
    /// them are accepted as well, since `copy a b` is unambiguous.
    ///
    /// Both paths are relative to the project root and go through
    /// [`safe_join`], so neither end can point outside the project.
    fn cmd_copy(&mut self, args: &[String]) -> Result<String> {
        let (from, into) = split_on_to(args)?;
        let root = self.tree.root_path().to_path_buf();
        let src = safe_join(&root, &from, &root)?;
        let dst = safe_join(&root, &into, &root)?;
        self.copy_entry(&src, &dst)
    }

    /// `*line 42`, `*go to 42`, or just `*42` — put the cursor on that line of
    /// the open file.
    ///
    /// Counted from 1, the way every error message and every other editor
    /// counts, and clamped to the end of the file rather than refused: asking
    /// for line 900 of a 400-line file plainly means the end.
    ///
    /// This always lands in the editor, never in the rendered view. "Line 42"
    /// is a fact about the source; the rendered version of a note has its own
    /// line count that has nothing to do with it.
    fn cmd_line(&mut self, args: &[String]) -> Result<String> {
        // `go to 42` reads better than `go 42`, so the joining word is allowed
        // here as well and simply skipped.
        let number = args
            .iter()
            .find(|a| !a.eq_ignore_ascii_case("to") && !a.eq_ignore_ascii_case("line"))
            .ok_or_else(|| anyhow!("say it like: line 42"))?;
        let wanted: usize = number
            .parse()
            .map_err(|_| anyhow!("`{number}` is not a line number"))?;
        if wanted == 0 {
            return Err(anyhow!("lines are counted from 1"));
        }
        if !matches!(self.preview, Preview::Buffer { .. }) {
            return Err(anyhow!("no file open to jump inside"));
        }

        self.read_mode = false;
        self.focus = Focus::Editor;
        let Some(ed) = self.active_buffer_mut() else {
            return Err(anyhow!("no file open to jump inside"));
        };
        let last = ed.line_count().saturating_sub(1);
        let line = (wanted - 1).min(last);
        ed.goto(line, 0);
        Ok(format!("line {}", line + 1))
    }

    /// One notch of the mouse wheel, over the pane at `column`.
    ///
    /// Always exactly one line. The wheel is the one input where a terminal
    /// will happily send three of something for one flick of a finger, and a
    /// note that jumps three lines at a time is hard to read against.
    ///
    /// What "one line" means depends on the pane. A rendered note or a picture
    /// has no cursor, so the view itself moves. The editor does have one, and
    /// its view is tied to it, so the cursor moves instead — which scrolls the
    /// view once it reaches an edge, and never leaves you looking at somewhere
    /// you cannot type.
    pub fn on_scroll(&mut self, down: bool, column: u16) {
        let step: isize = if down { 1 } else { -1 };
        if self.over_tree(column) {
            // The preview follows the tree cursor, so moving the view on its
            // own would only be undone on the next frame.
            self.move_selection(step);
            return;
        }
        if self.read_mode || !matches!(self.preview, Preview::Buffer { .. }) {
            let max = self.preview_len.saturating_sub(1);
            self.preview_scroll = if down {
                (self.preview_scroll + 1).min(max)
            } else {
                self.preview_scroll.saturating_sub(1)
            };
        } else if let Some(ed) = self.active_buffer_mut() {
            if down {
                ed.move_down();
            } else {
                ed.move_up();
            }
        }
    }

    /// Whether `column` fell inside the tree pane as it was last drawn.
    ///
    /// False whenever the tree is not on screen, which covers both `Ctrl+B`
    /// and the very first frame, before `ui` has said where anything is.
    fn over_tree(&self, column: u16) -> bool {
        matches!(self.last_tree_cols, Some((x0, x1)) if column >= x0 && column < x1)
    }

    /// `Ctrl+C`: remember what the cursor is on. Nothing is read or written
    /// until the paste.
    fn copy_selection(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.path == self.tree.root_path() {
            self.status = "cannot copy the project root".into();
            return;
        }
        self.status = format!("copied {} — ^V to paste", row.name);
        self.clipboard = Some(row.path);
    }

    /// `Ctrl+V`: drop the clipboard into the folder the cursor is in.
    ///
    /// Where `:copy` refuses to write over something — you named that
    /// destination, so a collision means you were wrong about it — a paste
    /// picks the next free name instead. The destination here is implied
    /// rather than stated, and pasting into the folder you copied from is the
    /// ordinary way to duplicate a file; erroring there would make the gesture
    /// useless.
    fn paste_clipboard(&mut self) {
        let Some(src) = self.clipboard.clone() else {
            self.status = "nothing copied — ^C first".into();
            return;
        };
        let base = self.creation_base();
        let dst = match free_name(&base, &src) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("{e:#}");
                return;
            }
        };
        match self.copy_entry(&src, &dst) {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Copy a file, or a whole folder, to a new path inside the project.
    ///
    /// Naming an existing folder as the destination copies *into* it, which is
    /// what `copy README.md to notes` reads like. Anything else is the new name
    /// itself, and an existing one is never written over.
    fn copy_entry(&mut self, src: &Path, dst: &Path) -> Result<String> {
        let meta = fs::symlink_metadata(src)
            .with_context(|| format!("cannot copy {}", display_name(src)))?;
        // `copy a to notes` means "put a in notes", not "rename a to notes".
        let dst = if dst.is_dir() {
            match src.file_name() {
                Some(name) => dst.join(name),
                None => return Err(anyhow!("cannot copy {}", display_name(src))),
            }
        } else {
            dst.to_path_buf()
        };
        if dst == src {
            return Err(anyhow!("{} is already there", display_name(src)));
        }
        if dst.exists() {
            return Err(anyhow!("{} already exists", display_name(&dst)));
        }
        // Copying a folder into itself would walk into what it is writing.
        if meta.is_dir() && dst.starts_with(src) {
            return Err(anyhow!("cannot copy {} into itself", display_name(src)));
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }

        if meta.is_dir() {
            copy_tree(src, &dst)
        } else {
            fs::copy(src, &dst).map(|_| ())
        }
        .with_context(|| {
            format!(
                "cannot copy {} to {}",
                display_name(src),
                display_name(&dst)
            )
        })?;

        self.tree.refresh_all();
        self.reveal(&dst);
        Ok(format!(
            "copied {} to {}",
            display_name(src),
            display_name(&dst)
        ))
    }

    /// `:delete`, or `:delete <path>`; `:rm` is the same command.
    ///
    /// With no argument it takes whatever the tree cursor is on, which is what
    /// `d` does. Opening a file moves that cursor onto it, so a bare `:delete`
    /// from inside the editor removes the file being edited — which is most of
    /// why this exists as a command as well as a key, since `d` in the editor
    /// is just the letter d.
    ///
    /// An argument is a path relative to the *project root*, not to the cursor,
    /// so `:delete notes/old.md` means the same thing wherever it is typed.
    /// [`safe_join`] keeps it inside the project.
    ///
    /// Like `d`, this only asks the question. Nothing is removed until `y`.
    fn cmd_delete(&mut self, args: &[String]) -> Result<String> {
        let path = match args.first() {
            None => self
                .selected_row()
                .map(|r| r.path.clone())
                .ok_or_else(|| anyhow!("nothing selected"))?,
            Some(name) => {
                let root = self.tree.root_path().to_path_buf();
                safe_join(&root, name, &root)?
            }
        };
        self.arm_delete(&path)?;
        Ok(String::new())
    }

    /// Re-derive everything a settings change can affect.
    ///
    /// Must be called after any successful `Config::set`, or the change sits in
    /// the config struct without reaching the screen. Rebuilds the palette and
    /// highlighter, re-reads the tree if dotfile visibility changed, and drops
    /// the media cache since a size change invalidates whatever was drawn.
    fn apply_config(&mut self) {
        self.palette = Palette::from_theme(&self.config.theme);
        let (hl, _) = Highlighter::with_theme(&self.config.syntax_theme);
        self.highlighter = hl;
        if self.tree.show_hidden() != self.config.show_hidden {
            self.tree.set_show_hidden(self.config.show_hidden);
            self.rebuild_rows();
        }
        // A media size change invalidates whatever was drawn.
        self.media = None;
    }

    // ---- settings area ----------------------------------------------------

    fn open_settings(&mut self) {
        self.mode = Mode::Settings(Settings::default());
        self.status = "settings — Enter to change, ^S to write tiny.conf, Esc to close".into();
    }

    /// Keys for the settings overlay.
    ///
    /// Two states in one function, separated by whether `s.editing` is set:
    /// navigating the list, or typing into one row's value. Taking the buffer
    /// out with `take()` at the top means each branch has to put it back to
    /// stay in edit mode — the same "closing is the default" pattern as the
    /// mode dispatch itself.
    ///
    /// `Ctrl+S` writes `tiny.conf` and works in both states, since finishing a
    /// value and immediately saving is the obvious thing to want.
    fn on_settings_key(&mut self, mut s: Settings, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let index = Config::settings_index();
        let last = index.len().saturating_sub(1);

        // Editing a value: the keys go into the field.
        if let Some(mut buf) = s.editing.take() {
            if ctrl {
                if key.code == KeyCode::Char('s') {
                    match self.config.save() {
                        Ok(p) => self.status = format!("wrote {}", p.display()),
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                s.editing = Some(buf);
                self.mode = Mode::Settings(s);
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    self.status = "unchanged".into();
                }
                KeyCode::Enter => {
                    let key_name = index[s.selected.min(last)].0;
                    match self.config.set(key_name, &buf) {
                        Ok(()) => {
                            self.apply_config();
                            self.status = format!("{key_name} = {buf}");
                        }
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                KeyCode::Backspace => {
                    if s.cursor > 0 {
                        let a = char_byte(&buf, s.cursor - 1);
                        let b = char_byte(&buf, s.cursor);
                        buf.replace_range(a..b, "");
                        s.cursor -= 1;
                    }
                    s.editing = Some(buf);
                }
                KeyCode::Left => {
                    s.cursor = s.cursor.saturating_sub(1);
                    s.editing = Some(buf);
                }
                KeyCode::Right => {
                    s.cursor = (s.cursor + 1).min(buf.chars().count());
                    s.editing = Some(buf);
                }
                KeyCode::Char(c) => {
                    let byte = char_byte(&buf, s.cursor);
                    buf.insert(byte, c);
                    s.cursor += 1;
                    s.editing = Some(buf);
                }
                _ => s.editing = Some(buf),
            }
            self.mode = Mode::Settings(s);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.status = "closed".into();
                return;
            }
            KeyCode::Char('s') if ctrl => match self.config.save() {
                Ok(p) => self.status = format!("wrote {}", p.display()),
                Err(e) => self.status = format!("{e:#}"),
            },
            KeyCode::Up if shift => s.selected = 0,
            KeyCode::Down if shift => s.selected = last,
            KeyCode::Char('I') => s.selected = 0,
            KeyCode::Char('K') => s.selected = last,
            KeyCode::Up | KeyCode::Char('i') => s.selected = s.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('k') => s.selected = (s.selected + 1).min(last),
            KeyCode::Home => s.selected = 0,
            KeyCode::End => s.selected = last,
            KeyCode::Enter => {
                let key_name = index[s.selected.min(last)].0;
                let current = self.config.get(key_name).unwrap_or_default();
                s.cursor = current.chars().count();
                s.editing = Some(current);
            }
            _ => {}
        }
        self.mode = Mode::Settings(s);
    }

    // ---- prompts & confirmations -----------------------------------------

    /// Keys for the status-bar text prompt. Ctrl chords are ignored outright
    /// rather than acted on — there is nothing useful to do with them here, and
    /// letting one through would type its letter into a filename.
    fn on_prompt_key(&mut self, mut p: Prompt, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode = Mode::Prompt(p);
            return;
        }
        match key.code {
            KeyCode::Esc => self.status = "cancelled".into(),
            KeyCode::Enter => self.commit_prompt(p),
            KeyCode::Backspace => {
                if p.cursor > 0 {
                    let b = char_byte(&p.input, p.cursor - 1);
                    let e = char_byte(&p.input, p.cursor);
                    p.input.replace_range(b..e, "");
                    p.cursor -= 1;
                }
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Delete => {
                let n = p.input.chars().count();
                if p.cursor < n {
                    let b = char_byte(&p.input, p.cursor);
                    let e = char_byte(&p.input, p.cursor + 1);
                    p.input.replace_range(b..e, "");
                }
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Left => {
                p.cursor = p.cursor.saturating_sub(1);
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Right => {
                p.cursor = (p.cursor + 1).min(p.input.chars().count());
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Home => {
                p.cursor = 0;
                self.mode = Mode::Prompt(p);
            }
            KeyCode::End => {
                p.cursor = p.input.chars().count();
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Char(c) => {
                let b = char_byte(&p.input, p.cursor);
                p.input.insert(b, c);
                p.cursor += 1;
                self.mode = Mode::Prompt(p);
            }
            _ => self.mode = Mode::Prompt(p),
        }
    }

    /// Keys for a yes/no question. Only `y` acts; `n` and Esc cancel, and
    /// anything else leaves the question standing rather than guessing.
    fn on_confirm_key(&mut self, c: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => match c.kind {
                ConfirmKind::Delete(path) => self.do_delete(&path),
                ConfirmKind::QuitUnsaved => self.should_quit = true,
                ConfirmKind::Replace { find, replace } => self.do_replace(&find, &replace),
            },
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.status = "cancelled".into()
            }
            _ => self.mode = Mode::Confirm(c),
        }
    }

    /// Carry out a confirmed project-wide replace.
    ///
    /// Files changed underneath any open buffer, so clean buffers are dropped
    /// and will re-read from disk on next view. Dirty ones are kept — their
    /// unsaved edits are worth more than consistency with a file the user has
    /// already diverged from.
    fn do_replace(&mut self, find: &str, replace: &str) {
        let opts = self.search_opts();
        match search::replace_all(self.tree.root_path(), find, replace, &opts) {
            Ok(report) => {
                // Files changed underneath any open buffer, so drop the clean
                // ones and let them re-read.
                self.buffers.retain(|_, e| e.dirty);
                self.tree.refresh_all();
                self.rebuild_rows();
                self.status = format!(
                    "replaced {} occurrence{} in {} file{}",
                    report.occurrences,
                    plural(report.occurrences),
                    report.files,
                    plural(report.files)
                );
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    // ---- actions ----------------------------------------------------------

    /// Right-arrow on the tree: expand a closed folder, step into an
    /// already-open one, or focus the preview for a file.
    ///
    /// This is the "inwards" key, and the mirror of
    /// [`App::collapse_or_parent`]: holding it down walks you down a branch.
    /// Enter is deliberately different — see [`App::toggle_or_open`].
    fn activate(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir {
            if row.expanded {
                self.move_selection(1);
            } else {
                self.tree.expand(&row.path);
                self.rebuild_rows();
            }
        } else {
            self.focus_editor();
        }
    }

    /// Enter on the tree: open a closed folder, close an open one, or focus
    /// the preview for a file.
    ///
    /// Unlike [`App::activate`], the cursor never moves. Pressing Enter twice
    /// on a folder leaves the tree exactly as it was found, which is what most
    /// file browsers do and what the key reads as — one thing, toggled.
    fn toggle_or_open(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir {
            self.tree.toggle(&row.path);
            self.rebuild_rows();
        } else {
            self.focus_editor();
        }
    }

    /// Hand the keyboard to the preview pane, choosing rendered or raw view by
    /// file kind: prose and markdown open to be read, code opens to be typed
    /// into. Does nothing for a directory, and explains itself for a binary.
    fn focus_editor(&mut self) {
        match &self.preview {
            Preview::Buffer { kind, .. } => {
                // Markdown opens rendered; code goes straight into the editor,
                // since there is nothing else to show for it.
                self.read_mode = kind.reads_first();
                self.focus = Focus::Editor;
                self.status = if self.read_mode {
                    "reading — e to edit, Esc back".into()
                } else {
                    "editing — ^S save, Esc back".into()
                };
            }
            Preview::Media { .. } => {
                self.read_mode = true;
                self.focus = Focus::Editor;
                self.status = "viewing — Esc back".into();
            }
            Preview::Binary { kind, .. } => {
                self.status = format!("{kind} — not editable");
            }
            _ => {}
        }
    }

    /// Left-arrow: close an open folder, or jump to the parent of anything
    /// else. Two behaviours on one key, which is what makes it feel like
    /// "outwards".
    fn collapse_or_parent(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir && row.expanded {
            self.tree.collapse(&row.path);
            self.rebuild_rows();
            return;
        }
        let Some(parent) = row.path.parent() else {
            return;
        };
        if let Some(i) = self.rows.iter().position(|r| r.path == parent) {
            self.select_index(i);
        }
    }

    /// `Ctrl+B`: fold the tree away, or bring it back.
    ///
    /// The keyboard cannot sit in a pane that is not on screen, so folding
    /// moves it to the preview and unfolding puts it back where it was. Press
    /// the key twice and nothing has changed, which is what a toggle should
    /// mean.
    fn toggle_tree_pane(&mut self) {
        if self.tree_hidden {
            self.tree_hidden = false;
            // Back to whichever pane had it, so the key is a true toggle.
            self.focus = self.focus_before_hide;
            self.status = "tree back — ^B to hide".into();
        } else {
            self.tree_hidden = true;
            self.focus_before_hide = self.focus;
            self.focus = Focus::Editor;
            self.status = "tree hidden — ^B to bring it back".into();
        }
    }

    /// Put the tree on screen and give it the keyboard. What Esc means from
    /// the preview, whether or not the tree was folded away — "back to the
    /// tree" has to end up at the tree either way.
    fn focus_tree(&mut self) {
        self.tree_hidden = false;
        self.focus = Focus::Tree;
    }

    /// `.`: show or hide dotfiles. Writes through to the config so the setting
    /// is consistent with `:set show_hidden`, but does not persist it — that
    /// still needs an explicit save.
    fn toggle_hidden(&mut self) {
        let next = !self.tree.show_hidden();
        self.tree.set_show_hidden(next);
        self.config.show_hidden = next;
        self.rebuild_rows();
        self.status = if next {
            "showing hidden files".into()
        } else {
            "hiding hidden files".into()
        };
    }

    /// Re-read the project from disk. The manual replacement for a file
    /// watcher (see `tree`'s module docs). Clean buffers are dropped so they
    /// pick up external changes; dirty ones are kept.
    fn refresh(&mut self) {
        self.tree.refresh_all();
        // Drop clean buffers so they re-read from disk; keep unsaved work.
        self.buffers.retain(|_, e| e.dirty);
        self.media = None;
        self.rebuild_rows();
        self.status = "refreshed".into();
    }

    /// Write the current buffer. Reports "nothing to save" for a non-text
    /// preview and "no changes" for a clean one, rather than silently doing
    /// nothing in either case.
    pub fn save_active(&mut self) {
        let Some(ed) = self.active_buffer_mut() else {
            self.status = "nothing to save".into();
            return;
        };
        if !ed.dirty {
            self.status = "no changes".into();
            return;
        }
        let name = display_name(&ed.path.clone());
        match ed.save() {
            Ok(()) => self.status = format!("saved {name}"),
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// Quit, or ask first if anything is unsaved. The prompt names every dirty
    /// file, since "discard changes?" is unanswerable without knowing which.
    fn request_quit(&mut self) {
        let dirty = self.dirty_buffers();
        if dirty.is_empty() {
            self.should_quit = true;
            return;
        }
        let names: Vec<String> = dirty.iter().map(|p| display_name(p)).collect();
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::QuitUnsaved,
            message: format!("Discard unsaved changes to {}?  (y/n)", names.join(", ")),
        });
    }

    /// Where a new file or folder should go: the selected directory, or the
    /// parent of the selected file. Matches what most file managers do, and
    /// means you rarely have to type a path.
    fn creation_base(&self) -> PathBuf {
        match self.selected_row() {
            Some(r) if r.is_dir => r.path.clone(),
            Some(r) => r
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.tree.root_path().to_path_buf()),
            None => self.tree.root_path().to_path_buf(),
        }
    }

    /// Open a naming prompt. Rename pre-fills the current name and refuses to
    /// touch the project root, which has no parent inside the tree to rename
    /// it within.
    fn begin_prompt(&mut self, kind: PromptKind) {
        let (label, input, base) = match kind {
            PromptKind::New => ("New".to_string(), String::new(), self.creation_base()),
            PromptKind::Rename => {
                let Some(row) = self.selected_row() else {
                    return;
                };
                if row.path == self.tree.root_path() {
                    self.status = "cannot rename the project root".into();
                    return;
                }
                let parent = row
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.tree.root_path().to_path_buf());
                ("Rename".to_string(), row.name.clone(), parent)
            }
        };
        let cursor = input.chars().count();
        self.mode = Mode::Prompt(Prompt {
            kind,
            label,
            input,
            cursor,
            base,
        });
    }

    /// Act on a confirmed prompt. An empty name cancels rather than erroring —
    /// pressing Enter on a blank field clearly means "never mind".
    fn commit_prompt(&mut self, p: Prompt) {
        let name = p.input.trim().to_string();
        if name.is_empty() {
            self.status = "cancelled".into();
            return;
        }
        let result = match p.kind {
            // The same rule the command line uses: a name with an extension is
            // a file, one without is a folder.
            PromptKind::New => {
                let is_dir = !project::names_a_file(&name, Path::new(&name));
                self.create_entry(&p.base, &name, is_dir)
            }
            PromptKind::Rename => self.rename_selected(&p.base, &name),
        };
        match result {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Create a file or directory. A name containing separators creates the
    /// intermediate directories too, so `notes/2026/today.md` works in one go.
    fn create_entry(&mut self, base: &Path, name: &str, is_dir: bool) -> Result<String> {
        let target = safe_join(base, name, self.tree.root_path())?;
        if target.exists() {
            return Err(anyhow!("{} already exists", display_name(&target)));
        }
        if is_dir {
            fs::create_dir_all(&target)
                .with_context(|| format!("cannot create {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            fs::write(&target, "")
                .with_context(|| format!("cannot create {}", target.display()))?;
        }
        self.tree.refresh_all();
        if is_dir {
            self.tree.expand(&target);
        }
        self.reveal(&target);
        Ok(format!("created {}", display_name(&target)))
    }

    /// Rename the selected entry, moving any open buffer with it so unsaved
    /// edits follow the file to its new name. Refuses to overwrite an existing
    /// path.
    fn rename_selected(&mut self, base: &Path, name: &str) -> Result<String> {
        let Some(row) = self.selected_row().cloned() else {
            return Err(anyhow!("nothing selected"));
        };
        let target = safe_join(base, name, self.tree.root_path())?;
        if target == row.path {
            return Ok("unchanged".into());
        }
        if target.exists() {
            return Err(anyhow!("{} already exists", display_name(&target)));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&row.path, &target)
            .with_context(|| format!("cannot rename {}", display_name(&row.path)))?;

        // Carry any open buffer, and its unsaved edits, to the new path.
        if let Some(mut ed) = self.buffers.remove(&row.path) {
            ed.path = target.clone();
            self.buffers.insert(target.clone(), ed);
        }
        self.tree.refresh_all();
        self.reveal(&target);
        Ok(format!("renamed to {}", display_name(&target)))
    }

    /// Ask before deleting. The message counts a directory's entries first,
    /// because `remove_dir_all` takes everything underneath and the user should
    /// see the number before pressing `y`.
    fn begin_delete(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if let Err(e) = self.arm_delete(&row.path) {
            self.status = format!("{e:#}");
        }
    }

    /// Put up the yes/no question for removing `path`.
    ///
    /// The single gate on every delete: `d` and `:delete` both arrive here, so
    /// there is one place deciding what may be removed and one question to
    /// answer. Nothing touches the disk until [`App::do_delete`].
    fn arm_delete(&mut self, path: &Path) -> Result<()> {
        if path == self.tree.root_path() {
            return Err(anyhow!("cannot delete the project root"));
        }
        // `symlink_metadata` rather than `exists`, which follows links and so
        // reports a broken symlink as missing — that is a thing you very much
        // want to be able to delete.
        let meta = fs::symlink_metadata(path)
            .with_context(|| format!("cannot delete {}", display_name(path)))?;
        let name = display_name(path);
        // Say how much is at stake before asking — deleting a folder takes
        // everything under it.
        let message = if meta.is_dir() {
            let n = fs::read_dir(path).map(|d| d.count()).unwrap_or(0);
            format!(
                "Delete folder {} and its {} entr{}?  (y/n)",
                name,
                n,
                if n == 1 { "y" } else { "ies" }
            )
        } else {
            format!("Delete {name}?  (y/n)")
        };
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::Delete(path.to_path_buf()),
            message,
        });
        Ok(())
    }

    /// Carry out a confirmed delete, then put the cursor somewhere sensible.
    ///
    /// Buffers under the deleted path are dropped — including unsaved ones,
    /// since there is no longer a file to save them to. The cursor moves to the
    /// parent directory where it can, rather than staying on an index that now
    /// points at a different file.
    fn do_delete(&mut self, path: &Path) {
        // `symlink_metadata` again, so a link is unlinked rather than followed
        // into: deleting a shortcut must never delete what it points at.
        let is_dir = fs::symlink_metadata(path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let result = if is_dir {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match result {
            Ok(()) => {
                self.buffers.retain(|p, _| !p.starts_with(path));
                self.media = None;
                let parent = path.parent().map(Path::to_path_buf);
                self.tree.refresh_all();
                self.rows = self.tree.flatten();
                if let Some(i) = parent.and_then(|p| self.rows.iter().position(|r| r.path == p)) {
                    self.selected = i;
                } else {
                    self.selected = self.selected.min(self.rows.len().saturating_sub(1));
                }
                self.sync_preview();
                // `:delete` can be run from the editor. If the pane holding the
                // keyboard just lost its file, the tree is the only place left
                // with something to point at.
                if self.focus == Focus::Editor
                    && !matches!(self.preview, Preview::Buffer { .. } | Preview::Media { .. })
                {
                    self.focus_tree();
                }
                self.status = format!("deleted {}", display_name(path));
            }
            Err(e) => self.status = format!("delete failed: {e}"),
        }
    }
}

// ---- free helpers ---------------------------------------------------------

/// A human word for a non-text file, so the preview can say "PDF" or "archive"
/// instead of just "binary".
fn binary_kind(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("pdf") => "PDF",
        Some("zip" | "gz" | "tar" | "xz" | "zst" | "7z") => "archive",
        Some("mp3" | "wav" | "flac" | "ogg") => "audio",
        Some("so" | "dll" | "dylib" | "o" | "a") => "object file",
        _ => "binary",
    }
}

/// Filename for a status message, falling back to the full path.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `""` or `"s"`, for building status messages that read properly at n = 1.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Byte offset of character index `ci`.
fn char_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

/// Entries that could finish the path fragment `typed`, as whole paths
/// relative to the project root.
///
/// Only the one directory being typed into is read — a `read_dir`, never a
/// walk — so completing a path costs the same in a huge project as in a small
/// one. Folders come back with a separator on the end so Tab can carry straight
/// on into them.
///
/// Dotfiles stay out of the way unless they are being shown, or unless the
/// fragment already starts with a dot, which is the only time someone is
/// plainly asking for one.
fn path_candidates(root: &Path, typed: &str, show_hidden: bool) -> Vec<String> {
    // Everything up to the last separator names the folder; the rest is the
    // part being completed inside it.
    let cut = typed.rfind(std::path::is_separator).map_or(0, |i| i + 1);
    let (dir_part, fragment) = typed.split_at(cut);
    let Ok(dir) = safe_join(root, dir_part, root) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !show_hidden && !fragment.starts_with('.') {
                return None;
            }
            let tail = if e.path().is_dir() {
                std::path::MAIN_SEPARATOR_STR
            } else {
                ""
            };
            Some(format!("{dir_part}{name}{tail}"))
        })
        .collect();
    out.sort();
    out
}

/// Split `copy <src> to <dst>` on the word `to`.
///
/// Everything before the separator is one path and everything after it is the
/// other, joined back together with the spaces they were typed with — which is
/// what lets `copy my notes to old work` mean two names rather than four
/// arguments. Without a `to`, exactly two arguments are still accepted, since
/// `copy a b` cannot be read any other way.
///
/// Returns the two sides as owned strings; the error is the sentence to show
/// when it does not parse.
fn split_on_to(args: &[String]) -> Result<(String, String)> {
    const HINT: &str = "say it like: copy README.md to notes";
    let sep = args.iter().position(|a| a.eq_ignore_ascii_case("to"));
    let (left, right) = match sep {
        Some(i) => (&args[..i], &args[i + 1..]),
        None if args.len() == 2 => (&args[..1], &args[1..]),
        None => return Err(anyhow!(HINT)),
    };
    if left.is_empty() || right.is_empty() {
        return Err(anyhow!(HINT));
    }
    Ok((left.join(" "), right.join(" ")))
}

/// Copy a directory and everything under it.
///
/// Recurses on directories and copies everything else with `fs::copy`, which
/// carries permissions across on every platform. A symlink counts as
/// "everything else": its target is copied as a plain file rather than
/// followed as a directory, so a link pointing back up its own tree cannot
/// send this into a loop.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        // `DirEntry::file_type` does not follow links, which is what keeps the
        // symlink case out of the recursive branch.
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// A free path in `dir` for something named after `src`: `notes.md`, then
/// `notes copy.md`, then `notes copy 2.md`.
///
/// Only paste uses this. The words are spelled out rather than punctuated —
/// `notes copy.md`, not `notes(1).md` — because everything else the command
/// bar shows reads as English too.
fn free_name(dir: &Path, src: &Path) -> Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| anyhow!("cannot copy {}", display_name(src)))?;
    let first = dir.join(name);
    if !first.exists() {
        return Ok(first);
    }
    let name = Path::new(name);
    let stem = name
        .file_stem()
        .unwrap_or(name.as_os_str())
        .to_string_lossy();
    let ext = name.extension().map(|e| e.to_string_lossy());
    for n in 1..100 {
        let base = if n == 1 {
            format!("{stem} copy")
        } else {
            format!("{stem} copy {n}")
        };
        let candidate = match &ext {
            Some(e) => dir.join(format!("{base}.{e}")),
            None => dir.join(base),
        };
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow!("too many copies of {}", display_name(src)))
}

/// Split a command line on whitespace, keeping double-quoted runs together so
/// `:replace "old thing" "new thing"` works.
///
/// The `any` flag tracks whether the current argument was *started*, which is
/// what lets `""` produce a deliberate empty argument rather than being
/// dropped as whitespace. There is no escaping — a literal quote cannot be
/// passed, which is a known limit rather than an oversight.
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.trim().chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Fill in the rest of whatever is being typed: a command name, a setting name
/// after `set`, the word `to` in the middle of a copy, or a path.
///
/// Completes to the longest common prefix of the matches rather than to the
/// first one, so Tab on an ambiguous prefix advances as far as it safely can
/// and then stops — the shell behaviour.
///
/// The whole of `copy README.md to notes/today.md` can be typed with Tab, which
/// is the point: a command that reads like a sentence is no use if every word
/// of it has to be spelled out by hand.
fn complete_command(b: &mut Bar, root: &Path, show_hidden: bool) {
    if let Some(rest) = completion_for(b, root, show_hidden) {
        b.input.push_str(&rest);
        b.cursor = b.input.chars().count();
    }
}

/// What `Tab` would add to the line, if anything.
///
/// Split out from [`complete_command`] so the bar can draw the same answer in
/// grey before the key is pressed — a suggestion you can see is worth more than
/// one you have to guess at, and it costs one `read_dir`.
///
/// Returns only the *remainder*: every candidate is filtered by what has been
/// typed already, so the completion always begins with it.
///
/// Any new command added to [`App::run_command`] needs adding to `COMMANDS`
/// here too, or it will work but never complete — and one that takes a path
/// needs adding to `TAKES_PATHS` as well.
pub fn completion_for(b: &Bar, root: &Path, show_hidden: bool) -> Option<String> {
    const COMMANDS: &[&str] = &[
        "set", "replace", "config", "settings", "map", "write", "save", "quit", "help", "reload",
        "new", "mkdir", "delete", "rm", "copy", "cp", "line",
    ];
    const TAKES_PATHS: &[&str] = &["new", "mkdir", "delete", "rm", "copy", "cp"];

    // The sigil is not part of the command, so completion never sees it. The
    // replacement at the bottom still works on `b.input`, because whatever is
    // being completed is a suffix of both.
    let line = b.command().to_string();
    let args = split_args(&line);
    let trailing_space = line.ends_with(' ');
    // Which argument the cursor is in the middle of. A trailing space means the
    // one after the last complete argument has been started but not typed into.
    let position = if trailing_space {
        args.len()
    } else {
        args.len().saturating_sub(1)
    };
    let typed = if trailing_space {
        String::new()
    } else {
        args.last().cloned().unwrap_or_default()
    };

    let (prefix, candidates): (String, Vec<String>) = match args.first().map(String::as_str) {
        // Still on the command name itself.
        _ if position == 0 => (typed, COMMANDS.iter().map(|s| s.to_string()).collect()),
        Some("set") => (
            typed,
            Config::settings_index()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect(),
        ),
        // `copy README.md ` — what comes next is the word joining the two
        // halves, so offer that and nothing else. Anyone copying a name with
        // spaces in it can type the two letters themselves.
        Some("copy" | "cp") if position == 2 => (typed, vec!["to".to_string()]),
        Some(c) if TAKES_PATHS.contains(&c) => {
            (typed.clone(), path_candidates(root, &typed, show_hidden))
        }
        _ => return None,
    };

    let matches: Vec<&String> = candidates
        .iter()
        .filter(|c| c.starts_with(&prefix))
        .collect();
    let first = matches.first()?;
    // With several options, fill in only what they all agree on.
    let common = matches.iter().skip(1).fold((*first).clone(), |acc, m| {
        acc.chars()
            .zip(m.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    if common.len() <= prefix.len() {
        return None;
    }
    Some(common[prefix.len()..].to_string())
}

/// Join a user-typed name onto `base`, refusing anything that would escape the
/// project root — a typed `../../etc/passwd` must not create a file there.
///
/// **This is a security boundary.** Every path that comes from user input and
/// is then created, renamed to, or written must pass through here. The check
/// is lexical, done by walking components and popping on `..`, so it does not
/// depend on the target existing — and it fails closed twice over: once if
/// `..` pops past the start, and again if the normalised result does not sit
/// under `root`.
///
/// It relies on `root` being canonicalized, which `project::resolve`
/// guarantees. Symlinks inside the project are not resolved, so a symlink
/// pointing outside is not caught here.
fn safe_join(base: &Path, name: &str, root: &Path) -> Result<PathBuf> {
    let candidate = base.join(name);
    let mut normalized = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(anyhow!("path escapes the project"));
                }
            }
            std::path::Component::CurDir => {}
            c => normalized.push(c.as_os_str()),
        }
    }
    if !normalized.starts_with(root) {
        return Err(anyhow!("path escapes the project"));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Markers, Position, Side};
    use crossterm::event::KeyEventKind;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    /// A small project with one of each thing the panes have to handle.
    fn build(dir: &Path) {
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("notes/design.md"),
            "# Design Notes\n\nThe core idea is a split view. See [[architecture]].\n\n- one\n- two\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/main.py"),
            "import utils\n\n\ndef main():\n    return utils.load()\n",
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Fixture\n\nhello widget\n").unwrap();
        fs::write(dir.join("logo.png"), [0x89u8, b'P', b'N', b'G', 0, 1, 2, 3]).unwrap();
    }

    fn target(root: &Path, file: Option<PathBuf>) -> project::Target {
        project::Target {
            root: root.to_path_buf(),
            file,
            created: false,
        }
    }

    fn fixture() -> (tempfile::TempDir, App) {
        fixture_with(Config::default())
    }

    fn fixture_with(cfg: Config) -> (tempfile::TempDir, App) {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let app = App::new(target(td.path(), None), cfg, None).unwrap();
        (td, app)
    }

    fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect::<String>()
            })
            .collect()
    }

    fn joined(app: &mut App) -> String {
        screen(app, 90, 24).join("\n")
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn ch(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::SHIFT,
            ..k(code)
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..k(KeyCode::Char(c))
        }
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(ch(c));
        }
    }

    /// Run a command line through the bar, exactly as a user would.
    /// Run a command from wherever the keyboard happens to be.
    ///
    /// Ctrl+P rather than `:`, because in the editor a colon is a colon — the
    /// same reason the binding exists at all.
    fn command(app: &mut App, line: &str) {
        app.on_key(ctrl('p'));
        type_str(app, line);
        app.on_key(k(KeyCode::Enter));
    }

    /// Type `line` into the command bar, press Tab, and give back what the bar
    /// holds afterwards.
    fn completed(app: &mut App, line: &str) -> String {
        app.on_key(ctrl('p'));
        type_str(app, line);
        app.on_key(k(KeyCode::Tab));
        let Mode::Bar(b) = &app.mode else {
            panic!("the bar closed")
        };
        let out = b.command().to_string();
        app.on_key(k(KeyCode::Esc));
        out
    }

    fn select(app: &mut App, name: &str) {
        for _ in 0..12 {
            let paths: Vec<PathBuf> = app
                .rows
                .iter()
                .filter(|r| r.is_dir && !r.expanded)
                .map(|r| r.path.clone())
                .collect();
            if paths.is_empty() {
                break;
            }
            for p in paths {
                app.tree.expand(&p);
            }
            app.rows = app.tree.flatten();
        }
        let i = app
            .rows
            .iter()
            .position(|r| r.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no row named {name} in {:?}",
                    app.rows.iter().map(|r| &r.name).collect::<Vec<_>>()
                )
            });
        app.selected = i;
        app.sync_preview();
    }

    // ---- layout ----------------------------------------------------------

    #[test]
    fn opens_with_the_project_tree_and_a_preview() {
        let (_td, mut app) = fixture();
        let out = joined(&mut app);
        assert!(out.contains("PROJECT"), "{out}");
        for name in ["notes", "src", "README.md", "logo.png"] {
            assert!(out.contains(name), "tree is missing {name}:\n{out}");
        }
    }

    #[test]
    fn the_tree_can_be_moved_to_the_right_hand_side() {
        let cfg = Config {
            tree_side: Side::Right,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        select(&mut app, "README.md");
        let rows = screen(&mut app, 90, 24);
        let header = &rows[0];
        let project_at = header.find("PROJECT").expect("tree pane is drawn");
        let file_at = header.find("README.md").expect("preview pane is drawn");
        assert!(
            project_at > file_at,
            "the tree should be on the right:\n{header}"
        );
    }

    #[test]
    fn borders_can_be_turned_off_for_a_quieter_screen() {
        let cfg = Config {
            borders: false,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        let out = joined(&mut app);
        assert!(!out.contains('┌'), "no box corners:\n{out}");
        assert!(!out.contains('│'), "no box sides:\n{out}");
        assert!(out.contains("PROJECT"), "the title still shows:\n{out}");
        assert!(out.contains("README.md"));
    }

    #[test]
    fn tree_markers_can_be_plain_ascii() {
        let cfg = Config {
            markers: Markers::Ascii,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        let out = joined(&mut app);
        assert!(!out.contains('▾'), "no geometric glyphs:\n{out}");
        assert!(out.contains("- "), "an open folder uses a dash:\n{out}");
    }

    #[test]
    fn the_status_line_can_move_to_the_top() {
        let cfg = Config {
            status_position: Position::Top,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        let rows = screen(&mut app, 90, 24);
        assert!(rows[0].contains("help"), "status is on row 0:\n{}", rows[0]);
        assert!(rows[1].contains("PROJECT"));
    }

    #[test]
    fn a_narrow_terminal_still_renders() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        for (w, h) in [(30u16, 10u16), (28, 6), (120, 40)] {
            let rows = screen(&mut app, w, h);
            assert_eq!(rows.len(), h as usize);
            assert!(rows.iter().all(|r| r.chars().count() == w as usize));
        }
    }

    // ---- the monochrome brief --------------------------------------------

    #[test]
    fn the_chrome_names_no_colours_at_all() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
        t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();

        // The tree pane holds only chrome — no syntax highlighting to excuse
        // a colour. Everything in it should inherit the terminal's palette.
        for y in 0..buf.area.height {
            for x in 0..28 {
                let Some(cell) = buf.cell((x, y)) else {
                    continue;
                };
                let allowed = matches!(
                    cell.fg,
                    Color::Reset | Color::White | Color::DarkGray | Color::Gray | Color::Black
                );
                assert!(
                    allowed,
                    "chrome at {x},{y} uses {:?}; the design brief is white and black",
                    cell.fg
                );
            }
        }
    }

    #[test]
    fn no_emoji_or_pictorial_icons_are_drawn() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('e'));
        type_str(&mut app, "x");
        let out = joined(&mut app);
        for bad in ['●', '☐', '☑', '🖼', '📁', '📄'] {
            assert!(!out.contains(bad), "found {bad:?} in:\n{out}");
        }
        assert!(
            out.contains(" *"),
            "unsaved work is marked with an asterisk"
        );
    }

    // ---- plain text -------------------------------------------------------

    #[test]
    fn file_kinds_are_classified_by_extension_then_by_name() {
        let prose: Vec<String> = ["md", "txt", "rst"].iter().map(|s| s.to_string()).collect();
        let k = |p: &str| text_kind(Path::new(p), &prose);
        assert_eq!(k("a.md"), TextKind::Markdown);
        assert_eq!(k("a.MD"), TextKind::Markdown, "case does not matter");
        assert_eq!(k("a.txt"), TextKind::Prose);
        assert_eq!(k("a.rst"), TextKind::Prose);
        assert_eq!(k("a.py"), TextKind::Code);
        assert_eq!(k("a.csv"), TextKind::Code);
        // Extensionless files are code unless they are prose by convention.
        assert_eq!(k("LICENSE"), TextKind::Prose);
        assert_eq!(k("COPYING"), TextKind::Prose);
        assert_eq!(k("Makefile"), TextKind::Code);
        assert_eq!(k("script"), TextKind::Code);
    }

    #[test]
    fn a_text_file_opens_wrapped_and_readable_not_in_the_editor() {
        let (td, mut app) = fixture();
        let long = "a long line of prose that will certainly need to wrap because it keeps going";
        fs::write(td.path().join("notes.txt"), format!("{long}\n")).unwrap();
        app.on_key(k(KeyCode::F(5)));
        select(&mut app, "notes.txt");

        let out = joined(&mut app);
        assert!(out.contains("READ"), "prose reads first:\n{out}");
        assert!(!out.contains(" 1 "), "no line numbers on prose:\n{out}");
        for line in screen(&mut app, 90, 24) {
            assert!(line.chars().count() == 90);
        }
        // Wrapped, not clipped: the last word of the line still made it.
        assert!(
            out.contains("keeps going"),
            "the tail of the line was lost:\n{out}"
        );
    }

    #[test]
    fn e_switches_plain_text_into_the_editor_and_back_out() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("notes.txt"), "first line\n").unwrap();
        app.on_key(k(KeyCode::F(5)));
        select(&mut app, "notes.txt");
        app.on_key(k(KeyCode::Enter));
        assert!(app.read_mode);

        app.on_key(ch('e'));
        assert!(!app.read_mode);
        type_str(&mut app, "X");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('X'));
        app.on_key(ctrl('s'));
        assert_eq!(
            fs::read_to_string(td.path().join("notes.txt")).unwrap(),
            "Xfirst line\n"
        );
    }

    #[test]
    fn a_code_file_still_opens_straight_into_the_editor() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        assert!(!app.read_mode, "code is for typing into");
        assert!(
            joined(&mut app).contains(" 1 "),
            "and keeps its line numbers"
        );
    }

    #[test]
    fn prose_extensions_are_configurable() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("a.csv"), "x,y\n").unwrap();
        app.on_key(k(KeyCode::F(5)));
        select(&mut app, "a.csv");
        assert!(joined(&mut app).contains("VIEW"), "csv is code by default");

        command(&mut app, "set prose_extensions md txt csv");
        app.on_key(k(KeyCode::F(5)));
        select(&mut app, "a.csv");
        assert!(joined(&mut app).contains("READ"), "now it reads as prose");
    }

    // ---- preview dispatch -------------------------------------------------

    #[test]
    fn hovering_a_markdown_file_shows_it_rendered_not_raw() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        let out = joined(&mut app);
        assert!(out.contains("Design Notes"), "{out}");
        assert!(
            !out.contains("# Design Notes"),
            "the '#' is consumed:\n{out}"
        );
        assert!(out.contains("• one"), "list is rendered as bullets:\n{out}");
        assert!(out.contains("READ"));
    }

    #[test]
    fn hovering_a_code_file_shows_source_with_line_numbers() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        let out = joined(&mut app);
        assert!(out.contains("import utils"), "{out}");
        assert!(out.contains(" 1 "), "line numbers are shown:\n{out}");
    }

    #[test]
    fn hovering_a_picture_tries_to_draw_it() {
        let (_td, mut app) = fixture();
        select(&mut app, "logo.png");
        assert!(matches!(app.preview, Preview::Media { .. }));
        // The fixture's png is deliberately corrupt, so the pane should say
        // so by name rather than drawing nothing at all.
        let out = joined(&mut app);
        assert!(out.contains("logo.png"), "{out}");
    }

    #[test]
    fn a_real_picture_is_drawn_as_half_blocks() {
        let (td, mut app) = fixture();
        let mut img = image::RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 8) as u8, (y * 8) as u8, 200, 255]);
        }
        img.save(td.path().join("real.png")).unwrap();
        app.on_key(k(KeyCode::F(5)));
        select(&mut app, "real.png");
        let out = joined(&mut app);
        assert!(out.contains('▀'), "expected half-blocks:\n{out}");
        assert!(out.contains("32x32"), "and a caption:\n{out}");
    }

    #[test]
    fn media_preview_can_be_turned_off() {
        let cfg = Config {
            media_preview: false,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        select(&mut app, "logo.png");
        assert!(matches!(app.preview, Preview::Binary { .. }));
    }

    #[test]
    fn hovering_a_folder_summarises_it() {
        let (_td, mut app) = fixture();
        select(&mut app, "notes");
        assert!(joined(&mut app).contains("1 entry"));
    }

    // ---- navigation & editing --------------------------------------------

    #[test]
    fn arrow_keys_move_the_cursor_and_open_folders() {
        let (_td, mut app) = fixture();
        assert_eq!(app.selected, 0, "starts on the project root");
        app.on_key(k(KeyCode::Down));
        assert_eq!(app.selected_row().unwrap().name, "notes");
        app.on_key(k(KeyCode::Right));
        assert!(app.selected_row().unwrap().expanded);
        assert_eq!(app.rows[2].name, "design.md");
        app.on_key(k(KeyCode::Left));
        assert!(!app.selected_row().unwrap().expanded);
    }

    #[test]
    fn enter_opens_and_closes_a_folder_without_moving_the_cursor() {
        let (_td, mut app) = fixture();
        app.on_key(k(KeyCode::Down));
        assert_eq!(app.selected_row().unwrap().name, "notes");

        app.on_key(k(KeyCode::Enter));
        assert!(app.selected_row().unwrap().expanded, "it opened");
        assert_eq!(app.selected_row().unwrap().name, "notes", "and stayed put");

        app.on_key(k(KeyCode::Enter));
        assert!(
            !app.selected_row().unwrap().expanded,
            "the same key shuts it"
        );
        assert_eq!(app.selected_row().unwrap().name, "notes");
    }

    #[test]
    fn right_still_steps_into_a_folder_that_is_already_open() {
        let (_td, mut app) = fixture();
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Right));
        assert!(app.selected_row().unwrap().expanded);
        app.on_key(k(KeyCode::Right));
        assert_eq!(
            app.selected_row().unwrap().name,
            "design.md",
            "right walks inwards where Enter toggles"
        );
    }

    #[test]
    fn an_open_folders_marker_is_drawn_in_the_text_colour() {
        let (_td, mut app) = fixture();
        // Move off the root row so nothing selected is being highlighted over.
        app.on_key(k(KeyCode::Down));
        let (text_fg, dim_fg) = (app.palette.text.fg, app.palette.dim.fg);

        let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
        t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let marker = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .find_map(|(x, y)| buf.cell((x, y)).filter(|c| c.symbol() == "\u{25be}"))
            .expect("the open root folder draws a marker");

        assert_eq!(marker.fg, text_fg.unwrap());
        assert_ne!(Some(marker.fg), dim_fg, "it is no longer chrome-coloured");
    }

    #[test]
    fn ctrl_b_folds_the_tree_away_and_brings_it_back() {
        let (_td, mut app) = fixture();
        assert!(joined(&mut app).contains("PROJECT"));

        app.on_key(ctrl('b'));
        let out = joined(&mut app);
        assert!(!out.contains("PROJECT"), "the pane is gone:\n{out}");
        assert!(
            !out.contains("┐┌"),
            "and the file has the whole width:\n{out}"
        );
        assert_eq!(app.focus, Focus::Editor, "keys cannot go to an unseen pane");

        app.on_key(ctrl('b'));
        assert!(joined(&mut app).contains("PROJECT"));
        assert_eq!(app.focus, Focus::Tree, "the keyboard comes back with it");
    }

    #[test]
    fn folding_the_tree_while_editing_leaves_you_editing() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));

        app.on_key(ctrl('b'));
        assert!(app.tree_hidden);
        assert_eq!(app.focus, Focus::Editor);
        app.on_key(ctrl('b'));
        assert_eq!(
            app.focus,
            Focus::Editor,
            "you were typing before and still are"
        );
        type_str(&mut app, "X");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('X'));
    }

    #[test]
    fn esc_brings_the_tree_back_and_lands_on_it() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ctrl('b'));

        app.on_key(k(KeyCode::Esc));
        assert!(
            !app.tree_hidden,
            "Esc means the tree, so the tree comes back"
        );
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn search_still_gets_a_pane_while_the_tree_is_folded_away() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('b'));
        app.on_key(ctrl('f'));
        type_str(&mut app, "widget");

        let out = joined(&mut app);
        assert!(
            out.contains("MATCH"),
            "results need somewhere to go:\n{out}"
        );
        assert!(out.contains("hello widget"), "{out}");
    }

    #[test]
    fn n_makes_a_file_when_the_name_has_an_extension() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "todo.txt");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("todo.txt").is_file(), "{}", app.status);
    }

    #[test]
    fn n_makes_a_folder_when_the_name_has_none() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "archive");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("archive").is_dir(), "{}", app.status);
    }

    #[test]
    fn a_trailing_slash_makes_a_folder_whatever_the_name_looks_like() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "site.v2/");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("site.v2").is_dir(), "{}", app.status);
    }

    #[test]
    fn the_new_command_still_makes_a_file_with_no_extension() {
        let (td, mut app) = fixture();
        command(&mut app, "new LICENSE");
        assert!(
            td.path().join("LICENSE").is_file(),
            "the explicit form is how you get an extensionless file: {}",
            app.status
        );
    }

    #[test]
    fn shift_is_not_needed_for_anything_that_makes_a_thing() {
        let (_td, mut app) = fixture();
        app.on_key(ch('N'));
        assert!(
            matches!(app.mode, Mode::Normal),
            "capital N does nothing now; n covers both"
        );
    }

    #[test]
    fn ijkl_move_the_tree_cursor_like_the_arrows() {
        let (_td, mut app) = fixture();
        app.on_key(ch('k'));
        assert_eq!(app.selected_row().unwrap().name, "notes", "k is down");
        app.on_key(ch('l'));
        assert!(app.selected_row().unwrap().expanded, "l is right");
        app.on_key(ch('j'));
        assert!(!app.selected_row().unwrap().expanded, "j is left");
        app.on_key(ch('i'));
        assert_eq!(app.selected, 0, "i is up");
    }

    #[test]
    fn shift_i_and_k_go_all_the_way_like_shift_and_an_arrow() {
        let (_td, mut app) = fixture();
        app.on_key(ch('K'));
        assert_eq!(app.selected, app.rows.len() - 1);
        app.on_key(ch('I'));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn ijkl_scroll_a_note_being_read() {
        let (_td, mut app) = fixture();
        select(&mut app, "README.md");
        app.on_key(k(KeyCode::Enter));
        joined(&mut app);
        app.on_key(ch('k'));
        assert_eq!(app.preview_scroll, 1);
        app.on_key(ch('i'));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn a_letter_in_the_editor_is_still_just_a_letter() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "ijkl");
        assert!(
            app.active_buffer().unwrap().lines()[0].starts_with("ijkl"),
            "typing is typing"
        );
    }

    #[test]
    fn e_is_the_only_key_that_switches_to_raw_editing() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.read_mode);
        // `i` used to mean "edit" here; it is an arrow now.
        app.on_key(ch('i'));
        assert!(app.read_mode, "i scrolls, it does not open the editor");
        app.on_key(ch('e'));
        assert!(!app.read_mode);
    }

    #[test]
    fn shift_and_an_arrow_goes_to_the_first_or_last_entry() {
        let (_td, mut app) = fixture();
        app.on_key(shift(KeyCode::Down));
        assert_eq!(app.selected, app.rows.len() - 1, "all the way down");
        app.on_key(shift(KeyCode::Up));
        assert_eq!(app.selected, 0, "and all the way back");
    }

    #[test]
    fn the_old_letter_keys_for_those_are_gone() {
        let (_td, mut app) = fixture();
        app.on_key(ch('G'));
        assert_eq!(app.selected, 0, "G no longer jumps to the end");
        app.on_key(ch('R'));
        assert!(
            matches!(app.mode, Mode::Normal),
            "R is *reload now, not a key"
        );
    }

    #[test]
    fn f5_still_re_reads_the_project() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("appeared.md"), "# new\n").unwrap();
        assert!(!app.rows.iter().any(|r| r.name == "appeared.md"));
        app.on_key(k(KeyCode::F(5)));
        assert!(app.rows.iter().any(|r| r.name == "appeared.md"));
    }

    #[test]
    fn the_reload_command_does_the_same() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("appeared.md"), "# new\n").unwrap();
        command(&mut app, "reload");
        assert!(app.rows.iter().any(|r| r.name == "appeared.md"));
    }

    #[test]
    fn the_cursor_cannot_run_off_either_end() {
        let (_td, mut app) = fixture();
        for _ in 0..50 {
            app.on_key(k(KeyCode::Up));
        }
        assert_eq!(app.selected, 0);
        for _ in 0..50 {
            app.on_key(k(KeyCode::Down));
        }
        assert_eq!(app.selected, app.rows.len() - 1);
    }

    #[test]
    fn enter_on_a_code_file_focuses_the_editor_and_typing_reaches_the_buffer() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        assert_eq!(app.focus, Focus::Editor);
        assert!(!app.read_mode, "code opens straight into editing");
        type_str(&mut app, "# hi");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("# hi"));
        assert!(app.active_buffer().unwrap().dirty);
    }

    #[test]
    fn q_types_a_letter_in_the_editor_instead_of_quitting() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('q'));
        assert!(!app.should_quit, "q must not quit while editing");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('q'));
    }

    #[test]
    fn slash_does_not_open_search_while_typing_code() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('/'));
        assert!(
            matches!(app.mode, Mode::Normal),
            "it is a comment, not a key"
        );
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('/'));
    }

    #[test]
    fn ctrl_s_writes_the_file_to_disk() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "x");
        app.on_key(ctrl('s'));
        assert!(!app.active_buffer().unwrap().dirty);
        let on_disk = fs::read_to_string(td.path().join("src/main.py")).unwrap();
        assert!(on_disk.starts_with("ximport utils"), "{on_disk:?}");
    }

    #[test]
    fn unsaved_edits_survive_navigating_away_and_back() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "MARKER");
        app.on_key(k(KeyCode::Esc));
        select(&mut app, "README.md");
        select(&mut app, "main.py");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("MARKER"));
    }

    #[test]
    fn markdown_opens_rendered_and_e_switches_to_raw_source() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.read_mode);
        app.on_key(ch('e'));
        assert!(!app.read_mode);
        let out = joined(&mut app);
        assert!(out.contains("# Design Notes"), "raw source shows:\n{out}");
        assert!(out.contains("EDIT"));
    }

    // ---- opening a single file -------------------------------------------

    #[test]
    fn naming_a_file_opens_its_folder_with_the_editor_already_on_it() {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let file = td.path().join("src/main.py");
        let mut app = App::new(
            target(td.path(), Some(file.clone())),
            Config::default(),
            None,
        )
        .unwrap();

        assert_eq!(app.selected_row().unwrap().path, file);
        assert_eq!(app.focus, Focus::Editor, "it is ready to type into");
        assert!(!app.read_mode);
        let out = joined(&mut app);
        assert!(out.contains("main.py"), "{out}");
        assert!(out.contains("import utils"), "{out}");
        assert!(
            out.contains("README.md"),
            "the folder is still listed:\n{out}"
        );
    }

    #[test]
    fn a_new_project_shows_its_readme_with_the_keyboard_still_on_the_tree() {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let mut app = App::new(
            project::Target {
                root: td.path().to_path_buf(),
                file: Some(td.path().join("README.md")),
                created: true,
            },
            Config::default(),
            None,
        )
        .unwrap();

        assert_eq!(app.selected_row().unwrap().name, "README.md");
        assert_eq!(app.focus, Focus::Tree, "there is nothing to type yet");
        assert!(app.status.contains("new project"), "{}", app.status);
        let out = joined(&mut app);
        assert!(out.contains("hello widget"), "the README is drawn:\n{out}");
    }

    #[test]
    fn naming_a_markdown_file_opens_it_rendered() {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let file = td.path().join("notes/design.md");
        let mut app = App::new(target(td.path(), Some(file)), Config::default(), None).unwrap();
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.read_mode, "a note opens to be read, not edited");
        assert!(joined(&mut app).contains("Design Notes"));
    }

    // ---- search -----------------------------------------------------------

    #[test]
    fn the_results_pane_is_the_width_of_the_tree_it_replaces() {
        let (_td, mut app) = fixture();
        let tree_row = screen(&mut app, 90, 24)
            .into_iter()
            .find(|r| r.contains("PROJECT"))
            .expect("the tree is drawn");
        let tree_edge = tree_row.find("┐").expect("the pane ends somewhere");

        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        let hits_row = screen(&mut app, 90, 24)
            .into_iter()
            .find(|r| r.contains("MATCH"))
            .expect("the results are drawn");
        assert_eq!(
            hits_row.find("┐"),
            Some(tree_edge),
            "the screen must not lurch sideways when you start typing"
        );
    }

    #[test]
    fn slash_opens_the_search_bar_and_typing_finds_matches() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        assert!(matches!(app.mode, Mode::Bar(_)));
        type_str(&mut app, "widget");

        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert!(!b.results.is_empty(), "found nothing");
        assert!(b.results.iter().any(|h| h.text.contains("hello widget")));

        let out = joined(&mut app);
        assert!(out.contains("MATCH"), "{out}");
        assert!(out.contains("hello widget"), "{out}");
    }

    #[test]
    fn the_search_bar_shows_the_query_as_it_is_typed() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "design");
        let rows = screen(&mut app, 90, 24);
        assert!(
            rows[0].contains("design"),
            "the bar is on top:\n{}",
            rows[0]
        );
        assert!(rows[0].trim_start().starts_with('/'));
    }

    #[test]
    fn enter_jumps_to_the_selected_match_and_puts_the_cursor_on_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "utils.load");
        app.on_key(k(KeyCode::Enter));

        assert!(matches!(app.mode, Mode::Normal), "the bar closes");
        assert_eq!(app.selected_row().unwrap().name, "main.py");
        assert_eq!(app.focus, Focus::Editor);
        let ed = app.active_buffer().unwrap();
        assert_eq!(ed.cursor_line, 4, "landed on the matching line");
        assert_eq!(ed.cursor_col, 11, "and on the matching column");
    }

    #[test]
    fn moving_through_results_previews_each_one() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "e");

        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        let paths: Vec<PathBuf> = b.results.iter().map(|h| h.path.clone()).collect();
        assert_eq!(
            app.selected_path().unwrap(),
            paths[0],
            "starts on the first"
        );

        // Walk down to the first result in a different file.
        let other = paths
            .iter()
            .position(|p| *p != paths[0])
            .expect("fixture should match in more than one file");
        for _ in 0..other {
            app.on_key(k(KeyCode::Down));
        }
        assert!(matches!(app.mode, Mode::Bar(_)), "still searching");
        assert_eq!(
            app.selected_path().unwrap(),
            paths[other],
            "the preview follows the highlighted result"
        );
    }

    #[test]
    fn a_search_with_no_matches_says_so() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "zzzznothing");
        assert!(joined(&mut app).contains("no matches"));
    }

    #[test]
    fn control_chords_do_not_type_letters_into_the_search_bar() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "wid");
        app.on_key(ctrl('x'));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert_eq!(b.input, "wid", "Ctrl+X must not append an x");
    }

    #[test]
    fn ctrl_q_quits_from_inside_the_search_bar() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        app.on_key(ctrl('q'));
        assert!(app.should_quit, "Ctrl+Q means quit everywhere");
    }

    #[test]
    fn control_chords_do_not_type_letters_into_a_prompt() {
        let (_td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "note");
        app.on_key(ctrl('s'));
        let Mode::Prompt(p) = &app.mode else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, "note", "Ctrl+S must not append an s");
    }

    #[test]
    fn ctrl_s_while_editing_a_setting_does_not_type_an_s_into_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Enter));
        app.on_key(ctrl('s'));
        let Mode::Settings(s) = &app.mode else {
            panic!("settings closed")
        };
        assert_eq!(
            s.editing.as_deref(),
            Some("4"),
            "the field still holds tab_width's value, not \"4s\""
        );
    }

    #[test]
    fn escape_closes_the_search_bar() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        app.on_key(k(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(joined(&mut app).contains("PROJECT"), "the tree is back");
    }

    // ---- commands ---------------------------------------------------------

    #[test]
    fn set_changes_a_setting_and_it_takes_effect() {
        let (_td, mut app) = fixture();
        assert_eq!(app.config.tab_width, 4);
        command(&mut app, "set tab_width 2");
        assert_eq!(app.config.tab_width, 2);

        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(k(KeyCode::Tab));
        assert!(
            app.active_buffer().unwrap().lines()[0].starts_with("  import"),
            "the new tab width is what the editor actually inserts"
        );
    }

    #[test]
    fn set_with_no_value_reports_the_current_one() {
        let (_td, mut app) = fixture();
        command(&mut app, "set tree_side");
        assert_eq!(app.status, "tree_side = left");
    }

    #[test]
    fn set_can_repaint_the_theme_without_a_restart() {
        let (_td, mut app) = fixture();
        command(&mut app, "set theme.heading cyan bold");
        assert_eq!(app.config.theme.heading, "cyan bold");
        assert_eq!(app.palette.heading.fg, Some(Color::Cyan));
    }

    #[test]
    fn set_rejects_nonsense_without_changing_anything() {
        let (_td, mut app) = fixture();
        command(&mut app, "set tab_width banana");
        assert_eq!(app.config.tab_width, 4);
        assert!(app.status.contains("number"), "{}", app.status);

        command(&mut app, "set no_such_setting 1");
        assert!(app.status.contains("unknown setting"), "{}", app.status);
    }

    #[test]
    fn an_unknown_command_says_so() {
        let (_td, mut app) = fixture();
        command(&mut app, "frobnicate");
        assert!(app.status.contains("unknown command"), "{}", app.status);
    }

    #[test]
    fn replace_asks_first_then_rewrites_the_whole_project() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("notes/more.md"), "widget widget\n").unwrap();
        app.on_key(k(KeyCode::F(5)));

        command(&mut app, "replace widget gadget");
        assert!(matches!(app.mode, Mode::Confirm(_)), "it asks first");
        let out = joined(&mut app);
        assert!(
            out.contains("3 occurrences"),
            "counts them up front:\n{out}"
        );
        assert!(out.contains("2 files"), "{out}");

        app.on_key(ch('n'));
        assert!(
            fs::read_to_string(td.path().join("README.md"))
                .unwrap()
                .contains("widget"),
            "answering no changes nothing"
        );

        command(&mut app, "replace widget gadget");
        app.on_key(ch('y'));
        assert!(
            fs::read_to_string(td.path().join("README.md"))
                .unwrap()
                .contains("gadget"),
            "answering yes rewrites it"
        );
        assert_eq!(
            fs::read_to_string(td.path().join("notes/more.md")).unwrap(),
            "gadget gadget\n"
        );
        assert!(app.status.contains("replaced 3"), "{}", app.status);
    }

    #[test]
    fn replace_needs_both_halves_and_reports_a_miss() {
        let (_td, mut app) = fixture();
        command(&mut app, "replace onlyone");
        assert!(app.status.contains(":replace"), "{}", app.status);
        command(&mut app, "replace absent-word x");
        assert!(app.status.contains("not in this project"), "{}", app.status);
    }

    #[test]
    fn replace_takes_quoted_strings_with_spaces_in_them() {
        let (td, mut app) = fixture();
        command(&mut app, "replace \"hello widget\" \"goodbye gadget\"");
        app.on_key(ch('y'));
        assert!(
            fs::read_to_string(td.path().join("README.md"))
                .unwrap()
                .contains("goodbye gadget"),
            "quoted arguments keep their spaces"
        );
    }

    // ---- the project map --------------------------------------------------

    /// The fixture plus the two files it already points at: `design.md` links
    /// to `[[architecture]]` and `main.py` imports `utils`, so once those
    /// exist the map has real edges to draw.
    fn linked_fixture_with(cfg: Config) -> (tempfile::TempDir, App) {
        let (td, mut app) = fixture_with(cfg);
        fs::write(
            td.path().join("notes/architecture.md"),
            "# Architecture\n\nback to [[design]]\n",
        )
        .unwrap();
        fs::write(
            td.path().join("src/utils.py"),
            "def load():\n    return 1\n",
        )
        .unwrap();
        app.on_key(k(KeyCode::F(5)));
        (td, app)
    }

    fn linked_fixture() -> (tempfile::TempDir, App) {
        linked_fixture_with(Config::default())
    }

    #[test]
    fn the_bottom_bar_says_only_what_is_not_already_on_screen() {
        let (_td, mut app) = fixture();
        let bar = screen(&mut app, 96, 14).pop().expect("a status line");
        assert!(bar.contains("| m map |"), "dividers are pipes:\n{bar}");
        assert!(!bar.contains('·'), "{bar}");
        assert!(
            !bar.contains("? help"),
            "the status already opens with `? for help`:\n{bar}"
        );
        assert!(
            !bar.contains(": commands"),
            "there is no colon key any more:\n{bar}"
        );
    }

    #[test]
    fn the_map_draws_every_file_in_a_box() {
        let (_td, mut app) = linked_fixture();
        app.on_key(ch('m'));
        let out = screen(&mut app, 100, 34).join("\n");
        assert!(out.contains("PROJECT MAP"), "{out}");
        // Names sit inside boxes, so the borders are right beside them.
        assert!(
            out.contains("│design.md│"),
            "a box with a name in it:\n{out}"
        );
        assert!(
            out.contains('╭') && out.contains('╯'),
            "box corners:\n{out}"
        );
    }

    #[test]
    fn the_map_joins_linked_files_with_a_line() {
        let (_td, mut app) = linked_fixture();
        app.on_key(ch('m'));
        let out = screen(&mut app, 100, 34).join("\n");
        assert!(out.contains('─') || out.contains('│'), "lines:\n{out}");
        assert!(
            ['◂', '▸', '▴', '▾'].iter().any(|a| out.contains(*a)),
            "an arrowhead says which way it runs:\n{out}"
        );
    }

    #[test]
    fn a_code_file_that_calls_another_is_joined_to_it() {
        let (_td, mut app) = linked_fixture();
        app.on_key(ch('m'));
        // Only calls and imports; the notes are switched off, so whatever is
        // left on screen is there because of the code.
        app.on_key(ch('1'));
        app.on_key(ch('2'));
        let out = screen(&mut app, 100, 34).join("\n");
        assert!(out.contains("│main.py│"), "{out}");
        assert!(out.contains("│utils.py│"), "{out}");
        assert!(
            !out.contains("│design.md│"),
            "the notes are unconnected now, so they drop out:\n{out}"
        );
    }

    #[test]
    fn nothing_is_drawn_on_top_of_a_box() {
        let (_td, mut app) = linked_fixture();
        app.on_key(ch('m'));
        let rows = screen(&mut app, 100, 34);
        // Every name on screen is whole, with its own border either side: a
        // line crossing the box would have overwritten one of those cells.
        for name in ["design.md", "architecture.md", "main.py", "utils.py"] {
            let row = rows
                .iter()
                .find(|r| r.contains(name))
                .unwrap_or_else(|| panic!("{name} is missing:\n{}", rows.join("\n")));
            let at = row.find(name).unwrap();
            assert_eq!(
                row[..at].chars().last().unwrap(),
                '│',
                "{name} sits inside its box:\n{row}"
            );
        }
    }

    #[test]
    fn the_map_can_be_drawn_without_box_characters() {
        let cfg = Config {
            markers: Markers::Ascii,
            ..Config::default()
        };
        let (_td, mut app) = linked_fixture_with(cfg);
        app.on_key(ch('m'));
        let out = screen(&mut app, 100, 34).join("\n");
        assert!(!out.contains('╭'), "no box drawing at all:\n{out}");
        assert!(
            out.contains("|design.md|"),
            "boxes are made of pipes:\n{out}"
        );
        assert!(out.contains('+'), "and corners of plus signs:\n{out}");
    }

    // ---- jumping to a line, and the wheel ---------------------------------

    #[test]
    fn the_line_command_puts_the_cursor_on_that_line() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        command(&mut app, "line 4");
        assert_eq!(app.focus, Focus::Editor);
        assert!(!app.read_mode, "a line number is a fact about the source");
        assert_eq!(
            app.active_buffer().unwrap().cursor_line,
            3,
            "counted from 1"
        );
        assert!(app.status.contains("line 4"), "{}", app.status);
    }

    #[test]
    fn a_bare_number_is_a_line_number_too() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        command(&mut app, "3");
        assert_eq!(app.active_buffer().unwrap().cursor_line, 2);
        // And the wordier forms mean the same thing.
        command(&mut app, "go to 2");
        assert_eq!(app.active_buffer().unwrap().cursor_line, 1);
    }

    #[test]
    fn a_line_past_the_end_lands_on_the_last_one() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        let last = app.active_buffer().unwrap().line_count() - 1;
        command(&mut app, "line 900");
        assert_eq!(app.active_buffer().unwrap().cursor_line, last);
    }

    #[test]
    fn the_line_command_says_when_there_is_nothing_to_jump_inside() {
        let (_td, mut app) = fixture();
        select(&mut app, "notes");
        command(&mut app, "line 4");
        assert!(app.status.contains("no file open"), "{}", app.status);
        command(&mut app, "line zero");
        assert!(app.status.contains("not a line number"), "{}", app.status);
    }

    #[test]
    fn a_wheel_notch_moves_exactly_one_line() {
        let (_td, mut app) = fixture();
        select(&mut app, "README.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.read_mode, "a note opens rendered");
        // Draw once so the preview knows how long it is.
        joined(&mut app);

        app.on_scroll(true, 60);
        assert_eq!(app.preview_scroll, 1, "one notch, one line");
        app.on_scroll(false, 60);
        assert_eq!(app.preview_scroll, 0);
        app.on_scroll(false, 60);
        assert_eq!(app.preview_scroll, 0, "and it stops at the top");
    }

    #[test]
    fn the_wheel_moves_the_cursor_in_a_file_being_edited() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        assert!(!app.read_mode, "code opens straight into editing");
        joined(&mut app);

        app.on_scroll(true, 60);
        assert_eq!(app.active_buffer().unwrap().cursor_line, 1);
        app.on_scroll(false, 60);
        assert_eq!(app.active_buffer().unwrap().cursor_line, 0);
    }

    #[test]
    fn the_wheel_over_the_tree_moves_the_tree_cursor() {
        let (_td, mut app) = fixture();
        joined(&mut app);
        let (x0, _) = app.last_tree_cols.expect("the tree was drawn");
        assert_eq!(app.selected, 0);

        app.on_scroll(true, x0 + 1);
        assert_eq!(app.selected, 1, "one notch, one row");
        app.on_scroll(false, x0 + 1);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn one_bar_switches_between_searching_and_commanding_as_you_type() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert!(!b.is_command(), "plain text searches");
        assert!(!b.results.is_empty(), "and found something");

        // Clear it and lead with the sigil instead.
        for _ in 0.."widget".len() {
            app.on_key(k(KeyCode::Backspace));
        }
        type_str(&mut app, "*copy");
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert!(b.is_command(), "a leading * is a command");
        assert_eq!(b.command(), "copy");
        assert!(b.results.is_empty(), "a command has nothing to list");
    }

    #[test]
    fn deleting_the_sigil_turns_a_command_back_into_a_search() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "*widget");
        assert!(matches!(&app.mode, Mode::Bar(b) if b.results.is_empty()));

        // Back to the front and take the star off.
        app.on_key(k(KeyCode::Home));
        app.on_key(k(KeyCode::Delete));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert!(!b.is_command());
        assert!(!b.results.is_empty(), "the results come straight back");
    }

    #[test]
    fn a_command_typed_into_the_search_bar_runs() {
        let (td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "*copy README.md to notes");
        app.on_key(k(KeyCode::Enter));
        assert!(
            td.path().join("notes/README.md").is_file(),
            "{}",
            app.status
        );
    }

    #[test]
    fn ctrl_p_opens_the_bar_with_the_sigil_already_there() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('p'));
        let Mode::Bar(b) = &app.mode else {
            panic!("no bar")
        };
        assert_eq!(b.input, "*");
        assert!(b.is_command());
        assert_eq!(b.cursor, 1, "and the cursor is past it");
    }

    #[test]
    fn the_bar_shows_which_of_the_two_it_currently_is() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        assert!(joined(&mut app).contains(" / "), "a search shows a slash");
        app.on_key(k(KeyCode::Home));
        type_str(&mut app, "*");
        assert!(joined(&mut app).contains(" * "), "a command shows a star");
    }

    #[test]
    fn a_colon_is_just_a_colon_now() {
        let (_td, mut app) = fixture();
        app.on_key(ch(':'));
        assert!(
            matches!(app.mode, Mode::Normal),
            "the bar is reached by / and Ctrl+P; it does not need a third key"
        );
    }

    #[test]
    fn the_bar_shows_the_star_once_not_twice() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('p'));
        type_str(&mut app, "copy");
        let bar = screen(&mut app, 90, 24)
            .into_iter()
            .find(|r| r.contains(" * "))
            .expect("the bar is drawn");
        assert!(bar.contains("* copy"), "{bar}");
        assert!(!bar.contains("**"), "the sigil is the block, not the text");
    }

    #[test]
    fn the_bar_offers_the_completion_in_grey_before_tab() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('p'));
        type_str(&mut app, "cop");
        let bar = screen(&mut app, 90, 24)
            .into_iter()
            .find(|r| r.contains(" * "))
            .expect("the bar is drawn");
        assert!(
            bar.contains("* copy"),
            "the y is offered before Tab is pressed:\n{bar}"
        );

        // And Tab takes up exactly what was offered.
        app.on_key(k(KeyCode::Tab));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert_eq!(b.command(), "copy");
    }

    #[test]
    fn the_offer_is_only_made_at_the_end_of_the_line() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('p'));
        type_str(&mut app, "cop");
        app.on_key(k(KeyCode::Home));
        let bar = screen(&mut app, 90, 24)
            .into_iter()
            .find(|r| r.contains(" * "))
            .expect("the bar is drawn");
        assert!(
            !bar.contains("copy"),
            "mid-line it would read as text already there:\n{bar}"
        );
    }

    #[test]
    fn a_search_is_not_offered_a_command_completion() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "cop");
        let bar = screen(&mut app, 90, 24)
            .into_iter()
            .find(|r| r.contains(" / "))
            .expect("the bar is drawn");
        assert!(!bar.contains("copy"), "{bar}");
    }

    #[test]
    fn the_right_arrow_takes_up_the_suggestion_too() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('p'));
        type_str(&mut app, "cop");
        app.on_key(k(KeyCode::Right));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert_eq!(b.command(), "copy", "the arrow does what Tab does");
        assert_eq!(b.cursor, b.input.chars().count(), "and lands at the end");
    }

    #[test]
    fn the_right_arrow_still_moves_along_the_text_mid_line() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('p'));
        type_str(&mut app, "cop");
        app.on_key(k(KeyCode::Home));
        app.on_key(k(KeyCode::Right));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert_eq!(b.command(), "cop", "nothing was completed");
        assert_eq!(b.cursor, 1, "it just moved");
    }

    #[test]
    fn the_right_arrow_leaves_a_search_alone() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "wid");
        app.on_key(k(KeyCode::Right));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert_eq!(b.input, "wid", "a search is never completed for you");
    }

    #[test]
    fn tab_completes_commands_and_setting_names() {
        let (_td, mut app) = fixture();
        assert_eq!(completed(&mut app, "repl"), "replace");
        assert_eq!(completed(&mut app, "set tree_w"), "set tree_width");
    }

    #[test]
    fn ctrl_p_reaches_the_command_bar_from_inside_the_editor() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "z");
        // A bare `:` here would be a character, so the editor needs its own way in.
        app.on_key(ctrl('p'));
        assert!(matches!(app.mode, Mode::Bar(_)));
        type_str(&mut app, "w");
        app.on_key(k(KeyCode::Enter));
        assert!(
            fs::read_to_string(td.path().join("src/main.py"))
                .unwrap()
                .starts_with('z')
        );
    }

    #[test]
    fn a_colon_in_the_editor_is_a_character_not_a_command() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch(':'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.active_buffer().unwrap().lines()[0].starts_with(':'));
    }

    #[test]
    fn commands_cover_saving_and_quitting() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "z");
        app.on_key(k(KeyCode::Esc));
        command(&mut app, "w");
        assert!(
            fs::read_to_string(td.path().join("src/main.py"))
                .unwrap()
                .starts_with('z')
        );

        command(&mut app, "q");
        assert!(app.should_quit);
    }

    // ---- settings area ----------------------------------------------------

    #[test]
    fn the_settings_area_lists_every_setting_with_its_value() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        assert!(matches!(app.mode, Mode::Settings(_)));
        let out = screen(&mut app, 100, 34).join("\n");
        assert!(out.contains("Settings"), "{out}");
        assert!(out.contains("tab_width"), "{out}");
        assert!(out.contains("tree_side"), "{out}");
        assert!(out.contains("left"), "values are shown:\n{out}");
    }

    #[test]
    fn a_setting_can_be_changed_from_the_settings_area() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        // Walk to tab_width, third in the index.
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Enter));

        // The field is prefilled with the current value.
        app.on_key(k(KeyCode::Backspace));
        type_str(&mut app, "7");
        app.on_key(k(KeyCode::Enter));
        assert_eq!(app.config.tab_width, 7, "status was: {}", app.status);
    }

    #[test]
    fn escape_while_editing_a_setting_leaves_it_alone() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Enter));
        app.on_key(k(KeyCode::Backspace));
        type_str(&mut app, "9");
        app.on_key(k(KeyCode::Esc));
        assert_eq!(app.config.tab_width, 4);
        assert!(matches!(app.mode, Mode::Settings(_)), "still in settings");
    }

    #[test]
    fn the_settings_area_can_be_opened_by_command_too() {
        let (_td, mut app) = fixture();
        command(&mut app, "config");
        assert!(matches!(app.mode, Mode::Settings(_)));
    }

    // ---- file operations --------------------------------------------------

    #[test]
    fn n_creates_a_file_in_the_selected_folder_and_selects_it() {
        let (td, mut app) = fixture();
        select(&mut app, "notes");
        app.on_key(ch('n'));
        type_str(&mut app, "today.md");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("notes/today.md").is_file());
        assert_eq!(app.selected_row().unwrap().name, "today.md");
    }

    #[test]
    fn a_nested_name_creates_the_folders_along_the_way() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "journal/2026/aug.md");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("journal/2026/aug.md").is_file());
        assert_eq!(app.selected_row().unwrap().name, "aug.md");
    }

    #[test]
    fn creating_over_an_existing_name_is_refused() {
        let (_td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "README.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.status.contains("already exists"), "{}", app.status);
    }

    #[test]
    fn rename_moves_the_file_and_carries_its_open_buffer() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "EDITED");
        app.on_key(k(KeyCode::Esc));

        app.on_key(ch('r'));
        for _ in 0.."main.py".len() {
            app.on_key(k(KeyCode::Backspace));
        }
        type_str(&mut app, "app.py");
        app.on_key(k(KeyCode::Enter));

        assert!(td.path().join("src/app.py").is_file());
        assert!(!td.path().join("src/main.py").exists());
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("EDITED"));
    }

    #[test]
    fn delete_asks_before_removing_and_n_backs_out() {
        let (td, mut app) = fixture();
        select(&mut app, "README.md");
        app.on_key(ch('d'));
        assert!(joined(&mut app).contains("Delete README.md?"));
        app.on_key(ch('n'));
        assert!(td.path().join("README.md").exists());
        app.on_key(ch('d'));
        app.on_key(ch('y'));
        assert!(!td.path().join("README.md").exists());
    }

    #[test]
    fn deleting_a_folder_takes_everything_under_it() {
        let (td, mut app) = fixture();
        select(&mut app, "notes");
        app.on_key(ch('d'));
        let out = joined(&mut app);
        assert!(out.contains("Delete folder notes"), "{out}");
        app.on_key(ch('y'));
        assert!(!td.path().join("notes").exists(), "the folder is gone");
        assert!(!td.path().join("notes/design.md").exists());
        assert!(app.status.contains("deleted"), "{}", app.status);
    }

    #[test]
    fn the_delete_command_removes_the_selection_when_given_no_path() {
        let (td, mut app) = fixture();
        select(&mut app, "README.md");
        command(&mut app, "delete");
        assert!(joined(&mut app).contains("Delete README.md?"));
        app.on_key(ch('y'));
        assert!(!td.path().join("README.md").exists());
    }

    #[test]
    fn the_delete_command_takes_a_path_from_the_project_root() {
        let (td, mut app) = fixture();
        // Cursor parked somewhere else entirely: the path is not relative to it.
        select(&mut app, "main.py");
        command(&mut app, "rm notes/design.md");
        app.on_key(ch('y'));
        assert!(!td.path().join("notes/design.md").exists());
        assert!(td.path().join("src/main.py").exists(), "nothing else went");
    }

    #[test]
    fn deleting_the_file_being_edited_hands_the_keyboard_back_to_the_tree() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        assert_eq!(app.focus, Focus::Editor);

        // In the editor `d` is the letter d and `:` is a colon — Ctrl+P is the
        // way to the command bar from here, and this is why delete needs to be
        // a command and not only a key.
        app.on_key(ctrl('p'));
        type_str(&mut app, "delete");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('y'));
        assert!(!td.path().join("src/main.py").exists());
        assert_eq!(app.focus, Focus::Tree);
        assert!(app.active_buffer().is_none(), "its buffer went with it");
    }

    #[test]
    fn the_delete_command_reports_a_path_that_is_not_there() {
        let (_td, mut app) = fixture();
        command(&mut app, "delete notes/nope.md");
        assert!(app.status.contains("cannot delete"), "{}", app.status);
        assert!(
            matches!(app.mode, Mode::Normal),
            "nothing to confirm, so nothing is armed"
        );
    }

    #[test]
    fn the_delete_command_cannot_escape_the_project() {
        let (_td, mut app) = fixture();
        command(&mut app, "delete ../../etc/hosts");
        assert!(app.status.contains("escapes the project"), "{}", app.status);
    }

    // ---- copying ----------------------------------------------------------

    #[test]
    fn copy_reads_like_a_sentence() {
        let (td, mut app) = fixture();
        command(&mut app, "copy README.md to notes");
        assert!(
            td.path().join("notes/README.md").is_file(),
            "naming a folder copies into it"
        );
        assert!(td.path().join("README.md").is_file(), "the original stays");
        assert!(app.status.contains("copied README.md"), "{}", app.status);
    }

    #[test]
    fn copy_to_a_name_that_is_not_there_yet_uses_that_name() {
        let (td, mut app) = fixture();
        command(&mut app, "copy README.md to notes/intro.md");
        assert!(td.path().join("notes/intro.md").is_file());
        assert_eq!(
            fs::read_to_string(td.path().join("notes/intro.md")).unwrap(),
            fs::read_to_string(td.path().join("README.md")).unwrap()
        );
    }

    #[test]
    fn copying_a_folder_takes_everything_under_it() {
        let (td, mut app) = fixture();
        fs::create_dir_all(td.path().join("notes/deep/deeper")).unwrap();
        fs::write(td.path().join("notes/deep/deeper/buried.md"), "# buried\n").unwrap();
        app.on_key(k(KeyCode::F(5)));

        command(&mut app, "copy notes to archive");
        assert!(td.path().join("archive/design.md").is_file());
        assert!(
            td.path().join("archive/deep/deeper/buried.md").is_file(),
            "subfolders come along"
        );
        assert_eq!(
            fs::read_to_string(td.path().join("archive/deep/deeper/buried.md")).unwrap(),
            "# buried\n"
        );
    }

    #[test]
    fn copy_takes_names_with_spaces_without_quoting_them() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("my notes.md"), "# mine\n").unwrap();
        app.on_key(k(KeyCode::F(5)));

        // `to` is the separator, so both sides can be several words.
        command(&mut app, "copy my notes.md to old work.md");
        assert!(td.path().join("old work.md").is_file(), "{}", app.status);
    }

    #[test]
    fn copy_without_the_word_to_still_works_with_two_names() {
        let (td, mut app) = fixture();
        command(&mut app, "cp README.md notes");
        assert!(
            td.path().join("notes/README.md").is_file(),
            "{}",
            app.status
        );
    }

    #[test]
    fn copy_says_how_to_say_it_when_it_cannot_parse() {
        let (_td, mut app) = fixture();
        command(&mut app, "copy");
        assert!(app.status.contains("say it like"), "{}", app.status);
        command(&mut app, "copy README.md to");
        assert!(app.status.contains("say it like"), "{}", app.status);
    }

    #[test]
    fn copy_never_writes_over_something_that_is_already_there() {
        let (td, mut app) = fixture();
        command(&mut app, "copy notes/design.md to README.md");
        assert!(app.status.contains("already exists"), "{}", app.status);
        assert_eq!(
            fs::read_to_string(td.path().join("README.md")).unwrap(),
            "# Fixture\n\nhello widget\n",
            "the original is untouched"
        );
    }

    #[test]
    fn a_folder_cannot_be_copied_inside_itself() {
        let (_td, mut app) = fixture();
        command(&mut app, "copy notes to notes/backup");
        assert!(app.status.contains("into itself"), "{}", app.status);
    }

    #[test]
    fn copy_cannot_reach_outside_the_project() {
        let (_td, mut app) = fixture();
        command(&mut app, "copy README.md to ../escaped.md");
        assert!(app.status.contains("escapes the project"), "{}", app.status);
    }

    #[test]
    fn ctrl_c_and_ctrl_v_move_a_file_into_another_folder() {
        let (td, mut app) = fixture();
        select(&mut app, "README.md");
        app.on_key(ctrl('c'));
        assert!(app.status.contains("copied README.md"), "{}", app.status);

        select(&mut app, "notes");
        app.on_key(ctrl('v'));
        assert!(
            td.path().join("notes/README.md").is_file(),
            "{}",
            app.status
        );
        assert!(td.path().join("README.md").is_file(), "a copy, not a move");
    }

    #[test]
    fn pasting_beside_the_original_picks_the_next_free_name() {
        let (td, mut app) = fixture();
        select(&mut app, "README.md");
        app.on_key(ctrl('c'));
        app.on_key(ctrl('v'));
        assert!(td.path().join("README copy.md").is_file(), "{}", app.status);

        select(&mut app, "README.md");
        app.on_key(ctrl('v'));
        assert!(
            td.path().join("README copy 2.md").is_file(),
            "{}",
            app.status
        );
    }

    #[test]
    fn pasting_a_folder_brings_its_contents() {
        let (td, mut app) = fixture();
        select(&mut app, "notes");
        app.on_key(ctrl('c'));
        select(&mut app, "src");
        app.on_key(ctrl('v'));
        assert!(
            td.path().join("src/notes/design.md").is_file(),
            "{}",
            app.status
        );
    }

    #[test]
    fn pasting_with_an_empty_clipboard_says_so() {
        let (_td, mut app) = fixture();
        app.on_key(ctrl('v'));
        assert!(app.status.contains("nothing copied"), "{}", app.status);
    }

    // ---- completion -------------------------------------------------------

    #[test]
    fn tab_completes_a_command_name() {
        let (_td, mut app) = fixture();
        assert_eq!(completed(&mut app, "cop"), "copy");
        assert_eq!(completed(&mut app, "del"), "delete");
    }

    #[test]
    fn tab_completes_a_path_argument() {
        let (_td, mut app) = fixture();
        assert_eq!(completed(&mut app, "copy REA"), "copy README.md");
        assert_eq!(
            completed(&mut app, "delete notes/des"),
            "delete notes/design.md"
        );
    }

    #[test]
    fn tab_completes_a_folder_with_a_separator_so_it_can_carry_on() {
        let (_td, mut app) = fixture();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(completed(&mut app, "copy not"), format!("copy notes{sep}"));
        assert_eq!(
            completed(&mut app, &format!("copy notes{sep}")),
            format!("copy notes{sep}design.md"),
        );
    }

    #[test]
    fn tab_fills_in_the_word_to_in_the_middle_of_a_copy() {
        let (_td, mut app) = fixture();
        assert_eq!(completed(&mut app, "copy README.md "), "copy README.md to");
        assert_eq!(completed(&mut app, "copy README.md t"), "copy README.md to");
    }

    #[test]
    fn tab_completes_the_destination_after_to() {
        let (_td, mut app) = fixture();
        assert_eq!(
            completed(&mut app, "copy README.md to sr"),
            format!("copy README.md to src{}", std::path::MAIN_SEPARATOR)
        );
    }

    #[test]
    fn tab_stops_where_the_candidates_stop_agreeing() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("report-one.md"), "").unwrap();
        fs::write(td.path().join("report-two.md"), "").unwrap();
        app.on_key(k(KeyCode::F(5)));
        // Two matches, so it fills in only what they share.
        assert_eq!(completed(&mut app, "copy rep"), "copy report-");
    }

    #[test]
    fn completion_leaves_dotfiles_alone_unless_asked_for() {
        let (td, mut app) = fixture();
        fs::write(td.path().join(".secret.md"), "").unwrap();
        app.on_key(k(KeyCode::F(5)));

        assert_eq!(
            completed(&mut app, "delete .sec"),
            "delete .secret.md",
            "a leading dot is someone asking for one"
        );
        // Without the dot it is not a candidate at all, so the visible entries
        // are all that is left and they share no prefix to fill in.
        assert_eq!(completed(&mut app, "delete "), "delete ");
    }

    #[test]
    fn the_project_root_cannot_be_deleted_or_renamed() {
        let (_td, mut app) = fixture();
        app.selected = 0;
        app.on_key(ch('d'));
        assert!(app.status.contains("cannot delete"));
        app.on_key(ch('r'));
        assert!(app.status.contains("cannot rename"));
    }

    #[test]
    fn a_typed_path_cannot_escape_the_project() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "../escaped.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.status.contains("escapes"), "{}", app.status);
        assert!(!td.path().parent().unwrap().join("escaped.md").exists());
    }

    // ---- quitting & misc --------------------------------------------------

    #[test]
    fn quitting_with_unsaved_work_asks_first() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "x");
        app.on_key(k(KeyCode::Esc));

        app.on_key(ch('q'));
        assert!(!app.should_quit);
        assert!(joined(&mut app).contains("Discard unsaved changes"));
        app.on_key(ch('n'));
        assert!(!app.should_quit);
        app.on_key(ch('q'));
        app.on_key(ch('y'));
        assert!(app.should_quit);
    }

    #[test]
    fn help_opens_and_any_key_closes_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        // Tall enough for the whole thing; the short-terminal case scrolls,
        // and has a test of its own.
        let out = screen(&mut app, 90, 70).join("\n");
        assert!(out.contains("Keys and commands"), "{out}");
        assert!(out.contains("search names and contents"), "{out}");
        assert!(out.contains("the map: boxes and lines"), "{out}");
        app.on_key(ch('x'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn help_lists_the_commands_as_well_as_the_keys() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        let out = screen(&mut app, 90, 70).join("\n");
        assert!(out.contains("*copy a to b"), "commands are in it:\n{out}");
        assert!(out.contains("*line 42"), "{out}");
        assert!(out.contains("*replace old new"), "{out}");
        assert!(
            out.contains("Ctrl+S"),
            "and the keys are still there:\n{out}"
        );
    }

    #[test]
    fn a_wide_window_puts_keys_and_commands_side_by_side() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        let rows = screen(&mut app, 130, 46);
        // Whichever command happens to line up with it — the point is that a
        // key and a command share a row.
        assert!(
            rows.iter()
                .any(|r| r.contains("move the cursor") && r.contains('*')),
            "two columns on one row:\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn a_narrow_window_stacks_them_into_one_column() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        let rows = screen(&mut app, 70, 70);
        let out = rows.join("\n");
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("move the cursor") && r.contains('*')),
            "no room for two:\n{out}"
        );
        assert!(out.contains("move the cursor"), "{out}");
        assert!(
            out.contains("*copy a to b"),
            "both are still listed:\n{out}"
        );
    }

    #[test]
    fn help_scrolls_on_a_terminal_too_short_for_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        let top = screen(&mut app, 80, 16).join("\n");
        assert!(top.contains("TREE"), "starts at the top:\n{top}");

        for _ in 0..12 {
            app.on_key(k(KeyCode::Down));
        }
        assert!(
            matches!(app.mode, Mode::Help(_)),
            "arrows scroll, not close"
        );
        let lower = screen(&mut app, 80, 16).join("\n");
        assert_ne!(top, lower, "the view moved");

        // The point of scrolling: the end of the list is reachable at all.
        app.on_key(k(KeyCode::End));
        let bottom = screen(&mut app, 80, 16).join("\n");
        assert!(
            bottom.contains("quit"),
            "the last entry is reachable:\n{bottom}"
        );
        assert!(
            !bottom.contains("move the cursor"),
            "and the top has scrolled off"
        );

        // Anything that is not a scroll key still puts it away.
        app.on_key(ch('z'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn dot_toggles_hidden_files() {
        let (td, mut app) = fixture();
        fs::write(td.path().join(".secret"), "x").unwrap();
        app.on_key(k(KeyCode::F(5)));
        assert!(!app.rows.iter().any(|r| r.name == ".secret"));
        app.on_key(ch('.'));
        assert!(app.rows.iter().any(|r| r.name == ".secret"));
    }

    #[test]
    fn refresh_keeps_buffers_that_have_unsaved_work() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "KEEP");
        app.on_key(k(KeyCode::Esc));
        app.on_key(k(KeyCode::F(5)));
        select(&mut app, "main.py");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("KEEP"));
    }

    #[test]
    fn split_args_keeps_quoted_runs_together() {
        assert_eq!(split_args("set tab_width 2"), ["set", "tab_width", "2"]);
        assert_eq!(
            split_args(r#"replace "old thing" "new thing""#),
            ["replace", "old thing", "new thing"]
        );
        assert_eq!(split_args("  spaced   out  "), ["spaced", "out"]);
        assert!(split_args("").is_empty());
        // An empty quoted string is a real argument, not nothing.
        assert_eq!(split_args(r#"replace "x" """#), ["replace", "x", ""]);
    }

    #[test]
    fn safe_join_rejects_paths_outside_the_root() {
        let root = Path::new("/project");
        assert!(safe_join(root, "notes/ok.md", root).is_ok());
        assert!(safe_join(root, "../outside.md", root).is_err());
        assert!(safe_join(root, "a/../../outside.md", root).is_err());
        assert!(safe_join(root, "/etc/passwd", root).is_err());
        assert_eq!(
            safe_join(root, "a/../b.md", root).unwrap(),
            PathBuf::from("/project/b.md")
        );
    }

    /// Not an assertion — a way to look at the panes.
    /// `TINY_SHOT=/path cargo test screenshot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn screenshot() {
        let dir = std::env::var("TINY_SHOT").expect("set TINY_SHOT");
        let file = std::env::var("TINY_SHOT_FILE").ok();
        let mut app = App::new(target(Path::new(&dir), None), Config::default(), None).unwrap();
        if let Some(f) = file {
            select(&mut app, &f);
        }
        if let Ok(keys) = std::env::var("TINY_SHOT_KEYS") {
            let mut chars = keys.chars();
            while let Some(c) = chars.next() {
                match c {
                    // `^b` sends Ctrl+B, so a shot can be taken of anything a
                    // key can reach.
                    '^' => {
                        if let Some(n) = chars.next() {
                            app.on_key(ctrl(n));
                        }
                    }
                    '\n' => app.on_key(k(KeyCode::Enter)),
                    '\t' => app.on_key(k(KeyCode::Tab)),
                    '↓' => app.on_key(k(KeyCode::Down)),
                    '⇧' => {
                        if let Some(n) = chars.next() {
                            let code = if n == '↑' {
                                KeyCode::Up
                            } else {
                                KeyCode::Down
                            };
                            app.on_key(shift(code));
                        }
                    }
                    _ => app.on_key(ch(c)),
                }
            }
        }
        let w: u16 = std::env::var("TINY_SHOT_W")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);
        let h: u16 = std::env::var("TINY_SHOT_H")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        for row in screen(&mut app, w, h) {
            println!("{row}");
        }
    }
}
