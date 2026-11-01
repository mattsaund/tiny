//! The two levels of the keyboard: picking a part of the window, and being
//! in one.
//!
//! The main window has parts: the browser, each pane, the terminal under
//! them. One of them is picked at a time, and the keyboard is either *in*
//! it, which its green outline says, or out of all of them, picking, which a
//! white one says. While picking, the arrows (and `i` `j` `k` `l`) move the
//! outline from part to part, the way they point on the screen, and Enter
//! goes into the one it is on. Esc comes back out to picking from anywhere.
//! It never quits; `Ctrl+Q` does that.
//!
//! What the keys do inside a part is what they always did: the browser's
//! arrows move through files, a file types, the terminal is the shell's.
//! Picking is the level above, which is also where the bare keys that are
//! letters inside a file are free: `/` searches and `*` starts a command
//! from wherever the outline is.
//!
//! The git window has the same two levels for its two halves, the list of
//! changes and what is drawn beside it. The map is one part, always in.
//!
//! # The pane the browser opens files into
//!
//! With several panes, the browser shows what its cursor is on in one of
//! them: the one last gone into, whatever the arrows have picked since. A
//! pane only picked keeps what it shows, so walking the outline across it to
//! reach the browser does not take it over. The terminal is never that
//! pane: going into it leaves the browser opening files where it did.
//!
//! # Where the parts are
//!
//! Only `ui` knows where anything was drawn, so picking reads the last frame:
//! where the browser, each pane and the terminal were. An arrow goes to the
//! nearest part that lies that way, preferring one that is level with the
//! picked part, then the pane last used. A browser folded away comes back
//! when the arrows go toward where it was.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::config::keys::{Action, Context as KeyContext};

use super::App;
use super::mode::{Focus, Window};
use super::preview::Preview;
use super::source::GitFocus;
use crate::config::Side;

/// Which way an arrow points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Way {
    Up,
    Down,
    Left,
    Right,
}

/// A part of the main window that can be picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    Tree,
    /// The pane last used, or with `Some`, one set aside.
    Pane(Option<usize>),
    Terminal,
}

/// How far `to` begins beyond the edge of `from` that faces `way`, or `None`
/// when it does not lie that way. Parts that touch share no cell, so the
/// one beside begins exactly where the other ends; a cell of overlap is
/// allowed for borders drawn on the same line.
fn ahead(from: Rect, to: Rect, way: Way) -> Option<u16> {
    match way {
        Way::Right => {
            (to.left() + 1 >= from.right()).then(|| to.left().saturating_sub(from.right()))
        }
        Way::Left => {
            (to.right() <= from.left() + 1).then(|| from.left().saturating_sub(to.right()))
        }
        Way::Down => {
            (to.top() + 1 >= from.bottom()).then(|| to.top().saturating_sub(from.bottom()))
        }
        Way::Up => (to.bottom() <= from.top() + 1).then(|| from.top().saturating_sub(to.bottom())),
    }
}

/// How much `to` is level with `from` across `way`, and how far apart their
/// middles are that way: a part beside is better the more of it is beside.
fn level(from: Rect, to: Rect, way: Way) -> (u16, u16) {
    let (a0, a1, b0, b1) = match way {
        Way::Left | Way::Right => (from.top(), from.bottom(), to.top(), to.bottom()),
        Way::Up | Way::Down => (from.left(), from.right(), to.left(), to.right()),
    };
    let overlap = a1.min(b1).saturating_sub(a0.max(b0));
    (overlap, (a0 + a1).abs_diff(b0 + b1) / 2)
}

/// The part `way` from the one at `from`, among `parts` as the last frame
/// drew them: the nearest that lies that way, one level with it before one
/// that is not, and between equals the pane last used, then the one whose
/// middle is nearest.
pub(super) fn toward(from: Rect, way: Way, parts: &[(Part, Rect)]) -> Option<Part> {
    parts
        .iter()
        .filter(|(_, r)| *r != from)
        .filter_map(|&(part, r)| {
            let gap = ahead(from, r, way)?;
            let (overlap, apart) = level(from, r, way);
            Some(((overlap == 0, gap, part != Part::Pane(None), apart), part))
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, part)| part)
}

impl App {
    /// Whether a file is being typed into: a pane of the main window, gone
    /// into, with a text file in it.
    pub fn editing(&self) -> bool {
        self.window == Window::Main
            && self.focus == Focus::Editor
            && self.entered
            && matches!(self.preview, Preview::Buffer { .. })
    }

    /// The keyboard into `focus`.
    pub(super) fn enter(&mut self, focus: Focus) {
        self.focus = focus;
        self.entered = true;
        self.picked_pane = None;
    }

