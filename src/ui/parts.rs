//! The small pieces every pane is built from.
//!
//! Nothing here draws a pane. These are the shared decisions — what a border
//! looks like, what "selected" looks like, how a match is marked, how a number
//! is written down — pulled out so that the answer is the same in the tree, in
//! the results list, in the settings window and in the editor. When a pane
//! disagrees with another about one of these, it is because it stopped calling
//! through here.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

/// Markdown longer than this is shown as its own source, in both panes.
///
/// Formatting is linear in the length of the file and is redone on every
/// frame — the preview renders the whole document, the editor re-renders every
/// block outside the cursor's. At four thousand lines that is a few
/// milliseconds, which is where a keystroke starts to feel slow.
///
/// The two panes share the number deliberately. A file the editor would not
/// format should not be formatted while you hover it either, or Enter would
/// change how the document looks rather than only where the cursor is.
pub(super) const FORMAT_LIMIT: usize = 4000;

use crate::app::App;
use crate::config::Palette;
use crate::text::search::Matcher;

/// A pane frame. With borders off, the title is drawn on its own line and the
/// box is dropped entirely — a quieter screen for people who want one.
///
/// Returns the *inner* rectangle to draw content into, so callers do not have
/// to know which of the two modes is in effect. Every pane goes through here,
/// which is what keeps `borders = false` from needing a second code path in
/// each drawing function.
pub(super) fn pane<'a>(
    f: &mut Frame,
    area: Rect,
    title: Vec<Span<'a>>,
    focused: bool,
    app: &App,
) -> Rect {
    let pal = app.palette;
    let style = if focused {
        pal.border_focus
    } else {
        pal.border
    };
    if !app.config.borders {
        if area.height == 0 {
            return area;
        }
        let head = Rect { height: 1, ..area };
        f.render_widget(Paragraph::new(Line::from(title)), head);
        return Rect {
            y: area.y + 1,
            height: area.height - 1,
            ..area
        };
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Line::from(title));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

// ---- tree pane ------------------------------------------------------------

/// Extend a row's styling across the pane so the cursor reads as a row, not
/// just as colored text.
///
/// Pads to the full width first, then restyles every span — otherwise a
/// reversed selection would stop at the end of the text and look like a
/// highlighted word rather than a selected line.
///
/// The row's own colors are *replaced*, not patched over. Patching kept each
/// span's foreground, and the default palette dims the indent and the fold
/// marker — so under reverse video that foreground became a background and the
/// left of every selected row was a grey block with the name in white beside
/// it. One style across the whole row is what makes it read as one row.
///
/// An unfocused pane still shows its cursor, dimmed, so you can see where you
/// will land when focus comes back.
pub(super) fn highlight_row(spans: &mut Vec<Span>, width: usize, pal: Palette, focused: bool) {
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    let style = if focused {
        pal.text.patch(pal.selection)
    } else {
        // An unfocused pane still shows where the cursor is, but quietly.
        pal.text.patch(pal.dim)
    };
    for s in spans.iter_mut() {
        *s = Span::styled(s.content.clone(), style);
    }
}

// ---- search results -------------------------------------------------------

/// Re-cut drawn spans so every occurrence of the search query stands out.
///
/// Used by both sides of the screen: the result list marks the snippet it
/// shows, and the preview marks the same word where it actually lives. One
/// function rather than two is what keeps a match looking the same in both
/// places, and the run is drawn reversed so it stands out in any terminal
/// theme without the palette having to name a highlight color.
///
/// Works on already-drawn spans rather than on source text, which is what lets
/// it serve both — rendered markdown has no source line to match against, and
/// the editor's spans have already been clipped to the horizontal scroll.
/// Splitting is by character index, matching how [`Matcher::all`] reports, and
/// a match that runs across a span boundary is marked in both halves.
///
/// Styling is added to whatever each piece already had, so syntax highlighting
/// survives underneath the mark.
pub(super) fn mark_query(spans: Vec<Span<'static>>, query: &Matcher) -> Vec<Span<'static>> {
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = query.all(&text);
    if ranges.is_empty() {
        return spans;
    }

    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + ranges.len() * 2);
    let mut push = |chars: &[char], style: Style| {
        if !chars.is_empty() {
            out.push(Span::styled(chars.iter().collect::<String>(), style));
        }
    };
    // `at` is the character position of the current span within the whole
    // line; `next` is the first range not yet dealt with.
    let mut at = 0usize;
    let mut next = 0usize;
    for span in spans {
        let chars: Vec<char> = span.content.chars().collect();
        let len = chars.len();
        let mut cut = 0usize;
        while next < ranges.len() && ranges[next].0 < at + len {
            let (start, end) = ranges[next];
            let from = start.saturating_sub(at);
            let to = (end - at).min(len);
            push(&chars[cut..from], span.style);
            push(
                &chars[from..to],
                span.style.add_modifier(Modifier::REVERSED),
            );
            cut = to;
            // A range that overruns this span is left for the next one.
            if end <= at + len {
                next += 1;
            } else {
                break;
            }
        }
        push(&chars[cut..], span.style);
        at += len;
    }
    out
}

// ---- preview pane ---------------------------------------------------------

/// The first row to draw so that `selected` is on screen, scrolling no further
/// than it has to.
///
/// Used by the lists that are rebuilt from scratch each frame — search results,
/// the settings, the keybinds. The tree keeps an offset of its own instead,
/// because there the cursor is the thing being moved around a list that stays
/// put, and remembering where you were reading matters.
pub(super) fn keep_visible(selected: usize, height: usize, total: usize) -> usize {
    selected
        .saturating_sub(height.saturating_sub(1))
        .min(total.saturating_sub(height))
}

/// A centred rectangle for an overlay, shrunk to fit if the window is smaller
/// than the requested size.
pub(super) fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// Filename for a pane title, falling back to the whole path for anything
/// without one.
pub(super) fn label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Decimal digit count, used to size the line-number gutter so it is exactly
/// wide enough for the longest number in the file.
pub(super) fn digits(n: usize) -> usize {
    let mut n = n.max(1);
    let mut d = 0;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}

/// Split a string at a character index, for drawing a text cursor. Character
/// index, not byte offset — slicing on a raw cursor position would panic on
/// any non-ASCII input.
pub(super) fn split_at_char(s: &str, ci: usize) -> (String, String) {
    let b = s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b);
    (s[..b].to_string(), s[b..].to_string())
}

