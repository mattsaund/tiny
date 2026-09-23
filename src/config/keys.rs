//! Key bindings: what every key does, and how to change it.
//!
//! Every key tiny acts on is named. [`Action`] is the list of those names,
//! [`Keymap`] maps keys onto them, and the handlers in `app` ask the keymap
//! what a keypress meant instead of matching on the key itself. That
//! indirection is the whole feature: a binding can then come from a config
//! file or from the window `?`-then-`k` opens, without a single `match` arm
//! moving.
//!
//! # What is not bindable
//!
//! Keys that put a character into a file or a field are not here, and cannot
//! be. In the editor, the bar, and every prompt, `n` is the letter n — there is
//! no binding to change, because the key is not a command. The same goes for
//! the text-editing primitives beside them: the arrows, `Home`, `End`,
//! `Backspace`, `Delete`, `Enter` and `Tab` move and edit text while you are
//! typing into it, and rebinding those would leave a keyboard that cannot type.
//!
//! Everything else — the tree, a note being read, the project map, and every
//! chord that reaches out of the pane you are in — is bindable.
//!
//! # Contexts
//!
//! The same key means different things in different panes: `k` is *down* in the
//! tree and the letter k in the editor. [`Context`] says which set of actions a
//! keypress is being read against. The chords in [`Context::Global`] are
//! checked first everywhere, which is why `Ctrl+S` saves from wherever you are.
//!
//! An action lists the panes it works in, and most movements work in several.
//! Moving up is one action, `up`, bound to one key; the browser, a note being
//! read, the map and the change list each answer it in their own way. It was
//! four actions with four names and four identical bindings, which is four
//! rows to find and change to rebind one key, and three more chances for them
//! to drift apart.
//!
//! Everything in `Global` is a chord. That is not a style choice: in the editor
//! a letter is a letter being typed, so a control that has to work while you
//! are writing has no other shape available. The tree keeps a bare-letter
//! binding for the common ones beside the chord — `n` as well as `Ctrl+N` —
//! because there, nothing is being typed and the short one costs nothing.
//!
//! # What is a key and what is a command
//!
//! Not everything tiny does is here, and that is the point. Keys are for
//! moving around and for changing a file: things done constantly, in the middle
//! of something else, where a key is the only shape fast enough. Deleting a
//! path, re-reading the disk, opening the settings — those are deliberate acts,
//! usually with something to type after them, and they are commands (`*delete`,
//! `*reload`, `*config`). They had keys as well until the two lists were told
//! apart, and a second way in earned nothing but a row in this table.
//!
//! It runs the other way too: saving and quitting are `Ctrl+S` and `Ctrl+Q`,
//! and the `*w`, `*q` and `*wq` that shadowed them are gone.
//!
//! # Keys a terminal cannot send
//!
//! Four of the shipped chords do not exist in a terminal's legacy encoding.
//! `Ctrl+.` has no byte at all; `Ctrl+/` arrives as 0x1F, which is
//! indistinguishable from `Ctrl+7`; `Ctrl` with a digit is not in the encoding
//! either. `main` asks the terminal for the disambiguating keyboard protocol,
//! which separates them all, and on a terminal that declines they do not
//! arrive.
//!
//! Where that would leave a function unreachable, the key that always arrives
//! is the one that is bound: dotfiles are the browser's bare `.` rather than
//! `Ctrl+.`, and the bar keeps `/` beside `Ctrl+/`. A control that cannot be
//! reached on a common terminal is not a control.
//!
//! The window switcher takes that further, because `Ctrl` with a digit is not
//! merely lost — it becomes another key. Rather than binding both and hoping,
//! tiny asks the terminal what it can send and ships a different keyboard
//! accordingly: see [`Keyboard`] and [`LEGACY_KEYS`].
//!
//! A key can also fail to arrive because something above the terminal answered
//! it first. `Ctrl+Shift` with an arrow starts a selection in a good many
//! emulators, and GNOME and KDE both ship with `Ctrl+Alt` and an arrow bound to
//! switching workspaces — a desktop shortcut is taken before the terminal sees
//! the key at all. Neither is used here for that reason.
//!
//! Two of the contexts belong to a window rather than a pane: [`Context::Map`]
//! and [`Context::Source`] are read when that window is on screen, and the
//! global chords are checked first there as everywhere — which is how `Ctrl+P`
//! reaches the command bar from the source window, where `*commit` is typed.
//!
//! # Three windows, on Ctrl and a number
//!
//! `Ctrl+1` is the browser and the file, `Ctrl+2` is source control, `Ctrl+3`
//! is the project map. They are windows and not overlays: there is no stack to
//! pop, so the way out of one is to name another, and Esc returns to the first.
//!
//! Numbers rather than initials because the set is small, ordered and unlikely
//! to grow much — and because `Ctrl+M` (the map's old key) *is* Enter in a
//! terminal's legacy encoding, which is the sort of collision a number cannot
//! have.
//!
//! What a number costs: `Ctrl` with a digit is not in the legacy encoding
//! either. `Ctrl+1` sends nothing at all, `Ctrl+2` arrives as `Ctrl+Space` and
//! `Ctrl+3` as `Esc`. On a terminal that speaks the disambiguating keyboard
//! protocol all three arrive properly; on one that does not — GNOME Terminal,
//! macOS Terminal, Konsole — `Ctrl+2` folds the browser and `Ctrl+3` is read
//! as Escape, which from the browser *quits*. No encoding trick avoids that: a
//! terminal that cannot say which key was pressed cannot be made to.
//!
//! So the keyboard is not fixed. `main` asks the terminal whether it speaks
//! the disambiguating protocol before the first key is read, and tiny binds
//! `Ctrl+1` `Ctrl+2` `Ctrl+3` where the answer is yes and `F2` `F3` `F4` —
//! a row under the `F1` that opens the keys — where it is no. One keyboard or
//! the other, never both: a key listed in the keybinds window is a key that
//! works, and the status line names whichever one this terminal has. See
//! [`Keyboard`].
//!
//! # No Alt at all
//!
//! There used to be an `Alt` half to the movement keys. There is none now, and
//! this is the reason: on macOS, Option is not a modifier. Terminal sends `⌥-`
//! as an en dash and `⌥←` as an escape sequence of its own, so an `Alt`
//! binding never arrived at all. A key that works on one machine and not
//! another is worse than no key, and every one of them had a twin that any
//! terminal can send — `Home`, `End`, `Ctrl+Home`, `Ctrl+End`, `I`, `K`, and
//! `Ctrl` with an arrow for the browser's width.
//!
//! # Shift
//!
//! `Shift` with an arrow is not used either, and never was: it is how terminals
//! have always started a selection, and a key the terminal may act on itself is
//! a key that works on some machines and not others. The capital letters `I`
//! and `K` are fine — they are characters, not a modifier, and a terminal
//! cannot mistake them for anything.
//!
//! # One key per thing
//!
//! Two keys that do the same thing are one key and one thing to remember for
//! nothing, so there are none. The exception is `i` `j` `k` `l`, which stand in
//! for the four arrows on a keyboard that has none, and `I` / `K` beside `Home`
//! / `End` for the same reason.
//!
//! The width pair is why [`Action::Narrower`] belongs to the panes that have a
//! left column rather than to `Global`: `Ctrl` with an arrow is a word motion
//! in the editor, and a global binding would take it from there.
//!
//! # Defaults and overrides
//!
//! [`Action::defaults`] is the shipped keyboard, written out in one table. The
//! config file holds *only* what differs from it, so an untouched `tiny.conf`
//! has no `[keys]` section at all and a new default reaches everyone who has
//! not overridden that action. Resetting is therefore just dropping the
//! overrides.

