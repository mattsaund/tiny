//! Changing the text.
//!
//! Every change is one [`Editor::splice`]: a range of the text replaced with
//! other text. Each edit below is worked out for one cursor and made at every
//! cursor in turn, see [`Editor::each_cursor`], with [`Editor::push_undo`]
//! called once first for all of them, which is what makes the history
//! complete without any edit having to remember to record anything. They all
//! leave the cursor somewhere sensible: an edit that moves it to a surprising
//! place is worse than one that does nothing.
//!
//! Typing over a selection replaces it, and Backspace or Delete with one
//! takes it out, as everywhere.
//!
//! Columns are character indices, never byte offsets. A buffer holds whatever
//! the user typed, and slicing a string at a raw cursor position panics the
//! first time someone writes an accented letter.

use super::{EditKind, Editor, Pos, byte_of_char};
use crate::text::tabs;

/// Where `p` goes when the text from `from` to `to` is replaced by text that
/// ends at `end`: before the change it stays, inside it goes to the end of
/// what took its place, and after it moves with the text it was in.
pub(super) fn shift(p: Pos, from: Pos, to: Pos, end: Pos) -> Pos {
    if p < from {
        p
    } else if p < to {
        end
    } else if p.line == to.line {
        Pos::new(end.line, end.col + (p.col - to.col))
    } else {
        Pos::new(end.line + (p.line - to.line), p.col)
    }
}

/// Where the selection from `anchor` to `pos` starts, when there is one.
fn span_start(anchor: Option<Pos>, pos: Pos) -> Option<Pos> {
    anchor.filter(|a| *a != pos).map(|a| a.min(pos))
}

/// `text` with each line break one `\n`, the way the buffer holds them.
fn plain_breaks(text: &str) -> String {
    text.replace("\r\n", "\n")
}

impl Editor {
    /// Replace the text from `from` to `to` with `text`, which may hold line
    /// breaks, and return where what was put in ends.
    ///
    /// The one change: every edit is made of these, and this is where every
    /// cursor and every end of a selection moves with the text around it,
    /// the main cursor's included, so a cursor at `from` ends up after what
    /// was put in. It also marks the buffer edited and keeps the record of
    /// the lowest changed line that the highlighter resumes from.
    pub(super) fn splice(&mut self, from: Pos, to: Pos, text: &str) -> Pos {
        let tail = {
            let last = &self.lines[to.line];
            last[byte_of_char(last, to.col)..].to_string()
        };
        let first = &mut self.lines[from.line];
        first.truncate(byte_of_char(first, from.col));
        let mut pieces = text.split('\n');
        first.push_str(pieces.next().unwrap_or(""));
        let mut added: Vec<String> = pieces.map(str::to_string).collect();
        let end = match added.last() {
            Some(last) => Pos::new(from.line + added.len(), last.chars().count()),
            None => Pos::new(from.line, self.lines[from.line].chars().count()),
        };
        match added.last_mut() {
            Some(last) => last.push_str(&tail),
            None => self.lines[from.line].push_str(&tail),
        }
        // The lines after `from` that the range ran over make way for the
        // new ones; a range within one line takes none away.
        self.lines.splice(from.line + 1..=to.line, added);

        let moved = |p: Pos| shift(p, from, to, end);
        self.set_pos(moved(self.pos()));
        self.anchor = self.anchor.map(moved);
        for c in &mut self.others {
            c.pos = moved(c.pos);
            c.anchor = c.anchor.map(moved);
        }
        self.touched = self.touched.min(from.line);
        self.dirty = true;
        end
    }

    /// Take out the main cursor's selection, if it has one, leaving the
    /// cursor where it began. Returns whether there was one.
    fn take_out_selection(&mut self) -> bool {
        let taken = match self.selection() {
            Some((start, end)) => {
                self.splice(start, end, "");
                true
            }
            None => false,
        };
        self.anchor = None;
        taken
    }

    /// Put `text` in at the main cursor, over its selection if it has one,
    /// and leave the cursor after it.
    fn put(&mut self, text: &str) {
        self.take_out_selection();
        let at = self.pos();
        self.splice(at, at, text);
        self.goal_col = self.cursor_col;
    }

