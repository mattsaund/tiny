//! Moving the cursor, and following it with the view.
//!
//! Nothing here changes the text, which is why none of it touches the undo
//! history, though moving *does* close the current undo group, so a run of
//! typing that you walked away from and came back to undoes in two steps
//! rather than one.
//!
//! Each motion is a step written for one cursor, made at every cursor by
//! [`Editor::move_cursors`], and comes in two: `move_` lets go of the
//! selection, `select_` keeps it going (Shift and the same key).
//!
//! # The goal column
//!
//! Moving down through a short line and out the other side puts the cursor
//! back where it was horizontally, not where the short line ended. That
//! remembered column is `goal`, and it survives vertical movement and is
//! cleared by anything horizontal, which is the behavior every editor has
//! and nobody notices until it is missing.

use super::Editor;
use crate::text::search::is_word_char;
use crate::text::tabs;

impl Editor {
    /// Left, or to the start of the selection when there is one.
    pub fn move_left(&mut self) {
        self.move_cursors(false, |ed| match ed.selection() {
            Some((start, _)) => {
                ed.set_pos(start);
                ed.goal_col = start.col;
            }
            None => ed.step_left(),
        });
    }

    /// Right, or to the end of the selection when there is one.
    pub fn move_right(&mut self) {
        self.move_cursors(false, |ed| match ed.selection() {
            Some((_, end)) => {
                ed.set_pos(end);
                ed.goal_col = end.col;
            }
            None => ed.step_right(),
        });
    }

    pub fn move_up(&mut self) {
        self.move_cursors(false, Self::step_up);
    }

    pub fn move_down(&mut self) {
        self.move_cursors(false, Self::step_down);
    }

    /// Home: first non-blank, then column 0; press twice to reach the margin.
    ///
    /// "Smart home", as in most editors. Because it is smart, it is the wrong
    /// tool for jumping to an exact column; [`Editor::goto`] exists for that.
    pub fn move_home(&mut self) {
        self.move_cursors(false, Self::step_home);
    }

    pub fn move_end(&mut self) {
        self.move_cursors(false, Self::step_end);
    }

    /// Move to the start of the previous word.
    ///
    /// A word is a run of letters, digits and underscores (and the accents
    /// on them, see [`is_word_char`]), so `foo_bar` is one word and `foo.bar`
    /// is two. Skips any separators first, then the word itself. At column 0
    /// it falls through to a plain left, which steps onto the end of the
    /// previous line.
    pub fn move_word_left(&mut self) {
        self.move_cursors(false, Self::step_word_left);
    }

    /// Move past the end of the current word and any separators after it, so
    /// repeated presses land on successive word starts.
    pub fn move_word_right(&mut self) {
        self.move_cursors(false, Self::step_word_right);
    }

    pub fn page_up(&mut self, page: usize) {
        self.move_cursors(false, |ed| ed.step_page(page, false));
    }

    pub fn page_down(&mut self, page: usize) {
        self.move_cursors(false, |ed| ed.step_page(page, true));
    }

    pub fn move_doc_start(&mut self) {
        self.move_cursors(false, Self::step_doc_start);
    }

    pub fn move_doc_end(&mut self) {
        self.move_cursors(false, Self::step_doc_end);
    }

    // ---- the same, selecting ----------------------------------------------

    pub fn select_left(&mut self) {
        self.move_cursors(true, Self::step_left);
    }

    pub fn select_right(&mut self) {
        self.move_cursors(true, Self::step_right);
    }

    pub fn select_up(&mut self) {
        self.move_cursors(true, Self::step_up);
    }

    pub fn select_down(&mut self) {
        self.move_cursors(true, Self::step_down);
    }

    pub fn select_home(&mut self) {
        self.move_cursors(true, Self::step_home);
    }

    pub fn select_end(&mut self) {
        self.move_cursors(true, Self::step_end);
    }

    pub fn select_word_left(&mut self) {
        self.move_cursors(true, Self::step_word_left);
    }

    pub fn select_word_right(&mut self) {
        self.move_cursors(true, Self::step_word_right);
    }

    pub fn select_page(&mut self, page: usize, down: bool) {
        self.move_cursors(true, |ed| ed.step_page(page, down));
    }

