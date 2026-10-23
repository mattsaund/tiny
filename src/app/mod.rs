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
//! handful of questions anyone can ask it: where the root is, what the
//! cursor is on, which buffers are dirty. Everything it *does* is split by
//! the kind of thing the user was doing at the time:
//!
//! | file | what it handles |
//! |------|-----------------|
//! | [`mode`] | the overlay types: the bar, confirmations, settings, help |
//! | [`preview`] | what the cursor is on, and what the right pane becomes |
//! | [`input`] | every keypress, dispatched; and the mouse wheel |
//! | [`bar`] | the one field that is both a search and a command line |
//! | [`sentence`] | reading a typed command into its parts |
//! | [`plan`] | what a command would do, and doing it |
//! | [`command`] | what each `*command` does |
//! | [`fileops`] | new, rename, delete, copy, paste, save |
//! | [`clip`] | copying and pasting text |
//! | [`macros`] | recording keys, and playing them back |
//! | [`actions`] | the plain navigation keys and the view toggles |
//! | [`settings`] | the settings area and the keybinds window |
//! | [`prompt`] | answering a confirmation |
//! | [`parts`] | small helpers more than one of the above needs |
//! | [`split`] | the panes, side by side and stacked |
//! | [`term`] | the terminal under the panes |
//! | [`source`] | the git window: what has changed, and what to do about it |
//! | [`watch`] | noticing files another program changed |
//! | [`update`] | asking whether to install a newer tiny |
//! | [`plugins`] | what plugins are shown, and doing what they ask |
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
//! - [`Focus`]: which pane the keyboard belongs to, `Tree` or `Editor`.
//! - [`Mode`]: what, if anything, is layered on top: a confirmation, the
//!   search or command bar, the settings area, the keybinds window, the help
//!   overlay, or `Normal` for none of them.
//!
//! [`App::handle_key`] asks three things in order. Is it a key that switches
//! windows? Those work from everywhere. Is an overlay up? Then the overlay
//! has it. Otherwise the window on screen does, and in the main window that
//! means whichever pane `Focus` names.
//!
//! # `i j k l` are the arrow keys
//!
//! Everywhere a pane is being navigated rather than typed into, `i j k l` do
//! what the four arrows do (up, left, down, right) in the inverted-T anyone
//! who has held a keyboard sideways already knows. `I` and `K` go all the way,
//! the same as Home and End. They exist for keyboards with no cursor
//! cluster, and they are deliberately *not* the vim `h j k l`: `j` and `k`
//! cannot mean both left-down and down-up at once, and a layout that matches
//! the arrows is the one that needs no explaining.
//!
//! The bar, the editor, the commit message and a settings field being
//! edited are exempt, because there a letter is a letter being typed.
//!
//! # There is no reading mode
//!
//! There used to be a third flag deciding whether a focused text file showed
//! rendered output or raw source. It is gone. Markdown keeps its formatting
//! while you edit it (every block but the one under the cursor, see
//! [`crate::ui`]) so a separate rendered view was a step to press through
//! rather than a thing you wanted. Every text file now opens the same way,
//! straight into the editor, and `Focus` alone says who has the keyboard.
//!
//! The rendered view survives in one place: the preview pane while the *tree*
//! still has the keyboard, where there is no cursor and the file is a picture
//! of itself.
//!
//! # The mode take-and-replace pattern
//!
//! `handle_key` does `std::mem::replace(&mut self.mode, Mode::Normal)` and
//! hands the owned mode to a handler. This is not a borrow-checker workaround
//! so much as a useful default: a handler that does nothing leaves `Normal`
//! behind, so **closing is the default and staying open is explicit**. Every
//! handler that wants its mode to persist has to put it back, which is why
//! you see `self.mode = Mode::Bar(b)` at the end of so many branches. Forget
//! it and the overlay closes; that is the intended failure direction.
//!
//! # Buffers outlive the selection
//!
//! `buffers` is keyed by path and is never cleared wholesale. Arrowing past a
//! file opens it; arrowing away leaves it open if anything in it is yours (an
//! edit, a step to undo, a cursor moved) and lets it go otherwise, since
//! reading the file again gives back exactly the same thing. Beyond that,
//! only three things remove entries: a delete (drops the file and anything
//! under it, once the question has said how many unsaved files go too), a
//! rename (moves the entry to the new key so edits follow the file), and
//! finding that a file with no unsaved edits has stopped being readable
//! text, which [`watch`] does on its own and a refresh or a replace does on
//! purpose. A delete you said yes to is the only way unsaved work is ever
//! dropped; there is nowhere else for it to live.
//!
//! # Where drawing state lives
//!
//! `preview_len` and the `last_*` fields (pane heights, widths and columns)
//! are written by `ui` during a draw and read here by the key handlers and
//! the mouse wheel. They are how paging and scroll clamping know the size of
//! a window this module cannot see. On the very first frame they hold their
//! defaults, which is harmless: every use is clamped. The saved highlighting
//! and formatting are written while drawing too, but only `ui` reads them.

