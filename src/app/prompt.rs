//! Answering a confirmation.
//!
//! One tiny keyboard: a yes or a no, modal, closing on Esc without doing
//! anything. What the question *means* is in its [`ConfirmKind`], and the work
//! it triggers lives with the code that asked: deleting a file is
//! [`super::fileops`]'s job, not this module's. This is only the part that
//! reads the answer.
//!
//! There used to be a text prompt here as well, for naming a new file and for
//! renaming one. Both are commands now (`*new`, `*rename`), and a command line
//! is a text prompt that already existed, so the second one went.

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::Config;
use crate::text::search::{self};

use super::App;
use super::mode::{Confirm, ConfirmKind, Keybinds, Mode, Settings};
use super::parts::plural;

impl App {
    /// Keys for a yes/no question. Anything else leaves it standing rather
    /// than guessing.
    ///
    /// `n` is not always the same key as Esc. For most of these there are two
    /// answers and no is the same as never mind, but the question on the way
    /// out has three: save and go, go without saving, or stay here. `n` is the
    /// middle one (it answers "no, do not save" to what was asked) and Esc
    /// is the one that takes the whole quit back.
    pub(super) fn on_confirm_key(&mut self, c: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => match c.kind {
                ConfirmKind::Delete(path) => self.do_delete(&path),
                ConfirmKind::QuitUnsaved => self.save_all_and_quit(),
                ConfirmKind::Replace { find, replace } => self.do_replace(&find, &replace),
                ConfirmKind::ResetSettings => self.do_reset_settings(),
                ConfirmKind::ResetKeybinds => self.do_reset_keybinds(),
                ConfirmKind::Update(version) => self.begin_update(version),
            },
            KeyCode::Char('n') | KeyCode::Char('N')
                if matches!(c.kind, ConfirmKind::QuitUnsaved) =>
            {
                self.should_quit = true;
                self.status = "quit without saving".into();
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                // A reset was asked for from a window; go back to it rather
                // than dropping the user out to the tree.
                match c.kind {
                    ConfirmKind::ResetSettings => self.mode = Mode::Settings(Settings::default()),
                    ConfirmKind::ResetKeybinds => self.mode = Mode::Keybinds(Keybinds::default()),
                    ConfirmKind::Update(_) => return self.decline_update(),
                    // Staying, after all, so an update that was waiting on
                    // the way out is not something a later Ctrl+Q should do.
                    ConfirmKind::QuitUnsaved => self.update_on_exit = None,
                    _ => {}
                }
                self.status = "canceled".into();
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
        // Which also shows or hides dotfiles again, if that changed.
        self.apply_config();
        self.mode = Mode::Settings(Settings::default());
        self.status = "settings reset, Ctrl+S to write it".into();
    }

    /// Put every key back, keeping the settings.
    fn do_reset_keybinds(&mut self) {
        self.config.keys.clear();
        self.apply_config();
        self.mode = Mode::Keybinds(Keybinds::default());
        self.status = "keybinds reset, Ctrl+S to write it".into();
    }

    /// Carry out a confirmed project-wide replace.
    ///
    /// Files are rewritten on disk, and the clean buffers open on them are
    /// brought up to date in place. A file open with unsaved edits is the
    /// exception: its buffer is the newer copy, so the replace happens there,
    /// as one step of undo left unsaved with the rest of its edits, rather
    /// than on a disk copy its next save would write over.
    fn do_replace(&mut self, find: &search::Needle, replace: &str) {
        let opts = self.search_opts();
        let unsaved = self.unsaved_in_reach();
        let written = search::replace_all(self.tree.root_path(), find, replace, &opts, &|path| {
            unsaved.contains(path)
        });
        let mut in_buffers = 0;
        let mut occurrences = 0;
        for path in &unsaved {
            if let Some(ed) = self.buffers.get_mut(path) {
                let (text, n) = find.replace_in(&ed.to_text(), replace);
                if n > 0 {
                    ed.replace_all(&text);
                    in_buffers += 1;
                    occurrences += n;
                }
            }
        }
        // Whatever was written, the open files on it are behind now.
        self.reread_clean_buffers();
        self.tree.refresh_all();
        self.rebuild_rows();
        self.status = match written {
            Ok(report) => {
                let files = report.files + in_buffers;
                let occurrences = report.occurrences + occurrences;
                let unsaved = match in_buffers {
                    0 => String::new(),
                    1 => ", one of them open and not saved yet".into(),
                    n => format!(", {n} of them open and not saved yet"),
                };
                format!(
                    "replaced {occurrences} occurrence{} in {files} file{}{unsaved}",
                    plural(occurrences),
                    plural(files)
                )
            }
            Err(e) => format!("{e:#}"),
        };
    }
}
