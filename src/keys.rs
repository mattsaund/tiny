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
//! keypress is being read against, and every action belongs to exactly one. The
//! chords in [`Context::Global`] are checked first everywhere, which is why
//! `Ctrl+S` saves from wherever you are.
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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

impl Context {
    pub fn title(self) -> &'static str {
        match self {
            Context::Global => "ANYWHERE",
            Context::Tree => "TREE",
            Context::View => "VIEWING",
            Context::Editor => "EDITOR",
            Context::Map => "PROJECT MAP",
        }
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
    // Global
    Save,
    Quit,
    Bar,
    CommandBar,
    ToggleTreePane,
    // Tree
    TreeUp,
    TreeDown,
    TreeFirst,
    TreeLast,
    TreeJumpUp,
    TreeJumpDown,
    TreePageUp,
    TreePageDown,
    TreeOpen,
    TreeInto,
    TreeOut,
    TreePreview,
    TreeNew,
    TreeRename,
    TreeDelete,
    TreeCopy,
    TreePaste,
    TreeHidden,
    TreeRefresh,
    TreeHelp,
    TreeSettings,
    TreeMap,
    TreeBar,
    TreeQuit,
    // Reading
    ViewUp,
    ViewDown,
    ViewTop,
    ViewBottom,
    ViewPageUp,
    ViewPageDown,
    ViewBar,
    // Editor
    EditorBack,
    EditorUndo,
    EditorRedo,
    EditorDeleteLine,
    EditorWordLeft,
    EditorWordRight,
    EditorJumpUp,
    EditorJumpDown,
    EditorLineStart,
    EditorLineEnd,
    EditorDocStart,
    EditorDocEnd,
    // Map
    MapClose,
    MapOpen,
    MapUp,
    MapDown,
    MapLeft,
    MapRight,
    MapNext,
    MapPrevious,
    MapFilter,
    MapOrphans,
    MapReload,
    MapWikilinks,
    MapLinks,
    MapCalls,
}