/// Byte count as `B` / `KB` / `MB` / …, with one decimal place above a
/// kilobyte. Exact bytes below that, since "0.4 KB" tells you less than "412 B".
pub(super) fn readable_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_counts_correctly() {
        assert_eq!(digits(0), 1);
        assert_eq!(digits(9), 1);
        assert_eq!(digits(10), 2);
        assert_eq!(digits(1234), 4);
    }

    #[test]
    fn readable_size_scales() {
        assert_eq!(readable_size(0), "0 B");
        assert_eq!(readable_size(512), "512 B");
        assert_eq!(readable_size(2048), "2.0 KB");
        assert_eq!(readable_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn split_at_char_respects_multibyte() {
        let (a, b) = split_at_char("héllo", 2);
        assert_eq!((a.as_str(), b.as_str()), ("hé", "llo"));
    }

    #[test]
    fn marking_leaves_the_text_alone_and_reverses_every_match() {
        let pal = Palette::default();
        let q = Matcher::new("widget").unwrap();
        let spans = mark_query(
            vec![Span::styled(
                "the widget plan, one widget".to_string(),
                pal.text,
            )],
            &q,
        );
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            text, "the widget plan, one widget",
            "no characters are lost"
        );
        assert_eq!(marked(&spans), ["widget", "widget"]);
    }

    #[test]
    fn marking_spans_a_match_that_straddles_two_of_them() {
        let pal = Palette::default();
        // What syntax highlighting does all the time: one word, several spans.
        let q = Matcher::new("widget").unwrap();
        let spans = mark_query(
            vec![
                Span::styled("a wid".to_string(), pal.text),
                Span::styled("get b".to_string(), pal.heading),
            ],
            &q,
        );
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "a widget b");
        assert_eq!(marked(&spans), ["wid", "get"], "marked in both halves");
    }

    #[test]
    fn marking_a_line_with_no_match_in_it_changes_nothing() {
        let pal = Palette::default();
        let q = Matcher::new("widget").unwrap();
        let spans = mark_query(vec![Span::styled("short".to_string(), pal.text)], &q);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "short");
        assert!(marked(&spans).is_empty());
    }

    /// The text of every span drawn reversed.
    fn marked<'a>(spans: &'a [Span<'a>]) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect()
    }
}
