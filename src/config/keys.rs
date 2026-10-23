//! Key bindings: what every key does, and how to change it.
//!
//! Every key tiny acts on is named. [`Action`] is the list of those names,
//! [`Keymap`] maps keys onto them, and the handlers in `app` ask the keymap
//! what a keypress meant instead of matching on the key itself. That
//! indirection is the whole feature: a binding can then come from a config
//! file or from the keybinds window, without a single `match` arm moving.
//!
//! # What is not bindable
//!
//! Keys that put a character into a file or a field are not here, and cannot
//! be. In the editor, the bar, and every prompt, `n` is the letter n — there is
//! no binding to change, because the key is not a command. The same goes for
//! the text-editing primitives beside them: the arrows, `Backspace`, `Delete`,
//! `Enter` and `Tab` move and edit text while you are typing into it, and
//! rebinding those would leave a keyboard that cannot type.
//!
//! Everything else — the browser, a note being read, the project map, the git
//! window, and every chord that reaches out of the pane you are in — is
//! bindable.
//!
//! # Contexts
//!
//! The same key means different things in different panes: `k` is *down* in the
//! browser and the letter k in the editor. [`Context`] says which set of
//! actions a keypress is being read against. The chords in [`Context::Global`]
//! are checked first everywhere, which is why `Ctrl+S` saves from wherever you
//! are.
//!
//! An action lists the panes it works in, and most movements work in several.
//! Moving up is one action, `up`, bound to one key; the browser, a note being
//! read, the map and the git window each answer it in their own way. `open` is
//! the same idea for Enter: it opens a file in the browser and on the map, and
//! stages one in the git window — one row to find, and one key to rebind.
//!
//! Everything in `Global` that has to work while you type is a chord. In the
//! editor a letter is a letter being typed, so a control that must work there
//! has no other shape available. A global may *also* carry a bare key, like
//! `/` beside `Ctrl+F`: [`Keymap::resolve_while_typing`] skips the bare one in
//! a pane that types, so it costs nothing there and saves a chord everywhere
//! else.
//!
//! # What is a key and what is a command
//!
//! Not everything tiny does is here, and that is the point. Keys are for
//! moving around and for changing a file: things done constantly, in the middle
//! of something else, where a key is the only shape fast enough. Deleting a
//! path, re-reading the disk, opening the settings — those are deliberate acts,
//! usually with something to type after them, and they are commands (`*delete`,
//! `*reload`, `*config`).
//!
//! # Every key arrives on every terminal
//!
//! A binding that some terminals never deliver is worse than none: the help
//! window promises it, and on the wrong machine nothing happens — or something
//! else does. So the shipped keyboard is made only of keys that arrive as
//! themselves on macOS Terminal, iTerm2, Windows Terminal and the usual Linux
//! emulators, with their stock settings:
//!
//! - **Ctrl and a letter.** Every terminal sends these, in raw mode, as
//!   themselves — except `Ctrl+H`, `Ctrl+I`, `Ctrl+J` and `Ctrl+M`, which *are*
//!   Backspace, Tab and Enter in the encoding, and `Ctrl+B`, which tmux keeps
//!   for its prefix. None of those five is bound.
//! - **Plain keys** — letters, `/`, `?`, `.`, the arrows, Enter, Esc, Tab — in
//!   the panes where nothing is being typed.
//!
//! What is *not* shipped, and why:
//!
//! - **Ctrl with a digit or a punctuation mark.** Not in a terminal's legacy
//!   encoding at all: `Ctrl+1` sends nothing, `Ctrl+2` arrives as `Ctrl+Space`
//!   and `Ctrl+3` as `Esc` — which, in the browser, used to quit.
//! - **Function keys.** On a Mac they are brightness and Mission Control unless
//!   Fn is held, and GNOME Terminal keeps `F1` for its own help. `F1` is still
//!   bound beside `?`, because on a PC keyboard it is where people look first.
//! - **Ctrl with an arrow.** macOS switches Spaces on `Ctrl+←` and `Ctrl+→`,
//!   and opens Mission Control on `Ctrl+↑` — the terminal never sees them.
//!   They stay bound in the editor beside a twin that a Mac does send, for the
//!   PC keyboards that expect them.
//! - **Ctrl+Space.** macOS switches input source on it.
//! - **Home, End, Page Up, Page Down** as the *only* way to do something. A
//!   MacBook has none of them, and Terminal uses all four to scroll its own
//!   history unless Shift is held. Each is bound beside a chord that works
//!   everywhere: `Ctrl+A` and `Ctrl+E` for the ends of a line, the way every
//!   Mac text field and every shell already does it, and `Ctrl+U` and `Ctrl+D`
//!   for a screen at a time, the way `less` and `vim` do.
//!
//! The one key a terminal takes for itself is `Ctrl+V`: Windows Terminal and
//! the Windows console paste on it before the program sees anything. That is
//! the whole of [`Keyboard`] — on Windows, file paste ships on `Ctrl+P`.
//!
//! # Option and the arrows
//!
//! On a Mac, `⌥←` and `⌥→` are how a word is skipped, and the terminals do not
//! agree on how to say so: iTerm2 sends them as `Alt` with an arrow, and macOS
//! Terminal sends the `Esc b` and `Esc f` that a shell's readline expects —
//! `Alt+B` and `Alt+F`. [`super::keyspec::from_terminal`] turns the second
//! spelling into the first before anything else looks at the key, so one
//! binding, `alt+left`, answers both.
//!
//! # One key per thing
//!
//! Two keys that do the same thing are one key and one thing to remember for
//! nothing, so the only second keys are ones that stand in for a key some
//! keyboards lack: `i` `j` `k` `l` for the arrows, `I` and `K` for Home and
//! End, the chords above for the Mac. And two *actions* that do the same thing
//! are worse, because each is a row to read past; the browser's Tab, which did
//! what Enter does, is gone, and so are the five-line jumps that a screen at a
//! time already covered.
//!
//! # Defaults and overrides
//!
//! [`Action::defaults`] is the shipped keyboard, written out in one table. The
//! config file holds *only* what differs from it, so an untouched `tiny.conf`
//! has no `[keys]` section at all and a new default reaches everyone who has
//! not overridden that action. Resetting is therefore just dropping the
//! overrides. Names that have changed are carried over by [`migrate`], so a
//! config written by an older tiny keeps meaning what it meant.