/// Every action, its context, its name in the config file, what it does, and
/// the keys it ships with.
///
/// One table rather than five methods: a row that goes missing is a row that
/// goes missing everywhere at once, which is far easier to notice.
type Row = (Action, Context, &'static str, &'static str, &'static str);

const TABLE: &[Row] = &[
    (
        Action::Save,
        Context::Global,
        "save",
        "save the open file",
        "ctrl+s",
    ),
    (
        Action::Quit,
        Context::Global,
        "quit",
        "leave tiny",
        "ctrl+q",
    ),
    (
        Action::Bar,
        Context::Global,
        "bar",
        "the search bar",
        "ctrl+f",
    ),
    (
        Action::CommandBar,
        Context::Global,
        "command",
        "the bar, as a command",
        "ctrl+p",
    ),
    (
        Action::ToggleTreePane,
        Context::Global,
        "fold_tree",
        "fold the tree away, and back",
        "ctrl+b",
    ),
    (Action::TreeUp, Context::Tree, "tree.up", "move up", "up i"),
    (
        Action::TreeDown,
        Context::Tree,
        "tree.down",
        "move down",
        "down k",
    ),
    (
        Action::TreeFirst,
        Context::Tree,
        "tree.first",
        "first entry",
        "shift+up I",
    ),
    (
        Action::TreeLast,
        Context::Tree,
        "tree.last",
        "last entry",
        "shift+down K",
    ),
    (
        Action::TreeJumpUp,
        Context::Tree,
        "tree.jump_up",
        "five entries up",
        "ctrl+up",
    ),
    (
        Action::TreeJumpDown,
        Context::Tree,
        "tree.jump_down",
        "five entries down",
        "ctrl+down",
    ),
    (
        Action::TreePageUp,
        Context::Tree,
        "tree.page_up",
        "a screen up",
        "pageup",
    ),
    (
        Action::TreePageDown,
        Context::Tree,
        "tree.page_down",
        "a screen down",
        "pagedown",
    ),
    (
        Action::TreeOpen,
        Context::Tree,
        "tree.open",
        "open or close a folder, or edit a file",
        "enter",
    ),
    (
        Action::TreeInto,
        Context::Tree,
        "tree.into",
        "open a folder, or step inside it",
        "right l",
    ),
    (
        Action::TreeOut,
        Context::Tree,
        "tree.out",
        "close a folder, or go to its parent",
        "left j",
    ),
    (
        Action::TreePreview,
        Context::Tree,
        "tree.preview",
        "hand the keyboard to the file",
        "tab",
    ),
    (
        Action::TreeNew,
        Context::Tree,
        "tree.new",
        "new file or folder",
        "n",
    ),
    (
        Action::TreeRename,
        Context::Tree,
        "tree.rename",
        "rename",
        "r",
    ),
    (
        Action::TreeDelete,
        Context::Tree,
        "tree.delete",
        "delete, after asking",
        "d",
    ),
    (
        Action::TreeCopy,
        Context::Tree,
        "tree.copy",
        "copy",
        "ctrl+c",
    ),
    (
        Action::TreePaste,
        Context::Tree,
        "tree.paste",
        "paste into this folder",
        "ctrl+v",
    ),
    (
        Action::TreeHidden,
        Context::Tree,
        "tree.hidden",
        "show or hide dotfiles",
        ".",
    ),
    (
        Action::TreeRefresh,
        Context::Tree,
        "tree.refresh",
        "re-read from disk — also *reload",
        "f5",
    ),
    (
        Action::TreeHelp,
        Context::Tree,
        "tree.help",
        "keys and commands",
        "?",
    ),
    (
        Action::TreeSettings,
        Context::Tree,
        "tree.settings",
        "the settings area",
        ",",
    ),
    (
        Action::TreeMap,
        Context::Tree,
        "tree.map",
        "the project map",
        "m",
    ),
    (
        Action::TreeBar,
        Context::Tree,
        "tree.bar",
        "the search bar",
        "/",
    ),
    (
        Action::TreeQuit,
        Context::Tree,
        "tree.quit",
        "leave tiny",
        "q esc",
    ),
    (
        Action::ViewUp,
        Context::View,
        "view.up",
        "scroll up",
        "up i",
    ),
    (
        Action::ViewDown,
        Context::View,
        "view.down",
        "scroll down",
        "down k",
    ),
    (
        Action::ViewTop,
        Context::View,
        "view.top",
        "to the top",
        "shift+up I",
    ),
    (
        Action::ViewBottom,
        Context::View,
        "view.bottom",
        "to the bottom",
        "shift+down K",
    ),
    (
        Action::ViewPageUp,
        Context::View,
        "view.page_up",
        "a screen up",
        "pageup",
    ),
    (
        Action::ViewPageDown,
        Context::View,
        "view.page_down",
        "a screen down",
        "pagedown",
    ),
    (
        Action::ViewBar,
        Context::View,
        "view.bar",
        "the search bar",
        "/",
    ),
    (
        Action::EditorBack,
        Context::Editor,
        "editor.back",
        "back to the tree",
        "esc",
    ),
    (
        Action::EditorUndo,
        Context::Editor,
        "editor.undo",
        "undo",
        "ctrl+z",
    ),
    (
        Action::EditorRedo,
        Context::Editor,
        "editor.redo",
        "redo",
        "ctrl+y",
    ),
    (
        Action::EditorDeleteLine,
        Context::Editor,
        "editor.delete_line",
        "delete this line",
        "ctrl+k",
    ),
    (
        Action::EditorWordLeft,
        Context::Editor,
        "editor.word_left",
        "a word left",
        "ctrl+left",
    ),
    (
        Action::EditorWordRight,
        Context::Editor,
        "editor.word_right",
        "a word right",
        "ctrl+right",
    ),
    (
        Action::EditorJumpUp,
        Context::Editor,
        "editor.jump_up",
        "five lines up",
        "ctrl+up",
    ),
    (
        Action::EditorJumpDown,
        Context::Editor,
        "editor.jump_down",
        "five lines down",
        "ctrl+down",
    ),
    (
        Action::EditorLineStart,
        Context::Editor,
        "editor.line_start",
        "to the start of the line",
        "shift+left",
    ),
    (
        Action::EditorLineEnd,
        Context::Editor,
        "editor.line_end",
        "to the end of the line",
        "shift+right",
    ),
    (
        Action::EditorDocStart,
        Context::Editor,
        "editor.start",
        "to the first line",
        "shift+up",
    ),
    (
        Action::EditorDocEnd,
        Context::Editor,
        "editor.end",
        "to the last line",
        "shift+down",
    ),
    (
        Action::MapClose,
        Context::Map,
        "map.close",
        "back to the tree",
        "esc q m",
    ),
    (
        Action::MapOpen,
        Context::Map,
        "map.open",
        "open this file",
        "enter",
    ),
    (
        Action::MapUp,
        Context::Map,
        "map.up",
        "the nearest file up",
        "up i",
    ),
    (
        Action::MapDown,
        Context::Map,
        "map.down",
        "the nearest file down",
        "down k",
    ),
    (
        Action::MapLeft,
        Context::Map,
        "map.left",
        "the nearest file left",
        "left j",
    ),
    (
        Action::MapRight,
        Context::Map,
        "map.right",
        "the nearest file right",
        "right l",
    ),
    (
        Action::MapNext,
        Context::Map,
        "map.next",
        "step through the files",
        "tab",
    ),
    (
        Action::MapPrevious,
        Context::Map,
        "map.previous",
        "step back through them",
        "backtab",
    ),
    (
        Action::MapFilter,
        Context::Map,
        "map.filter",
        "filter by path",
        "/",
    ),
    (
        Action::MapOrphans,
        Context::Map,
        "map.orphans",
        "show unconnected files",
        "o",
    ),
    (
        Action::MapReload,
        Context::Map,
        "map.reload",
        "build the map again",
        "r",
    ),
    (
        Action::MapWikilinks,
        Context::Map,
        "map.wikilinks",
        "draw wikilinks",
        "1",
    ),
    (
        Action::MapLinks,
        Context::Map,
        "map.links",
        "draw markdown links",
        "2",
    ),
    (
        Action::MapCalls,
        Context::Map,
        "map.calls",
        "draw calls",
        "3",
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

    pub fn context(self) -> Context {
        self.row().1
    }

    /// The name in the config file, e.g. `tree.down`.
    pub fn name(self) -> &'static str {
        self.row().2
    }

    pub fn describe(self) -> &'static str {
        self.row().3
    }

    /// The keys this action ships with, as they would be written in the config.
    pub fn defaults(self) -> &'static str {
        self.row().4
    }

    pub fn from_name(name: &str) -> Option<Action> {
        TABLE.iter().find(|r| r.2 == name).map(|r| r.0)
    }
}

