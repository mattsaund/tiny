//! Two editors side by side.
//!
//! `Ctrl+W` splits the space beside the browser into two equal panes, and
//! `Ctrl+W` again goes back to one, keeping the pane that has the keyboard.
//! `Shift+Left` and `Shift+Right` move the keyboard between them.
//!
//! # One live pane, and one set aside
//!
//! Everything tiny knows about showing a file describes one pane: the preview
//! that follows the browser's cursor, the buffer and its cursor, a rendered
//! note and its scroll. Rather than teach all of that about two, a split keeps
//! exactly one pane live: the one with the keyboard, which is also the one the
//! browser opens files into. The other is a [`Pane`], what that pane showed
//! and where, set aside. It is drawn by lending it the live state for one draw
//! (see `ui::draw_pinned`), and moving between panes swaps the two.
//!
//! So both panes behave exactly as the single one always did, because there
//! is only ever one of each thing doing the behaving. It is also what lets one
//! file sit in both panes, each with its own place in it: a buffer's cursor
//! and scroll are a [`View`], and each pane keeps its own.
//!
//! # Opening and closing
//!
//! A new split shows the file you are in, twice: the right pane takes the
//! keyboard and the left keeps your place, so the first thing you do in the
//! new pane, whether that is scrolling or opening another file from the
//! browser, leaves the old view where it was. Closing keeps the pane with
//! the keyboard, because the one you are working in is the one you meant.

use crate::config::keys::Action;
use crate::text::editor::View;
use crate::text::highlight::Resume;

use super::App;
use super::mode::{Focus, Window};
use super::preview::Preview;

/// The narrowest the two panes may be together. Below this each would be too
/// narrow to read a line of code in, so a split is refused rather than drawn.
pub const MIN_SPLIT_WIDTH: u16 = 48;

/// The main window, split in two.
pub struct Split {
    /// Whether the live pane, the one with the keyboard, is the right one.
    pub live_right: bool,
    /// The other pane, as it was left.
    pub pinned: Pane,
}

/// A pane without the keyboard: what it showed, and where.
#[derive(Default)]
pub struct Pane {
    pub preview: Preview,
    /// How far down a rendered note was scrolled.
    pub preview_scroll: usize,
    /// Where in the file the pane was, when it shows a file with a buffer.
    pub view: Option<View>,
    /// Its own saved highlighting, so two files drawn every frame do not throw
    /// each other's away and each start again from the top.
    pub highlight: Resume,
}

impl App {
    /// `Ctrl+W`: two panes, or back to one.
    pub(super) fn toggle_split(&mut self) {
        // Panes belong to the main window, so the key brings you to them.
        if self.window != Window::Main {
            self.show_window(Window::Main);
        }
        if self.split.take().is_some() {
            self.status = "one pane".into();
            return;
        }
        if self.last_panes_width < MIN_SPLIT_WIDTH {
            self.status = "too narrow for two panes".into();
            return;
        }
        // Set aside a copy of where you are; the live pane carries on from
        // the same place on the right.
        let pinned = Pane {
            preview: self.preview.clone(),
            preview_scroll: self.preview_scroll,
            view: self.active_buffer().map(|ed| ed.view()),
            highlight: Resume::default(),
        };
        self.split = Some(Split {
            live_right: true,
            pinned,
        });
        self.status = format!(
            "two panes: {} and {} move between them, {} closes one",
            self.key_name(Action::PaneLeft),
            self.key_name(Action::PaneRight),
            self.key_name(Action::Split),
        );
    }

    /// `Shift+Left` or `Shift+Right`: the keyboard to that pane.
    ///
    /// Returns false when there is only one pane, so the key goes on to do
    /// whatever it did before there were two, like moving the cursor.
    pub(super) fn focus_pane(&mut self, right: bool) -> bool {
        let Some(split) = &self.split else {
            return false;
        };
        if split.live_right != right {
            self.swap_panes();
        }
        // Straight to the pane, not through `focus_editor`: on a picture that
        // would open it in another program, and moving between panes is not
        // asking to open anything.
        self.focus = Focus::Editor;
        self.status = match self.preview.path() {
            Some(path) => super::parts::display_name(path),
            None => "an empty pane".into(),
        };
        true
    }

    /// Set the live pane aside and take up the other one.
    fn swap_panes(&mut self) {
        let Some(mut split) = self.split.take() else {
            return;
        };
        let leaving = Pane {
            preview: std::mem::take(&mut self.preview),
            preview_scroll: self.preview_scroll,
            view: None,
            highlight: std::mem::take(&mut self.highlight_cache),
        };
        // The view comes from the buffer the live pane was showing, read
        // before anything moves the browser's cursor.
        let leaving_view = leaving
            .preview
            .path()
            .and_then(|p| self.buffers.get(p))
            .map(|ed| ed.view());
        let arriving = std::mem::replace(
            &mut split.pinned,
            Pane {
                view: leaving_view,
                ..leaving
            },
        );
        split.live_right = !split.live_right;
        self.split = Some(split);
        self.take_up(arriving);
    }

    /// Make a pane set aside the live one again: the browser's cursor goes to
    /// its file, and the file comes back where that pane left it.
    fn take_up(&mut self, pane: Pane) {
        let found = match pane.preview.path() {
            Some(path) => {
                let path = path.to_path_buf();
                self.reveal(&path)
            }
            None => false,
        };
        if !found {
            // Gone since, or never a file: show it as it was rather than
            // something the browser happens to be resting on.
            self.preview = pane.preview;
        }
        self.preview_scroll = pane.preview_scroll;
        self.highlight_cache = pane.highlight;
        if let (Some(view), Some(ed)) = (pane.view, self.active_buffer_mut()) {
            ed.set_view(view);
        }
    }

    /// One notch of the wheel over the pane without the keyboard.
    ///
    /// Moves that pane and nothing else. A file with a buffer moves by its
    /// cursor, the way the live editor does, so the view cannot be left
    /// somewhere the cursor would snap it back from; anything else scrolls.
    pub(super) fn scroll_pinned(&mut self, down: bool) {
        let Some(split) = &mut self.split else {
            return;
        };
        let pane = &mut split.pinned;
        // A note set aside is drawn rendered, which scrolls by its own count
        // and has no cursor to move. Drawing clamps both.
        let rendered = matches!(&pane.preview, Preview::Buffer { kind, .. } if kind.reads_first());
        match (&mut pane.view, rendered) {
            (Some(view), false) => {
                view.cursor_line = if down {
                    view.cursor_line + 1
                } else {
                    view.cursor_line.saturating_sub(1)
                };
            }
            _ => {
                pane.preview_scroll = if down {
                    pane.preview_scroll + 1
                } else {
                    pane.preview_scroll.saturating_sub(1)
                };
            }
        }
    }
}
