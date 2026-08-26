//! Undo, and the grouping that makes it usable.
//!
//! Snapshots of whole buffers, not diffs. At note scale a buffer is a few
//! kilobytes and a snapshot is a clone; a diff-based history would be less
//! memory and a great deal more code to be wrong in, and [`MAX_UNDO`] caps
//! how much of it can pile up.
//!
//! # Why a typing run undoes as one step
//!
//! An undo that took back one character at a time would need forty presses to
//! remove a sentence. So consecutive edits of the same kind, on the same line,
//! coalesce into one group, and anything that is not more of the same — moving
//! the cursor, a different kind of edit — closes the group. That is what
//! [`EditKind`] and [`Editor::break_undo_group`] are for.
//!
//! # The one line everything else depends on
//!
//! [`Editor::push_undo`] is the funnel every mutation goes through, which
//! makes it the only place that has to record *what changed*. The low-water
//! mark it keeps is what lets the syntax highlighter resume instead of
//! re-parsing from the top — see [`crate::text::highlight::Resume`].

use super::{EditKind, Editor, Snapshot};

/// Undo depth. Beyond this the oldest snapshot is dropped, so memory stays
/// bounded on a long editing session.
const MAX_UNDO: usize = 200;

impl Editor {
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
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }

    /// Break the coalescing run — cursor movement ends a typing group.
    ///
    /// Called by every movement method. Without it, typing "foo", arrowing
    /// away, and typing "bar" would revert both runs in one press.
    pub(super) fn break_undo_group(&mut self) {
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
        });
        self.cursor_line = next.cursor_line;
        self.cursor_col = next.cursor_col;
        self.dirty = true;
        self.last_edit = None;
        self.touched = 0;
        self.clamp_cursor();
        true
    }

    // ---- editing ----------------------------------------------------------
}