/// One key, with the modifiers that have to be held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    /// Read a key out of a config file or a keybinds window: `ctrl+s`,
    /// `shift+up`, `f5`, `enter`, `.`, `?`.
    ///
    /// Case matters for a single letter and nowhere else, because a letter is
    /// how the terminal reports Shift being held: `I` *is* Shift+i.
    pub fn parse(spec: &str) -> Option<Key> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = spec.trim();
        loop {
            let lower = rest.to_ascii_lowercase();
            if let Some(tail) = lower.strip_prefix("ctrl+") {
                mods |= KeyModifiers::CONTROL;
                rest = &rest[rest.len() - tail.len()..];
            } else if let Some(tail) = lower.strip_prefix("alt+") {
                mods |= KeyModifiers::ALT;
                rest = &rest[rest.len() - tail.len()..];
            } else if let Some(tail) = lower.strip_prefix("shift+") {
                mods |= KeyModifiers::SHIFT;
                rest = &rest[rest.len() - tail.len()..];
            } else {
                break;
            }
        }
        if rest.is_empty() {
            return None;
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" | "shift+tab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            other => {
                if let Some(n) = other.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
                    && (1..=12).contains(&n)
                    && other.len() <= 3
                {
                    KeyCode::F(n)
                } else {
                    let mut chars = rest.chars();
                    let c = chars.next()?;
                    if chars.next().is_some() {
                        return None;
                    }
                    KeyCode::Char(c)
                }
            }
        };
        Some(Key { code, mods })
    }

    /// Whether a keypress is this key.
    ///
    /// Shift is only compared for keys that are not characters. A terminal
    /// reports Shift+i as the character `I`, sometimes with the modifier set
    /// and sometimes without, so the character is the reliable half and the
    /// modifier is noise.
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        let want = |m: KeyModifiers| self.mods.contains(m) == ev.modifiers.contains(m);
        if !want(KeyModifiers::CONTROL) || !want(KeyModifiers::ALT) {
            return false;
        }
        match (self.code, ev.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => a == b,
            (a, b) => a == b && want(KeyModifiers::SHIFT),
        }
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) && !matches!(self.code, KeyCode::Char(_)) {
            f.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => f.write_fmt(format_args!("{c}")),
            KeyCode::F(n) => f.write_fmt(format_args!("f{n}")),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("backtab"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Insert => f.write_str("insert"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            other => f.write_fmt(format_args!("{other:?}")),
        }
    }
}