use std::collections::BTreeMap;
use std::fmt::Write as _;

pub use super::keyspec::{Key, from_terminal, spec_of};
use crossterm::event::KeyEvent;

/// Which set of actions a keypress is read against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Chords that work from any pane. Checked before the pane's own set.
    Global,
    Tree,
    /// A preview with no text buffer behind it — a picture, a directory, a
    /// binary. There is nothing to type into, so the arrows scroll the view.
    View,
    /// The editor, and the one key that leaves it.
    Editor,
    Map,
    /// The source-control window: the change list and its buttons.
    Source,
}

impl Context {
    pub fn title(self) -> &'static str {
        match self {
            Context::Global => "ANYWHERE",
            Context::Tree => "BROWSER",
            Context::View => "VIEWING",
            Context::Editor => "EDITOR",
            Context::Map => "MAP",
            Context::Source => "GIT",
        }
    }
}

/// Which keys the terminal keeps for itself.
///
/// One key differs, and only on Windows: Windows Terminal, and the Windows
/// console with its Ctrl shortcuts on, paste the clipboard on `Ctrl+V` before
/// tiny sees the key — so a file paste there ships on `Ctrl+P`, which reaches
/// the program. Everything else is the same keyboard everywhere.
///
/// Decided once, by [`Keyboard::detect`] in `main`, and passed in rather than
/// looked up inside the keymap, so the tests can build either keyboard on any
/// machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keyboard {
    #[default]
    Standard,
    /// Windows Terminal — native, or under WSL — or the Windows console.
    Windows,
}

/// What changes on [`Keyboard::Windows`]. Substitutions, not additions: the
/// `Ctrl+V` that would never arrive is not listed, because a key the keybinds
/// window shows is a key that works.
const WINDOWS_KEYS: &[(Action, &str)] = &[(Action::Paste, "ctrl+p")];

impl Keyboard {
    /// Which keyboard this terminal gets.
    ///
    /// A native Windows build always runs in a Windows terminal. Under WSL the
    /// program is a Linux one, so the terminal has to say so itself: Windows
    /// Terminal sets `WT_SESSION`, and passes it through to WSL.
    pub fn detect() -> Self {
        if cfg!(windows) || std::env::var_os("WT_SESSION").is_some() {
            Keyboard::Windows
        } else {
            Keyboard::Standard
        }
    }

    /// The keys `action` ships with on this keyboard.
    pub fn defaults(self, action: Action) -> &'static str {
        if self == Keyboard::Windows
            && let Some((_, keys)) = WINDOWS_KEYS.iter().find(|(a, _)| *a == action)
        {
            return keys;
        }
        action.defaults()
    }
}

/// Every key tiny acts on, by name.
///
/// The order here is the order the keybinds window lists them in, and the
/// index into [`Keymap`]'s table — so adding one in the middle is fine, but it
/// must also gain a row in [`TABLE`]. The `every_action_is_in_the_table` test
/// checks that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // Everywhere. Each of these works while you are typing into a file as
    // well as from the browser, so each has a chord.
    Save,
    Quit,
    Bar,
    Help,
    WindowSource,
    WindowMap,
    ToggleTreePane,
    NewWindow,
    Split,
    PaneLeft,
    PaneRight,
    Copy,
    Paste,
    // In every pane that has them. One name for one movement, however many
    // panes can make that movement — see the module docs.
    Up,
    Down,
    Left,
    Right,
    First,
    Last,
    PageUp,
    PageDown,
    Open,
    Back,
    Refresh,
    Narrower,
    Wider,
    // The browser alone
    TreeHidden,
    // A file being edited
    EditorUndo,
    EditorRedo,
    EditorDeleteLine,
    EditorWordLeft,
    EditorWordRight,
    EditorLineStart,
    EditorLineEnd,
    EditorDocStart,
    EditorDocEnd,
    // The project map
    MapNext,
    MapPrevious,
    MapWikilinks,
    MapLinks,
    MapCalls,
    // Source control
    SourceOpen,
}

