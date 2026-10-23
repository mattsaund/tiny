//! Undo, and the grouping that makes it usable.
//!
//! Snapshots of whole buffers, not diffs. At note scale a buffer is a few
//! kilobytes and a snapshot is a clone; a diff-based history would be less
//! memory and a great deal more code to be wrong in, and [`MAX_UNDO`] and
//! [`UNDO_BYTES`] cap how much of it can pile up.
//!
//! # Why a typing run undoes as one step
//!
//! An undo that took back one character at a time would need forty presses to
//! remove a sentence. So consecutive edits of the same kind coalesce into one
//! group, and anything that is not more of the same (moving the cursor, a new
//! line, a different kind of edit) closes the group. That is what
//! [`EditKind`] and [`Editor::break_undo_group`] are for.
//!
//! # The one line everything else depends on
//!
//! [`Editor::push_undo`] is the funnel every mutation goes through, which
//! makes it the only place that has to record *what changed*. The low-water
//! mark it keeps is what lets the syntax highlighter resume instead of
//! re-parsing from the top, see [`crate::text::highlight::Resume`].

use super::{EditKind, Editor, Snapshot};

/// Undo depth. Beyond this the oldest snapshot is dropped, so memory stays
/// bounded on a long editing session.
const MAX_UNDO: usize = 200;

/// How much one file's snapshots may hold between them. A note keeps all
/// [`MAX_UNDO`] of its steps well inside this; a file of several megabytes
/// keeps fewer, down to [`MIN_UNDO`], where two hundred copies of it would
/// have been gigabytes.
const UNDO_BYTES: usize = 64 * 1024 * 1024;

/// The fewest steps kept, however large the file.
const MIN_UNDO: usize = 8;

impl Editor {
    /// Record the pre-edit state, coalescing runs of the same edit kind.
    ///
    /// Called at the top of every mutating method, *before* the mutation, so
    /// the snapshot captures the state to return to. The coalescing rule is:
    ///
    /// - Same kind as the last edit, and not `Structural` → no new snapshot,
    ///   so a run of typing reverts in one press.
    /// - Different kind, or `Structural` → new snapshot. Structural edits
    ///   (newline, line join, cut line) never merge, because each one is a
    ///   change a user thinks of as a single deliberate act.
    ///
    /// Any edit clears the redo stack, whether or not it coalesced.
    pub(super) fn push_undo(&mut self, kind: EditKind) {
        // Every mutation acts at the cursor and every mutation calls this, so
        // this one line records what changed for the whole editor. One line of
        // slack because a backspace at column 0 joins the cursor's line into
        // the one above, changing that one instead.
        self.touched = self.touched.min(self.cursor_line.saturating_sub(1));
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
            anchor: self.anchor,
            others: self.others.clone(),
        });
        // Every snapshot of a file is about the size of the file, so the
        // budget comes down to a number of them.
        let size = self.lines.iter().map(String::len).sum::<usize>()
            + self.lines.len() * std::mem::size_of::<String>();
        let depth = (UNDO_BYTES / size.max(1)).clamp(MIN_UNDO, MAX_UNDO);
        if self.undo.len() > depth {
            self.undo.drain(..self.undo.len() - depth);
        }
    }

    /// Break the coalescing run: cursor movement ends a typing group.
    ///
    /// Called by every movement method. Without it, typing "foo", arrowing
    /// away, and typing "bar" would revert both runs in one press.
    pub fn break_undo_group(&mut self) {
        self.last_edit = None;
    }

    /// Step back one group. Returns false when there is nothing left, which
    /// the caller turns into a "nothing to undo" status message.
    ///
    /// Note that `dirty` is set unconditionally, even when undoing all the way
    /// back to the file's opening state. Tracking whether the buffer matches
    /// disk again would need a saved-generation counter; erring toward "there
    /// might be something to save" is the safe direction.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            lines: std::mem::replace(&mut self.lines, prev.lines),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
            anchor: self.anchor,
            others: std::mem::replace(&mut self.others, prev.others),
        });
        self.cursor_line = prev.cursor_line;
        self.cursor_col = prev.cursor_col;
        self.anchor = prev.anchor;
        self.dirty = true;
        self.last_edit = None;
        // A snapshot replaces every line at once, so nothing above is safe.
        self.touched = 0;
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
            anchor: self.anchor,
            others: std::mem::replace(&mut self.others, next.others),
        });
        self.cursor_line = next.cursor_line;
        self.cursor_col = next.cursor_col;
        self.anchor = next.anchor;
        self.dirty = true;
        self.last_edit = None;
        self.touched = 0;
        self.clamp_cursor();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Presses Enter `n` times: each one a step of its own.
    fn steps(ed: &mut Editor, n: usize) {
        for _ in 0..n {
            ed.insert_newline();
        }
    }

    #[test]
    fn a_note_keeps_every_step() {
        let mut ed = Editor::from_str(PathBuf::from("n.md"), "a short note\n");
        steps(&mut ed, MAX_UNDO + 10);
        assert_eq!(ed.undo.len(), MAX_UNDO);
    }

    #[test]
    fn a_file_of_megabytes_keeps_fewer_steps_but_never_too_few() {
        // Four megabytes: two hundred copies would be most of a gigabyte.
        let line = "x".repeat(1023) + "\n";
        let mut ed = Editor::from_str(PathBuf::from("big.txt"), &line.repeat(4 * 1024));
        steps(&mut ed, 40);
        assert!(
            (MIN_UNDO..40).contains(&ed.undo.len()),
            "{} steps kept",
            ed.undo.len()
        );
        // And what is kept still undoes.
        assert!(ed.undo());
    }
}