#[cfg(test)]
mod tests;

mod actions;
mod bar;
mod clip;
mod command;
mod fileops;
mod input;
mod macros;
mod mode;
mod parts;
mod plan;
mod plugins;
mod preview;
mod prompt;
mod sentence;
mod settings;
mod source;
mod split;
mod term;
mod update;
mod watch;

pub use self::bar::completion_for;
pub use self::mode::{
    BUTTONS, Bar, ConfirmKind, Focus, KEYBIND_BUTTONS, Keybinds, Mode, Settings, Window,
};
pub use self::preview::{Preview, TextKind};
pub use self::source::{BUTTONS as GIT_BUTTONS, GitFocus, GitPane, Row as GitRow};
pub use self::split::Split;
use self::watch::Watch;

use self::parts::display_name;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::config::keys::{Keyboard, Keymap};
use crate::config::{Config, Host, Palette};
use crate::files::project;
use crate::files::size::Size;
use crate::files::tree::{Row, Tree};
use crate::map::graph;
use crate::map::view::ProjectMap;
use crate::text::editor::Editor;
use crate::text::highlight::{Highlighter, Resume};
use crate::text::search::{self};

/// Everything mutable. One of these exists for the life of the program;
/// `ui::draw` reads it and `handle_key` writes it.
pub struct App {
    pub tree: Tree,
    /// Flattened tree, rebuilt after every structural change. The cursor
    /// indexes into this.
    pub rows: Vec<Row>,
    /// Cursor position in `rows`. Kept valid by every mutation path: a stale
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
    /// How much disk the thing under the cursor takes, for the right-hand end
    /// of the status line. Derived from the cursor by [`App::note_size`],
    /// never set directly.
    pub cursor_size: Option<Size>,
    /// Folder sizes already measured, so arrowing back onto one does not walk
    /// it again. Emptied by [`App::rebuild_rows`]; every way the tree can
    /// change goes through it, and any of them can change what a folder holds.
    size_cache: HashMap<PathBuf, Size>,
    /// Open files, keyed by path. Outlives the selection so unsaved edits
    /// survive arrowing away and back, see the module docs.
    pub buffers: HashMap<PathBuf, Editor>,
    /// The live config. Mutated by `*set` and the settings area; persisted
    /// only on an explicit `Ctrl+S`.
    pub config: Config,
    /// What every key does. Rebuilt from `config.keys` by
    /// [`App::apply_config`], so a rebinding takes effect on the next
    /// keypress without a restart.
    pub keymap: Keymap,
    /// What this terminal turned out to be able to send, which decides which
    /// keyboard the program has. Assumed whole until `main` has asked, see
    /// [`App::set_keyboard`].
    pub keyboard: Keyboard,
    /// What the terminal tiny is running in can show, which the `glyphs` and
    /// `colors` settings follow while they are `auto`. Assumed modern until
    /// `main` has looked, see [`App::set_host`].
    pub host: Host,
    /// Parsed styles. Rebuilt from `config.theme` by [`App::apply_config`], so
    /// a theme change repaints without a restart.
    pub palette: Palette,
    /// The theme is swapped in place when `syntax_theme` changes; the grammars
    /// are loaded once and kept.
    pub highlighter: Highlighter,
    /// Saved syntax-parser state for the buffer being edited, so a window deep
    /// in a long file does not have to be reached by parsing everything above
    /// it on every keystroke. Written by `ui` while drawing, see the module
    /// docs.
    pub highlight_cache: Resume,
    /// Which of the three windows is on screen. `Ctrl+G` and `Ctrl+L` switch
    /// to the other two, and back.
    pub window: Window,
    /// Everything the source-control window draws, see [`source`].
    pub git: GitPane,
    /// The project map, built when [`Window::Map`] is switched to and kept
    /// until the next time it is.
    pub project_map: Option<ProjectMap>,
    /// What `Ctrl+C` picked up, waiting for a `Ctrl+V`. A path, not a copy of
    /// the bytes: the file is read at paste time, so editing it in between
    /// pastes what is there now rather than a stale snapshot.
    pub clipboard: Option<PathBuf>,
    /// Text copied in a file, see [`clip`]. Tiny's own until `main` hands it
    /// the system's, so a test never touches the real clipboard.
    pub text_clipboard: crate::clipboard::Clipboard,
    /// The keys typed since recording started, while it goes on, see
    /// [`macros`].
    recording: Option<Vec<macros::Recorded>>,
    /// What was recorded last, which `Ctrl+P` plays.
    recorded: Vec<macros::Recorded>,
    /// A recording is playing, so it cannot be asked to play again from
    /// inside itself.
    playing: bool,
    /// `Ctrl+T` folds the tree away to give the whole window to the file.
    /// Purely a view state; nothing about the tree itself changes, so
    /// bringing it back costs no reading.
    pub tree_hidden: bool,
    /// The release check, while it is out asking, see [`update`].
    update_check: Option<crate::update::Check>,
    /// A newer version the check found, waiting for a moment it can be asked
    /// about without catching a keystroke meant for something else.
    update_found: Option<String>,
    /// The check was asked for with `*update`, so its answer is reported
    /// whatever it is, and the question goes up wherever you are.
    update_by_hand: bool,
    /// The version to install once tiny has quit, set by a `y` to the update
    /// question, and read by `main` after the terminal is handed back.
    pub update_on_exit: Option<String>,
    /// The project's files and folders, listed for looking up the names a
    /// command says. Kept while the bar is open, so it is not walked again on
    /// every keystroke; dropped when a command runs, see `plan`.
    names: Option<plan::Names>,
    /// The last count across the project of what a replace would change, so
    /// typing its replacement does not count the same thing again on every
    /// pause. Forgotten along with `names`.
    replace_count: Option<(search::Needle, search::Report)>,
    /// The last `goto` that went somewhere, and which of its places it went
    /// to, so running it again goes on to the next one.
    last_jump: Option<(String, usize)>,
    /// A plan is being made to describe under the bar, not to be carried
    /// out: anything too slow to do on a keystroke can say so and wait.
    planning_for_preview: bool,
    /// Every plugin, loaded, see [`crate::plugins`].
    pub plugins: crate::plugins::Plugins,
    /// Where plugins are loaded from. `None` until `main` says, so a test,
    /// which builds an `App` directly, never runs or opens anyone's real
    /// plugins folder unless it hands this one of its own.
    pub plugins_dir: Option<PathBuf>,
    /// Doing what a plugin's event hook asked, during which no further
    /// events fire, see `App::plugin_event`.
    in_plugin_event: bool,
    /// Where `Ctrl+S` in the settings writes `tiny.conf`. `None` until `main`
    /// says, for the same reason as `plugins_dir`: a test that saves the
    /// settings must never write over anybody's real config file.
    pub config_path: Option<PathBuf>,
    /// The rows each block of a note was formatted into, by its text and the
    /// width it was drawn at, so a keystroke formats the one block being
    /// typed in rather than the whole note again. Written by `ui` while
    /// drawing, like the highlighting; emptied when a setting changes how
    /// anything is drawn.
    pub block_rows: std::cell::RefCell<HashMap<u64, Vec<ratatui::text::Line<'static>>>>,
    /// Which pane had the keyboard when the tree was folded away, so unfolding
    /// puts it back and the key is a true toggle.
    focus_before_hide: Focus,
    /// The message on the status line. Every action sets this; it is the
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
    /// The panes, while the space beside the browser is split. See
    /// [`split`].
    pub split: Option<Split>,
    /// Where the pane with the keyboard was in the last frame, so a split
    /// that would leave a pane too small to read is refused before it is
    /// drawn.
    pub last_live_area: ratatui::layout::Rect,
    /// Where each pane without the keyboard was in the last frame, so the
    /// wheel can scroll the one it is over. Empty with one pane.
    pub last_pinned: Vec<(usize, ratatui::layout::Rect)>,
    /// The shell in the terminal pane, while there is one. See [`term`].
    pub term: Option<crate::shell::Shell>,
    /// Where the keyboard was before the terminal took it, so the key that
    /// leaves can give it back.
    focus_before_term: Focus,
    /// Where the terminal was in the last frame, so the wheel can scroll it.
    pub last_term_area: Option<ratatui::layout::Rect>,
    /// What the files and folders on screen looked like at the last scan,
    /// see [`watch`]. Nothing outside that module reads it.
    watch: Watch,
}

