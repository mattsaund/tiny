//! The settings area and the keybinds window.
//!
//! Both are lists of name-and-value rows with one of them editable in place,
//! and both work the same way: change it, see it immediately, and only write
//! it to disk on an explicit `Ctrl+S`. A setting can therefore be tried and
//! backed out of without `tiny.conf` ever changing.
//!
//! [`App::apply_config`] is the seam that makes "see it immediately" true;
//! every successful change goes through it, and it re-derives the palette, the
//! keymap and the syntax theme from the config. A change that does not reach
//! the screen is usually a change that did not call it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::keys::{Action, Keyboard, Keymap};
use crate::config::{Config, Palette};

use super::App;
use super::mode::{BUTTONS, Confirm, ConfirmKind, KEYBIND_BUTTONS, Keybinds, Mode, Settings};
use super::parts::{edit_field, list_move, plural};

/// How many rows an overlay's page keys move. The overlays do not know how
/// tall they are drawn (`ui` decides that) and a fixed step reads the same
/// in a tall window as in a short one.
const OVERLAY_PAGE: usize = 10;

impl App {
    /// Take on which keys this terminal keeps for itself, see
    /// [`Keyboard`].
    ///
    /// Called once, from `main`, before the first frame. Rebuilds the keymap,
    /// because what the shipped keys are depends on the answer; every binding
    /// the user has set in `tiny.conf` still wins, since those are laid over
    /// the shipped keyboard either way.
    pub fn set_keyboard(&mut self, keyboard: Keyboard) {
        if self.keyboard == keyboard {
            return;
        }
        self.keyboard = keyboard;
        let (keymap, warning) = Keymap::new(&self.config.keys, keyboard);
        self.keymap = keymap;
        if let Some(w) = warning {
            self.status = w;
        }
    }

    /// Re-derive everything a settings change can affect.
    ///
    /// Must be called after any successful `Config::set`, or the change sits in
    /// the config struct without reaching the screen. Rebuilds the palette and
    /// keymap, swaps the syntax theme, and re-reads the tree if dotfile
    /// visibility changed.
    ///
    /// Returns what is wrong with the config, if anything (a key that cannot
    /// be bound, a theme that does not exist), for the caller to say in
    /// place of whatever it would have said.
    pub(super) fn apply_config(&mut self) -> Option<String> {
        let mut warning = None;
        self.palette = Palette::from_theme(&self.config.theme).with_glyphs(self.config.glyphs);
        // Formatted in the old palette.
        self.block_rows.borrow_mut().clear();
        // The map's names were cut with the old glyph set's ellipsis.
        if let Some(map) = &mut self.project_map {
            map.label_with(self.palette.glyphs.ellipsis);
        }
        let (keymap, keys_warning) = Keymap::new(&self.config.keys, self.keyboard);
        self.keymap = keymap;
        if keys_warning.is_some() {
            warning = keys_warning;
        }
        // Swap the theme in place, and only when it is a different one.
        // Rebuilding the highlighter would re-unpack all 213 grammars, and
        // even a swap re-reads the theme set; this runs after every change to
        // any setting, a key, or a plugin's `tiny.set`.
        if self.highlighter.theme_name() != self.config.syntax_theme {
            if let Some(w) = self.highlighter.set_theme(&self.config.syntax_theme) {
                warning = Some(w);
            }
            self.forget_highlighting();
        }
        if self.tree.show_hidden() != self.config.show_hidden {
            self.tree.set_show_hidden(self.config.show_hidden);
            self.rebuild_rows();
        }
        warning
    }

    /// Throw away every saved highlighting state, in every pane: they carry
    /// styles taken from the theme they were made under.
    fn forget_highlighting(&mut self) {
        self.highlight_cache.clear();
        if let Some(split) = &mut self.split {
            split.pinned.highlight.clear();
        }
        self.git.diff_highlight = Default::default();
    }

    pub(super) fn open_settings(&mut self) {
        self.mode = Mode::Settings(Settings::default());
        self.status = "settings".into();
    }

    /// Keys for the settings overlay.
    ///
    /// Two states in one function, separated by whether `s.editing` is set:
    /// navigating the list, or typing into one row's value. Taking the buffer
    /// out with `take()` at the top means each branch has to put it back to
    /// stay in edit mode: the same "closing is the default" pattern as the
    /// mode dispatch itself.
    ///
    /// `Ctrl+S` writes `tiny.conf` and works in both states, since finishing a
    /// value and immediately saving is the obvious thing to want: in the
    /// middle of a value it takes the value first, the way Enter does.
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
                    let key_name = setting_at(s.selected).map(|r| r.0).unwrap_or("");
                    match self.config.set(key_name, &buf) {
                        Ok(()) => {
                            let warning = self.apply_config();
                            self.write_config();
                            // What is wrong says more than where it went.
                            if let Some(w) = warning {
                                self.status = w;
                            }
                            self.mode = Mode::Settings(s);
                            return;
                        }
                        // Nothing is written while the field holds something
                        // that will not parse; it stays open to be fixed.
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
                    let key_name = setting_at(s.selected).map(|r| r.0).unwrap_or("");
                    match self.config.set(key_name, &buf) {
                        Ok(()) => {
                            self.status = self
                                .apply_config()
                                .unwrap_or_else(|| format!("{key_name} = {buf}"));
                        }
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                _ => {
                    edit_field(&mut buf, &mut s.cursor, 0, key);
                    s.editing = Some(buf);
                }
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
                    self.status = "keybinds".into();
                    return;
                }
                // The folder opens beside tiny, and the settings stay open.
                1 => self.open_plugins_folder(),
                2 => {
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
        let Some(path) = self.config_path.clone() else {
            self.status = "there is no config file to write".into();
            return;
        };
        self.status = match self.config.save_to(&path) {
            Ok(()) => format!("wrote {}", path.display()),
            Err(e) => format!("{e:#}"),
        };
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
    /// key that will become a binding. In the second, *every* key is data, so
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
                self.status = "settings".into();
                return;
            }
            KeyCode::Char('s') if ctrl => self.write_config(),
            KeyCode::Delete | KeyCode::Backspace => {
                if kb.selected >= KEYBIND_BUTTONS.len() {
                    let action = actions[kb.selected - KEYBIND_BUTTONS.len()];
                    self.config.keys.remove(action.name());
                    let warning = self.apply_config();
                    let shipped = self.keymap.shipped(action);
                    self.status =
                        warning.unwrap_or_else(|| format!("{} back to {}", action.name(), shipped));
                }
            }
            KeyCode::Enter => {
                if kb.selected < KEYBIND_BUTTONS.len() {
                    self.confirm_reset_keybinds();
                    return;
                }
                kb.capturing = true;
                let action = actions[kb.selected - KEYBIND_BUTTONS.len()];
                self.status = format!("press a key for {}", action.name());
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
    /// it, so what the window shows is what actually happens: two rows
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
        if let Some(warning) = self.apply_config() {
            self.status = warning;
            return;
        }
        self.status = match clashes.first() {
            // Say what was taken away, rather than letting a key quietly stop
            // doing what it used to.
            Some(other) => format!("{} = {spec}, taken from {}", action.name(), other.name()),
            None => format!("{} = {spec}", action.name()),
        };
    }

    /// Record what reaches an action, dropping the override entirely when it is
    /// back to the shipped keys, so the config file only ever holds what has
    /// actually been changed.
    fn set_binding(&mut self, action: Action, spec: &str) {
        if spec == self.keymap.shipped(action) {
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