/// Every action, the panes it works in, its name in the config file, a word
/// or two on what it does, and the keys it ships with.
///
/// One table rather than five methods: a row that goes missing is a row that
/// goes missing everywhere at once, which is far easier to notice.
///
/// The description is a hint, not a sentence: the keybinds window shows it
/// only beside the row under the cursor, and leaves it empty where the name
/// already says everything.
type Row = (
    Action,
    &'static [Context],
    &'static str,
    &'static str,
    &'static str,
);

/// The panes that are navigated rather than typed into.
const PANES: &[Context] = &[Context::Tree, Context::View, Context::Map, Context::Source];

const TABLE: &[Row] = &[
    (
        Action::Save,
        &[Context::Global],
        "save",
        "a folder saves all",
        "ctrl+s",
    ),
    (
        Action::Quit,
        &[Context::Global],
        "quit",
        "asks if unsaved",
        "ctrl+q",
    ),
    (
        Action::Bar,
        &[Context::Global],
        "search",
        "* for commands",
        "/ ctrl+f",
    ),
    (Action::Help, &[Context::Global], "help", "", "? f1"),
    (
        Action::WindowSource,
        &[Context::Global],
        "git",
        "source control",
        "ctrl+g",
    ),
    (
        Action::WindowMap,
        &[Context::Global],
        "map",
        "project map",
        "ctrl+l",
    ),
    (
        Action::ToggleTreePane,
        &[Context::Global],
        "browser",
        "hide or show",
        "ctrl+t",
    ),
    (
        Action::NewWindow,
        &[Context::Global],
        "new_window",
        "another tiny, here",
        "ctrl+n",
    ),
    // Ctrl with a letter and Shift with an arrow are the two kinds of chord
    // every terminal sends the same way, on every system and every keyboard
    // layout. `Ctrl+\`, the usual split key elsewhere, is neither: it reaches
    // tiny as `Ctrl+4` on Linux and macOS, and needs AltGr on many layouts.
    (
        Action::Split,
        &[Context::Global],
        "split",
        "two panes, and back",
        "ctrl+w",
    ),
    (
        Action::PaneLeft,
        &[Context::Global],
        "pane_left",
        "with two panes",
        "shift+left",
    ),
    (
        Action::PaneRight,
        &[Context::Global],
        "pane_right",
        "with two panes",
        "shift+right",
    ),
    (
        Action::Copy,
        &[Context::Global],
        "copy",
        "this file",
        "ctrl+c",
    ),
    (
        Action::Paste,
        &[Context::Global],
        "paste",
        "into this folder",
        "ctrl+v",
    ),
    (Action::Up, PANES, "up", "", "up i"),
    (Action::Down, PANES, "down", "", "down k"),
    (
        Action::Left,
        &[Context::Tree, Context::Map, Context::Source],
        "left",
        "out of a folder",
        "left j",
    ),
    (
        Action::Right,
        &[Context::Tree, Context::Map, Context::Source],
        "right",
        "into a folder",
        "right l",
    ),
    (
        Action::First,
        &[Context::Tree, Context::View],
        "first",
        "",
        "home I",
    ),
    (
        Action::Last,
        &[Context::Tree, Context::View],
        "last",
        "",
        "end K",
    ),
    (
        Action::PageUp,
        &[
            Context::Tree,
            Context::View,
            Context::Source,
            Context::Editor,
        ],
        "page_up",
        "",
        "pageup ctrl+u",
    ),
    (
        Action::PageDown,
        &[
            Context::Tree,
            Context::View,
            Context::Source,
            Context::Editor,
        ],
        "page_down",
        "",
        "pagedown ctrl+d",
    ),
    (
        Action::Open,
        &[Context::Tree, Context::Map, Context::Source],
        "open",
        "or stage, in git",
        "enter",
    ),
    (
        Action::Back,
        &[
            Context::Tree,
            Context::Editor,
            Context::Map,
            Context::Source,
        ],
        "back",
        "or quit, from the browser",
        "esc",
    ),
    (
        Action::Refresh,
        &[Context::Map, Context::Source],
        "refresh",
        "",
        "r",
    ),
    (
        Action::Narrower,
        &[Context::Tree, Context::Source],
        "narrower",
        "the browser",
        "<",
    ),
    (
        Action::Wider,
        &[Context::Tree, Context::Source],
        "wider",
        "the browser",
        ">",
    ),
    (
        Action::TreeHidden,
        &[Context::Tree],
        "tree.hidden",
        "dotfiles",
        ".",
    ),
    (
        Action::EditorUndo,
        &[Context::Editor],
        "editor.undo",
        "",
        "ctrl+z",
    ),
    (
        Action::EditorRedo,
        &[Context::Editor],
        "editor.redo",
        "",
        "ctrl+y",
    ),
    (
        Action::EditorDeleteLine,
        &[Context::Editor],
        "editor.delete_line",
        "",
        "ctrl+k",
    ),
    (
        Action::EditorWordLeft,
        &[Context::Editor],
        "editor.word_left",
        "",
        "ctrl+left alt+left",
    ),
    (
        Action::EditorWordRight,
        &[Context::Editor],
        "editor.word_right",
        "",
        "ctrl+right alt+right",
    ),
    (
        Action::EditorLineStart,
        &[Context::Editor],
        "editor.line_start",
        "",
        "home ctrl+a",
    ),
    (
        Action::EditorLineEnd,
        &[Context::Editor],
        "editor.line_end",
        "",
        "end ctrl+e",
    ),
    (
        Action::EditorDocStart,
        &[Context::Editor],
        "editor.start",
        "first line",
        "ctrl+home",
    ),
    (
        Action::EditorDocEnd,
        &[Context::Editor],
        "editor.end",
        "last line",
        "ctrl+end",
    ),
    (Action::MapNext, &[Context::Map], "map.next", "", "tab"),
    (
        Action::MapPrevious,
        &[Context::Map],
        "map.previous",
        "",
        "backtab",
    ),
    (
        Action::MapWikilinks,
        &[Context::Map],
        "map.wikilinks",
        "show or hide",
        "1",
    ),
    (
        Action::MapLinks,
        &[Context::Map],
        "map.links",
        "show or hide",
        "2",
    ),
    (
        Action::MapCalls,
        &[Context::Map],
        "map.calls",
        "show or hide",
        "3",
    ),
    (
        Action::SourceOpen,
        &[Context::Source],
        "source.open",
        "edit the file",
        "tab",
    ),
];