use std::collections::BTreeMap;
use std::fmt::Write as _;

pub use super::keyspec::{Key, spec_of};
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
            Context::Tree => "TREE",
            Context::View => "VIEWING",
            Context::Editor => "EDITOR",
            Context::Map => "PROJECT MAP",
            Context::Source => "SOURCE CONTROL",
        }
    }
}

/// What the terminal turned out to be able to send.
///
/// Not every terminal can deliver every chord, and the ones that cannot do not
/// fail quietly — see [`LEGACY_KEYS`]. tiny asks at startup and builds the
/// keyboard around the answer, so what the keybinds window lists is what the
/// keys in front of you actually do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keyboard {
    /// The terminal speaks the disambiguating keyboard protocol: every chord
    /// in [`TABLE`] arrives as itself. kitty, foot, WezTerm, Ghostty.
    #[default]
    Full,
    /// The legacy encoding, where `Ctrl` with a digit or a punctuation mark is
    /// another key's byte or no byte at all. GNOME Terminal, macOS Terminal,
    /// Konsole.
    Legacy,
}

/// What changes on a terminal that cannot send the whole of [`TABLE`].
///
/// Substitutions, not additions, which is the point: on such a terminal
/// `Ctrl+1` arrives as a bare `1`, `Ctrl+2` as `Ctrl+Space` and `Ctrl+3` as
/// `Esc`. Those three are not merely unreachable — they are *other keys
/// wearing the wrong name*, and `Ctrl+3` in the browser would quit. Leaving
/// them bound would have the keybinds window promise a key that does something
/// else, so on this keyboard they are not bound at all. The function keys
/// below reach the same three windows through an encoding every terminal has
/// sent since the 1980s.
///
/// Anything not named here is the same on both keyboards.
const LEGACY_KEYS: &[(Action, &str)] = &[
    (Action::WindowMain, "f2"),
    (Action::WindowSource, "f3"),
    (Action::WindowMap, "f4"),
];

