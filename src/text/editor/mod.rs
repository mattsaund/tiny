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
//! normalizes CRLF to LF, or appends a trailing newline that was not there,
//! turns "I opened a file" into a diff. Three fields exist purely to prevent
//! that: `line_ending`, `trailing_newline`, and `was_empty`. If you add a new
//! way to construct or write a buffer, keep all three honest: the tests in
//! this file's `preserves_crlf_and_missing_trailing_newline` case are the
//! contract.
//!
//! # Characters, not bytes
//!
//! `cursor_col` is a *character* index into the line, never a byte offset.
//! Every mutation converts to bytes at the last moment via [`byte_of_char`].
//! Getting this wrong does not produce a wrong-looking cursor; it panics, by
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
//!
//! # Where things are
//!
//! - here: the buffer, the cursor, and reading and writing the file.
//! - [`edit`]: everything that changes the text.
//! - [`motion`]: everything that moves the cursor.
//! - [`undo`]: the history, and the grouping that makes it usable.

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
/// same kind coalesce into one undo step, so Ctrl+Z undoes a run of typing
/// rather than one character.
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
/// runs once per run of typing rather than once per key, and the alternative
/// (a proper piece table or per-edit delta log) is a large amount of machinery
/// for a text editor of this size. A file of megabytes keeps fewer steps
/// instead, see `UNDO_BYTES`.
#[derive(Debug, Clone)]
pub(super) struct Snapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

/// Where one view of a file is: the cursor, the column it is trying to get
/// back to, and the scroll.
///
/// A buffer keeps one of these as its own fields, which is all a file needs
/// until it is open in two panes at once. Then each pane keeps a view, and
/// the pane with the keyboard lends its view to the buffer while it types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct View {
    pub cursor_line: usize,
    pub cursor_col: usize,
    goal_col: usize,
    pub scroll_y: usize,
    pub scroll_rows: usize,
    pub scroll_x: usize,
}

/// One open file. `App` keeps a map of these keyed by path, so a buffer with
/// unsaved edits survives arrowing away from it and back.
#[derive(Debug)]
pub struct Editor {
    /// Where `save` writes. Updated in place by a rename, so unsaved edits
    /// follow the file to its new name.
    pub path: PathBuf,
    lines: Vec<String>,
    /// Index into `lines`. Always valid, see [`Editor::clamp_cursor`].
    pub cursor_line: usize,
    /// Character index into the current line, not a byte offset.
    pub cursor_col: usize,
    /// Set by any mutation, cleared by `save`. Drives the `*` marker in the
    /// tree and the confirm-on-quit prompt.
    pub dirty: bool,
    /// Column the cursor tries to return to when moving vertically past
    /// short lines: the behavior every editor has and nobody notices
    /// until it is missing.
    goal_col: usize,
    /// Viewport offsets: down in lines, across in columns as the text is
    /// drawn. Owned here but written by [`Editor::sync_scroll`], which `ui`
    /// calls once the pane size is known.
    pub scroll_y: usize,
    /// In a note drawn formatted, how many drawn rows of the block that
    /// starts at `scroll_y` are above the pane: one block can be many rows,
    /// and a pane can begin part way down it. Written by `ui`.
    pub scroll_rows: usize,
    pub scroll_x: usize,
    /// The file's own line ending, sampled on open and restored on save, so a
    /// CRLF file edited on Linux stays CRLF.
    line_ending: LineEnding,
    /// Whether the file ended with a newline. Preserved either way: adding
    /// one that was not there is a diff nobody asked for.
    trailing_newline: bool,
    /// Whether the file was empty on open. An empty buffer is written back
    /// as zero bytes rather than a lone newline, so opening and saving an
    /// empty file leaves it empty.
    was_empty: bool,
    /// Pre-edit states, most recent last.
    undo: Vec<Snapshot>,
    /// States popped off `undo`, cleared as soon as a new edit happens: the
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
    /// constructor: `App` reads and validates the bytes itself so it can fall
    /// back to a binary preview, and hands the decoded string here.
    ///
    /// Splitting is done by hand rather than with `str::lines` because that
    /// discards the information this module exists to preserve: whether the
    /// file ended with a newline, and which ending it used.
    ///
    /// A file is CRLF only when every line break in it is: one stray `\r\n`
    /// among LF lines leaves the file LF, with that `\r` kept in its line,
    /// so a save writes back the endings it read rather than turning every
    /// other line's into it.
    pub fn from_str(path: PathBuf, content: &str) -> Self {
        let breaks = content.matches('\n').count();
        let crlf = breaks > 0 && content.matches("\r\n").count() == breaks;
        let line_ending = if crlf {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        };
        let trailing_newline = content.ends_with('\n') || content.is_empty();
        let body = match content.strip_suffix('\n') {
            Some(body) if crlf => body.strip_suffix('\r').unwrap_or(body),
            Some(body) => body,
            None => content,
        };
        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            let mut parts: Vec<&str> = body.split('\n').collect();
            // Every part but the last came before a break, which in a CRLF
            // file is where its `\r` belongs. The last came before nothing,
            // and a `\r` there is the file's own.
            let last = parts.len() - 1;
            if crlf {
                for part in &mut parts[..last] {
                    *part = part.strip_suffix('\r').unwrap_or(part);
                }
            }
            parts.into_iter().map(str::to_string).collect()
        };
        Self {
            path,
            lines,
            cursor_line: 0,
            cursor_col: 0,
            dirty: false,
            goal_col: 0,
            scroll_y: 0,
            scroll_rows: 0,
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
    /// `App` loads through its own classification path (it has to decide
    /// whether a file is text at all first) so nothing but the tests below
    /// comes in this way.
    #[cfg(test)]
    pub fn open(path: &std::path::Path) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(Self::from_str(path.to_path_buf(), &content))
    }

