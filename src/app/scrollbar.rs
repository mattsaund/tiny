//! Scrollbars, and moving them with the mouse.
//!
//! A part of the window gets one down its right side while what it shows
//! runs past what fits, and only then, see `ui::parts::scrollbar`. Every
//! frame writes down where each was drawn and what it scrolls (a
//! [`ScrollHit`]), so a press can be read against what was on screen.
//!
//! A press on the thumb takes hold of it where it was pressed. A press
//! elsewhere on the bar brings the thumb there, its middle under the
//! pointer. Either way a drag goes on moving it until the button comes up.
//! What follows a cursor (the browser, a file, the git window's lists)
//! brings its cursor along into what is shown, the way the wheel does,
//! since the next frame would otherwise scroll back to the cursor.

use ratatui::layout::Position;

use super::App;

/// How far down a part is scrolled: how many rows there are to show, how
/// many fit, and which is the first one shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scroll {
    pub total: usize,
    pub shown: usize,
    pub at: usize,
}

impl Scroll {
    /// Where the thumb sits on a bar `track` rows long, as its first row and
    /// how many rows it covers, or `None` when everything fits, which is when
    /// there is no scrollbar at all.
    ///
    /// The thumb is as much of the bar as what is shown is of all there is,
    /// a row at least. It runs from the top of the bar to the bottom as the
    /// part runs from its first row to its last.
    pub fn thumb(self, track: u16) -> Option<(u16, u16)> {
        if self.total <= self.shown || track == 0 {
            return None;
        }
        let track = usize::from(track);
        let len = ((track * self.shown + self.total / 2) / self.total).clamp(1, track);
        let room = self.total - self.shown;
        let at = self.at.min(room);
        let start = ((track - len) * at + room / 2) / room;
        Some((start as u16, len as u16))
    }

    /// The first row to show with the thumb's top at `start` on a bar
    /// `track` rows long: the way back from [`Scroll::thumb`].
    pub fn at_for(self, start: u16, track: u16) -> usize {
        let Some((_, len)) = self.thumb(track) else {
            return 0;
        };
        let room = self.total - self.shown;
        let free = usize::from(track - len);
        if free == 0 {
            // A thumb as long as the bar has nowhere to go.
            return self.at.min(room);
        }
        (usize::from(start).min(free) * room + free / 2) / free
    }
}

/// What a scrollbar scrolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scrolls {
    Browser,
    /// A pane's page: a note drawn for reading, by its own rows. `None` is
    /// the pane with the keyboard, otherwise one set aside.
    Page(Option<usize>),
    /// A pane's file, a row to a line.
    Lines(Option<usize>),
    /// The pane's note drawn formatted while it is edited, where rows are
    /// not lines: the line each row is of.
    Blocks(Vec<usize>),
    /// The git window's list of changes: the row of the list each drawn
    /// line is of, the buttons taking as many lines as they wrap to.
    GitList(Vec<usize>),
    GitBranches,
    GitDiff,
    GitMessage,
}

/// A scrollbar as the last frame drew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollHit {
    pub scrolls: Scrolls,
    /// The column it is in, its first row, and how many rows it runs.
    pub x: u16,
    pub top: u16,
    pub track: u16,
    pub scroll: Scroll,
}

impl ScrollHit {
    fn contains(&self, at: Position) -> bool {
        at.x == self.x && at.y >= self.top && at.y < self.top + self.track
    }
}

impl App {
    /// A press on a scrollbar, if it was on one: take hold of it, and move
    /// it there unless it was the thumb that was pressed. Returns whether
    /// the press was a scrollbar's.
    pub(super) fn press_scrollbar(&mut self, at: Position) -> bool {
        let Some(hit) = self
            .last_scrollbars
            .iter()
            .find(|h| h.contains(at))
            .cloned()
        else {
            return false;
        };
        let Some((start, len)) = hit.scroll.thumb(hit.track) else {
            return false;
        };
        let row = at.y - hit.top;
        let grab = if (start..start + len).contains(&row) {
            row - start
        } else {
            len / 2
        };
        self.move_scrollbar(&hit, row.saturating_sub(grab));
        self.scrolling = Some((hit, grab));
        true
    }

    /// The button moving while a scrollbar is held: the thumb follows the
    /// pointer, held where it was taken hold of. Returns whether anything
    /// moved.
    pub(super) fn drag_scrollbar(&mut self, at: Position) -> bool {
        let Some((hit, grab)) = self.scrolling.clone() else {
            return false;
        };
        let row = at.y.saturating_sub(hit.top);
        self.move_scrollbar(&hit, row.saturating_sub(grab));
        true
    }

