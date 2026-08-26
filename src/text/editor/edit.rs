//! Changing the text.
//!
//! Every one of these calls [`Editor::push_undo`] first, which is what makes
//! the history complete without any of them having to remember to record
//! anything. They all act at the cursor, and they all leave it somewhere
//! sensible afterwards — an edit that moves the cursor to a surprising place
//! is worse than one that does nothing.
//!
//! Columns are character indices, never byte offsets. A buffer holds whatever
//! the user typed, and slicing a string at a raw cursor position panics the
//! first time someone writes an accented letter.

use super::{EditKind, Editor, byte_of_char};

impl Editor {
    pub fn insert_char(&mut self, c: char) {
        self.push_undo(EditKind::Insert);
        let byte = byte_of_char(&self.lines[self.cursor_line], self.cursor_col);
        self.lines[self.cursor_line].insert(byte, c);
        self.cursor_col += 1;
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    pub fn insert_tab(&mut self, width: usize) {
        self.push_undo(EditKind::Insert);
        let byte = byte_of_char(&self.lines[self.cursor_line], self.cursor_col);
        let spaces = " ".repeat(width.max(1));
        self.lines[self.cursor_line].insert_str(byte, &spaces);
        self.cursor_col += width.max(1);
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    /// Split the line at the cursor, carrying the current indentation onto the
    /// new line — the auto-indent every editor has. Structural, so it always
    /// begins a fresh undo group.
    pub fn insert_newline(&mut self) {
        self.push_undo(EditKind::Structural);
        let byte = byte_of_char(&self.lines[self.cursor_line], self.cursor_col);
        let tail = self.lines[self.cursor_line].split_off(byte);
        // Carry the current line's indentation onto the new line.
        let indent: String = self.lines[self.cursor_line]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let indent_len = indent.chars().count();
        self.lines.insert(self.cursor_line + 1, indent + &tail);
        self.cursor_line += 1;
        self.cursor_col = indent_len;
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    /// Delete backwards. Two quite different operations share the key: within
    /// a line it removes a character (a `Delete` group, which coalesces), and
    /// at column 0 it joins with the previous line (a `Structural` group,
    /// which does not).
    ///
    /// At the very start of the buffer it returns early *before* touching
    /// `dirty`, so a stray keypress on a clean file does not make it look
    /// edited.
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.push_undo(EditKind::Delete);
            let line = &mut self.lines[self.cursor_line];
            let start = byte_of_char(line, self.cursor_col - 1);
            let end = byte_of_char(line, self.cursor_col);
            line.replace_range(start..end, "");
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            // Join with the previous line.
            self.push_undo(EditKind::Structural);
            let line = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.line_chars(self.cursor_line);
            self.lines[self.cursor_line].push_str(&line);
        } else {
            return;
        }
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    /// Delete under the cursor. The mirror of [`Editor::backspace`]: within a
    /// line it removes a character, at the end of one it pulls the next line
    /// up, and at the end of the buffer it does nothing.
    pub fn delete_forward(&mut self) {
        let len = self.line_chars(self.cursor_line);
        if self.cursor_col < len {
            self.push_undo(EditKind::Delete);
            let line = &mut self.lines[self.cursor_line];
            let start = byte_of_char(line, self.cursor_col);
            let end = byte_of_char(line, self.cursor_col + 1);
            line.replace_range(start..end, "");
        } else if self.cursor_line + 1 < self.lines.len() {
            self.push_undo(EditKind::Structural);
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        } else {
            return;
        }
        self.dirty = true;
    }

    /// Delete the whole current line, the way micro's Ctrl+K does.
    pub fn delete_line(&mut self) {
        self.push_undo(EditKind::Structural);
        if self.lines.len() == 1 {
            self.lines[0].clear();
        } else {
            self.lines.remove(self.cursor_line);
        }
        self.cursor_col = 0;
        self.goal_col = 0;
        self.dirty = true;
        self.clamp_cursor();
    }

    // ---- movement ---------------------------------------------------------
}
