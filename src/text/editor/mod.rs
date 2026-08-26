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
//! find-replace lives in `search` and works on files rather than buffers.//!
//! # Where things are
//!
//! - here — the buffer, the cursor, and reading and writing the file.
//! - [`edit`] — everything that changes the text.
//! - [`motion`] — everything that moves the cursor.
//! - [`undo`] — the history, and the grouping that makes it usable.

mod edit;
mod motion;
mod undo;

use std::fs;
use std::path::PathBuf;

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
pub(super) enum EditKind {
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
pub(super) struct Snapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

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
    /// Lowest line index changed since [`Editor::take_touched`] last ran, or
    /// `usize::MAX` when nothing has. See that method for who asks.
    touched: usize,
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
            touched: usize::MAX,
        }
    }

    /// Read a file straight into a buffer.
    ///
    /// `App` loads through its own classification path — it has to decide
    /// whether a file is text at all first — so nothing but the tests below
    /// comes in this way.
    #[cfg(test)]
    pub fn open(path: &std::path::Path) -> std::io::Result<Self> {
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

    /// The lowest line changed since the last call, clearing the record.
    ///
    /// The syntax highlighter's resume cache is the only caller. It keeps
    /// parser state every few dozen lines so that a window deep in a file does
    /// not have to be reached by parsing from line 0, and this is how it learns
    /// how much of that state an edit threw away: everything from here down.
    ///
    /// Clearing on read is what makes it cheap — the editor does not have to
    /// know whether anyone is listening, and the answer is only ever asked for
    /// once per frame.
    pub fn take_touched(&mut self) -> Option<usize> {
        (self.touched != usize::MAX).then(|| std::mem::replace(&mut self.touched, usize::MAX))
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line_chars(&self, i: usize) -> usize {
        self.lines.get(i).map_or(0, |l| l.chars().count())
    }

    /// Clamp the cursor into the buffer. Called after every mutation so no
    /// operation can leave the cursor pointing past the end of a line.
    pub(super) fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.line_chars(self.cursor_line));
    }

    // ---- undo bookkeeping -------------------------------------------------
}

/// Byte offset of character index `ci`, or the string length if past the end.
///
/// The bridge between the character-indexed cursor and byte-indexed `String`
/// operations. Every `insert`, `replace_range` and `split_off` in this module
/// goes through it; slicing with a raw `cursor_col` would panic on any file
/// containing a non-ASCII character.
pub(super) fn byte_of_char(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::from_str(PathBuf::from("/tmp/t.txt"), text)
    }

    #[test]
    fn nothing_is_touched_until_something_is_edited() {
        let mut e = ed("a\nb\nc\nd\n");
        assert_eq!(e.take_touched(), None);
        e.move_down();
        e.move_end();
        assert_eq!(e.take_touched(), None, "moving about changes no line");
    }

    #[test]
    fn an_edit_reports_its_line_once() {
        let mut e = ed("a\nb\nc\nd\n");
        e.goto(2, 0);
        e.insert_char('x');
        assert_eq!(
            e.take_touched(),
            Some(1),
            "line 2, with the one line of slack"
        );
        assert_eq!(e.take_touched(), None, "reading clears the record");
    }

    #[test]
    fn a_backspace_at_the_start_of_a_line_reports_the_line_above() {
        let mut e = ed("a\nb\nc\nd\n");
        e.goto(2, 0);
        e.backspace();
        assert_eq!(e.to_text(), "a\nbc\nd\n", "the line joined upwards");
        assert_eq!(
            e.take_touched(),
            Some(1),
            "line 1 is what actually changed, and it is covered"
        );
    }

    #[test]
    fn the_lowest_of_several_edits_is_the_one_reported() {
        let mut e = ed("a\nb\nc\nd\ne\n");
        e.goto(4, 0);
        e.insert_char('x');
        e.goto(1, 0);
        e.insert_char('y');
        e.goto(3, 0);
        e.insert_char('z');
        assert_eq!(e.take_touched(), Some(0), "the earliest line wins");
    }

    #[test]
    fn an_undo_puts_every_line_back_in_doubt() {
        let mut e = ed("a\nb\nc\nd\n");
        e.goto(3, 0);
        e.insert_char('x');
        e.take_touched();
        assert!(e.undo());
        assert_eq!(
            e.take_touched(),
            Some(0),
            "a snapshot replaces the whole buffer"
        );
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