    /// Scroll what `hit` scrolls so its thumb begins at `start`.
    fn move_scrollbar(&mut self, hit: &ScrollHit, start: u16) {
        let at = hit.scroll.at_for(start, hit.track);
        let shown = hit.scroll.shown.max(1);
        match &hit.scrolls {
            Scrolls::Browser => {
                self.tree_scroll = at;
                let last = self.rows.len().saturating_sub(1);
                let row = self.selected.clamp(at, at + shown - 1).min(last);
                self.select_index(row);
            }
            Scrolls::Page(None) => self.preview_scroll = at,
            Scrolls::Page(Some(id)) => {
                if let Some(pane) = self.pinned_mut(*id) {
                    pane.preview_scroll = at;
                }
            }
            Scrolls::Lines(None) => {
                if let Some(ed) = self.active_buffer_mut() {
                    ed.scroll_to(at, shown);
                }
            }
            Scrolls::Lines(Some(id)) => {
                if let Some(view) = self.pinned_mut(*id).and_then(|p| p.view.as_mut()) {
                    view.scroll_y = at;
                    view.scroll_rows = 0;
                    view.cursor_line = view.cursor_line.clamp(at, at + shown - 1);
                }
            }
            // The line the row at the top is of, with the cursor on it: a
            // note drawn formatted has no rows to put a cursor on but its
            // lines, and the view begins where the cursor's block does.
            Scrolls::Blocks(lines) => {
                let Some(&line) = lines.get(at.min(lines.len().saturating_sub(1))) else {
                    return;
                };
                if let Some(ed) = self.active_buffer_mut() {
                    ed.scroll_to(line, 1);
                }
            }
            Scrolls::GitList(rows) => {
                self.git.list_scroll = at;
                let first = |row: usize| rows.iter().position(|&r| r == row);
                let Some(on) = first(self.git.selected) else {
                    return;
                };
                let to = if on < at {
                    rows.get(at)
                } else if on >= at + shown {
                    rows.get(at + shown - 1)
                } else {
                    None
                };
                if let Some(&row) = to {
                    self.move_git_selection(row as isize - self.git.selected as isize);
                }
            }
            Scrolls::GitBranches => {
                self.git.branch_scroll = at;
                let row = self.git.branch_row.clamp(at, at + shown - 1);
                if row != self.git.branch_row {
                    self.move_branch_cursor(row as isize - self.git.branch_row as isize);
                }
            }
            Scrolls::GitDiff => self.git.diff_scroll = at,
            Scrolls::GitMessage => {
                if let Some(editor) = self.git.message.as_mut() {
                    editor.scroll_to(at, shown);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_scroll_is_no_scrollbar() {
        let all = Scroll {
            total: 10,
            shown: 10,
            at: 0,
        };
        assert_eq!(all.thumb(10), None);
        let empty = Scroll::default();
        assert_eq!(empty.thumb(10), None);
    }

    #[test]
    fn the_thumb_is_the_share_shown_and_runs_the_whole_bar() {
        let s = |at| Scroll {
            total: 100,
            shown: 20,
            at,
        };
        assert_eq!(
            s(0).thumb(20),
            Some((0, 4)),
            "a fifth of the bar, at the top"
        );
        assert_eq!(s(80).thumb(20), Some((16, 4)), "at the bottom at the end");
        assert_eq!(s(40).thumb(20), Some((8, 4)), "half way");
        assert_eq!(s(500).thumb(20), Some((16, 4)), "past the end is the end");
        let long = Scroll {
            total: 100_000,
            shown: 20,
            at: 0,
        };
        assert_eq!(long.thumb(20), Some((0, 1)), "never less than a row");
    }

    #[test]
    fn a_thumb_put_somewhere_shows_what_puts_it_there() {
        for (total, shown, track) in [(100, 20, 20), (1000, 30, 30), (45, 40, 40), (300, 9, 9)] {
            let s = Scroll {
                total,
                shown,
                at: 0,
            };
            let (_, len) = s.thumb(track).unwrap();
            let mut last = 0;
            for start in 0..=track - len {
                let at = s.at_for(start, track);
                assert!(at >= last, "it only goes down as the thumb does");
                last = at;
                let back = Scroll { at, ..s }.thumb(track).unwrap().0;
                if total - shown >= usize::from(track - len) {
                    assert_eq!(back, start, "{total} {shown} {track} from {start}");
                }
            }
            assert_eq!(s.at_for(0, track), 0);
            assert_eq!(
                s.at_for(track, track),
                total - shown,
                "the bottom is the end"
            );
        }
    }
}