/// Names an older tiny wrote, and what they are called now.
///
/// Several old names can land on one new one: `open` is what the browser's,
/// the map's and the git window's Enter each used to be called on their own.
const RENAMED: &[(&str, &str)] = &[
    ("bar", "search"),
    ("window_source", "git"),
    ("window_map", "map"),
    ("fold_tree", "browser"),
    ("tree.open", "open"),
    ("map.open", "open"),
    ("source.stage", "open"),
];

/// Names an older tiny wrote for actions that are gone. Each was a second way
/// to do something another key already does — see the module docs — so an
/// override of one is dropped rather than guessed onto a neighbor, where it
/// could take a key like Esc away from the action that needs it.
const RETIRED: &[&str] = &[
    "command",
    "window_main",
    "jump_up",
    "jump_down",
    "tree.preview",
    "tree.quit",
];

/// Bring a `[keys]` section written by an older tiny up to date, in place.
///
/// A renamed action keeps its override under the new name — unless the new
/// name has one of its own, which is the more recent choice and wins. A retired
/// one is dropped. Anything else is left alone, including names nobody
/// answers to: those are typos, and [`Keymap::new`] says so.
pub fn migrate(keys: &mut BTreeMap<String, String>) {
    for (old, new) in RENAMED {
        if let Some(spec) = keys.remove(*old) {
            keys.entry((*new).to_string()).or_insert(spec);
        }
    }
    keys.retain(|name, _| !RETIRED.contains(&name.as_str()));
}

impl Action {
    /// Every action, in the order the keybinds window shows them.
    pub fn all() -> impl Iterator<Item = Action> {
        TABLE.iter().map(|r| r.0)
    }

    /// Where this action sits in [`TABLE`], which is also where its keys sit
    /// in a [`Keymap`].
    fn index(self) -> usize {
        TABLE
            .iter()
            .position(|r| r.0 == self)
            .expect("every action is in TABLE")
    }

    fn row(self) -> &'static Row {
        &TABLE[self.index()]
    }

    /// Every pane this action works in. Most movements work in several.
    pub fn contexts(self) -> &'static [Context] {
        self.row().1
    }

    /// The heading the keybinds window files this action under.
    ///
    /// An action that works in more than one pane belongs to none of them, so
    /// it gets a heading of its own rather than being filed under whichever
    /// pane happened to be listed first.
    pub fn group(self) -> &'static str {
        match self.contexts() {
            [one] => one.title(),
            _ => "PANES",
        }
    }

    /// The name in the config file, e.g. `editor.undo`.
    pub fn name(self) -> &'static str {
        self.row().2
    }

    /// A word or two more than the name says. Often empty.
    pub fn describe(self) -> &'static str {
        self.row().3
    }

    /// The keys this action ships with on a [`Keyboard::Standard`] terminal,
    /// as they would be written in the config.
    ///
    /// The keyboard someone actually has may differ — see [`Keyboard`] — so
    /// anything comparing against "the shipped key" wants
    /// [`Keymap::shipped`], not this.
    pub fn defaults(self) -> &'static str {
        self.row().4
    }

    pub fn from_name(name: &str) -> Option<Action> {
        TABLE.iter().find(|r| r.2 == name).map(|r| r.0)
    }
}

