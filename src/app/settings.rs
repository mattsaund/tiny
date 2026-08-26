//! The settings area and the keybinds window.
//!
//! Both are lists of name-and-value rows with one of them editable in place,
//! and both work the same way: change it, see it immediately, and only write
//! it to disk on an explicit `Ctrl+S`. A setting can therefore be tried and
//! backed out of without `tiny.conf` ever changing.
//!
//! [`App::apply_config`] is the seam that makes "see it immediately" true —
//! every successful change goes through it, and it re-derives the palette, the
//! keymap and the syntax theme from the config. A change that does not reach
//! the screen is usually a change that did not call it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::keys::{Action, Keymap};
use crate::config::{Config, Palette};

use super::App;
use super::mode::{BUTTONS, Confirm, ConfirmKind, KEYBIND_BUTTONS, Keybinds, Mode, Settings};
use super::parts::{char_byte, list_move, plural};

/// How many rows an overlay's page keys move. The overlays do not know how
/// tall they are drawn — `ui` decides that — and a fixed step reads the same
/// in a tall window as in a short one.
const OVERLAY_PAGE: usize = 10;

impl App {
    /// Re-derive everything a settings change can affect.
    ///
    /// Must be called after any successful `Config::set`, or the change sits in
    /// the config struct without reaching the screen. Rebuilds the palette and
    /// keymap, swaps the syntax theme, and re-reads the tree if dotfile
    /// visibility changed.
    pub(super) fn apply_config(&mut self) {
        self.palette = Palette::from_theme(&self.config.theme);
        let (keymap, warning) = Keymap::new(&self.config.keys);
        self.keymap = keymap;
        if let Some(w) = warning {
            self.status = w;
        }
        // Swap the theme in place. Rebuilding the highlighter here would
        // re-unpack all 213 grammars, which is the startup cost paid again
        // every time any setting changes — including ones it has nothing to
        // do with, since this runs after every successful `Config::set`.
        self.highlighter.set_theme(&self.config.syntax_theme);
        // The saved states carry styles taken from the old theme.
        self.highlight_cache.clear();
        if self.tree.show_hidden() != self.config.show_hidden {
            self.tree.set_show_hidden(self.config.show_hidden);
            self.rebuild_rows();
        }
    }

