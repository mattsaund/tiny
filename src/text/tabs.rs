//! Tabs, as they have to be drawn.
//!
//! A terminal cell holds one character, and `unicode-width` calls a tab zero
//! columns wide, because how wide a tab is depends on where it sits. Left
//! alone it reaches the screen as a cell holding nothing: `a\tb` reads as
//! `ab`, and a Makefile loses the indentation it is made of.
//!
//! So nothing is drawn with a tab still in it. Every path that puts text on
//! screen expands them first, to the next multiple of the tab width, which is
//! what every editor means by a tab stop. The file is never touched: this is
//! a rule about drawing, not an edit.
//!
//! # Two widths, on purpose
//!
//! The editor expands to the `tab_width` setting, because that is the width
//! the person chose and the one their file was written against. Markdown
//! expands to [`MARKDOWN`], because CommonMark fixes a tab at four columns,
//! and a note should not re-flow because a setting changed.
//!
//! # Measured the way the screen draws
//!
//! Width is counted per grapheme cluster, which is what ratatui puts in a
//! cell, not per character. `⚠️` is a sign and a selector that asks for the
//! picture, two columns where the sign alone is one; `👩‍💻` is three
//! characters drawn as one two-column picture. Counted a character at a time,
//! each would put everything after it (a tab stop, the cursor) a cell or two
//! away from where it is drawn.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The tab stop CommonMark defines, for anything drawn as markdown.
pub const MARKDOWN: usize = 4;

/// How many columns a tab at `col` reaches across.
///
/// Never zero: a tab always moves at least one column, and a `tab_width` of
/// zero would otherwise expand to nothing and put the bug back.
pub fn stop(col: usize, tab_width: usize) -> usize {
    let w = tab_width.max(1);
    w - (col % w)
}

/// `text` with every tab replaced by the spaces that reach the next stop,
/// starting from column `start`.
///
/// A newline returns to column zero, so a chunk holding several lines expands
/// each of them against its own left edge rather than against the one before.
///
/// Text with no tab in it, which is nearly all of it, comes back borrowed:
/// this runs on every piece of every line drawn.
pub fn expand(text: &str, tab_width: usize, start: usize) -> Cow<'_, str> {
    if !text.contains('\t') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut col = start;
    for g in text.graphemes(true) {
        match g {
            "\t" => {
                let n = stop(col, tab_width);
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            "\n" | "\r\n" => {
                out.push_str(g);
                col = 0;
            }
            _ => {
                col += g.width();
                out.push_str(g);
            }
        }
    }
    Cow::Owned(out)
}

/// The column that `chars` characters of `line` reach, with tabs taken to
/// their stops.
///
/// This is what turns a cursor's character index into the cell it belongs in.
/// Drawing and the cursor have to agree about it, or the cursor blinks
/// somewhere the text is not. A cursor that has stopped part way through a
/// cluster is drawn at the cluster's start, where the picture is.
pub fn column(line: &str, chars: usize, tab_width: usize) -> usize {
    let mut col = 0;
    let mut seen = 0;
    for g in line.graphemes(true) {
        seen += g.chars().count();
        if seen > chars {
            break;
        }
        col += if g == "\t" {
            stop(col, tab_width)
        } else {
            g.width()
        };
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_reaches_the_next_stop_rather_than_a_fixed_width() {
        assert_eq!(expand("\tx", 4, 0), "    x");
        assert_eq!(expand("a\tb", 4, 0), "a   b", "three spaces, not four");
        assert_eq!(expand("abc\td", 4, 0), "abc d", "one space is enough");
        assert_eq!(expand("abcd\te", 4, 0), "abcd    e", "a full stop over");
    }

    #[test]
    fn expanding_continues_from_where_the_text_starts() {
        // A piece in the middle of a line is handed the column it begins at,
        // so its stops line up with the ones before it.
        assert_eq!(expand("\tx", 4, 2), "  x");
        assert_eq!(expand("\tx", 4, 3), " x");
    }

    #[test]
    fn each_line_of_a_chunk_starts_from_its_own_left_edge() {
        assert_eq!(expand("ab\tc\n\td", 4, 0), "ab  c\n    d");
    }

    #[test]
    fn a_column_is_counted_the_way_it_is_drawn() {
        assert_eq!(column("\tx", 1, 4), 4, "the tab alone");
        assert_eq!(column("\tx", 2, 4), 5, "and the x after it");
        assert_eq!(column("a\tb", 2, 4), 4, "a, then the tab to the stop");
        // Width, not character count: the rule the cursor has always used.
        assert_eq!(column("文a", 1, 4), 2, "a wide character is two columns");
    }

    #[test]
    fn text_with_no_tabs_is_returned_as_it_was() {
        assert_eq!(expand("plain text", 4, 0), "plain text");
        assert_eq!(column("plain", 5, 4), 5);
    }

    #[test]
    fn a_picture_made_of_several_characters_is_measured_as_drawn() {
        // A sign with the selector that makes it a picture, and three
        // characters joined into one: two columns each, as ratatui draws
        // them, so the tab after each still lands on the stop.
        assert_eq!(column("⚠️x", 2, 4), 2, "the sign and its selector");
        assert_eq!(column("👩‍💻x", 3, 4), 2, "one picture, three characters");
        assert_eq!(expand("⚠️\tx", 4, 0), "⚠️  x");
        // Part way through a picture, the cursor is drawn at its start.
        assert_eq!(column("👩‍💻x", 1, 4), 0);
    }

    #[test]
    fn a_tab_width_of_zero_still_moves() {
        // `tab_width` is a setting, and settings reach zero.
        assert_eq!(expand("\tx", 0, 0), " x");
    }
}