/// What every key does, right now.
///
/// Built from [`Action::defaults`] with the config's overrides laid on top, so
/// a binding that is not mentioned in the config is whatever tiny shipped with
/// — including one added in a later version.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// One entry per row of [`TABLE`], in the same order, so an action's keys
    /// are found by position rather than by searching for the action again.
    binds: Vec<Vec<Key>>,
    /// Which shipped keyboard this was built from, so the settings area can
    /// tell a changed binding from one that was always going to be this.
    keyboard: Keyboard,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new(&BTreeMap::new(), Keyboard::default()).0
    }
}

impl Keymap {
    /// Build the live keymap. Returns anything worth telling the user about a
    /// line in their config that could not be read.
    pub fn new(overrides: &BTreeMap<String, String>, keyboard: Keyboard) -> (Self, Option<String>) {
        let mut warning = None;
        let mut binds = Vec::with_capacity(TABLE.len());
        for (action, _, name, _, _) in TABLE {
            let shipped = keyboard.defaults(*action);
            let spec = overrides.get(*name).map(String::as_str).unwrap_or(shipped);
            let mut keys = Vec::new();
            for word in spec.split_whitespace() {
                match Key::parse(word) {
                    Some(k) => keys.push(k),
                    None => {
                        warning
                            .get_or_insert_with(|| format!("keys: `{word}` is not a key ({name})"));
                    }
                }
            }
            binds.push(keys);
        }
        // A name nobody answers to is almost always a typo, and silently doing
        // nothing about it is how someone spends ten minutes wondering why
        // their config has no effect.
        if let Some(unknown) = overrides.keys().find(|n| Action::from_name(n).is_none()) {
            warning.get_or_insert_with(|| format!("keys: nothing is called `{unknown}`"));
        }
        (Self { binds, keyboard }, warning)
    }

