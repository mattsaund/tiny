//! Finishing a word with Tab.
//!
//! Tab at the end of a word finishes it with a longer word from the same
//! file, and Tab again puts in the next one instead, round to the word as it
//! was typed. The words come nearest first, so the name used on the line
//! above comes before one used a thousand lines away: the word wanted is
//! nearly always one just written.
//!
//! Only where Tab would otherwise put spaces after a word. At the start of a
//! line, after a space, with text selected or with more cursors than one,
//! Tab is what it always was, and so it is when nothing in the file starts
//! with what has been typed.
//!
//! The match keeps the case typed, so `Wid` finishes as `Widget` and never
//! as `widget`. Only when that finds nothing, and the word typed so far is
//! all lowercase, is case ignored: `wid` can finish as `Widget`, whose
//! capital then replaces what was typed.

use super::{EditKind, Editor, Pos};
use crate::text::search::is_word_char;

/// The most words offered for one Tab, nearest first.
const MOST: usize = 50;

/// A word Tab is finishing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Completing {
    /// Where the word begins.
    start: Pos,
    /// What had been typed of it, the last stop of the round.
    typed: String,
    /// What it can be finished as.
    words: Vec<String>,
    /// Which of them is in the text now; `words.len()` is the typed word.
    shown: usize,
    /// Where the cursor was left, so a Tab can tell nothing has happened
    /// since.
    at: Pos,
}

impl Editor {
    /// Tab: finish the word before the cursor, or put in the next way of
    /// finishing it. Returns the word now in the text, which of the ways it
    /// is (counting from one, and none for the word as it was typed), and
    /// how many there are; `None` when there is nothing to finish, and Tab
    /// should do what it always did.
    pub fn complete(&mut self) -> Option<(String, usize, usize)> {
        if self.anchor.is_some_and(|a| a != self.pos()) || !self.others.is_empty() {
            return None;
        }
        let (mut c, next) = match self.completing.take() {
            // The same place as the last Tab left it: the next way round.
            Some(c) if c.at == self.pos() => {
                let next = (c.shown + 1) % (c.words.len() + 1);
                (c, next)
            }
            _ => (self.start_completing()?, 0),
        };
        c.shown = next;
        let word = c
            .words
            .get(c.shown)
            .cloned()
            .unwrap_or_else(|| c.typed.clone());
        // An edit like typing, so a word and its finishing undo together.
        self.push_undo(EditKind::Insert);
        let end = self.splice(c.start, c.at, &word);
        self.goal_col = end.col;
        c.at = end;
        let which = if c.shown < c.words.len() {
            c.shown + 1
        } else {
            0
        };
        let of = c.words.len();
        self.completing = Some(c);
        Some((word, which, of))
    }

    /// The word before the cursor and what it could be finished as, when
    /// there is any.
    fn start_completing(&self) -> Option<Completing> {
        let here = self.pos();
        let line: Vec<char> = self.lines[here.line].chars().collect();
        // At the end of a word: one being typed into the middle of is not one
        // to finish.
        if line.get(here.col).is_some_and(|c| is_word_char(*c)) {
            return None;
        }
        let start = (0..here.col)
            .rev()
            .take_while(|&i| is_word_char(line[i]))
            .last()?;
        let typed: String = line[start..here.col].iter().collect();
        let start = Pos::new(here.line, start);
        let mut words = self.words_like(&typed, start, false);
        if words.is_empty() && !typed.chars().any(char::is_uppercase) {
            words = self.words_like(&typed, start, true);
        }
        (!words.is_empty()).then_some(Completing {
            start,
            typed,
            words,
            shown: 0,
            at: here,
        })
    }

