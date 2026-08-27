//! Answering a prompt, or a confirmation.
//!
//! Two tiny keyboards. A prompt collects a line of text and hands it to
//! whoever asked; a confirmation collects a yes or a no. Both are modal, and
//! both close on Esc without doing anything.
//!
//! What each one *means* is in its `Kind`, and the work it triggers lives with
//! the code that asked the question — creating a file is [`super::fileops`]'s
//! job, not this module's. This is only the part that reads the answer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Config;
use crate::text::search::{self};

use super::App;
use super::mode::{Confirm, ConfirmKind, Keybinds, Mode, Prompt, Settings};
use super::parts::{char_byte, plural};

impl App {
    // ---- prompts & confirmations -----------------------------------------

    /// Keys for the status-bar text prompt. Ctrl chords are ignored outright
    /// rather than acted on — there is nothing useful to do with them here, and
    /// letting one through would type its letter into a filename.
    pub(super) fn on_prompt_key(&mut self, mut p: Prompt, key: KeyEvent) {
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
    pub(super) fn on_confirm_key(&mut self, c: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => match c.kind {
                ConfirmKind::Delete(path) => self.do_delete(&path),
                ConfirmKind::QuitUnsaved => self.should_quit = true,
                ConfirmKind::Replace { find, replace } => self.do_replace(&find, &replace),
                ConfirmKind::ResetSettings => self.do_reset_settings(),
                ConfirmKind::ResetKeybinds => self.do_reset_keybinds(),
            },
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                // A reset was asked for from a window; go back to it rather
                // than dropping the user out to the tree.
                match c.kind {
                    ConfirmKind::ResetSettings => self.mode = Mode::Settings(Settings::default()),
                    ConfirmKind::ResetKeybinds => self.mode = Mode::Keybinds(Keybinds::default()),
                    _ => {}
                }
                self.status = "cancelled".into();
            }
            _ => self.mode = Mode::Confirm(c),
        }
    }

    /// Put every setting back to what tiny ships with, keeping the rebindings:
    /// they are a different question and were reset by a different button.
    ///
    /// The file on disk is untouched until `Ctrl+S`, so a reset answered by
    /// mistake costs nothing as long as you do not save.
    fn do_reset_settings(&mut self) {
        let keys = std::mem::take(&mut self.config.keys);
        self.config = Config {
            keys,
            ..Config::default()
        };
        self.apply_config();
        self.tree.set_show_hidden(self.config.show_hidden);
        self.rebuild_rows();
        self.mode = Mode::Settings(Settings::default());
        self.status = "settings reset — Ctrl+S to write it".into();
    }

    /// Put every key back, keeping the settings.
    fn do_reset_keybinds(&mut self) {
        self.config.keys.clear();
        self.apply_config();
        self.mode = Mode::Keybinds(Keybinds::default());
        self.status = "keybinds reset — Ctrl+S to write it".into();
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
}
