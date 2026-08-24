//! The text buffer behind the right-hand pane.
//!
//! Lines are held as a `Vec<String>` and the cursor column is a *character*
//! index, so multi-byte text behaves. The original line ending and trailing
//! newline are remembered and restored on save, so opening and saving a file
//! without typing anything leaves it byte-identical.
//!
//! # Round-tripping is the point
//!
//! tiny's promise is that your files stay yours. An editor that silently
//! normalises CRLF to LF, or appends a trailing newline that was not there,
//! turns "I opened a file" into a diff. Three fields exist purely to prevent
//! that: `line_ending`, `trailing_newline`, and `was_empty`. If you add a new
//! way to construct or write a buffer, keep all three honest — the tests in
//! this file's `preserves_crlf_and_missing_trailing_newline` case are the
//! contract.
//!
//! # Characters, not bytes
//!
//! `cursor_col` is a *character* index into the line, never a byte offset.
//! Every mutation converts to bytes at the last moment via [`byte_of_char`].
//! Getting this wrong does not produce a wrong-looking cursor — it panics, by
//! slicing a `String` in the middle of a multi-byte character. Note that this
//! is still not the same as *display width*: a CJK character is one column
//! here but two cells on screen, which is why `ui` recomputes widths with
//! `unicode-width` when it places the terminal cursor.
//!
//! # What this module is not
//!
//! No selection, no clipboard, no search-within-buffer, no syntax awareness.
//! Highlighting is applied at draw time by `highlight`, and project-wide
//! find-replace lives in `search` and works on files rather than buffers.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    /// The bytes to write between lines. Chosen once at open time from the
    /// file's existing content, never from the host platform.
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// What kind of edit produced the current undo group. Consecutive edits of the
/// same kind coalesce into one undo step, so Ctrl+Z undoes a typed word rather
/// than one character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Structural,
}

/// A whole-buffer copy, taken before an edit that starts a new undo group.
///
/// Snapshotting every line is wasteful in principle and completely fine in
/// practice: notes and source files are small, edits are coalesced so this
/// runs once per typed word rather than once per key, and the alternative —
/// a proper piece table or per-edit delta log — is a large amount of machinery
/// for a text editor of this size. If you ever open a 50 MB file in here, that
/// is the thing to revisit.
#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

/// Undo depth. Beyond this the oldest snapshot is dropped, so memory stays
/// bounded on a long editing session.
const MAX_UNDO: usize = 200;

/// One open file. `App` keeps a map of these keyed by path, so a buffer with
/// unsaved edits survives arrowing away from it and back.
#[derive(Debug)]
pub struct Editor {
    /// Where `save` writes. Updated in place by a rename, so unsaved edits
    /// follow the file to its new name.
    pub path: PathBuf,
    lines: Vec<String>,
    /// Index into `lines`. Always valid — see [`Editor::clamp_cursor`].
    pub cursor_line: usize,
    /// Character index into the current line, not a byte offset.
    pub cursor_col: usize,
    /// Set by any mutation, cleared by `save`. Drives the `*` marker in the
    /// tree and the confirm-on-quit prompt.
    pub dirty: bool,
    /// Column the cursor tries to return to when moving vertically past
    /// short lines — the behaviour every editor has and nobody notices
    /// until it is missing.
    goal_col: usize,
    /// Viewport offsets, in lines and characters. Owned here but written by
    /// [`Editor::sync_scroll`], which `ui` calls once the pane size is known.
    pub scroll_y: usize,
    pub scroll_x: usize,
    /// The file's own line ending, sampled on open and restored on save, so a
    /// CRLF file edited on Linux stays CRLF.
    line_ending: LineEnding,
    /// Whether the file ended with a newline. Preserved either way — adding
    /// one that was not there is a diff nobody asked for.
    trailing_newline: bool,
    /// Whether the file was empty on open. An empty buffer is written back
    /// as zero bytes rather than a lone newline, so opening and saving an
    /// empty file leaves it empty.
    was_empty: bool,
    /// Pre-edit states, most recent last.
    undo: Vec<Snapshot>,
    /// States popped off `undo`, cleared as soon as a new edit happens — the
    /// usual editor rule that typing after an undo abandons the redo branch.
    redo: Vec<Snapshot>,
    /// Kind of the last edit, for coalescing. `None` means the next edit
    /// starts a fresh group whatever it is.
    last_edit: Option<EditKind>,
}

