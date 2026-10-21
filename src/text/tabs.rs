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

use unicode_width::UnicodeWidthChar;

/// The tab stop CommonMark defines, for anything drawn as markdown.
pub const MARKDOWN: usize = 4;

/// How many columns a tab at `col` reaches across.
///
/// Never zero: a tab always moves at least one column, and a `tab_width` of
/// zero would otherwise expand to nothing and put the bug back.
fn stop(col: usize, tab_width: usize) -> usize {
    let w = tab_width.max(1);
    w - (col % w)
}

/// `text` with every tab replaced by the spaces that reach the next stop,
/// starting from column `start`.
///
/// A newline returns to column zero, so a chunk holding several lines expands
/// each of them against its own left edge rather than against the one before.
pub fn expand(text: &str, tab_width: usize, start: usize) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut col = start;
    for c in text.chars() {
        match c {
            '\t' => {
                let n = stop(col, tab_width);
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            '\n' => {
                out.push(c);
                col = 0;
            }
            _ => {
                col += c.width().unwrap_or(0);
                out.push(c);
            }
        }
    }
    out
}

/// The column that `chars` characters of `line` reach, with tabs taken to
/// their stops.
///
/// This is what turns a cursor's character index into the cell it belongs in.
/// Drawing and the cursor have to agree about it, or the cursor blinks
/// somewhere the text is not.
pub fn column(line: &str, chars: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for c in line.chars().take(chars) {
        match c {
            '\t' => col += stop(col, tab_width),
            _ => col += c.width().unwrap_or(0),
        }
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
    fn a_tab_width_of_zero_still_moves() {
        // `tab_width` is a setting, and settings reach zero.
        assert_eq!(expand("\tx", 0, 0), " x");
    }
}