    /// Serialize the buffer back to the exact bytes that should hit the disk,
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

    /// Take on the file's current contents, keeping the reader where they
    /// were on screen.
    ///
    /// For a file another program has rewritten while it is open here. Cursor
    /// and viewport survive (clamped, since the file may have got shorter)
    /// because the whole point of noticing an external change is that you are
    /// looking at the file while it happens, and a jump back to line 1 would
    /// lose your place every time a formatter ran.
    ///
    /// Everything derived from the bytes is resampled, including the line
    /// ending and the trailing newline: they are a different file's bytes now.
    /// The undo stack goes too. Its snapshots describe text that no longer
    /// exists, and an undo that pasted them back would silently revert
    /// somebody else's write.
    pub fn reload_from(&mut self, content: &str) {
        let (line, col) = (self.cursor_line, self.cursor_col);
        let (y, rows, x) = (self.scroll_y, self.scroll_rows, self.scroll_x);
        let path = std::mem::take(&mut self.path);
        *self = Self::from_str(path, content);
        self.cursor_line = line;
        self.cursor_col = col;
        self.scroll_y = y.min(self.lines.len().saturating_sub(1));
        self.scroll_rows = rows;
        self.scroll_x = x;
        self.clamp_cursor();
        // Every line may differ from what was there, so anything saved about
        // the old text (each pane's highlighting) starts again from the top.
        self.touched = 0;
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
    /// Clearing on read is what makes it cheap: the editor does not have to
    /// know whether anyone is listening, and the answer is only ever asked for
    /// once per frame.
    pub fn take_touched(&mut self) -> Option<usize> {
        (self.touched != usize::MAX).then(|| std::mem::replace(&mut self.touched, usize::MAX))
    }

    /// The lowest line touched, without clearing it.
    pub fn touched(&self) -> Option<usize> {
        (self.touched != usize::MAX).then_some(self.touched)
    }

    /// Put back a mark that was taken, for a second reader.
    ///
    /// A file open in two panes is drawn twice a frame, and each pane keeps
    /// its own highlighting state; the first draw takes the mark, and this is
    /// how the second still learns about the edit.
    pub fn retouch(&mut self, from: Option<usize>) {
        if let Some(line) = from {
            self.touched = self.touched.min(line);
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line_chars(&self, i: usize) -> usize {
        self.lines.get(i).map_or(0, |l| l.chars().count())
    }

    /// Where this view of the file is.
    pub fn view(&self) -> View {
        View {
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
            goal_col: self.goal_col,
            scroll_y: self.scroll_y,
            scroll_rows: self.scroll_rows,
            scroll_x: self.scroll_x,
        }
    }

    /// Whether this buffer is exactly what opening its file again would
    /// make: no edits, nothing to undo or redo, and the cursor and the
    /// scroll still at the top. Letting go of one loses nothing.
    pub fn untouched(&self) -> bool {
        !self.dirty
            && self.undo.is_empty()
            && self.redo.is_empty()
            && self.view() == View::default()
    }

    /// Take up `view`, kept inside the file as it is now: another pane may
    /// have deleted the very lines it was on.
    pub fn set_view(&mut self, view: View) {
        self.cursor_line = view.cursor_line;
        self.cursor_col = view.cursor_col;
        self.goal_col = view.goal_col;
        self.scroll_x = view.scroll_x;
        self.clamp_cursor();
        self.scroll_y = view.scroll_y.min(self.lines.len().saturating_sub(1));
        self.scroll_rows = view.scroll_rows;
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
}

/// Byte offset of character index `ci`, or the string length if past the end.
///
/// The bridge between a cursor counted in characters and byte-indexed
/// `String` operations. Every `insert`, `replace_range` and `split_off` on a
/// line goes through it, here and in the one-line fields: slicing with a raw
/// cursor would panic on any text containing a non-ASCII character.
pub fn byte_of_char(s: &str, ci: usize) -> usize {
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
    fn a_word_with_an_accent_written_apart_is_still_one_word() {
        // `café` as an `e` and a combining acute, the way macOS names files.
        let mut e = ed("cafe\u{301} noir\n");
        e.move_word_right();
        assert_eq!(e.cursor_col, 6, "past the accent and the space");
        e.move_end();
        e.move_word_left();
        e.move_word_left();
        assert_eq!(e.cursor_col, 0, "not stopped at the accent");
    }

    #[test]
    fn tab_types_spaces_to_the_next_stop() {
        let mut e = ed("ab\n");
        e.move_end();
        e.insert_tab(4);
        assert_eq!(e.lines()[0], "ab  ", "two to reach column four");
        e.insert_tab(4);
        assert_eq!(e.lines()[0], "ab      ", "and a whole stop from one");
    }

    #[test]
    fn deleting_the_only_line_of_an_empty_file_changes_nothing() {
        let mut e = ed("");
        e.delete_line();
        assert!(!e.dirty, "nothing to save");
        assert!(!e.undo(), "and nothing to undo");
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
    fn inserted_text_lands_as_given_and_undoes_in_one_step() {
        let mut e = ed("    fn a() {}\n");
        e.goto(0, 4);
        e.insert_text("first\n  second\n");
        assert_eq!(
            e.to_text(),
            "    first\n  second\nfn a() {}\n",
            "no indentation added, and the rest of the line follows"
        );
        assert_eq!((e.cursor_line, e.cursor_col), (2, 0), "after what went in");
        assert!(e.dirty);
        assert!(e.undo());
        assert_eq!(e.to_text(), "    fn a() {}\n", "one undo takes it all back");
    }

    #[test]
    fn replacing_everything_keeps_the_file_s_own_ending_and_undoes() {
        let mut e = ed("a\r\nb\r\n");
        e.goto(1, 1);
        e.replace_all("one\ntwo\nthree\n");
        assert_eq!(
            e.to_text(),
            "one\r\ntwo\r\nthree\r\n",
            "still CRLF, one newline"
        );
        assert_eq!(
            (e.cursor_line, e.cursor_col),
            (1, 1),
            "the cursor stays put"
        );
        assert_eq!(e.take_touched(), Some(0), "every line is in doubt");
        assert!(e.undo());
        assert_eq!(e.to_text(), "a\r\nb\r\n");
    }

    #[test]
    fn mixed_line_endings_are_written_back_as_they_were_read() {
        for text in [
            "a\nb\r\nc\n",
            "a\r\nb\nc",
            "a\nb\r",
            "a\r\nb\r",
            "\r\n",
            "only\r",
        ] {
            assert_eq!(ed(text).to_text(), text, "{text:?}");
        }
        let mut e = ed("a\nb\r\nc\n");
        e.insert_char('x');
        assert_eq!(
            e.to_text(),
            "xa\nb\r\nc\n",
            "an edit on one line leaves every other line's ending alone"
        );
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
        e.sync_scroll(20, 10, 4);
        assert!(e.scroll_y > 0, "scrolled down to reach the end");
        assert!(e.cursor_line >= e.scroll_y && e.cursor_line < e.scroll_y + 10);
        e.move_doc_start();
        e.sync_scroll(20, 10, 4);
        assert_eq!(e.scroll_y, 0, "scrolled back up");
    }

    #[test]
    fn scrolling_across_counts_columns_as_they_are_drawn() {
        // Forty CJK characters are eighty columns: the end of the line is
        // well past a sixty-column view even though forty is not.
        let mut e = ed(&format!("{}\n", "文".repeat(40)));
        e.move_end();
        e.sync_scroll(60, 10, 4);
        assert_eq!(e.scroll_x, 80 + 1 - 60, "the cursor's column is on screen");

        // Three tabs and a word: the tabs alone reach column twelve.
        let mut e = ed("\t\t\tword\n");
        e.move_end();
        e.sync_scroll(10, 10, 4);
        assert_eq!(e.scroll_x, 16 + 1 - 10);
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