    /// Every word in the file longer than `typed` that starts with it (in
    /// any case, with `fold`), nearest to `here` first, each once. The word
    /// at `here` itself is the one being typed, and is left out.
    fn words_like(&self, typed: &str, here: Pos, fold: bool) -> Vec<String> {
        let lowered = typed.to_lowercase();
        let starts = |w: &str| {
            if fold {
                w.to_lowercase().starts_with(&lowered)
            } else {
                w.starts_with(typed)
            }
        };
        // Lines outward from the cursor's: its own, then one above and one
        // below, and so on.
        let n = self.lines.len();
        let order = (0..n).flat_map(|d| {
            let above = here.line.checked_sub(d);
            let below = (d > 0).then_some(here.line + d).filter(|l| *l < n);
            above.into_iter().chain(below)
        });
        let mut out: Vec<String> = Vec::new();
        for l in order {
            let mut found: Vec<(usize, String)> = Vec::new();
            let chars: Vec<char> = self.lines[l].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if !is_word_char(chars[i]) {
                    i += 1;
                    continue;
                }
                let begin = i;
                while i < chars.len() && is_word_char(chars[i]) {
                    i += 1;
                }
                let word: String = chars[begin..i].iter().collect();
                let itself = l == here.line && begin == here.col;
                if !itself && word.chars().count() > typed.chars().count() && starts(&word) {
                    found.push((begin.abs_diff(here.col), word));
                }
            }
            // On one line, nearest the cursor first too.
            found.sort_by_key(|(d, _)| *d);
            for (_, word) in found {
                if !out.contains(&word) {
                    out.push(word);
                    if out.len() == MOST {
                        return out;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ed(text: &str) -> Editor {
        Editor::from_str(PathBuf::from("/tmp/t.rs"), text)
    }

    fn at_end_of(e: &mut Editor, line: usize) {
        e.goto(line, usize::MAX);
    }

    #[test]
    fn tab_finishes_a_word_from_the_file_and_again_gives_the_next() {
        let mut e = ed("let widget = 1;\nlet width = 2;\nwid\n");
        at_end_of(&mut e, 2);
        let (word, which, of) = e.complete().unwrap();
        assert_eq!(
            (word.as_str(), which, of),
            ("width", 1, 2),
            "the nearer first"
        );
        assert_eq!(e.lines()[2], "width");
        assert_eq!(e.complete().unwrap().0, "widget");
        assert_eq!(e.lines()[2], "widget");
        // Round to what was typed, then round again.
        assert_eq!(e.complete().unwrap().0, "wid");
        assert_eq!(e.complete().unwrap().0, "width");
    }

    #[test]
    fn nothing_to_finish_leaves_tab_as_it_was() {
        let mut e = ed("alpha\nbeta\nzz\n");
        at_end_of(&mut e, 2);
        assert_eq!(e.complete(), None, "nothing starts with zz");
        e.goto(0, 0);
        assert_eq!(e.complete(), None, "nothing typed yet");
        e.goto(0, 2);
        assert_eq!(e.complete(), None, "in the middle of a word");
        assert!(!e.dirty, "and none of it changed anything");
    }

    #[test]
    fn moving_away_starts_afresh() {
        let mut e = ed("width widget\nwi\nwid\n");
        at_end_of(&mut e, 1);
        e.complete();
        at_end_of(&mut e, 2);
        assert_eq!(e.complete().unwrap().0, "width", "a new word, a new round");
        assert_eq!(e.lines()[1], "width", "the last one stays as it was put in");
    }

    #[test]
    fn the_case_typed_is_kept_unless_nothing_matches_it() {
        let mut e = ed("Widget wide\nWid\nwidg\n");
        at_end_of(&mut e, 1);
        assert_eq!(e.complete().unwrap().0, "Widget", "not wide");
        at_end_of(&mut e, 2);
        assert_eq!(
            e.complete().unwrap().0,
            "Widget",
            "lowercase may find a capital"
        );
        assert_eq!(e.lines()[2], "Widget");
    }

    #[test]
    fn a_finished_word_undoes_with_its_typing() {
        let mut e = ed("together\n\n");
        e.goto(1, 0);
        for c in "tog".chars() {
            e.insert_char(c);
        }
        e.complete();
        assert_eq!(e.lines()[1], "together");
        assert!(e.undo());
        assert_eq!(e.lines()[1], "", "the word and its typing, in one step");
    }

    #[test]
    fn tab_with_a_selection_or_more_cursors_is_left_alone() {
        let mut e = ed("width\nwid\nwid\n");
        at_end_of(&mut e, 1);
        e.select_left();
        assert_eq!(e.complete(), None);
        e.click(Pos::new(1, 3));
        e.add_cursor_vertically(true);
        assert_eq!(e.complete(), None);
    }
}
