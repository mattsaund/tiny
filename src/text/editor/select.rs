//! Selections, and the cursors beyond the first.
//!
//! A selection is an anchor and the cursor: it runs between them, whichever
//! comes first, and it is held by each cursor for itself. The main cursor's
//! is `anchor`; the others carry theirs in [`Cursor`].
//!
//! # Doing something at every cursor
//!
//! The edits and motions are each written for one cursor, the main one,
//! and [`Editor::each_cursor`] runs one at every cursor in turn by making
//! each the main cursor while it does. For an edit the order matters: from
//! the last cursor in the text to the first, so that an edit never moves a
//! cursor still waiting for its turn; the ones it does move, behind it, are
//! carried along by [`Editor::splice`](super::Editor).

use super::{Cursor, Editor, Pos};

/// What a drag with the mouse selects by: characters, or after a double or a
/// triple click, whole words or whole lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Char,
    Word,
    Line,
}

impl Editor {
    /// Where the main cursor's selection runs, start first, when it has one.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        span(self.anchor, self.pos())
    }

    /// Whether any cursor has text selected.
    pub fn has_selection(&self) -> bool {
        self.selection().is_some() || self.others.iter().any(|c| span(c.anchor, c.pos).is_some())
    }

    /// Every cursor, the main one included, in the order they come in the
    /// text, each with its selection.
    pub fn cursors(&self) -> Vec<(Pos, Option<(Pos, Pos)>)> {
        let mut all: Vec<(Pos, Option<(Pos, Pos)>)> =
            std::iter::once((self.pos(), self.selection()))
                .chain(self.others.iter().map(|c| (c.pos, span(c.anchor, c.pos))))
                .collect();
        all.sort_by_key(|(pos, sel)| sel.map_or(*pos, |(start, _)| start));
        all
    }

    /// The text from `from` to `to`, line breaks as `\n`.
    pub fn text_between(&self, from: Pos, to: Pos) -> String {
        let chars = |line: usize, a: usize, b: Option<usize>| -> String {
            let text = &self.lines[line];
            let n = b.unwrap_or(usize::MAX);
            text.chars().skip(a).take(n.saturating_sub(a)).collect()
        };
        if from.line == to.line {
            return chars(from.line, from.col, Some(to.col));
        }
        let mut out = chars(from.line, from.col, None);
        for line in from.line + 1..to.line {
            out.push('\n');
            out.push_str(&self.lines[line]);
        }
        out.push('\n');
        out.push_str(&chars(to.line, 0, Some(to.col)));
        out
    }

    /// What is selected, every cursor's in the order they come, one to a
    /// line; `None` when nothing is.
    pub fn selected_text(&self) -> Option<String> {
        let texts: Vec<String> = self
            .cursors()
            .into_iter()
            .filter_map(|(_, sel)| sel.map(|(a, b)| self.text_between(a, b)))
            .collect();
        (!texts.is_empty()).then(|| texts.join("\n"))
    }

    /// How many characters are selected, at every cursor, a line break
    /// counting as one: the selection's size, without copying it out.
    pub fn selected_count(&self) -> usize {
        self.cursors()
            .into_iter()
            .filter_map(|(_, sel)| sel)
            .map(|(a, b)| {
                if a.line == b.line {
                    return b.col - a.col;
                }
                let middle: usize = (a.line + 1..b.line).map(|l| self.line_chars(l) + 1).sum();
                self.line_chars(a.line) - a.col + 1 + middle + b.col
            })
            .sum()
    }

    /// The whole of every line a cursor is on, each with its line break:
    /// what copying takes when nothing is selected.
    pub fn cursor_lines_text(&self) -> String {
        let mut lines: Vec<usize> = self.cursors().into_iter().map(|(p, _)| p.line).collect();
        lines.dedup();
        lines
            .iter()
            .map(|&l| format!("{}\n", self.lines[l]))
            .collect()
    }

    /// Select everything, with one cursor, at the end.
    pub fn select_all(&mut self) {
        self.break_undo_group();
        self.others.clear();
        let last = self.lines.len() - 1;
        self.anchor = Some(Pos::new(0, 0));
        self.set_pos(Pos::new(last, self.line_chars(last)));
    }

    /// Down to one cursor and nothing selected. Returns whether there was
    /// anything to let go of, so Esc can do this first and leave the pane
    /// only when there is not.
    pub fn collapse(&mut self) -> bool {
        let had = self.anchor.is_some_and(|a| a != self.pos()) || !self.others.is_empty();
        self.anchor = None;
        self.others.clear();
        had
    }

    /// Another cursor on the line below the last one, or above the first,
    /// at the column they are trying to keep. The new one is the main
    /// cursor, so the view follows it.
    pub fn add_cursor_vertically(&mut self, down: bool) {
        self.break_undo_group();
        let all = self.cursors();
        let edge = if down { all.last() } else { all.first() };
        let Some(&(from, _)) = edge else { return };
        let line = if down {
            if from.line + 1 >= self.lines.len() {
                return;
            }
            from.line + 1
        } else {
            let Some(line) = from.line.checked_sub(1) else {
                return;
            };
            line
        };
        let goal = if from == self.pos() {
            self.goal_col
        } else {
            self.others
                .iter()
                .find(|c| c.pos == from)
                .map_or(from.col, |c| c.goal)
        };
        let at = Pos::new(line, goal.min(self.line_chars(line)));
        self.add_cursor(at, None, goal);
    }

    /// The next place the main cursor's selection appears, after the last
    /// cursor and round from the top, gets a cursor of its own with it
    /// selected. With nothing selected, the word under the cursor is
    /// selected first, which is what the next press looks for.
    pub fn add_cursor_at_next_match(&mut self) {
        self.break_undo_group();
        let Some((start, end)) = self.selection() else {
            if let Some((a, b)) = self.word_at(self.pos()) {
                self.anchor = Some(a);
                self.set_pos(b);
            }
            return;
        };
        let needle: Vec<char> = self.text_between(start, end).chars().collect();
        if needle.is_empty() || needle.contains(&'\n') {
            return;
        }
        let after = self
            .cursors()
            .last()
            .map_or(end, |(p, sel)| sel.map_or(*p, |(_, b)| b));
        let taken: Vec<Pos> = self
            .cursors()
            .into_iter()
            .filter_map(|(_, sel)| sel.map(|(a, _)| a))
            .collect();
        let Some(found) = self
            .find_after(&needle, after)
            .filter(|p| !taken.contains(p))
        else {
            return;
        };
        let to = Pos::new(found.line, found.col + needle.len());
        self.add_cursor(to, Some(found), to.col);
    }

    // ---- the mouse ----------------------------------------------------------

    /// A click: one cursor, at `p`, nothing selected.
    pub fn click(&mut self, p: Pos) {
        self.break_undo_group();
        self.collapse();
        let p = self.clamped(p);
        self.set_pos(p);
        self.goal_col = p.col;
    }

    /// Alt and a click: another cursor at `p`, the main one from now on.
    pub fn click_another(&mut self, p: Pos) {
        self.break_undo_group();
        let p = self.clamped(p);
        self.add_cursor(p, None, p.col);
    }

    /// What a press at `p` selects by `unit`, start and end: nothing, the
    /// word under it (or the one character, when it is not on a word), or
    /// the whole line, its line break included.
    pub fn unit_at(&self, p: Pos, unit: Unit) -> (Pos, Pos) {
        let p = self.clamped(p);
        match unit {
            Unit::Char => (p, p),
            Unit::Word => self.word_at(p).unwrap_or_else(|| {
                let end = Pos::new(p.line, (p.col + 1).min(self.line_chars(p.line)));
                (p, end)
            }),
            Unit::Line if p.line + 1 < self.lines.len() => {
                (Pos::new(p.line, 0), Pos::new(p.line + 1, 0))
            }
            Unit::Line => (
                Pos::new(p.line, 0),
                Pos::new(p.line, self.line_chars(p.line)),
            ),
        }
    }

    /// A drag that began on `origin` (what the press selected) and is now at
    /// `to`: everything between, by whole `unit`s, with the cursor at the end
    /// the mouse is at. One cursor, as a drag always means.
    pub fn drag(&mut self, origin: (Pos, Pos), to: Pos, unit: Unit) {
        self.break_undo_group();
        self.others.clear();
        let (here_start, here_end) = self.unit_at(to, unit);
        let (anchor, pos) = if here_start < origin.0 {
            (origin.1, here_start)
        } else {
            (origin.0, here_end.max(origin.1))
        };
        self.anchor = Some(self.clamped(anchor));
        let pos = self.clamped(pos);
        self.set_pos(pos);
        self.goal_col = pos.col;
    }

    /// Shift and a click: the selection made to reach `p`, from where it
    /// began, or from the cursor when nothing was selected.
    pub fn click_extend(&mut self, p: Pos) {
        let from = self.anchor.unwrap_or(self.pos());
        self.drag((from, from), p, Unit::Char);
    }

    /// Make a new main cursor at `pos`, keeping the old one among the rest.
    fn add_cursor(&mut self, pos: Pos, anchor: Option<Pos>, goal: usize) {
        if self.pos() == pos || self.others.iter().any(|c| c.pos == pos) {
            return;
        }
        self.others.push(Cursor {
            pos: self.pos(),
            goal: self.goal_col,
            anchor: self.anchor,
        });
        self.set_pos(pos);
        self.goal_col = goal;
        self.anchor = anchor;
    }

    /// The word around `at`, start and end, when it is on one.
    fn word_at(&self, at: Pos) -> Option<(Pos, Pos)> {
        use crate::text::search::is_word_char;
        let chars: Vec<char> = self.lines[at.line].chars().collect();
        let on = |i: usize| chars.get(i).is_some_and(|c| is_word_char(*c));
        let mut start = at.col;
        if !on(start) && start > 0 && on(start - 1) {
            start -= 1;
        }
        if !on(start) {
            return None;
        }
        while start > 0 && on(start - 1) {
            start -= 1;
        }
        let mut end = start;
        while on(end) {
            end += 1;
        }
        Some((Pos::new(at.line, start), Pos::new(at.line, end)))
    }

    /// Where `needle` next starts after `after`, round from the top of the
    /// text if it has to; it never spans a line break.
    fn find_after(&self, needle: &[char], after: Pos) -> Option<Pos> {
        let in_line = |line: usize, from: usize| -> Option<usize> {
            let chars: Vec<char> = self.lines[line].chars().collect();
            (from..=chars.len().saturating_sub(needle.len()))
                .find(|&i| i + needle.len() <= chars.len() && chars[i..i + needle.len()] == *needle)
        };
        let n = self.lines.len();
        if let Some(col) = in_line(after.line, after.col) {
            return Some(Pos::new(after.line, col));
        }
        for k in 1..=n {
            let line = (after.line + k) % n;
            if let Some(col) = in_line(line, 0) {
                return Some(Pos::new(line, col));
            }
        }
        None
    }

    /// Run `f` at every cursor, each made the main one while it does; `f`
    /// is told which it is in the order the cursors come in the text.
    /// `last_first` runs them from the last in the text up, which an edit
    /// needs, see the module docs. Cursors that end in the same place
    /// become one.
    pub(super) fn each_cursor(&mut self, last_first: bool, mut f: impl FnMut(&mut Self, usize)) {
        if self.others.is_empty() {
            f(self, 0);
            return;
        }
        let main = Cursor {
            pos: self.pos(),
            goal: self.goal_col,
            anchor: self.anchor,
        };
        let mut all: Vec<(bool, Cursor)> = std::iter::once((true, main))
            .chain(self.others.drain(..).map(|c| (false, c)))
            .collect();
        let key = |c: &Cursor| span(c.anchor, c.pos).map_or(c.pos, |(start, _)| start);
        all.sort_by_key(|(_, c)| key(c));
        let count = all.len();
        for step in 0..count {
            let i = if last_first { count - 1 - step } else { step };
            let me = all[i].1;
            self.set_pos(me.pos);
            self.goal_col = me.goal;
            self.anchor = me.anchor;
            self.others = all
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, (_, c))| *c)
                .collect();
            f(self, i);
            let me = Cursor {
                pos: self.pos(),
                goal: self.goal_col,
                anchor: self.anchor,
            };
            let mut rest = std::mem::take(&mut self.others).into_iter();
            for (j, (_, c)) in all.iter_mut().enumerate() {
                if j == i {
                    *c = me;
                } else if let Some(moved) = rest.next() {
                    *c = moved;
                }
            }
        }
        let main = all
            .iter()
            .find(|(is_main, _)| *is_main)
            .map(|(_, c)| *c)
            .unwrap_or(main);
        self.set_pos(main.pos);
        self.goal_col = main.goal;
        self.anchor = main.anchor;
        let mut seen = vec![main.pos];
        self.others = all
            .into_iter()
            .filter(|(is_main, _)| !is_main)
            .map(|(_, c)| c)
            .filter(|c| {
                let fresh = !seen.contains(&c.pos);
                seen.push(c.pos);
                fresh
            })
            .collect();
    }

    /// Move every cursor by `step`, written for the main one. With `select`
    /// each keeps the selection it had, or starts one where it was; without,
    /// selections are let go of once the step has seen them.
    pub(super) fn move_cursors(&mut self, select: bool, step: impl Fn(&mut Self)) {
        self.break_undo_group();
        self.each_cursor(false, |ed, _| {
            if select && ed.anchor.is_none() {
                ed.anchor = Some(ed.pos());
            }
            step(ed);
            if !select {
                ed.anchor = None;
            }
        });
    }

    pub(super) fn set_pos(&mut self, p: Pos) {
        self.cursor_line = p.line;
        self.cursor_col = p.col;
    }
}

