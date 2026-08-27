//! What is layered on top of the two panes.
//!
//! [`Mode`] is one of the two axes of state — see the folder's docs — and
//! every variant of it that carries something is defined here: the bar's
//! typed text and its results, a prompt waiting for a name, a confirmation
//! waiting for a yes, the settings area's cursor, the keybinds window's.
//!
//! These are data, not behaviour. Every one of them is *handled* somewhere
//! else ([`super::bar`], [`super::prompt`], [`super::settings`]) and what
//! lives here is only the shape of what those handlers are working on, so
//! that `App` can hold one of them without depending on any of them.

use std::path::PathBuf;

use crate::text::search::Hit;

/// Which pane has the keyboard. Combined with [`Mode`], this determines how
/// any given key is interpreted.
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
pub(super) const COMMAND_SIGIL: char = '*';

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
    /// Set when a search found nothing, so the bar can say so.
    pub searched: bool,
    /// The file that had the keyboard when the bar was opened, if any. Hits
    /// in it are listed first — see `App::run_search`.
    ///
    /// Remembered here rather than read back off the app, because stepping
    /// through results changes which file is open: read live, "the file you
    /// are in" would become the last result you looked at, and the ordering
    /// would rearrange itself under your cursor as you typed.
    pub home: Option<PathBuf>,
}

impl Bar {
    pub(super) fn new(input: String, home: Option<PathBuf>) -> Self {
        Self {
            cursor: input.chars().count(),
            input,
            results: Vec::new(),
            selected: 0,
            searched: false,
            home,
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

/// The two rows above the settings themselves: things you do, rather than
/// values you set.
///
/// They live at the top because they are what someone opening this area is
/// most often looking for — and because a button below thirty rows of settings
/// is a button nobody finds.
pub const BUTTONS: &[&str] = &["Keybinds", "Reset settings"];

/// The in-program settings area. Rows come from [`BUTTONS`] and then
/// `Config::settings_index`, so this holds only the cursor and whatever is
/// being typed.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Row under the cursor: the buttons first, then the settings.
    pub selected: usize,
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
    /// Throw away every setting and go back to the shipped ones.
    ResetSettings,
    /// Throw away every rebinding and go back to the shipped keyboard.
    ResetKeybinds,
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
/// [`App::on_key`](super::App::on_key), so an open overlay owns the keyboard.
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
    /// The keybinds window, opened from the settings area.
    Keybinds(Keybinds),
}

/// The keybinds window: every action, and the keys that reach it.
#[derive(Debug, Clone, Default)]
pub struct Keybinds {
    /// Row under the cursor: the reset button first, then one row per action.
    pub selected: usize,
    /// Set while the next keypress is being read as a new binding rather than
    /// as a key. This is the only place in tiny where a key is data.
    pub capturing: bool,
}

/// The button above the keybinds list.
pub const KEYBIND_BUTTONS: &[&str] = &["Reset keybinds"];