    pub(super) fn open_settings(&mut self) {
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
    pub(super) fn on_settings_key(&mut self, mut s: Settings, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let index = Config::settings_index();
        // The buttons sit above the settings, so a row number has to be turned
        // into one or the other before it means anything.
        let last = BUTTONS.len() + index.len() - 1;
        let setting_at = |row: usize| index.get(row.saturating_sub(BUTTONS.len()));

        // Editing a value: the keys go into the field.
        if let Some(mut buf) = s.editing.take() {
            if ctrl {
                if key.code == KeyCode::Char('s') {
                    self.write_config();
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
                    let key_name = setting_at(s.selected).map(|r| r.0).unwrap_or("");
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

        if let Some(next) = list_move(key, s.selected, last, OVERLAY_PAGE) {
            s.selected = next;
            self.mode = Mode::Settings(s);
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.status = "closed".into();
                return;
            }
            KeyCode::Char('s') if ctrl => self.write_config(),
            KeyCode::Enter => match s.selected {
                0 => {
                    self.mode = Mode::Keybinds(Keybinds::default());
                    self.status = "keybinds — Enter to change a key, Esc to close".into();
                    return;
                }
                1 => {
                    self.confirm_reset_settings();
                    return;
                }
                row => {
                    let key_name = setting_at(row).map(|r| r.0).unwrap_or("");
                    let current = self.config.get(key_name).unwrap_or_default();
                    s.cursor = current.chars().count();
                    s.editing = Some(current);
                }
            },
            _ => {}
        }
        self.mode = Mode::Settings(s);
    }

    /// Write the whole config to disk, and say where it went.
    ///
    /// `Ctrl+S` in the settings area and in the keybinds window both land
    /// here: they edit the same file, and one of them writing a different
    /// message than the other would only suggest otherwise.
    fn write_config(&mut self) {
        match self.config.save() {
            Ok(p) => self.status = format!("wrote {}", p.display()),
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Ask before throwing away every setting. There is no undo for this, and
    /// the answer takes the settings area back with it either way.
    fn confirm_reset_settings(&mut self) {
        let changed = self.config.changed_from_default();
        if changed == 0 {
            self.status = "already the shipped settings".into();
            self.mode = Mode::Settings(Settings::default());
            return;
        }
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::ResetSettings,
            message: format!(
                "Reset {changed} setting{} to what tiny ships with?  (y/n)",
                plural(changed)
            ),
        });
    }

    /// Keys for the keybinds window.
    ///
    /// Two states, like the settings area: reading the list, or waiting for the
    /// key that will become a binding. In the second, *every* key is data — so
    /// there is no Esc to cancel with, and the way out is to bind Esc to the
    /// action or to press it twice.
    pub(super) fn on_keybinds_key(&mut self, mut kb: Keybinds, key: KeyEvent) {
        let actions: Vec<Action> = Action::all().collect();
        let last = KEYBIND_BUTTONS.len() + actions.len() - 1;

        if kb.capturing {
            kb.capturing = false;
            let action = actions[kb.selected.saturating_sub(KEYBIND_BUTTONS.len())];
            match crate::config::keys::spec_of(&key) {
                Some(spec) => self.bind(action, &spec),
                None => self.status = "that key cannot be written down".into(),
            }
            self.mode = Mode::Keybinds(kb);
            return;
        }

        if let Some(next) = list_move(key, kb.selected, last, OVERLAY_PAGE) {
            kb.selected = next;
            self.mode = Mode::Keybinds(kb);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                // Back to where this was opened from, not out to the tree.
                self.mode = Mode::Settings(Settings::default());
                self.status =
                    "settings — Enter to change, ^S to write tiny.conf, Esc to close".into();
                return;
            }
            KeyCode::Char('s') if ctrl => self.write_config(),
            KeyCode::Delete | KeyCode::Backspace => {
                if kb.selected >= KEYBIND_BUTTONS.len() {
                    let action = actions[kb.selected - KEYBIND_BUTTONS.len()];
                    self.config.keys.remove(action.name());
                    self.apply_config();
                    self.status = format!("{} back to {}", action.name(), action.defaults());
                }
            }
            KeyCode::Enter => {
                if kb.selected < KEYBIND_BUTTONS.len() {
                    self.confirm_reset_keybinds();
                    return;
                }
                kb.capturing = true;
                let action = actions[kb.selected - KEYBIND_BUTTONS.len()];
                self.status = format!("press the key for {} — Delete restores it", action.name());
            }
            _ => {}
        }
        self.mode = Mode::Keybinds(kb);
    }

    /// Bind one key to an action, replacing whatever reached it before.
    ///
    /// One key per action, not a list: the window shows what you pressed, and
    /// a second binding you cannot see is worse than no second binding. The
    /// config file still takes several, space-separated, and the window shows
    /// them all.
    ///
    /// The key is taken off anything else in the same pane that answered to
    /// it, so what the window shows is what actually happens — two rows
    /// claiming the same key, only one of which works, would be a lie.
    fn bind(&mut self, action: Action, spec: &str) {
        let Some(key) = crate::config::keys::Key::parse(spec) else {
            self.status = format!("`{spec}` is not a key");
            return;
        };
        let clashes = self.keymap.clashes(action, &key);
        for other in &clashes {
            let left: Vec<String> = self
                .keymap
                .keys(*other)
                .iter()
                .filter(|k| **k != key)
                .map(|k| k.to_string())
                .collect();
            self.set_binding(*other, &left.join(" "));
        }
        self.set_binding(action, spec);
        self.apply_config();
        self.status = match clashes.first() {
            // Say what was taken away, rather than letting a key quietly stop
            // doing what it used to.
            Some(other) => format!("{} = {spec} — taken from {}", action.name(), other.name()),
            None => format!("{} = {spec}", action.name()),
        };
    }

    /// Record what reaches an action, dropping the override entirely when it is
    /// back to the shipped keys — so the config file only ever holds what has
    /// actually been changed.
    fn set_binding(&mut self, action: Action, spec: &str) {
        if spec == action.defaults() {
            self.config.keys.remove(action.name());
        } else {
            self.config
                .keys
                .insert(action.name().to_string(), spec.to_string());
        }
    }

    fn confirm_reset_keybinds(&mut self) {
        if self.config.keys.is_empty() {
            self.status = "already the shipped keyboard".into();
            self.mode = Mode::Keybinds(Keybinds::default());
            return;
        }
        let n = self.config.keys.len();
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::ResetKeybinds,
            message: format!(
                "Put {n} key{} back to what tiny ships with?  (y/n)",
                plural(n)
            ),
        });
    }
}
