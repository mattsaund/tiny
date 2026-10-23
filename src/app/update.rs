//! Asking whether to install a newer tiny.
//!
//! The asking of GitHub is [`crate::update`]'s; this is the asking of the
//! person. The answer comes back on a thread whenever it comes back, and the
//! question is put only once they are somewhere it cannot be answered by
//! accident: in the browser, with nothing else open. Put while they were
//! typing, the very next letter of a sentence — a `y` — would answer it.
//!
//! `y` does not install anything here. It asks to quit, the way `Ctrl+Q`
//! does — saving first, or not, if anything is unsaved — and leaves the
//! version in [`App::update_on_exit`] for `main` to install once the terminal
//! is an ordinary terminal again.

use crate::update;

use super::App;
use super::mode::{Confirm, ConfirmKind, Focus, Mode, Window};

impl App {
    /// Ask GitHub, on a thread, whether a newer tiny is out.
    ///
    /// `by_hand` is `*update`: the answer is then reported whatever it is, and
    /// the question is put wherever you are, since you just asked for it.
    pub fn start_update_check(&mut self, by_hand: bool) {
        self.update_by_hand = by_hand;
        if self.update_check.is_none() {
            self.update_check = Some(update::Check::start());
        }
    }

    /// Collect the check's answer, and put the question if it is time to.
    /// Returns whether anything on screen changed. Called on the event loop's
    /// idle tick.
    pub fn poll_update(&mut self) -> bool {
        let mut changed = false;
        if let Some(answer) = self.update_check.as_ref().and_then(|c| c.finished()) {
            self.update_check = None;
            match answer {
                Some(version) => self.update_found = Some(version),
                None if self.update_by_hand => {
                    self.status = format!("tiny {} is the newest", update::running());
                    changed = true;
                }
                None => {}
            }
        }
        if self.update_found.is_some() && self.can_ask_about_update() {
            let version = self.update_found.take().unwrap_or_default();
            self.offer_update(version);
            changed = true;
        }
        changed
    }

    /// Whether the question can go up now without catching a keystroke meant
    /// for something else.
    fn can_ask_about_update(&self) -> bool {
        matches!(self.mode, Mode::Normal)
            && (self.update_by_hand || (self.window == Window::Main && self.focus == Focus::Tree))
    }

    /// Put the question: the window the user asked for, in the words they
    /// asked for it.
    pub(super) fn offer_update(&mut self, version: String) {
        self.update_by_hand = false;
        self.mode = Mode::Confirm(Confirm {
            message: format!(
                "tiny has been updated to version {version}, would you like to update now?  (y/n)"
            ),
            kind: ConfirmKind::Update(version),
        });
    }

    /// `y`: leave, and install on the way out. Unsaved work gets the same
    /// question a quit gets, and taking that question back takes the update
    /// back with it.
    pub(super) fn begin_update(&mut self, version: String) {
        self.update_on_exit = Some(version);
        self.request_quit_to("updating");
    }

    /// `n`: not now. Not asked again this run; `*update` asks again on purpose.
    pub(super) fn decline_update(&mut self) {
        self.status = format!(
            "staying on {}, *update installs it later",
            update::running()
        );
    }

    /// `*update`: offer what is already known to be newer, or go and ask.
    pub(super) fn cmd_update(&mut self) -> anyhow::Result<String> {
        if let Some(version) = self.update_found.take() {
            self.offer_update(version);
            return Ok(String::new());
        }
        self.start_update_check(true);
        Ok("asking GitHub for a newer tiny".into())
    }
}