    /// How a run of typing is grouped for undo: on its own when it replaces
    /// a selection, so the selection comes back in one step.
    fn typing(&self) -> EditKind {
        if self.has_selection() {
            EditKind::Structural
        } else {
            EditKind::Insert
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.push_undo(self.typing());
        // What is typed after a selection it replaced undoes along with it.
        self.last_edit = Some(EditKind::Insert);
        let mut buf = [0u8; 4];
        let text: &str = c.encode_utf8(&mut buf);
        self.each_cursor(true, |ed, _| ed.put(text));
    }

    /// Tab types spaces up to the next tab stop, the way a tab character
    /// would be drawn there, so text indented from part way along a line
    /// still lines up with the stops. With lines selected it indents them
    /// all instead, the way every editor does.
    pub fn insert_tab(&mut self, width: usize) {
        if self.selects_lines() {
            self.indent(width);
            return;
        }
        self.push_undo(self.typing());
        self.each_cursor(true, |ed, _| {
            ed.take_out_selection();
            let line = &ed.lines[ed.cursor_line];
            let n = tabs::stop(tabs::column(line, ed.cursor_col, width), width);
            ed.put(&" ".repeat(n));
        });
    }

    /// Split the line at the cursor, carrying the indentation before it onto
    /// the new line: the auto-indent every editor has. Structural, so it
    /// always begins a fresh undo group.
    pub fn insert_newline(&mut self) {
        self.push_undo(EditKind::Structural);
        self.each_cursor(true, |ed, _| {
            ed.take_out_selection();
            let indent: String = ed.lines[ed.cursor_line]
                .chars()
                .take(ed.cursor_col)
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            ed.put(&format!("\n{indent}"));
        });
    }

    /// Delete backwards. Two quite different operations share the key: within
    /// a line it removes a character (a `Delete` group, which coalesces), and
    /// at column 0 it joins with the previous line (a `Structural` group,
    /// which does not). With a selection it takes that out.
    ///
    /// At the very start of the buffer, with nothing selected, it returns
    /// early *before* touching anything, so a stray keypress on a clean file
    /// does not make it look edited.
    pub fn backspace(&mut self) {
        let cursors = self.cursors();
        if cursors
            .iter()
            .all(|(p, sel)| sel.is_none() && *p == Pos::default())
        {
            return;
        }
        let joins = cursors.iter().any(|(p, sel)| sel.is_none() && p.col == 0);
        let kind = if self.has_selection() || joins {
            EditKind::Structural
        } else {
            EditKind::Delete
        };
        self.push_undo(kind);
        self.each_cursor(true, |ed, _| {
            if ed.take_out_selection() {
                return;
            }
            let p = ed.pos();
            if p.col > 0 {
                ed.splice(Pos::new(p.line, p.col - 1), p, "");
            } else if p.line > 0 {
                let end_of_previous = Pos::new(p.line - 1, ed.line_chars(p.line - 1));
                ed.splice(end_of_previous, p, "");
            }
            ed.goal_col = ed.cursor_col;
        });
    }

    /// Delete under the cursor. The mirror of [`Editor::backspace`]: within a
    /// line it removes a character, at the end of one it pulls the next line
    /// up, at the end of the buffer it does nothing, and with a selection it
    /// takes that out.
    pub fn delete_forward(&mut self) {
        let last = self.lines.len() - 1;
        let at_end = |p: &Pos| p.line == last && p.col >= self.line_chars(last);
        let cursors = self.cursors();
        if cursors.iter().all(|(p, sel)| sel.is_none() && at_end(p)) {
            return;
        }
        let joins = cursors
            .iter()
            .any(|(p, sel)| sel.is_none() && p.col >= self.line_chars(p.line));
        let kind = if self.has_selection() || joins {
            EditKind::Structural
        } else {
            EditKind::Delete
        };
        self.push_undo(kind);
        self.each_cursor(true, |ed, _| {
            if ed.take_out_selection() {
                return;
            }
            let p = ed.pos();
            if p.col < ed.line_chars(p.line) {
                ed.splice(p, Pos::new(p.line, p.col + 1), "");
            } else if p.line + 1 < ed.lines.len() {
                ed.splice(p, Pos::new(p.line + 1, 0), "");
            }
            ed.goal_col = ed.cursor_col;
        });
    }

    /// Delete the whole current line, the way micro's Ctrl+K does: every line
    /// a selection runs over, and every cursor's line once. In a file that
    /// is one empty line there is nothing to delete, and nothing is marked
    /// unsaved.
    pub fn delete_line(&mut self) {
        if self.lines.len() == 1 && self.lines[0].is_empty() {
            return;
        }
        self.push_undo(EditKind::Structural);
        // One cursor per line, or the second on a line would go on to delete
        // the line that moved up into its place.
        self.keep_one_cursor_per_line();
        self.each_cursor(true, |ed, _| {
            let (first, last) = ed.lines_selected();
            ed.anchor = None;
            if first == 0 && last + 1 >= ed.lines.len() {
                let end = Pos::new(last, ed.line_chars(last));
                ed.splice(Pos::new(0, 0), end, "");
            } else if last + 1 < ed.lines.len() {
                ed.splice(Pos::new(first, 0), Pos::new(last + 1, 0), "");
            } else {
                let end_of_previous = Pos::new(first - 1, ed.line_chars(first - 1));
                ed.splice(end_of_previous, Pos::new(last, ed.line_chars(last)), "");
            }
            ed.cursor_col = 0;
            ed.goal_col = 0;
            ed.clamp_cursor();
        });
    }

    /// Put `text` in at the cursor, newlines and all, as one step of undo:
    /// what a plugin's `tiny.insert` does, and a paste. The cursor ends up
    /// after it.
    ///
    /// Unlike typing it, no indentation is carried onto the new lines: the
    /// text arrives exactly as it was given, so a paste of an indented block
    /// does not get it indented twice.
    pub fn insert_text(&mut self, text: &str) {
        let text = plain_breaks(text);
        if text.is_empty() {
            return;
        }
        self.push_undo(EditKind::Structural);
        self.each_cursor(true, |ed, _| ed.put(&text));
    }

    /// Paste `text` at every cursor, as one step of undo.
    ///
    /// With as many cursors as `text` has lines, each cursor gets its own
    /// line, the way they were copied from as many cursors. `whole_lines` is
    /// text copied with nothing selected, which was whole lines: it goes in
    /// above the cursor's line rather than into the middle of it.
    pub fn paste(&mut self, text: &str, whole_lines: bool) {
        let text = plain_breaks(text);
        if text.is_empty() {
            return;
        }
        self.push_undo(EditKind::Structural);
        let count = 1 + self.others.len();
        let lines: Vec<&str> = text
            .strip_suffix('\n')
            .unwrap_or(&text)
            .split('\n')
            .collect();
        let shared = count > 1 && lines.len() == count;
        self.each_cursor(true, |ed, n| {
            let piece = match (shared, whole_lines) {
                (true, true) => format!("{}\n", lines[n]),
                (true, false) => lines[n].to_string(),
                (false, _) => text.clone(),
            };
            if whole_lines && !ed.has_own_selection() {
                let start = Pos::new(ed.cursor_line, 0);
                ed.splice(start, start, &piece);
            } else {
                ed.put(&piece);
            }
            ed.goal_col = ed.cursor_col;
        });
    }

    /// Take out what is selected and return it, or with nothing selected,
    /// every cursor's whole line. The second element says which: whole lines
    /// paste back in above a line rather than into it.
    pub fn cut(&mut self) -> (String, bool) {
        if let Some(text) = self.selected_text() {
            self.push_undo(EditKind::Structural);
            self.each_cursor(true, |ed, _| {
                ed.take_out_selection();
            });
            return (text, false);
        }
        let text = self.cursor_lines_text();
        self.delete_line();
        (text, true)
    }

    /// Indent every line a selection runs over, or the cursor is on, by
    /// `width` spaces. What Tab does to selected lines.
    pub fn indent(&mut self, width: usize) {
        self.push_undo(EditKind::Structural);
        let pad = " ".repeat(width.max(1));
        for line in self.lines_under_cursors() {
            let start = Pos::new(line, 0);
            // A selection that begins at the very start of the line goes on
            // beginning there, so the indent is selected along with the line.
            let begins = |anchor: Option<Pos>, pos: Pos| span_start(anchor, pos) == Some(start);
            let main = begins(self.anchor, self.pos()).then_some(self.anchor == Some(start));
            let others: Vec<Option<bool>> = self
                .others
                .iter()
                .map(|c| begins(c.anchor, c.pos).then_some(c.anchor == Some(start)))
                .collect();
            self.splice(start, start, &pad);
            match main {
                Some(true) => self.anchor = Some(start),
                Some(false) => self.set_pos(start),
                None => {}
            }
            for (c, keep) in self.others.iter_mut().zip(others) {
                match keep {
                    Some(true) => c.anchor = Some(start),
                    Some(false) => c.pos = start,
                    None => {}
                }
            }
        }
    }

    /// Take up to `width` columns of indentation off every line a selection
    /// runs over, or the cursor is on: Shift+Tab.
    pub fn outdent(&mut self, width: usize) {
        let lines = self.lines_under_cursors();
        let cut: Vec<(usize, usize)> = lines
            .iter()
            .map(|&l| {
                let line = &self.lines[l];
                let n = if line.starts_with('\t') {
                    1
                } else {
                    line.chars()
                        .take(width.max(1))
                        .take_while(|c| *c == ' ')
                        .count()
                };
                (l, n)
            })
            .filter(|(_, n)| *n > 0)
            .collect();
        if cut.is_empty() {
            return;
        }
        self.push_undo(EditKind::Structural);
        for (l, n) in cut {
            self.splice(Pos::new(l, 0), Pos::new(l, n), "");
        }
    }

    /// Replace everything with `text`, as one step of undo: what a plugin's
    /// `tiny.set_text` does. The cursor keeps its line and column, as near as
    /// the new text allows; other cursors and the selection go.
    ///
    /// One trailing newline is read as the end of the last line rather than as
    /// an empty line after it: the buffer already knows whether the file ends
    /// with one, and puts it back on save.
    pub fn replace_all(&mut self, text: &str) {
        self.push_undo(EditKind::Structural);
        self.collapse();
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

    /// The first and last line the main cursor's selection runs over, or its
    /// own line twice. A selection that ends at the very start of a line does
    /// not take that line in.
    fn lines_selected(&self) -> (usize, usize) {
        match self.selection() {
            Some((start, end)) if end.col == 0 && end.line > start.line => {
                (start.line, end.line - 1)
            }
            Some((start, end)) => (start.line, end.line),
            None => (self.cursor_line, self.cursor_line),
        }
    }

    /// Whether the main cursor has text selected (the others aside).
    fn has_own_selection(&self) -> bool {
        self.selection().is_some()
    }

    /// Whether any selection runs over more than one line.
    fn selects_lines(&self) -> bool {
        self.cursors()
            .iter()
            .any(|(_, sel)| sel.is_some_and(|(a, b)| b.line > a.line))
    }

    /// Every line under a cursor or a selection, each once, last first, so
    /// a change to one does not move those still to come.
    fn lines_under_cursors(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = Vec::new();
        for (pos, sel) in self.cursors() {
            match sel {
                Some((a, b)) => {
                    let last = if b.col == 0 && b.line > a.line {
                        b.line - 1
                    } else {
                        b.line
                    };
                    lines.extend(a.line..=last);
                }
                None => lines.push(pos.line),
            }
        }
        lines.sort_unstable();
        lines.dedup();
        lines.reverse();
        lines
    }

    /// Down to one cursor on each line.
    fn keep_one_cursor_per_line(&mut self) {
        let mut seen = vec![self.cursor_line];
        self.others.retain(|c| {
            let fresh = !seen.contains(&c.pos.line);
            seen.push(c.pos.line);
            fresh
        });
    }
}