impl Keyboard {
    /// The keys `action` ships with on this keyboard.
    pub fn defaults(self, action: Action) -> &'static str {
        if self == Keyboard::Legacy
            && let Some((_, keys)) = LEGACY_KEYS.iter().find(|(a, _)| *a == action)
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
/// must also gain a row in [`Action::defaults`] and a name in
/// [`Action::name`]. The `every_action_is_complete` test checks both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // Everywhere. Every one of these is a chord, so it works while you are
    // typing into a file as well as from the tree.
    Save,
    Quit,
    Bar,
    CommandBar,
    ToggleTreePane,
    Copy,
    Paste,
    WindowMain,
    WindowSource,
    WindowMap,
    Help,
    // In every pane that has them. One name for one movement, however many
    // panes can make that movement — see the module docs.
    Up,
    Down,
    Left,
    Right,
    First,
    Last,
    JumpUp,
    JumpDown,
    PageUp,
    PageDown,
    Back,
    Refresh,
    Narrower,
    Wider,
    // The browser alone
    TreeOpen,
    TreePreview,
    TreeHidden,
    TreeQuit,
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
    MapOpen,
    MapNext,
    MapPrevious,
    MapWikilinks,
    MapLinks,
    MapCalls,
    // Source control
    SourceEnter,
    SourceOpen,
}

/// Every action, the panes it works in, its name in the config file, what it
/// does, and the keys it ships with.
///
/// One table rather than five methods: a row that goes missing is a row that
/// goes missing everywhere at once, which is far easier to notice.
///
/// The contexts are a list because most movements are the same movement in
/// several panes. `up` is one action bound to one key that four panes each
/// answer in their own way, rather than four actions, four rows in the
/// keybinds window, and four lines to change to rebind one key.
type Row = (
    Action,
    &'static [Context],
    &'static str,
    &'static str,
    &'static str,
);

