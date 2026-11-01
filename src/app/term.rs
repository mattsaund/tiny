//! The terminal pane: a shell under the panes, see [`crate::shell`].
//!
//! `Ctrl+O` (or `*terminal`) starts one in the project's folder and gives it
//! the keyboard, and `Ctrl+O` again gives the keyboard back to wherever it
//! was, leaving the shell on screen and running. While it has the keyboard,
//! every key goes to the shell as it was typed, `Ctrl+C` and `Ctrl+W`
//! included, which is what makes it a terminal: the key kept back is the one
//! that leaves. The shell exiting closes the pane, and so does `*close
//! terminal`.
//!
//! Esc steps out to picking another part, as it does everywhere, while the
//! shell is at its prompt. A program that has taken the whole screen (an
//! editor, a pager, `htop`) gets Esc instead, because it needs it; `Ctrl+O`
//! still leaves from there.

use crossterm::event::KeyEvent;

use crate::config::keys::{Action, Context as KeyContext};
use crate::shell::Shell;

use super::App;
use super::mode::{Focus, Window};

impl App {
    /// `Ctrl+O`: the terminal, and back.
    pub(super) fn toggle_terminal(&mut self) {
        if self.focus == Focus::Terminal {
            return self.leave_terminal();
        }
        self.open_terminal();
    }

    /// `*terminal`: the terminal, started if there is none, with the
    /// keyboard.
    pub(super) fn open_terminal(&mut self) {
        if self.window != Window::Main {
            self.show_window(Window::Main);
        }
        if self.term.is_none() {
            let room = self.last_live_area;
            // As tall as the last one was left, and drawing fits it after.
            let rows = ((f32::from(room.height) * self.term_share) as u16).max(4);
            let root = self.root().to_path_buf();
            match Shell::start(&root, rows, room.width.saturating_sub(2)) {
                Ok(shell) => self.term = Some(shell),
                Err(e) => {
                    self.status = format!("{e:#}");
                    return;
                }
            }
        }
        if !(self.focus == Focus::Terminal && self.entered) {
            self.focus_before_term = (self.focus, self.entered);
        }
        self.enter(Focus::Terminal);
        self.status = format!("terminal, {} comes back", self.key_name(Action::Terminal));
    }

    /// The keyboard back to where it was before the terminal took it.
    fn leave_terminal(&mut self) {
        let (before, entered) = self.focus_before_term;
        (self.focus, self.entered) = match before {
            // The pane may have stopped showing a file in the meantime.
            Focus::Editor if self.active_buffer().is_none() => (Focus::Tree, entered),
            Focus::Terminal => (Focus::Tree, true),
            before => (before, entered),
        };
        self.status = "terminal still running".into();
    }

    /// `*close terminal`: the shell goes, and whatever it was running.
    pub(super) fn close_terminal(&mut self) {
        if self.term.take().is_none() {
            self.status = "no terminal open".into();
            return;
        }
        if self.focus == Focus::Terminal {
            self.leave_terminal();
        }
        self.status = "closed the terminal".into();
    }

    /// Whether there is a shell running, which the event loop keeps an eye
    /// on.
    pub fn has_terminal(&self) -> bool {
        self.term.is_some()
    }

    /// Take in what the shell has written. Returns whether there is anything
    /// new to draw. A shell that has exited takes its pane with it.
    pub fn poll_terminal(&mut self) -> bool {
        let Some(shell) = &mut self.term else {
            return false;
        };
        let news = shell.poll();
        if shell.done {
            self.term = None;
            if self.focus == Focus::Terminal {
                self.leave_terminal();
            }
            self.status = "the shell exited".into();
        }
        news
    }

    /// A key while the terminal has the keyboard: the shell's, unless it is
    /// the one that leaves, or Esc at the shell's prompt, see the module docs.
    pub(super) fn on_terminal_key(&mut self, key: KeyEvent) {
        if self.keymap.find(KeyContext::Global, &key) == Some(Action::Terminal) {
            return self.leave_terminal();
        }
        let whole_screen = self
            .term
            .as_ref()
            .is_some_and(|shell| shell.screen().alternate_screen());
        if !whole_screen && self.keymap.find(KeyContext::Tree, &key) == Some(Action::Back) {
            return self.step_out();
        }
        match &mut self.term {
            Some(shell) => shell.key(key),
            // Nothing to type into: the keyboard goes back rather than
            // into nowhere.
            None => self.leave_terminal(),
        }
    }

    /// Text pasted while the terminal has the keyboard.
    pub(super) fn paste_into_terminal(&mut self, text: &str) {
        if let Some(shell) = &mut self.term {
            shell.paste(text);
        }
    }
}
