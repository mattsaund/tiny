//! Fitting styled text into a fixed number of cells.
//!
//! The reason this is not `Paragraph::wrap`: every line here carries per-span
//! styling — a bold word inside a sentence, a link inside a bullet — and a
//! re-wrap that does not know where the spans end smears those styles across
//! the break. So the wrapping is done on the spans themselves, and a span that
//! straddles a break is cut in two with both halves keeping the style.
//!
//! Widths are measured with `unicode-width`, never `chars().count()`: a CJK
//! character is one character and two cells, and a wrap that confuses the two
//! overflows the pane.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Total display width of a run of spans, in terminal cells.
pub(super) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Truncate to a display width, respecting character boundaries.
///
/// Width, not character count: a double-width character is only kept if both
/// its cells fit, so clipping can never leave half a glyph behind.
pub(super) fn clip(s: &str, max_w: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max_w {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

/// Push a wrapped line, dropping whitespace that ran off the end of it.
///
/// Trailing spaces are invisible until they meet a reversed or backgrounded
/// style, at which point they draw as a bright tail hanging off the end of the
/// line. The `spans.len() > 1` guard keeps the prefix span, so an
/// indent-only line does not collapse to nothing.
pub(super) fn push_trimmed(lines: &mut Vec<Line<'static>>, mut spans: Vec<Span<'static>>) {
    while spans.len() > 1
        && spans
            .last()
            .is_some_and(|s| s.content.trim_end().is_empty())
    {
        spans.pop();
    }
    if let Some(last) = spans.last_mut()
        && (last.content.ends_with(' ') || last.content.ends_with('\t'))
    {
        *last = Span::styled(last.content.trim_end().to_string(), last.style);
    }
    lines.push(Line::from(spans));
}

/// Wrap a styled inline run to `width`, prefixing the first line with
/// `first_prefix` and every continuation line with `cont_prefix`.
///
/// The two prefixes are what make every indented construct work with one
/// function: a list item passes its bullet as `first_prefix` and matching
/// spaces as `cont_prefix`, so wrapped text lines up under the item's text
/// rather than under its bullet. A blockquote passes its `│ ` gutter as both.
///
/// # How it works
///
/// 1. Flatten the styled spans into alternating word and whitespace tokens,
///    each remembering the style it came from. This is why styling survives a
///    break: the token is re-emitted with its own style on whichever line it
///    lands.
/// 2. Place tokens one at a time. A word that will not fit starts a new line.
/// 3. Whitespace is dropped at a break rather than carried over, so a wrapped
///    line never begins with the space that caused the wrap.
/// 4. A single token wider than the pane — a long URL or path — is hard-broken
///    across lines, since there is no break opportunity inside it.
///
/// `indent_w` tracks the current line's prefix width and exists to stop rule
/// 2 firing on a line that holds nothing but its own indent, which would loop
/// forever.
pub(super) fn wrap(
    spans: &[Span<'static>],
    width: usize,
    first_prefix: Vec<Span<'static>>,
    cont_prefix: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    let first_w = spans_width(&first_prefix);
    let cont_w = spans_width(&cont_prefix);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur = first_prefix;
    let mut cur_w = first_w;
    let mut indent_w = first_w;

    // Flatten into whitespace/word tokens, carrying each token's style.
    let mut tokens: Vec<(Style, String)> = Vec::new();
    for s in spans {
        let mut buf = String::new();
        let mut buf_space = false;
        for c in s.content.chars() {
            let is_space = c == ' ' || c == '\t';
            if !buf.is_empty() && is_space != buf_space {
                tokens.push((s.style, std::mem::take(&mut buf)));
            }
            buf_space = is_space;
            buf.push(c);
        }
        if !buf.is_empty() {
            tokens.push((s.style, buf));
        }
    }

    for (style, tok) in tokens {
        let tw = tok.width();
        let is_space = tok.starts_with(' ') || tok.starts_with('\t');
        if is_space {
            // Never start a wrapped line with the space that caused the break,
            // and never let a trailing space push the line past the width.
            if cur_w > indent_w && cur_w + tw <= width {
                cur.push(Span::styled(tok, style));
                cur_w += tw;
            }
            continue;
        }
        if cur_w + tw > width && cur_w > indent_w {
            push_trimmed(&mut lines, std::mem::replace(&mut cur, cont_prefix.clone()));
            cur_w = cont_w;
            indent_w = cont_w;
        }
        if cur_w + tw > width && tw > width.saturating_sub(indent_w) {
            // A single token wider than the pane (a long URL): hard-break it.
            let mut rest = tok.as_str();
            while !rest.is_empty() {
                let room = width.saturating_sub(cur_w).max(1);
                let chunk = clip(rest, room);
                if chunk.is_empty() {
                    break;
                }
                cur_w += chunk.width();
                rest = &rest[chunk.len()..];
                cur.push(Span::styled(chunk, style));
                if !rest.is_empty() {
                    push_trimmed(&mut lines, std::mem::replace(&mut cur, cont_prefix.clone()));
                    cur_w = cont_w;
                    indent_w = cont_w;
                }
            }
        } else {
            cur.push(Span::styled(tok, style));
            cur_w += tw;
        }
    }
    push_trimmed(&mut lines, cur);
    lines
}