/// The palette `config` draws with in a terminal that can show what `host`
/// says: its theme, in the characters and colors the settings come to there.
fn palette_for(config: &Config, host: &Host) -> Palette {
    Palette::from_theme(&config.theme)
        .with_glyphs(config.glyphs.resolve(host))
        .with_colors(config.colors.resolve(host))
}

impl App {
    /// Build the initial state from a resolved target and a loaded config.
    ///
    /// The startup warning, if any, is shown in place of the usual greeting:
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
        let keyboard = Keyboard::default();
        let (keymap, keys_warning) = Keymap::new(&config.keys, keyboard);
        let tree = Tree::new(root, config.show_hidden);
        let rows = tree.flatten();
        let host = Host::default();
        let palette = palette_for(&config, &host);

        let opening = if target.created {
            "new project, ? for help".to_string()
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
            cursor_size: None,
            size_cache: HashMap::new(),
            buffers: HashMap::new(),
            config,
            keymap,
            keyboard,
            host,
            palette,
            highlighter,
            highlight_cache: Resume::default(),
            window: Window::Main,
            git: GitPane::default(),
            project_map: None,
            clipboard: None,
            text_clipboard: crate::clipboard::Clipboard::new(false),
            recording: None,
            recorded: Vec::new(),
            playing: false,
            tree_hidden: false,
            update_check: None,
            update_found: None,
            update_by_hand: false,
            update_on_exit: None,
            names: None,
            replace_count: None,
            last_jump: None,
            planning_for_preview: false,
            plugins: crate::plugins::Plugins::default(),
            plugins_dir: None,
            in_plugin_event: false,
            config_path: None,
            block_rows: Default::default(),
            focus_before_hide: Focus::Tree,
            status: warning
                .or(keys_warning)
                .or(theme_warning)
                .unwrap_or(opening),
            should_quit: false,
            last_edit_height: 20,
            last_tree_height: 20,
            last_tree_cols: None,
            split: None,
            // Until the first frame says otherwise, big enough: the very
            // first key cannot arrive before a frame has been drawn.
            last_live_area: ratatui::layout::Rect::new(0, 0, 1000, 1000),
            last_pinned: Vec::new(),
            term: None,
            focus_before_term: Focus::Tree,
            last_term_area: None,
            watch: Watch::default(),
        };
        app.note_dirs();
        app.sync_preview();