impl Editor {
    /// Build a buffer from text already in memory. This is the real
    /// constructor — `App` reads and validates the bytes itself so it can fall
    /// back to a binary preview, and hands the decoded string here.
    ///
    /// Splitting is done by hand rather than with `str::lines` because that
    /// discards the information this module exists to preserve: whether the
    /// file ended with a newline, and which ending it used.
    pub fn from_str(path: PathBuf, content: &str) -> Self {
        let line_ending = if content.contains("\r\n") {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        };
        let trailing_newline = content.ends_with('\n') || content.is_empty();
        let body = content
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(content);
        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            body.split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
                .collect()
        };
        Self {
            path,
            lines,
            cursor_line: 0,
            cursor_col: 0,
            dirty: false,
            goal_col: 0,
            scroll_y: 0,
            scroll_x: 0,
            line_ending,
            trailing_newline,
            was_empty: content.is_empty(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        }
    }

    /// Read a file straight into a buffer. `App` loads through its own
    /// classification path instead, so this is used by tests and by callers
    /// that already know the file is text.
    #[allow(dead_code)]
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(Self::from_str(path.to_path_buf(), &content))
    }

    /// Serialise the buffer back to the exact bytes that should hit the disk,
    /// restoring the original line ending and trailing newline.
    ///
    /// Also used by `ui` to re-render markdown from the buffer rather than
    /// from the file, which is why edits show up in the rendered view the
    /// moment you step out of the editor.
    pub fn to_text(&self) -> String {
        if self.was_empty && self.lines.len() == 1 && self.lines[0].is_empty() {
            return String::new();
        }
        let mut s = self.lines.join(self.line_ending.as_str());
        if self.trailing_newline {
            s.push_str(self.line_ending.as_str());
        }
        s
    }

    /// Write the buffer to `path` and clear `dirty`.
    ///
    /// A plain truncating write, not write-to-temp-then-rename: tiny is
    /// single-user and edits files in place, and an atomic replace would break
    /// hardlinks and lose the file's permissions and ownership.
    pub fn save(&mut self) -> std::io::Result<()> {
        fs::write(&self.path, self.to_text())?;
        self.dirty = false;
        Ok(())
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line_chars(&self, i: usize) -> usize {
        self.lines.get(i).map_or(0, |l| l.chars().count())
    }

    /// Clamp the cursor into the buffer. Called after every mutation so no
    /// operation can leave the cursor pointing past the end of a line.
    fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.line_chars(self.cursor_line));
    }

    // ---- undo bookkeeping -------------------------------------------------

    /// Record the pre-edit state, coalescing runs of the same edit kind.
    ///
    /// Called at the top of every mutating method, *before* the mutation, so
    /// the snapshot captures the state to return to. The coalescing rule is:
    ///
    /// - Same kind as the last edit, and not `Structural` → no new snapshot,
    ///   so a typed word reverts in one press.
    /// - Different kind, or `Structural` → new snapshot. Structural edits
    ///   (newline, line join, cut line) never merge, because each one is a
    ///   change a user thinks of as a single deliberate act.
    ///
    /// Any edit clears the redo stack, whether or not it coalesced.
    fn push_undo(&mut self, kind: EditKind) {
        let coalesce = kind != EditKind::Structural && self.last_edit == Some(kind);
        self.last_edit = Some(kind);
        self.redo.clear();
        if coalesce {
            return;
        }
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }

    /// Break the coalescing run — cursor movement ends a typing group.
    ///
    /// Called by every movement method. Without it, typing "foo", arrowing
    /// away, and typing "bar" would revert both runs in one press.
    fn break_undo_group(&mut self) {
        self.last_edit = None;
    }

    /// Step back one group. Returns false when there is nothing left, which
    /// the caller turns into a "nothing to undo" status message.
    ///
    /// Note that `dirty` is set unconditionally, even when undoing all the way
    /// back to the file's opening state. Tracking whether the buffer matches
    /// disk again would need a saved-generation counter; erring towards "there
    /// might be something to save" is the safe direction.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            lines: std::mem::replace(&mut self.lines, prev.lines),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
        self.cursor_line = prev.cursor_line;
        self.cursor_col = prev.cursor_col;
        self.dirty = true;
        self.last_edit = None;
        self.clamp_cursor();
        true
    }

    /// Step forward one group, undoing an undo. Mirror image of
    /// [`Editor::undo`], moving snapshots the other way.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snapshot {
            lines: std::mem::replace(&mut self.lines, next.lines),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
        self.cursor_line = next.cursor_line;
        self.cursor_col = next.cursor_col;
        self.dirty = true;
        self.last_edit = None;
        self.clamp_cursor();
        true
    }

    // ---- editing ----------------------------------------------------------

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