/// Turn a keypress into the spec that would bind it, or `None` for a key that
/// cannot be written down.
pub fn spec_of(ev: &KeyEvent) -> Option<String> {
    let mut mods = ev.modifiers;
    // A character already carries its own shift; saying so twice would produce
    // `shift+I`, which reads as a different key than the one just pressed.
    if matches!(ev.code, KeyCode::Char(_)) {
        mods.remove(KeyModifiers::SHIFT);
    }
    let key = Key {
        code: ev.code,
        mods,
    };
    match ev.code {
        KeyCode::Char(_) | KeyCode::F(_) => Some(key.to_string()),
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::Tab
        | KeyCode::BackTab
        | KeyCode::Backspace
        | KeyCode::Delete
        | KeyCode::Insert
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown => Some(key.to_string()),
        _ => None,
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
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new(&BTreeMap::new()).0
    }
}

impl Keymap {
    /// Build the live keymap. Returns anything worth telling the user about a
    /// line in their config that could not be read.
    pub fn new(overrides: &BTreeMap<String, String>) -> (Self, Option<String>) {
        let mut warning = None;
        let mut binds = Vec::with_capacity(TABLE.len());
        for (_, _, name, _, shipped) in TABLE {
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
        (Self { binds }, warning)
    }

    /// What this keypress means in `ctx`, with the global chords checked first
    /// so `Ctrl+S` cannot be shadowed by a pane's own binding.
    pub fn resolve(&self, ctx: Context, ev: &KeyEvent) -> Option<Action> {
        self.find(Context::Global, ev)
            .or_else(|| self.find(ctx, ev))
    }

    /// What this keypress means in `ctx` alone. For the panes that do their own
    /// global handling.
    pub fn find(&self, ctx: Context, ev: &KeyEvent) -> Option<Action> {
        TABLE
            .iter()
            .zip(&self.binds)
            .find(|((_, c, ..), keys)| *c == ctx && keys.iter().any(|k| k.matches(ev)))
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
    pub fn clashes(&self, action: Action, key: &Key) -> Vec<Action> {
        let context = action.context();
        TABLE
            .iter()
            .zip(&self.binds)
            .filter(|((a, c, ..), keys)| *a != action && *c == context && keys.contains(key))
            .map(|((a, ..), _)| *a)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

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
    fn a_key_survives_being_written_down_and_read_back() {
        for spec in [
            "ctrl+s",
            "shift+up",
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
        assert_eq!(map.resolve(Context::Tree, &k), Some(Action::TreeDown));
        assert_eq!(map.resolve(Context::View, &k), Some(Action::ViewDown));
        // The editor has no bare-letter bindings: there, k is the letter k.
        assert_eq!(map.resolve(Context::Editor, &k), None);
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
        over.insert("tree.down".to_string(), "n".to_string());
        let (map, warning) = Keymap::new(&over);
        assert!(warning.is_none());
        assert_eq!(
            map.resolve(Context::Tree, &ev(KeyCode::Char('n'), KeyModifiers::NONE)),
            Some(Action::TreeDown)
        );
        assert_eq!(
            map.resolve(Context::Tree, &ev(KeyCode::Char('k'), KeyModifiers::NONE)),
            None,
            "the old key is no longer bound to it"
        );
        assert_eq!(
            map.spec(Action::TreeUp),
            "up i",
            "everything else is untouched"
        );
    }

    #[test]
    fn an_action_that_does_not_exist_warns() {
        let mut over = BTreeMap::new();
        over.insert("tree.dowm".to_string(), "n".to_string());
        let (map, warning) = Keymap::new(&over);
        assert!(
            warning.is_some_and(|w| w.contains("tree.dowm")),
            "it says so"
        );
        assert_eq!(
            map.spec(Action::TreeDown),
            "down k",
            "and the real binding is untouched"
        );
    }

    #[test]
    fn a_line_that_is_not_a_key_warns_and_keeps_the_rest() {
        let mut over = BTreeMap::new();
        over.insert("tree.down".to_string(), "wibble k".to_string());
        let (map, warning) = Keymap::new(&over);
        assert!(warning.is_some_and(|w| w.contains("wibble")), "it says so");
        assert_eq!(
            map.resolve(Context::Tree, &ev(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(Action::TreeDown),
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