const TABLE: &[Row] = &[
    (
        Action::Save,
        &[Context::Global],
        "save",
        "save the open file",
        "ctrl+s",
    ),
    (
        Action::Quit,
        &[Context::Global],
        "quit",
        "leave tiny",
        "ctrl+q",
    ),
    (
        Action::Bar,
        &[Context::Global],
        "bar",
        "search — type a star first for a command",
        "ctrl+/ /",
    ),
    (
        Action::CommandBar,
        &[Context::Global],
        "command",
        "the bar, already starred",
        "ctrl+p",
    ),
    (
        Action::ToggleTreePane,
        &[Context::Global],
        "fold_tree",
        "fold the browser away, and back",
        "ctrl+space",
    ),
    (
        Action::Copy,
        &[Context::Global],
        "copy",
        "copy what the cursor is on",
        "ctrl+c",
    ),
    (
        Action::Paste,
        &[Context::Global],
        "paste",
        "paste into this folder",
        "ctrl+v",
    ),
    (
        Action::WindowMain,
        &[Context::Global],
        "window_main",
        "the browser and the file",
        "ctrl+1",
    ),
    (
        Action::WindowSource,
        &[Context::Global],
        "window_source",
        "git and what has changed",
        "ctrl+2",
    ),
    (
        Action::WindowMap,
        &[Context::Global],
        "window_map",
        "the project map",
        "ctrl+3",
    ),
    (
        Action::Help,
        &[Context::Global],
        "help",
        "keys and commands",
        "f1",
    ),
    (
        Action::Up,
        &[Context::Tree, Context::View, Context::Map, Context::Source],
        "up",
        "move up",
        "up i",
    ),
    (
        Action::Down,
        &[Context::Tree, Context::View, Context::Map, Context::Source],
        "down",
        "move down",
        "down k",
    ),
    (
        Action::Left,
        &[Context::Tree, Context::Map, Context::Source],
        "left",
        "out of a folder, or one to the left",
        "left j",
    ),
    (
        Action::Right,
        &[Context::Tree, Context::Map, Context::Source],
        "right",
        "into a folder, or one to the right",
        "right l",
    ),
    (
        Action::First,
        &[Context::Tree, Context::View],
        "first",
        "to the first row, or the top",
        "home I",
    ),
    (
        Action::Last,
        &[Context::Tree, Context::View],
        "last",
        "to the last row, or the bottom",
        "end K",
    ),
    (
        Action::JumpUp,
        &[Context::Tree, Context::Editor, Context::Source],
        "jump_up",
        "five at a time, up",
        "ctrl+up",
    ),
    (
        Action::JumpDown,
        &[Context::Tree, Context::Editor, Context::Source],
        "jump_down",
        "five at a time, down",
        "ctrl+down",
    ),
    (
        Action::PageUp,
        &[Context::Tree, Context::View, Context::Source],
        "page_up",
        "a screen up",
        "pageup",
    ),
    (
        Action::PageDown,
        &[Context::Tree, Context::View, Context::Source],
        "page_down",
        "a screen down",
        "pagedown",
    ),
    (
        Action::Back,
        &[Context::Editor, Context::Map, Context::Source],
        "back",
        "back to the browser",
        "esc",
    ),
    (
        Action::Refresh,
        &[Context::Map, Context::Source],
        "refresh",
        "read it from disk again",
        "r",
    ),
    (
        Action::Narrower,
        &[Context::Tree, Context::Source],
        "narrower",
        "a narrower left column",
        "ctrl+left",
    ),
    (
        Action::Wider,
        &[Context::Tree, Context::Source],
        "wider",
        "a wider left column",
        "ctrl+right",
    ),
    (
        Action::TreeOpen,
        &[Context::Tree],
        "tree.open",
        "open or close a folder, or edit a file",
        "enter",
    ),
    (
        Action::TreePreview,
        &[Context::Tree],
        "tree.preview",
        "hand the keyboard to the file",
        "tab",
    ),
    (
        Action::TreeHidden,
        &[Context::Tree],
        "tree.hidden",
        "show or hide dotfiles",
        ".",
    ),
    (
        Action::TreeQuit,
        &[Context::Tree],
        "tree.quit",
        "leave tiny",
        "esc",
    ),
    (
        Action::EditorUndo,
        &[Context::Editor],
        "editor.undo",
        "undo",
        "ctrl+z",
    ),
    (
        Action::EditorRedo,
        &[Context::Editor],
        "editor.redo",
        "redo",
        "ctrl+y",
    ),
    (
        Action::EditorDeleteLine,
        &[Context::Editor],
        "editor.delete_line",
        "delete this line",
        "ctrl+k",
    ),
    (
        Action::EditorWordLeft,
        &[Context::Editor],
        "editor.word_left",
        "a word left",
        "ctrl+left",
    ),
    (
        Action::EditorWordRight,
        &[Context::Editor],
        "editor.word_right",
        "a word right",
        "ctrl+right",
    ),
    (
        Action::EditorLineStart,
        &[Context::Editor],
        "editor.line_start",
        "to the start of the line",
        "home",
    ),
    (
        Action::EditorLineEnd,
        &[Context::Editor],
        "editor.line_end",
        "to the end of the line",
        "end",
    ),
    (
        Action::EditorDocStart,
        &[Context::Editor],
        "editor.start",
        "to the first line",
        "ctrl+home",
    ),
    (
        Action::EditorDocEnd,
        &[Context::Editor],
        "editor.end",
        "to the last line",
        "ctrl+end",
    ),
    (
        Action::MapOpen,
        &[Context::Map],
        "map.open",
        "open this file",
        "enter",
    ),
    (
        Action::MapNext,
        &[Context::Map],
        "map.next",
        "step through the files",
        "tab",
    ),
    (
        Action::MapPrevious,
        &[Context::Map],
        "map.previous",
        "step back through them",
        "backtab",
    ),
    (
        Action::MapWikilinks,
        &[Context::Map],
        "map.wikilinks",
        "draw wikilinks",
        "1",
    ),
    (
        Action::MapLinks,
        &[Context::Map],
        "map.links",
        "draw markdown links",
        "2",
    ),
    (
        Action::MapCalls,
        &[Context::Map],
        "map.calls",
        "draw calls",
        "3",
    ),
    (
        Action::SourceEnter,
        &[Context::Source],
        "source.stage",
        "stage or unstage — a file, or a whole section",
        "enter",
    ),
    (
        Action::SourceOpen,
        &[Context::Source],
        "source.open",
        "open this file in the editor",
        "tab",
    ),
];

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
            _ => "IN MORE THAN ONE PANE",
        }
    }

    /// The name in the config file, e.g. `tree.down`.
    pub fn name(self) -> &'static str {
        self.row().2
    }

    pub fn describe(self) -> &'static str {
        self.row().3
    }

    /// The keys this action ships with on a terminal that can send all of
    /// them, as they would be written in the config.
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
    /// This is what lets one `bar` action carry both `ctrl+/` and `/`, instead
    /// of one action per pane that wanted the short key.
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
    /// key is how `esc` leaves the editor and quits from the browser, and how
    /// `ctrl+left` is a word in a file and a narrower column beside it.
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
        for action in Action::all() {
            let spec = action.defaults();
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

    #[test]
    fn no_two_actions_in_one_context_ship_with_the_same_key() {
        let map = Keymap::default();
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

    #[test]
    fn every_window_is_reachable_on_a_terminal_that_cannot_send_ctrl_and_a_digit() {
        // The failure this exists to stop: on GNOME Terminal `Ctrl+3` arrives
        // as Escape, which from the browser quits — so the map was not merely
        // unreachable there, the key for it closed the program.
        let legacy = Keymap::new(&BTreeMap::new(), Keyboard::Legacy).0;
        for action in [Action::WindowMain, Action::WindowSource, Action::WindowMap] {
            let spec = legacy.spec(action);
            let sendable = spec.split_whitespace().all(|key| {
                key.strip_prefix('f')
                    .is_some_and(|n| n.parse::<u8>().is_ok())
            });
            assert!(
                sendable,
                "{} is on {spec}, which a legacy terminal cannot send",
                action.name()
            );
        }
        // And they actually answer.
        for (key, action) in [
            (KeyCode::F(2), Action::WindowMain),
            (KeyCode::F(3), Action::WindowSource),
            (KeyCode::F(4), Action::WindowMap),
        ] {
            assert_eq!(
                legacy.resolve(Context::Tree, &ev(key, KeyModifiers::NONE)),
                Some(action)
            );
        }
    }

    #[test]
    fn one_keyboard_or_the_other_and_never_both() {
        let full = Keymap::new(&BTreeMap::new(), Keyboard::Full).0;
        let legacy = Keymap::new(&BTreeMap::new(), Keyboard::Legacy).0;
        assert_eq!(full.spec(Action::WindowMap), "ctrl+3");
        assert_eq!(legacy.spec(Action::WindowMap), "f4");

        // A key listed in the keybinds window is a key that works here, so the
        // one belonging to the other keyboard is not bound at all.
        let f4 = ev(KeyCode::F(4), KeyModifiers::NONE);
        let ctrl3 = ev(KeyCode::Char('3'), KeyModifiers::CONTROL);
        assert_eq!(full.resolve(Context::Tree, &f4), None, "no spare key");
        assert_eq!(legacy.resolve(Context::Tree, &ctrl3), None, "nor here");

        // Everything else is the same keyboard either way.
        for action in Action::all().filter(|a| {
            !matches!(
                a,
                Action::WindowMain | Action::WindowSource | Action::WindowMap
            )
        }) {
            assert_eq!(full.spec(action), legacy.spec(action), "{}", action.name());
        }
    }

    #[test]
    fn a_rebinding_wins_on_either_keyboard() {
        let mut over = BTreeMap::new();
        over.insert("window_map".to_string(), "ctrl+g".to_string());
        for keyboard in [Keyboard::Full, Keyboard::Legacy] {
            let (map, warning) = Keymap::new(&over, keyboard);
            assert!(warning.is_none());
            assert_eq!(map.spec(Action::WindowMap), "ctrl+g", "{keyboard:?}");
            // What it would go back to still depends on the terminal, which is
            // what `shipped` is for.
            let shipped = if keyboard == Keyboard::Full {
                "ctrl+3"
            } else {
                "f4"
            };
            assert_eq!(map.shipped(Action::WindowMap), shipped);
        }
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
    fn a_key_can_mean_different_things_in_different_panes() {
        let map = Keymap::default();
        // What the contexts are actually for: `esc` leaves the file, and
        // quits from the browser. `ctrl+left` is a word in a file, and a
        // narrower column beside it.
        let esc = ev(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(map.resolve(Context::Editor, &esc), Some(Action::Back));
        assert_eq!(map.resolve(Context::Tree, &esc), Some(Action::TreeQuit));
        let left = ev(KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(
            map.resolve(Context::Editor, &left),
            Some(Action::EditorWordLeft)
        );
        assert_eq!(map.resolve(Context::Tree, &left), Some(Action::Narrower));
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