/// The range between an anchor and a cursor, start first, when it is not
/// empty.
fn span(anchor: Option<Pos>, pos: Pos) -> Option<(Pos, Pos)> {
    let a = anchor?;
    (a != pos).then(|| (a.min(pos), a.max(pos)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ed(text: &str) -> Editor {
        Editor::from_str(PathBuf::from("/tmp/t.txt"), text)
    }

    fn type_str(e: &mut Editor, s: &str) {
        for c in s.chars() {
            e.insert_char(c);
        }
    }

    #[test]
    fn typing_replaces_the_selection_and_undo_brings_both_back() {
        let mut e = ed("hello world\n");
        e.goto(0, 6);
        for _ in 0..5 {
            e.select_right();
        }
        assert_eq!(e.selected_text().as_deref(), Some("world"));
        type_str(&mut e, "there");
        assert_eq!(e.to_text(), "hello there\n");
        assert!(!e.has_selection());
        assert!(e.undo(), "one undo for the replacement and the typing");
        assert_eq!(e.to_text(), "hello world\n");
        assert_eq!(
            e.selected_text().as_deref(),
            Some("world"),
            "selected again"
        );
    }

    #[test]
    fn a_selection_over_lines_goes_in_one_backspace() {
        let mut e = ed("one\ntwo\nthree\n");
        e.goto(0, 1);
        e.select_down();
        e.select_down();
        assert_eq!(e.selected_text().as_deref(), Some("ne\ntwo\nt"));
        e.backspace();
        assert_eq!(e.to_text(), "ohree\n");
        assert_eq!(e.pos(), Pos::new(0, 1));
    }

    #[test]
    fn left_and_right_let_go_of_a_selection_at_its_ends() {
        let mut e = ed("abcdef\n");
        e.goto(0, 2);
        e.select_right();
        e.select_right();
        e.move_left();
        assert_eq!(e.pos(), Pos::new(0, 2), "at the start");
        assert!(!e.has_selection());
        e.select_right();
        e.select_right();
        e.move_right();
        assert_eq!(e.pos(), Pos::new(0, 4), "at the end");
        e.select_left();
        e.select_left();
        e.select_right();
        assert_eq!(e.selected_text().as_deref(), Some("d"), "it shrinks back");
    }

    #[test]
    fn select_all_takes_everything() {
        let mut e = ed("a\nbc\n");
        e.select_all();
        assert_eq!(e.selected_text().as_deref(), Some("a\nbc"));
        assert_eq!(e.selected_count(), 4, "the line break is one");
        e.insert_char('x');
        assert_eq!(e.to_text(), "x\n");
    }

    #[test]
    fn cursors_added_below_all_type() {
        let mut e = ed("ab\ncd\nef\n");
        e.goto(0, 1);
        e.add_cursor_vertically(true);
        e.add_cursor_vertically(true);
        assert_eq!(e.others.len(), 2);
        type_str(&mut e, "X");
        assert_eq!(e.to_text(), "aXb\ncXd\neXf\n");
        e.backspace();
        e.backspace();
        assert_eq!(e.to_text(), "b\nd\nf\n");
        assert!(e.undo());
        assert_eq!(e.to_text(), "aXb\ncXd\neXf\n");
        assert_eq!(e.others.len(), 2, "the cursors come back too");
    }

    #[test]
    fn a_cursor_above_keeps_the_column_it_wanted() {
        let mut e = ed("long line\nx\nanother\n");
        e.goto(2, 5);
        e.add_cursor_vertically(false);
        e.add_cursor_vertically(false);
        let mut at: Vec<Pos> = e.cursors().into_iter().map(|(p, _)| p).collect();
        at.sort();
        assert_eq!(at, [Pos::new(0, 5), Pos::new(1, 1), Pos::new(2, 5)]);
    }

    #[test]
    fn cursors_that_meet_become_one() {
        let mut e = ed("abc\n");
        e.goto(0, 2);
        e.others.push(Cursor {
            pos: Pos::new(0, 3),
            goal: 3,
            anchor: None,
        });
        e.backspace();
        e.backspace();
        assert_eq!(e.to_text(), "\n");
        assert!(e.others.is_empty(), "both at the start of the line");
    }

    #[test]
    fn the_next_match_gets_a_cursor_of_its_own() {
        let mut e = ed("foo bar foo\nbaz foo\n");
        e.goto(0, 1);
        e.add_cursor_at_next_match();
        assert_eq!(e.selected_text().as_deref(), Some("foo"), "the word first");
        e.add_cursor_at_next_match();
        e.add_cursor_at_next_match();
        assert_eq!(e.others.len(), 2);
        e.add_cursor_at_next_match();
        assert_eq!(e.others.len(), 2, "round to the first, already taken");
        type_str(&mut e, "qux");
        assert_eq!(e.to_text(), "qux bar qux\nbaz qux\n");
    }

    #[test]
    fn copying_from_several_cursors_puts_each_on_a_line() {
        let mut e = ed("one two\nthree four\n");
        e.goto(0, 0);
        e.add_cursor_vertically(true);
        e.select_word_right();
        assert_eq!(e.selected_text().as_deref(), Some("one \nthree "));
        e.select_word_right();
        assert_eq!(e.selected_text().as_deref(), Some("one two\nthree four"));
    }

    #[test]
    fn as_many_lines_as_cursors_paste_one_each() {
        let mut e = ed("a\nb\nc\n");
        e.goto(0, 1);
        e.add_cursor_vertically(true);
        e.add_cursor_vertically(true);
        e.paste("1\n2\n3", false);
        assert_eq!(e.to_text(), "a1\nb2\nc3\n");
        e.paste("!", false);
        assert_eq!(
            e.to_text(),
            "a1!\nb2!\nc3!\n",
            "otherwise all of it at each"
        );
    }

    #[test]
    fn a_whole_line_cut_pastes_back_above_the_line() {
        let mut e = ed("first\nsecond\nthird\n");
        e.goto(0, 2);
        let (text, whole) = e.cut();
        assert_eq!((text.as_str(), whole), ("first\n", true));
        assert_eq!(e.to_text(), "second\nthird\n");
        e.goto(1, 3);
        e.paste(&text, whole);
        assert_eq!(e.to_text(), "second\nfirst\nthird\n");
        assert_eq!(e.pos(), Pos::new(2, 3), "the cursor stays with its text");
    }

    #[test]
    fn cutting_a_selection_takes_only_it() {
        let mut e = ed("keep this part\n");
        e.goto(0, 4);
        for _ in 0..5 {
            e.select_right();
        }
        let (text, whole) = e.cut();
        assert_eq!((text.as_str(), whole), (" this", false));
        assert_eq!(e.to_text(), "keep part\n");
    }

    #[test]
    fn tab_and_shift_tab_move_selected_lines() {
        let mut e = ed("a\nb\nc\n");
        e.goto(0, 0);
        e.select_down();
        e.select_down();
        e.insert_tab(4);
        assert_eq!(e.to_text(), "    a\n    b\nc\n", "c is not selected into");
        assert_eq!(e.selected_text().as_deref(), Some("    a\n    b\n"));
        e.outdent(4);
        assert_eq!(e.to_text(), "a\nb\nc\n");
    }

    #[test]
    fn deleting_lines_at_several_cursors() {
        let mut e = ed("1\n2\n3\n4\n");
        e.goto(0, 0);
        e.add_cursor_vertically(true);
        e.add_cursor_vertically(true);
        e.delete_line();
        assert_eq!(e.to_text(), "4\n");
    }

    #[test]
    fn enter_at_several_cursors() {
        let mut e = ed("  ab\n  cd\n");
        e.goto(0, 3);
        e.add_cursor_vertically(true);
        e.insert_newline();
        assert_eq!(e.to_text(), "  a\n  b\n  c\n  d\n");
    }

    #[test]
    fn esc_lets_go_of_cursors_then_has_nothing_to_do() {
        let mut e = ed("a\nb\n");
        e.add_cursor_vertically(true);
        assert!(e.collapse());
        assert!(e.others.is_empty());
        assert!(!e.collapse());
    }
}
