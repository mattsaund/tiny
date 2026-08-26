//! Application state and input handling.
//!
//! Rendering lives in [`crate::ui`]; this folder owns everything else. The two
//! panes share one selection: moving the tree cursor changes what the preview
//! shows, and focusing the preview starts editing that same file.
//!
//! Open buffers are kept in a map rather than reloaded per selection, so
//! arrowing away from a file with unsaved edits and back does not lose them.
//!
//! # Where each part lives
//!
//! There is exactly one [`App`], and it is defined here along with the
//! handful of questions anyone can ask it — where the root is, what the
//! cursor is on, which buffers are dirty. Everything it *does* is split by
//! the kind of thing the user was doing at the time:
//!
//! | file | what it handles |
//! |------|-----------------|
//! | [`mode`] | the overlay types: the bar, prompts, confirmations, settings |
//! | [`preview`] | what the cursor is on, and what the right pane becomes |
//! | [`input`] | every keypress, dispatched; and the mouse wheel |
//! | [`bar`] | the one field that is both a search and a command line |
//! | [`command`] | what each `*command` does |
//! | [`fileops`] | new, rename, delete, copy, paste, save |
//! | [`actions`] | the plain navigation keys and the view toggles |
//! | [`settings`] | the settings area and the keybinds window |
//! | [`prompt`] | answering a prompt or a confirmation |
//! | [`parts`] | small helpers more than one of the above needs |
//!
//! They are all `impl App` blocks on the same struct. Splitting an impl across
//! files means anything used by a sibling has to say so with `pub(super)`,
//! which turns out to be a feature: the seams between these files are visible
//! in the signatures.
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
//! # There is no reading mode
//!
//! There used to be a third flag deciding whether a focused text file showed
//! rendered output or raw source. It is gone. Markdown keeps its formatting
//! while you edit it — every block but the one under the cursor, see
//! [`crate::ui`] — so a separate rendered view was a step to press through
//! rather than a thing you wanted. Every text file now opens the same way,
//! straight into the editor, and `Focus` alone says who has the keyboard.
//!
//! The rendered view survives in one place: the preview pane while the *tree*
//! still has the keyboard, where there is no cursor and the file is a picture
//! of itself.
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

#[cfg(test)]
mod tests;

mod actions;
mod bar;
mod command;
mod fileops;
mod input;
mod mode;
mod parts;
mod preview;
mod prompt;
mod settings;

pub use self::bar::completion_for;
pub use self::mode::{BUTTONS, Bar, Focus, KEYBIND_BUTTONS, Keybinds, Mode, Settings};
pub use self::preview::{Preview, TextKind};

use self::parts::display_name;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::config::keys::Keymap;
use crate::config::{Config, Markers, Palette};
use crate::files::project;
use crate::files::tree::{Row, Tree};
use crate::map::graph;
use crate::map::view::ProjectMap;
use crate::text::editor::Editor;
use crate::text::highlight::{Highlighter, Resume};
use crate::text::search::{self};

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
    /// Open files, keyed by path. Outlives the selection so unsaved edits
    /// survive arrowing away and back — see the module docs.
    pub buffers: HashMap<PathBuf, Editor>,
    /// The live config. Mutated by `*set` and the settings area; persisted
    /// only on an explicit `Ctrl+S`.
    pub config: Config,
    /// What every key does. Rebuilt from `config.keys` by
    /// [`App::apply_config`], so a rebinding takes effect on the next
    /// keypress without a restart.
    pub keymap: Keymap,
    /// Parsed styles. Rebuilt from `config.theme` by [`App::apply_config`], so
    /// a theme change repaints without a restart.
    pub palette: Palette,
    /// The theme is swapped in place when `syntax_theme` changes; the grammars
    /// are loaded once and kept.
    pub highlighter: Highlighter,
    /// Saved syntax-parser state for the buffer being edited, so a window deep
    /// in a long file does not have to be reached by parsing everything above
    /// it on every keystroke. Written by `ui` while drawing, like the media
    /// cache and for the same reason — see the module docs.
    pub highlight_cache: Resume,
    /// The project map, while it is being looked at.
    pub project_map: Option<ProjectMap>,
    /// What `Ctrl+C` picked up, waiting for a `Ctrl+V`. A path, not a copy of
    /// the bytes: the file is read at paste time, so editing it in between
    /// pastes what is there now rather than a stale snapshot.
    pub clipboard: Option<PathBuf>,
    /// `Ctrl+Space` folds the tree away to give the whole window to the file.
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
        let (keymap, keys_warning) = Keymap::new(&config.keys);
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
            buffers: HashMap::new(),
            config,
            keymap,
            palette,
            highlighter,
            highlight_cache: Resume::default(),
            project_map: None,
            clipboard: None,
            tree_hidden: false,
            focus_before_hide: Focus::Tree,
            status: warning
                .or(keys_warning)
                .or(theme_warning)
                .unwrap_or(opening),
            should_quit: false,
            last_edit_height: 20,
            last_tree_height: 20,
            last_tree_cols: None,
        };
        app.sync_preview();

        // `tiny <file>` opens as an editor and nothing else: the tree folded
        // away, the file across the whole window, the keyboard already in it.
        // Naming one file is asking to edit that file, and everything else on
        // screen is in the way of it. Esc, or the fold key, brings the project
        // back — nothing is lost, it is just not shown first.
        //
        // Only for a file there is something to type into. A picture cannot
        // take the keyboard, so it keeps the tree.
        if let Some(file) = target.file {
            app.reveal(&file);
            if matches!(app.preview, Preview::Buffer { .. }) {
                app.focus_editor();
                app.tree_hidden = true;
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

    /// Whether a tree row should wear the unsaved marker: the file itself is
    /// dirty, or — for a directory — something inside it is.
    ///
    /// Collapsing a folder used to hide the fact that there was unsaved work
    /// under it. Marking every ancestor means the star is always visible from
    /// the root, whatever is folded away.
    pub fn dirty_here_or_below(&self, path: &Path) -> bool {
        self.buffers
            .iter()
            // `starts_with` is component-wise, so it is true for the path
            // itself and never for a sibling that merely shares a prefix.
            .any(|(p, e)| e.dirty && p.starts_with(path))
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
}