        // `tiny <file>` opens as an editor and nothing else: the tree folded
        // away, the file across the whole window, the keyboard already in it.
        // Naming one file is asking to edit that file, and everything else on
        // screen is in the way of it. Esc, or the fold key, brings the project
        // back. Nothing is lost; it is just not shown first.
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
    /// which names them so you know what you are being asked about.
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
    /// dirty, or (for a directory) something inside it is.
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

    /// The first key bound to an action, written the way someone would say
    /// it: `Ctrl+G`, `F1`, `/`, `r`.
    ///
    /// Read from the live keymap, so a hint or a message names what will
    /// actually work: after a rebinding, and on Windows, where a few keys
    /// differ. Never caret notation; `^V` is a thing you have to know already.
    pub fn key_name(&self, action: crate::config::keys::Action) -> String {
        let spec = self.keymap.spec(action);
        let first = spec.split_whitespace().next().unwrap_or_default();
        // A character on its own is shown as it is typed: `r` is not `R`,
        // which is Shift and r. In a chord the letter is a capital, the way
        // keyboards and menus write Ctrl+S.
        if first.chars().count() == 1 {
            return first.to_string();
        }
        first
            .split('+')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("+")
    }

    /// What goes in front of an open folder, a closed folder, and a file.
    pub fn tree_markers(&self) -> (&'static str, &'static str, &'static str) {
        let g = self.palette.glyphs;
        (g.folder_open, g.folder_closed, "  ")
    }

    /// Settings the link graph is built with, derived from the live config so
    /// `*set show_hidden true` changes the map too.
    pub fn graph_options(&self) -> graph::Options {
        graph::Options::from_config(&self.config)
    }

    /// Switch windows.
    ///
    /// The one place the three of them are chosen between, so what each switch
    /// costs is visible in one screen: the map is rebuilt on the way in and
    /// the other two cost nothing at all.
    pub(super) fn show_window(&mut self, window: Window) {
        if window == self.window && window != Window::Map {
            return;
        }
        self.window = window;
        match window {
            // Rebuilt on every switch rather than cached: it is the only way
            // to be sure it reflects files changed since the last look, and no
            // invalidation scheme would be simpler than building it again.
            Window::Map => self.open_map(),
            Window::Source => {
                self.refresh_git();
                self.status = match &self.git.error {
                    Some(e) => e.clone(),
                    None if self.git.status.is_clean() => {
                        format!(
                            "{}, nothing to commit",
                            self.git.status.branch.summary(self.palette.glyphs.arrow)
                        )
                    }
                    None => self.git.status.branch.summary(self.palette.glyphs.arrow),
                };
            }
            Window::Main => self.status = "back to the browser".into(),
        }
    }

    /// Build the map. Synchronous, so a large project pauses here, see
    /// `graph::build`.
    fn open_map(&mut self) {
        let root = self.root().to_path_buf();
        let options = self.graph_options();
        let view = ProjectMap::build(&root, &options, self.palette.glyphs.ellipsis);
        self.status = if view.graph.nodes.is_empty() {
            "nothing on the map yet".into()
        } else {
            format!("project map: {}", view.summary())
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
            max_results: search::MAX_RESULTS,
            ignore: self.config.search_ignore.clone(),
            show_hidden: self.config.show_hidden,
        }
    }
}