    pub fn select_doc_start(&mut self) {
        self.move_cursors(true, Self::step_doc_start);
    }

    pub fn select_doc_end(&mut self) {
        self.move_cursors(true, Self::step_doc_end);
    }

    /// Put the cursor at an exact position, clamped into the buffer, with
    /// nothing selected and no other cursors. Jumping to a search hit needs
    /// this: simulating Home and arrow keys goes wrong, because Home is
    /// smart-home and stops at the indent.
    pub fn goto(&mut self, line: usize, col: usize) {
        self.break_undo_group();
        self.collapse();
        self.cursor_line = line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = col.min(self.line_chars(self.cursor_line));
        self.goal_col = self.cursor_col;
    }

    // ---- one cursor's steps -----------------------------------------------

    fn step_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.line_chars(self.cursor_line);
        }
        self.goal_col = self.cursor_col;
    }

    fn step_right(&mut self) {
        if self.cursor_col < self.line_chars(self.cursor_line) {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.goal_col = self.cursor_col;
    }

    fn step_up(&mut self) {
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
        }
    }

    fn step_down(&mut self) {
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
        }
    }

    fn step_home(&mut self) {
        let indent = self.lines[self.cursor_line]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        self.cursor_col = if self.cursor_col == indent { 0 } else { indent };
        self.goal_col = self.cursor_col;
    }

    fn step_end(&mut self) {
        self.cursor_col = self.line_chars(self.cursor_line);
        self.goal_col = self.cursor_col;
    }

    fn step_word_left(&mut self) {
        if self.cursor_col == 0 {
            self.step_left();
            return;
        }
        let chars: Vec<char> = self.lines[self.cursor_line].chars().collect();
        let mut i = self.cursor_col;
        while i > 0 && !is_word_char(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word_char(chars[i - 1]) {
            i -= 1;
        }
        self.cursor_col = i;
        self.goal_col = i;
    }

    fn step_word_right(&mut self) {
        let chars: Vec<char> = self.lines[self.cursor_line].chars().collect();
        if self.cursor_col >= chars.len() {
            self.step_right();
            return;
        }
        let mut i = self.cursor_col;
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        while i < chars.len() && !is_word_char(chars[i]) {
            i += 1;
        }
        self.cursor_col = i;
        self.goal_col = i;
    }

    fn step_page(&mut self, page: usize, down: bool) {
        self.cursor_line = if down {
            (self.cursor_line + page.max(1)).min(self.lines.len() - 1)
        } else {
            self.cursor_line.saturating_sub(page.max(1))
        };
        self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
    }

    fn step_doc_start(&mut self) {
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.goal_col = 0;
    }

    fn step_doc_end(&mut self) {
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.line_chars(self.cursor_line);
        self.goal_col = self.cursor_col;
    }

    /// Scroll the viewport so the cursor is on screen. Called before drawing,
    /// once the real pane size is known.
    ///
    /// This is the one place `ui` legitimately mutates editor state: the key
    /// handler that moved the cursor has no idea how tall the pane is, and the
    /// answer changes when the terminal is resized. Scrolls by the minimum
    /// needed to bring the cursor into view, which keeps the text still when
    /// moving inside the visible region.
    ///
    /// Across, the cursor is measured in columns as the line is drawn, tabs
    /// at their stops and wide characters two cells, because `scroll_x` is in
    /// columns too: a count of characters would let a line of tabs or of CJK
    /// run the cursor off the edge without ever scrolling.
    pub fn sync_scroll(&mut self, view_w: usize, view_h: usize, tab_width: usize) {
        if view_h > 0 {
            if self.cursor_line < self.scroll_y {
                self.scroll_y = self.cursor_line;
            } else if self.cursor_line >= self.scroll_y + view_h {
                self.scroll_y = self.cursor_line + 1 - view_h;
            }
        }
        if view_w > 0 {
            let line = self
                .lines
                .get(self.cursor_line)
                .map(String::as_str)
                .unwrap_or("");
            let col = tabs::column(line, self.cursor_col, tab_width);
            if col < self.scroll_x {
                self.scroll_x = col;
            } else if col >= self.scroll_x + view_w {
                self.scroll_x = col + 1 - view_w;
            }
        }
    }
}
