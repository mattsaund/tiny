//! What a key binding looks like written down, and how it is matched.
//!
//! A binding is a string in the config file — `"ctrl+s"`, `"shift+f3"`,
//! `"ctrl+space"` — and this is the only place that turns one into something
//! a `KeyEvent` can be compared against, and back again for display. Keeping
//! the two directions in one file is what makes them round-trip: whatever
//! [`Key`]'s `Display` writes, its `FromStr` reads.
//!
//! # Matching is deliberately loose about shift
//!
//! Terminals do not agree about whether a shifted letter arrives as `A` with
//! no modifier or as `a` with `SHIFT`, and some report both. A comparison
//! strict about that would work on one terminal and not the next, so [`Key`]
//! normalises before comparing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
