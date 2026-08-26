//! Moving the cursor, and following it with the view.
//!
//! Nothing here changes the text, which is why none of it touches the undo
//! history — though moving *does* close the current undo group, so a run of
//! typing that you walked away from and came back to undoes in two steps
//! rather than one.
//!
//! # The goal column
//!
//! Moving down through a short line and out the other side puts the cursor
//! back where it was horizontally, not where the short line ended. That
//! remembered column is `goal`, and it survives vertical movement and is
//! cleared by anything horizontal — which is the behaviour every editor has
//! and nobody notices until it is missing.

use super::Editor;

impl Editor {
    pub fn move_left(&mut self) {
        self.break_undo_group();
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.line_chars(self.cursor_line);
        }
        self.goal_col = self.cursor_col;
    }

    pub fn move_right(&mut self) {
        self.break_undo_group();
        if self.cursor_col < self.line_chars(self.cursor_line) {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
        self.goal_col = self.cursor_col;
    }

    pub fn move_up(&mut self) {
        self.break_undo_group();
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
        }
    }

    pub fn move_down(&mut self) {
        self.break_undo_group();
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
        }
    }

    /// Home: first non-blank, then column 0 — press twice to reach the margin.
    ///
    /// "Smart home", as in most editors. Because it is smart, it is the wrong
    /// tool for jumping to an exact column; [`Editor::goto`] exists for that.
    pub fn move_home(&mut self) {
        self.break_undo_group();
        let indent = self.lines[self.cursor_line]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        self.cursor_col = if self.cursor_col == indent { 0 } else { indent };
        self.goal_col = self.cursor_col;
    }

    pub fn move_end(&mut self) {
        self.break_undo_group();
        self.cursor_col = self.line_chars(self.cursor_line);
        self.goal_col = self.cursor_col;
    }

    /// Move to the start of the previous word.
    ///
    /// A word is a run of alphanumerics and underscores, so `foo_bar` is one
    /// word and `foo.bar` is two. Skips any separators first, then the word
    /// itself. At column 0 it falls through to a plain left, which steps onto
    /// the end of the previous line.
    pub fn move_word_left(&mut self) {
        self.break_undo_group();
        if self.cursor_col == 0 {
            self.move_left();
            return;
        }
        let chars: Vec<char> = self.lines[self.cursor_line].chars().collect();
        let mut i = self.cursor_col;
        while i > 0 && !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_' {
            i -= 1;
        }
        while i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
            i -= 1;
        }
        self.cursor_col = i;
        self.goal_col = i;
    }

    /// Move past the end of the current word and any separators after it, so
    /// repeated presses land on successive word starts.
    pub fn move_word_right(&mut self) {
        self.break_undo_group();
        let chars: Vec<char> = self.lines[self.cursor_line].chars().collect();
        if self.cursor_col >= chars.len() {
            self.move_right();
            return;
        }
        let mut i = self.cursor_col;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_alphanumeric() && chars[i] != '_' {
            i += 1;
        }
        self.cursor_col = i;
        self.goal_col = i;
    }

    pub fn page_up(&mut self, page: usize) {
        self.break_undo_group();
        self.cursor_line = self.cursor_line.saturating_sub(page.max(1));
        self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
    }

    pub fn page_down(&mut self, page: usize) {
        self.break_undo_group();
        self.cursor_line = (self.cursor_line + page.max(1)).min(self.lines.len() - 1);
        self.cursor_col = self.goal_col.min(self.line_chars(self.cursor_line));
    }

    /// Put the cursor at an exact position, clamped into the buffer. Jumping
    /// to a search hit needs this: simulating Home and arrow keys goes wrong,
    /// because Home is smart-home and stops at the indent.
    pub fn goto(&mut self, line: usize, col: usize) {
        self.break_undo_group();
        self.cursor_line = line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = col.min(self.line_chars(self.cursor_line));
        self.goal_col = self.cursor_col;
    }

    pub fn move_doc_start(&mut self) {
        self.break_undo_group();
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.goal_col = 0;
    }

    pub fn move_doc_end(&mut self) {
        self.break_undo_group();
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
    pub fn sync_scroll(&mut self, view_w: usize, view_h: usize) {
        if view_h > 0 {
            if self.cursor_line < self.scroll_y {
                self.scroll_y = self.cursor_line;
            } else if self.cursor_line >= self.scroll_y + view_h {
                self.scroll_y = self.cursor_line + 1 - view_h;
            }
        }
        if view_w > 0 {
            if self.cursor_col < self.scroll_x {
                self.scroll_x = self.cursor_col;
            } else if self.cursor_col >= self.scroll_x + view_w {
                self.scroll_x = self.cursor_col + 1 - view_w;
            }
        }
    }
}