    /// Esc, in a part of the main window: out of it, to pick another. The
    /// part stays picked, so Enter goes straight back in.
    pub(super) fn step_out(&mut self) {
        self.entered = false;
        self.status = format!(
            "arrows pick a window, {} goes in",
            self.key_name(Action::Open)
        );
    }

    /// Every part of the main window, where the last frame drew it.
    fn parts(&self) -> Vec<(Part, Rect)> {
        let mut parts = Vec::new();
        if let Some(r) = self.last_tree_inner.filter(|_| !self.tree_hidden) {
            parts.push((Part::Tree, r));
        }
        parts.push((Part::Pane(None), self.last_live_area));
        parts.extend(
            self.last_pinned
                .iter()
                .map(|&(id, r)| (Part::Pane(Some(id)), r)),
        );
        if let Some(r) = self.last_term_area.filter(|_| self.term.is_some()) {
            parts.push((Part::Terminal, r));
        }
        parts
    }

    /// Where the picked part is.
    fn picked_area(&self) -> Option<Rect> {
        match self.focus {
            Focus::Tree => self.last_tree_inner.filter(|_| !self.tree_hidden),
            Focus::Editor => match self.picked_pane {
                Some(id) => self
                    .last_pinned
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map(|(_, r)| *r)
                    .or(Some(self.last_live_area)),
                None => Some(self.last_live_area),
            },
            Focus::Terminal => self.last_term_area,
        }
    }

    /// What a pane set aside shows, for the status line.
    fn pinned_name(&self, id: usize) -> String {
        let pane = self
            .split
            .as_ref()
            .and_then(|s| s.pinned.iter().find(|(p, _)| *p == id));
        match pane.and_then(|(_, pane)| pane.preview.path()) {
            Some(path) => super::parts::display_name(path),
            None => "an empty pane".into(),
        }
    }

    /// An arrow while picking: the outline to the next part that way.
    pub(super) fn pick_toward(&mut self, way: Way) {
        let parts = self.parts();
        let next = self
            .picked_area()
            .and_then(|from| toward(from, way, &parts));
        let Some(next) = next else {
            // Toward where the folded browser would be: it comes back.
            let side = match self.config.tree_side {
                Side::Left => Way::Left,
                Side::Right => Way::Right,
            };
            if self.tree_hidden && way == side {
                self.tree_hidden = false;
                self.focus = Focus::Tree;
                self.picked_pane = None;
                self.status = "browser".into();
            }
            return;
        };
        self.picked_pane = None;
        match next {
            Part::Tree => {
                self.focus = Focus::Tree;
                self.status = "browser".into();
            }
            Part::Pane(None) => {
                self.focus = Focus::Editor;
                self.status = self.pane_name();
            }
            // Picked, and nothing more: the pane the browser opens files
            // into stays the one last gone into.
            Part::Pane(Some(id)) => {
                self.focus = Focus::Editor;
                self.picked_pane = Some(id);
                self.status = self.pinned_name(id);
            }
            Part::Terminal => {
                self.focus = Focus::Terminal;
                self.status = "terminal".into();
            }
        }
    }

    /// What the pane last used shows, for the status line.
    fn pane_name(&self) -> String {
        match self.preview.path() {
            Some(path) => super::parts::display_name(path),
            None => "an empty pane".into(),
        }
    }

    /// Enter while picking: into the part the outline is on.
    pub(super) fn enter_picked(&mut self) {
        match self.focus {
            Focus::Tree => {
                self.tree_hidden = false;
                self.enter(Focus::Tree);
                self.status = "browser".into();
            }
            // A file types, a picture opens in the desktop's own program,
            // and a folder or a binary has nothing to go into. A pane set
            // aside becomes the one the browser opens files into first.
            Focus::Editor => {
                if let Some(id) = self.picked_pane.take() {
                    self.focus_pane(id);
                }
                self.enter_picked_pane();
            }
            Focus::Terminal => self.open_terminal(),
        }
    }

    /// Enter on the pane the browser opens files into.
    fn enter_picked_pane(&mut self) {
        match &self.preview {
            Preview::Buffer { .. } | Preview::Media { .. } => self.focus_editor(),
            Preview::Binary { kind, .. } => self.status = format!("{kind}, not editable"),
            _ => self.status = "nothing to go into here".into(),
        }
    }