/// Byte offset of character index `ci`, or the string length if past the end.
///
/// The bridge between the character-indexed cursor and byte-indexed `String`
/// operations. Every `insert`, `replace_range` and `split_off` in this module
/// goes through it; slicing with a raw `cursor_col` would panic on any file
/// containing a non-ASCII character.
fn byte_of_char(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::from_str(PathBuf::from("/tmp/t.txt"), text)
    }

    #[test]
    fn roundtrips_lf_content_unchanged() {
        let src = "one\ntwo\nthree\n";
        assert_eq!(ed(src).to_text(), src);
    }

    #[test]
    fn preserves_crlf_and_missing_trailing_newline() {
        assert_eq!(ed("a\r\nb\r\n").to_text(), "a\r\nb\r\n");
        assert_eq!(ed("a\nb").to_text(), "a\nb", "no newline is not invented");
        assert_eq!(ed("").to_text(), "", "an empty file stays empty");
        assert_eq!(
            ed("\n").to_text(),
            "\n",
            "a lone blank line is not an empty file"
        );
    }

    #[test]
    fn empty_file_has_one_empty_line_to_type_into() {
        let e = ed("");
        assert_eq!(e.line_count(), 1);
        assert_eq!(e.lines()[0], "");
    }

    #[test]
    fn insert_and_backspace_are_dirty_and_reversible() {
        let mut e = ed("ac\n");
        assert!(!e.dirty);
        e.move_right();
        e.insert_char('b');
        assert_eq!(e.lines()[0], "abc");
        assert!(e.dirty);
        e.backspace();
        assert_eq!(e.lines()[0], "ac");
    }

    #[test]
    fn cursor_column_counts_characters_not_bytes() {
        let mut e = ed("héllo\n");
        e.move_end();
        assert_eq!(e.cursor_col, 5, "5 chars even though 'é' is 2 bytes");
        e.insert_char('!');
        assert_eq!(e.lines()[0], "héllo!");
        // Deleting the multi-byte char must not split it.
        e.move_doc_start();
        e.move_right();
        e.delete_forward();
        assert_eq!(e.lines()[0], "hllo!");
    }

    #[test]
    fn newline_splits_line_and_carries_indentation() {
        let mut e = ed("    foo(bar)\n");
        e.cursor_col = 8; // after "    foo("
        e.insert_newline();
        assert_eq!(e.lines(), &["    foo(", "    bar)"]);
        assert_eq!((e.cursor_line, e.cursor_col), (1, 4), "cursor after indent");
    }

    #[test]
    fn backspace_at_column_zero_joins_with_previous_line() {
        let mut e = ed("ab\ncd\n");
        e.move_down();
        assert_eq!(e.cursor_col, 0);
        e.backspace();
        assert_eq!(e.lines(), &["abcd"]);
        assert_eq!((e.cursor_line, e.cursor_col), (0, 2));
    }

    #[test]
    fn delete_forward_at_end_of_line_pulls_up_the_next() {
        let mut e = ed("ab\ncd\n");
        e.move_end();
        e.delete_forward();
        assert_eq!(e.lines(), &["abcd"]);
    }

    #[test]
    fn backspace_at_start_of_file_does_nothing() {
        let mut e = ed("ab\n");
        e.backspace();
        assert_eq!(e.lines(), &["ab"]);
        assert!(!e.dirty, "a no-op must not mark the buffer dirty");
    }

    #[test]
    fn vertical_movement_remembers_the_goal_column() {
        let mut e = ed("longest line\nx\nanother long one\n");
        e.move_end();
        let goal = e.cursor_col;
        e.move_down();
        assert_eq!(e.cursor_col, 1, "clamped to the short line");
        e.move_down();
        assert_eq!(e.cursor_col, goal.min(16), "restored on the longer line");
    }

    #[test]
    fn home_toggles_between_indent_and_margin() {
        let mut e = ed("    indented\n");
        e.move_end();
        e.move_home();
        assert_eq!(e.cursor_col, 4, "first non-blank");
        e.move_home();
        assert_eq!(e.cursor_col, 0, "then the true start");
    }

    #[test]
    fn word_movement_steps_over_identifiers() {
        let mut e = ed("foo_bar baz(qux)\n");
        e.move_word_right();
        assert_eq!(e.cursor_col, 8, "past 'foo_bar' and the space");
        e.move_word_left();
        assert_eq!(e.cursor_col, 0);
    }

    #[test]
    fn typing_run_undoes_as_one_group() {
        let mut e = ed("\n");
        for c in "hello".chars() {
            e.insert_char(c);
        }
        assert_eq!(e.lines()[0], "hello");
        e.undo();
        assert_eq!(e.lines()[0], "", "the whole typed run reverts at once");
        e.redo();
        assert_eq!(e.lines()[0], "hello");
    }

    #[test]
    fn cursor_movement_breaks_the_undo_group() {
        let mut e = ed("\n");
        e.insert_char('a');
        e.move_left();
        e.move_right();
        e.insert_char('b');
        e.undo();
        assert_eq!(e.lines()[0], "a", "only the second run reverts");
    }

    #[test]
    fn undo_on_a_fresh_buffer_is_a_noop() {
        let mut e = ed("x\n");
        assert!(!e.undo());
        assert!(!e.redo());
    }

    #[test]
    fn delete_line_leaves_at_least_one_line() {
        let mut e = ed("only\n");
        e.delete_line();
        assert_eq!(e.lines(), &[""]);
        assert_eq!(e.line_count(), 1);
    }

    #[test]
    fn scroll_follows_the_cursor_both_ways() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut e = ed(&text);
        e.move_doc_end();
        e.sync_scroll(20, 10);
        assert!(e.scroll_y > 0, "scrolled down to reach the end");
        assert!(e.cursor_line >= e.scroll_y && e.cursor_line < e.scroll_y + 10);
        e.move_doc_start();
        e.sync_scroll(20, 10);
        assert_eq!(e.scroll_y, 0, "scrolled back up");
    }

    #[test]
    fn goto_lands_exactly_where_asked_even_inside_indentation() {
        let mut e = ed("first\n    return utils.load()\n");
        e.goto(1, 11);
        assert_eq!((e.cursor_line, e.cursor_col), (1, 11));
        // Home would have stopped at the indent; goto does not.
        assert_eq!(
            e.lines()[1].chars().skip(11).take(5).collect::<String>(),
            "utils"
        );
    }

    #[test]
    fn goto_clamps_instead_of_panicking() {
        let mut e = ed("a\nbb\n");
        e.goto(99, 99);
        assert_eq!((e.cursor_line, e.cursor_col), (1, 2));
        e.goto(0, 99);
        assert_eq!((e.cursor_line, e.cursor_col), (0, 1));
    }

    #[test]
    fn save_writes_the_file_and_clears_dirty() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("note.md");
        fs::write(&p, "before\n").unwrap();
        let mut e = Editor::open(&p).unwrap();
        e.move_end();
        e.insert_char('!');
        assert!(e.dirty);
        e.save().unwrap();
        assert!(!e.dirty);
        assert_eq!(fs::read_to_string(&p).unwrap(), "before!\n");
    }
}
