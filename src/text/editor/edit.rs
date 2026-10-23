//! Changing the text.
//!
//! Every one of these calls [`Editor::push_undo`] first, which is what makes
//! the history complete without any of them having to remember to record
//! anything. They all act at the cursor, and they all leave it somewhere
//! sensible afterwards: an edit that moves the cursor to a surprising place
//! is worse than one that does nothing.
//!
//! Columns are character indices, never byte offsets. A buffer holds whatever
//! the user typed, and slicing a string at a raw cursor position panics the
//! first time someone writes an accented letter.

use super::{EditKind, Editor, byte_of_char};
use crate::text::tabs;

impl Editor {
    pub fn insert_char(&mut self, c: char) {
        self.push_undo(EditKind::Insert);
        let byte = byte_of_char(&self.lines[self.cursor_line], self.cursor_col);
        self.lines[self.cursor_line].insert(byte, c);
        self.cursor_col += 1;
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    /// Tab types spaces up to the next tab stop, the way a tab character
    /// would be drawn there, so text indented from part way along a line
    /// still lines up with the stops.
    pub fn insert_tab(&mut self, width: usize) {
        self.push_undo(EditKind::Insert);
        let line = &self.lines[self.cursor_line];
        let n = tabs::stop(tabs::column(line, self.cursor_col, width), width);
        let byte = byte_of_char(line, self.cursor_col);
        self.lines[self.cursor_line].insert_str(byte, &" ".repeat(n));
        self.cursor_col += n;
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    /// Split the line at the cursor, carrying the current indentation onto the
    /// new line: the auto-indent every editor has. Structural, so it always
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

    /// Delete the whole current line, the way micro's Ctrl+K does. In a file
    /// that is one empty line there is nothing to delete, and nothing is
    /// marked unsaved.
    pub fn delete_line(&mut self) {
        if self.lines.len() == 1 && self.lines[0].is_empty() {
            return;
        }
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

    /// Put `text` in at the cursor, newlines and all, as one step of undo:
    /// what a plugin's `tiny.insert` does. The cursor ends up after it.
    ///
    /// Unlike typing it, no indentation is carried onto the new lines: the
    /// text arrives exactly as it was given, so a plugin inserting an indented
    /// block does not get it indented twice.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.push_undo(EditKind::Structural);
        let byte = byte_of_char(&self.lines[self.cursor_line], self.cursor_col);
        let tail = self.lines[self.cursor_line].split_off(byte);
        let mut pieces = text.split('\n').map(|p| p.strip_suffix('\r').unwrap_or(p));
        let mut line = self.cursor_line;
        self.lines[line].push_str(pieces.next().unwrap_or(""));
        for piece in pieces {
            line += 1;
            self.lines.insert(line, piece.to_string());
        }
        self.cursor_line = line;
        self.cursor_col = self.lines[line].chars().count();
        self.lines[line].push_str(&tail);
        self.goal_col = self.cursor_col;
        self.dirty = true;
    }

    /// Replace everything with `text`, as one step of undo: what a plugin's
    /// `tiny.set_text` does. The cursor keeps its line and column, as near as
    /// the new text allows.
    ///
    /// One trailing newline is read as the end of the last line rather than as
    /// an empty line after it: the buffer already knows whether the file ends
    /// with one, and puts it back on save.
    pub fn replace_all(&mut self, text: &str) {
        self.push_undo(EditKind::Structural);
        let body = text
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(text);
        self.lines = body
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        // Every line may have changed, not only the ones near the cursor.
        self.touched = 0;
        self.dirty = true;
        self.clamp_cursor();
        self.goal_col = self.cursor_col;
    }
}