    /// A key while picking a part of the main window.
    ///
    /// The movements are the browser's, which are bound in every pane that
    /// is navigated rather than typed into: the arrows, and `i` `j` `k` `l`.
    pub(super) fn on_pick_key(&mut self, key: KeyEvent) {
        let Some(action) = self.keymap.resolve(KeyContext::Tree, &key) else {
            return;
        };
        if self.on_global_action(action) {
            return;
        }
        match action {
            Action::Up => self.pick_toward(Way::Up),
            Action::Down => self.pick_toward(Way::Down),
            Action::Left => self.pick_toward(Way::Left),
            Action::Right => self.pick_toward(Way::Right),
            Action::Open => self.enter_picked(),
            // What is shown is what is saved: the file in a picked pane, and
            // otherwise what the browser's cursor is on, all of a folder.
            Action::Save => {
                let pinned = self.picked_pane.and_then(|id| {
                    let split = self.split.as_ref()?;
                    let (_, pane) = split.pinned.iter().find(|(p, _)| *p == id)?;
                    pane.preview.path().map(std::path::Path::to_path_buf)
                });
                if let Some(path) = pinned.filter(|p| self.buffers.contains_key(p)) {
                    let (saved, failed) = self.save_each(std::slice::from_ref(&path));
                    self.status = match failed.first() {
                        Some(why) => format!("save failed: {why}"),
                        None if saved == 0 => "no changes".into(),
                        None => format!("saved {}", super::parts::display_name(&path)),
                    };
                } else if self.focus == Focus::Editor
                    && matches!(self.preview, Preview::Buffer { .. })
                {
                    self.save_active();
                } else {
                    self.save_from_tree();
                }
            }
            Action::Back => {
                self.status = format!(
                    "arrows pick a window, {} goes in, {} quits",
                    self.key_name(Action::Open),
                    self.key_name(Action::Quit)
                );
            }
            _ => {}
        }
    }

    /// A key while picking a half of the git window: the list, or what is
    /// drawn beside it. Esc from here is back to the main window.
    pub(super) fn on_git_pick_key(&mut self, key: KeyEvent) {
        let Some(action) = self.keymap.resolve(KeyContext::Source, &key) else {
            return;
        };
        if self.on_global_action(action) {
            return;
        }
        match action {
            Action::Left => self.git.focus = GitFocus::List,
            Action::Right => {
                if self.git.message.is_some() {
                    self.git.focus = GitFocus::Message;
                } else if !self.git.diff.is_empty() {
                    self.git.focus = GitFocus::Diff;
                }
            }
            Action::Open => {
                self.git.entered = true;
                self.status = match self.git.focus {
                    GitFocus::List => "the changes".into(),
                    GitFocus::Diff => "reading the change".into(),
                    GitFocus::Message => {
                        format!("the message: {} commits", self.key_name(Action::Save))
                    }
                };
            }
            Action::Back => self.show_window(Window::Main),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser down the left, two panes side by side, and the terminal
    /// under the panes.
    fn screen() -> Vec<(Part, Rect)> {
        vec![
            (Part::Tree, Rect::new(0, 0, 30, 40)),
            (Part::Pane(None), Rect::new(30, 0, 35, 28)),
            (Part::Pane(Some(1)), Rect::new(65, 0, 35, 28)),
            (Part::Terminal, Rect::new(30, 28, 70, 12)),
        ]
    }

    fn from(part: Part) -> Rect {
        screen().into_iter().find(|(p, _)| *p == part).unwrap().1
    }

    #[test]
    fn the_arrows_go_to_the_part_that_lies_that_way() {
        let s = screen();
        assert_eq!(
            toward(from(Part::Tree), Way::Right, &s),
            Some(Part::Pane(None))
        );
        assert_eq!(
            toward(from(Part::Pane(None)), Way::Right, &s),
            Some(Part::Pane(Some(1)))
        );
        assert_eq!(
            toward(from(Part::Pane(Some(1))), Way::Left, &s),
            Some(Part::Pane(None)),
            "the pane beside, not the browser further on"
        );
        assert_eq!(
            toward(from(Part::Pane(None)), Way::Down, &s),
            Some(Part::Terminal)
        );
        assert_eq!(
            toward(from(Part::Terminal), Way::Left, &s),
            Some(Part::Tree)
        );
        assert_eq!(
            toward(from(Part::Tree), Way::Left, &s),
            None,
            "nothing there"
        );
        assert_eq!(toward(from(Part::Pane(None)), Way::Up, &s), None);
    }

    #[test]
    fn up_from_the_terminal_is_the_pane_last_used() {
        let s = screen();
        assert_eq!(
            toward(from(Part::Terminal), Way::Up, &s),
            Some(Part::Pane(None)),
            "both panes are above it; the one last used wins"
        );
    }

    #[test]
    fn a_part_level_with_this_one_comes_before_a_nearer_one_that_is_not() {
        // A pane under the browser's foot, and one beside it but further.
        let parts = vec![
            (Part::Tree, Rect::new(0, 0, 20, 10)),
            (Part::Pane(Some(2)), Rect::new(20, 10, 30, 10)),
            (Part::Pane(None), Rect::new(24, 0, 30, 10)),
        ];
        assert_eq!(
            toward(Rect::new(0, 0, 20, 10), Way::Right, &parts),
            Some(Part::Pane(None))
        );
    }
}