    /// The keys `action` ships with on the keyboard this map was built for.
    ///
    /// What "back to the shipped key" means, and what an override is compared
    /// against — both of which differ by terminal, so neither can read the
    /// table directly.
    pub fn shipped(&self, action: Action) -> &'static str {
        self.keyboard.defaults(action)
    }

    /// What this keypress means in `ctx`, with the global chords checked first
    /// so `Ctrl+S` cannot be shadowed by a pane's own binding.
    pub fn resolve(&self, ctx: Context, ev: &KeyEvent) -> Option<Action> {
        self.find(Context::Global, ev)
            .or_else(|| self.find(ctx, ev))
    }

    /// [`resolve`](Self::resolve), for a pane that is typing.
    ///
    /// A global bound to a bare character does not fire here: in a file, or a
    /// commit message, that character is the character. Chords still do, which
    /// is what they are for — `Ctrl+S` saves from inside a file and `s` is an
    /// s.
    ///
    /// This is what lets one `search` action carry both `ctrl+f` and `/`,
    /// instead of one action per pane that wanted the short key.
    pub fn resolve_while_typing(&self, ctx: Context, ev: &KeyEvent) -> Option<Action> {
        self.find_chord(Context::Global, ev)
            .or_else(|| self.find(ctx, ev))
    }

    /// Like [`find`](Self::find), skipping any binding that is a plain
    /// character.
    fn find_chord(&self, ctx: Context, ev: &KeyEvent) -> Option<Action> {
        TABLE
            .iter()
            .zip(&self.binds)
            .find(|((_, cs, ..), keys)| {
                cs.contains(&ctx) && keys.iter().any(|k| !k.is_typing() && k.matches(ev))
            })
            .map(|((a, ..), _)| *a)
    }

    /// What this keypress means in `ctx` alone. For the panes that do their own
    /// global handling.
    pub fn find(&self, ctx: Context, ev: &KeyEvent) -> Option<Action> {
        TABLE
            .iter()
            .zip(&self.binds)
            .find(|((_, cs, ..), keys)| cs.contains(&ctx) && keys.iter().any(|k| k.matches(ev)))
            .map(|((a, ..), _)| *a)
    }

    /// The keys bound to an action, as they would be written in the config.
    pub fn spec(&self, action: Action) -> String {
        let keys = self.keys(action);
        let mut out = String::new();
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let _ = write!(out, "{k}");
        }
        out
    }

    pub fn keys(&self, action: Action) -> &[Key] {
        &self.binds[action.index()]
    }

    /// Every other action already using `key`, so a rebinding can say what it
    /// is about to shadow.
    ///
    /// Two actions clash when they share a key *and* a pane. Sharing only a
    /// key is how Tab steps through the map and opens a file from the git
    /// window.
    pub fn clashes(&self, action: Action, key: &Key) -> Vec<Action> {
        let contexts = action.contexts();
        TABLE
            .iter()
            .zip(&self.binds)
            .filter(|((a, cs, ..), keys)| {
                *a != action && cs.iter().any(|c| contexts.contains(c)) && keys.contains(key)
            })
            .map(|((a, ..), _)| *a)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn every_action_is_in_the_table_exactly_once() {
        let mut names: Vec<&str> = Action::all().map(|a| a.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two actions share a name");
        assert_eq!(count, TABLE.len());
    }

    #[test]
    fn every_default_is_a_key_that_parses() {
        for keyboard in [Keyboard::Standard, Keyboard::Windows] {
            for action in Action::all() {
                let spec = keyboard.defaults(action);
                assert!(!spec.is_empty(), "{} ships unbound", action.name());
                for word in spec.split_whitespace() {
                    assert!(
                        Key::parse(word).is_some(),
                        "{}: `{word}` does not parse",
                        action.name()
                    );
                }
            }
        }
    }

    #[test]
    fn no_two_actions_in_one_context_ship_with_the_same_key() {
        for keyboard in [Keyboard::Standard, Keyboard::Windows] {
            let map = Keymap::new(&BTreeMap::new(), keyboard).0;
            for action in Action::all() {
                for key in map.keys(action) {
                    let clash = map.clashes(action, key);
                    assert!(
                        clash.is_empty(),
                        "{} and {:?} both ship with {key}",
                        action.name(),
                        clash.iter().map(|a| a.name()).collect::<Vec<_>>()
                    );
                }
            }
        }
    }

    #[test]
    fn a_global_chord_never_shadows_a_pane_key() {
        // Globals are checked first, so a pane binding on the same key would
        // be listed in the keybinds window and never fire.
        let map = Keymap::default();
        let globals: Vec<&Key> = Action::all()
            .filter(|a| a.contexts() == [Context::Global])
            .flat_map(|a| map.keys(a))
            .collect();
        for action in Action::all().filter(|a| a.contexts() != [Context::Global]) {
            for key in map.keys(action) {
                assert!(
                    !globals.contains(&key),
                    "{} ships with {key}, which a global answers first",
                    action.name()
                );
            }
        }
    }

    /// The keys a stock macOS Terminal never delivers, or delivers as
    /// something else — see the module docs.
    fn reaches_a_mac(key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        let dead = key.starts_with('f') && key.len() > 1 && key[1..].parse::<u8>().is_ok()
            || matches!(
                key.as_str(),
                "home" | "end" | "pageup" | "pagedown" | "ctrl+space"
            )
            || key.starts_with("ctrl+") && {
                let rest = &key["ctrl+".len()..];
                matches!(rest, "up" | "down" | "left" | "right" | "home" | "end")
                    || rest.len() == 1 && !rest.chars().all(|c| c.is_ascii_alphabetic())
            };
        !dead
    }

    #[test]
    fn every_action_has_a_key_that_reaches_a_mac() {
        let map = Keymap::default();
        // The two ends of a document are the honest exception: there is no
        // chord a stock Mac terminal sends for them that is not already
        // something else. A screen at a time, and `*line`, cover the distance.
        let allowed = [Action::EditorDocStart, Action::EditorDocEnd];
        for action in Action::all().filter(|a| !allowed.contains(a)) {
            let spec = map.spec(action);
            assert!(
                spec.split_whitespace().any(reaches_a_mac),
                "{} is only on `{spec}`, which macOS Terminal does not send",
                action.name()
            );
        }
    }

    #[test]
    fn nothing_ships_on_a_chord_a_terminal_cannot_spell() {
        // Ctrl with a digit or punctuation, and the five Ctrl letters that are
        // other keys or tmux's: each arrives as something else, or not at all.
        let map = Keymap::default();
        for action in Action::all() {
            for key in map.spec(action).split_whitespace() {
                if let Some(rest) = key.strip_prefix("ctrl+")
                    && rest.chars().count() == 1
                {
                    let c = rest.chars().next().unwrap();
                    assert!(
                        c.is_ascii_alphabetic() && !"bhijm".contains(c),
                        "{} ships on {key}",
                        action.name()
                    );
                }
            }
        }
    }

    #[test]
    fn windows_pastes_on_a_key_its_terminal_lets_through() {
        let windows = Keymap::new(&BTreeMap::new(), Keyboard::Windows).0;
        let standard = Keymap::default();
        assert_eq!(standard.spec(Action::Paste), "ctrl+v");
        assert_eq!(windows.spec(Action::Paste), "ctrl+p");
        assert_eq!(
            windows.resolve(
                Context::Tree,
                &ev(KeyCode::Char('p'), KeyModifiers::CONTROL)
            ),
            Some(Action::Paste)
        );
        // Everything else is the same keyboard either way.
        for action in Action::all().filter(|a| *a != Action::Paste) {
            assert_eq!(
                windows.spec(action),
                standard.spec(action),
                "{}",
                action.name()
            );
        }
    }

    #[test]
    fn a_rebinding_wins_on_either_keyboard() {
        let mut over = BTreeMap::new();
        over.insert("paste".to_string(), "ctrl+o".to_string());
        for keyboard in [Keyboard::Standard, Keyboard::Windows] {
            let (map, warning) = Keymap::new(&over, keyboard);
            assert!(warning.is_none());
            assert_eq!(map.spec(Action::Paste), "ctrl+o", "{keyboard:?}");
            // What it would go back to still depends on the terminal, which is
            // what `shipped` is for.
            let shipped = if keyboard == Keyboard::Standard {
                "ctrl+v"
            } else {
                "ctrl+p"
            };
            assert_eq!(map.shipped(Action::Paste), shipped);
        }
    }

    #[test]
    fn an_older_config_keeps_meaning_what_it_meant() {
        let mut keys = BTreeMap::new();
        keys.insert("bar".to_string(), "ctrl+o".to_string());
        keys.insert("window_map".to_string(), "ctrl+w".to_string());
        keys.insert("tree.open".to_string(), "o".to_string());
        keys.insert("jump_up".to_string(), "ctrl+up".to_string());
        keys.insert("editor.undo".to_string(), "ctrl+u".to_string());
        migrate(&mut keys);

        assert_eq!(keys.get("search").map(String::as_str), Some("ctrl+o"));
        assert_eq!(keys.get("map").map(String::as_str), Some("ctrl+w"));
        assert_eq!(keys.get("open").map(String::as_str), Some("o"));
        assert_eq!(keys.get("editor.undo").map(String::as_str), Some("ctrl+u"));
        assert!(!keys.contains_key("jump_up"), "a retired action is dropped");
        assert!(!keys.contains_key("bar"), "the old name is gone");

        let (map, warning) = Keymap::new(&keys, Keyboard::Standard);
        assert!(warning.is_none(), "nothing left to complain about");
        assert_eq!(
            map.resolve(
                Context::Tree,
                &ev(KeyCode::Char('w'), KeyModifiers::CONTROL)
            ),
            Some(Action::WindowMap)
        );
    }

    #[test]
    fn a_new_name_written_alongside_an_old_one_wins() {
        let mut keys = BTreeMap::new();
        keys.insert("search".to_string(), "ctrl+o".to_string());
        keys.insert("bar".to_string(), "ctrl+n".to_string());
        migrate(&mut keys);
        assert_eq!(keys.get("search").map(String::as_str), Some("ctrl+o"));
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn a_key_survives_being_written_down_and_read_back() {
        for spec in [
            "ctrl+s",
            "alt+up",
            "f5",
            "enter",
            "esc",
            "tab",
            "backtab",
            "pageup",
            "pagedown",
            "home",
            "end",
            "space",
            "backspace",
            "delete",
            ".",
            "?",
            "<",
            "i",
            "I",
            "1",
        ] {
            let key = Key::parse(spec).unwrap_or_else(|| panic!("{spec} did not parse"));
            assert_eq!(key.to_string(), spec, "round trip");
        }
    }

    #[test]
    fn a_letter_carries_its_own_shift() {
        let upper = Key::parse("I").unwrap();
        let lower = Key::parse("i").unwrap();
        assert!(upper.matches(&ev(KeyCode::Char('I'), KeyModifiers::SHIFT)));
        assert!(upper.matches(&ev(KeyCode::Char('I'), KeyModifiers::NONE)));
        assert!(!upper.matches(&ev(KeyCode::Char('i'), KeyModifiers::NONE)));
        assert!(!lower.matches(&ev(KeyCode::Char('I'), KeyModifiers::SHIFT)));
    }

    #[test]
    fn shift_is_compared_for_keys_that_are_not_letters() {
        let shifted = Key::parse("shift+up").unwrap();
        let plain = Key::parse("up").unwrap();
        assert!(shifted.matches(&ev(KeyCode::Up, KeyModifiers::SHIFT)));
        assert!(!shifted.matches(&ev(KeyCode::Up, KeyModifiers::NONE)));
        assert!(!plain.matches(&ev(KeyCode::Up, KeyModifiers::SHIFT)));
        assert!(plain.matches(&ev(KeyCode::Up, KeyModifiers::NONE)));
    }

    #[test]
    fn ctrl_must_match_both_ways() {
        let chord = Key::parse("ctrl+s").unwrap();
        assert!(chord.matches(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert!(!chord.matches(&ev(KeyCode::Char('s'), KeyModifiers::NONE)));
        let bare = Key::parse("s").unwrap();
        assert!(!bare.matches(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        assert!(Key::parse("").is_none());
        assert!(Key::parse("ctrl+").is_none());
        assert!(Key::parse("wibble").is_none());
        assert!(Key::parse("f13").is_none());
    }

    #[test]
    fn resolving_reads_a_key_against_one_context() {
        let map = Keymap::default();
        let k = ev(KeyCode::Char('k'), KeyModifiers::NONE);
        // One action, answered by each pane in its own way.
        assert_eq!(map.resolve(Context::Tree, &k), Some(Action::Down));
        assert_eq!(map.resolve(Context::View, &k), Some(Action::Down));
        // The editor has no bare-letter bindings: there, k is the letter k.
        assert_eq!(map.resolve(Context::Editor, &k), None);
    }

    #[test]
    fn esc_is_back_everywhere_and_back_from_the_browser_is_out() {
        let map = Keymap::default();
        let esc = ev(KeyCode::Esc, KeyModifiers::NONE);
        for ctx in [
            Context::Tree,
            Context::Editor,
            Context::Map,
            Context::Source,
        ] {
            assert_eq!(map.resolve(ctx, &esc), Some(Action::Back), "{ctx:?}");
        }
    }

    #[test]
    fn enter_is_one_action_in_every_pane_that_opens_things() {
        let map = Keymap::default();
        let enter = ev(KeyCode::Enter, KeyModifiers::NONE);
        for ctx in [Context::Tree, Context::Map, Context::Source] {
            assert_eq!(map.resolve(ctx, &enter), Some(Action::Open), "{ctx:?}");
        }
        assert_eq!(
            map.resolve_while_typing(Context::Editor, &enter),
            None,
            "in a file, Enter is a new line"
        );
    }

    #[test]
    fn a_word_is_ctrl_and_an_arrow_or_option_and_one() {
        let map = Keymap::default();
        for (code, mods) in [
            (KeyCode::Left, KeyModifiers::CONTROL),
            (KeyCode::Left, KeyModifiers::ALT),
        ] {
            assert_eq!(
                map.resolve_while_typing(Context::Editor, &ev(code, mods)),
                Some(Action::EditorWordLeft)
            );
        }
        // macOS Terminal's spelling of ⌥← arrives as Alt+B, and is turned into
        // the arrow before the keymap is asked.
        let terminal = from_terminal(ev(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(
            map.resolve_while_typing(Context::Editor, &terminal),
            Some(Action::EditorWordLeft)
        );
    }

    #[test]
    fn one_movement_is_one_action_in_every_pane_that_has_it() {
        let map = Keymap::default();
        let down = ev(KeyCode::Down, KeyModifiers::NONE);
        for ctx in [Context::Tree, Context::View, Context::Map, Context::Source] {
            assert_eq!(
                map.resolve(ctx, &down),
                Some(Action::Down),
                "{ctx:?} answers the same action"
            );
        }
        // So rebinding it is one line, not four.
        let mut over = BTreeMap::new();
        over.insert("down".to_string(), "n".to_string());
        let (map, warning) = Keymap::new(&over, Keyboard::default());
        assert!(warning.is_none());
        let n = ev(KeyCode::Char('n'), KeyModifiers::NONE);
        for ctx in [Context::Tree, Context::View, Context::Map, Context::Source] {
            assert_eq!(map.resolve(ctx, &n), Some(Action::Down), "{ctx:?}");
        }
    }

    #[test]
    fn the_global_chords_reach_every_context() {
        let map = Keymap::default();
        let save = ev(KeyCode::Char('s'), KeyModifiers::CONTROL);
        for ctx in [Context::Tree, Context::View, Context::Editor] {
            assert_eq!(map.resolve(ctx, &save), Some(Action::Save));
        }
    }

    #[test]
    fn an_override_replaces_the_shipped_keys_for_that_action_only() {
        let mut over = BTreeMap::new();
        over.insert("down".to_string(), "n".to_string());
        let (map, warning) = Keymap::new(&over, Keyboard::default());
        assert!(warning.is_none());
        assert_eq!(
            map.resolve(Context::Tree, &ev(KeyCode::Char('n'), KeyModifiers::NONE)),
            Some(Action::Down)
        );
        assert_eq!(
            map.resolve(Context::Tree, &ev(KeyCode::Char('k'), KeyModifiers::NONE)),
            None,
            "the old key is no longer bound to it"
        );
        assert_eq!(map.spec(Action::Up), "up i", "everything else is untouched");
    }

    #[test]
    fn an_action_that_does_not_exist_warns() {
        let mut over = BTreeMap::new();
        over.insert("dowm".to_string(), "n".to_string());
        let (map, warning) = Keymap::new(&over, Keyboard::default());
        assert!(warning.is_some_and(|w| w.contains("dowm")), "it says so");
        assert_eq!(
            map.spec(Action::Down),
            "down k",
            "and the real binding is untouched"
        );
    }

    #[test]
    fn a_line_that_is_not_a_key_warns_and_keeps_the_rest() {
        let mut over = BTreeMap::new();
        over.insert("down".to_string(), "wibble k".to_string());
        let (map, warning) = Keymap::new(&over, Keyboard::default());
        assert!(warning.is_some_and(|w| w.contains("wibble")), "it says so");
        assert_eq!(
            map.resolve(Context::Tree, &ev(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(Action::Down),
            "the half that parsed still works"
        );
    }

    #[test]
    fn a_pressed_key_can_be_written_back_down() {
        assert_eq!(
            spec_of(&ev(KeyCode::Char('I'), KeyModifiers::SHIFT)).as_deref(),
            Some("I")
        );
        assert_eq!(
            spec_of(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)).as_deref(),
            Some("ctrl+s")
        );
        assert_eq!(
            spec_of(&ev(KeyCode::Up, KeyModifiers::SHIFT)).as_deref(),
            Some("shift+up")
        );
        assert_eq!(
            spec_of(&ev(KeyCode::F(5), KeyModifiers::NONE)).as_deref(),
            Some("f5")
        );
    }
}
