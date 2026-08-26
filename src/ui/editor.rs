//! The file, with the real cursor in it.
//!
//! The one pane you can type into, and the only place a terminal cursor is
//! positioned. Three jobs that have to agree with each other: the gutter of
//! line numbers, the syntax-highlighted text beside it, and the horizontal
//! scroll that keeps the cursor on screen in a long line.
//!
//! # Formatting while you type
//!
//! Markdown is drawn formatted *as it is edited* — see [`live_rows`]. The line
//! the cursor is on stays raw, because you cannot put a cursor in the middle
//! of a heading that has had its `#` taken away and have the arrow keys still
//! mean anything. Every other line renders. The result is that a document
//! looks like itself while you work on it, which is what the reading mode this
//! replaced was for.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::parts::{FORMAT_LIMIT, digits, mark_query};

use crate::app::{App, TextKind};
use crate::config::Palette;
use crate::text::markdown;
use crate::text::search::Matcher;

/// Raw source with line numbers, syntax highlighting, and the real cursor.
///
/// Only the visible window is highlighted — see `highlight::highlight_window`
/// for why that is both correct and cheap.
///
/// The cursor is placed with `set_cursor_position` so the terminal's own
/// blinking cursor lands in the right cell, rather than being faked with a
/// reversed span. That means converting the character-indexed cursor column
/// into a *display width* offset, accounting for the horizontal scroll — which
/// is what the `before` / `skipped` width subtraction is doing.
pub(super) fn draw_editor(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    path: &std::path::Path,
    kind: TextKind,
    focused: bool,
    query: Option<&Matcher>,
) {
    let pal = app.palette;
    if !app.buffers.contains_key(path) {
        return;
    }
    let height = inner.height as usize;
    app.last_edit_height = height;

    let gutter = if app.config.line_numbers {
        digits(app.buffers[path].line_count()).max(2) + 1
    } else {
        0
    };
    let text_w = (inner.width as usize).saturating_sub(gutter);
    let live = kind == TextKind::Markdown && app.buffers[path].line_count() <= FORMAT_LIMIT;

    // Scrolling needs the pane size, so it is resolved here rather than in the
    // key handler, which has no idea how big the window is. Live preview does
    // its own vertical scrolling in drawn rows rather than source lines, since
    // the two stop being the same thing once a block is formatted.
    // `touched` comes out here because this is the one place the buffer is
    // held mutably, and asking clears it — so it has to be asked exactly once
    // per frame, whether or not anything ends up being highlighted.
    let (scroll_y, scroll_x, cur_line, cur_col, touched) = {
        let ed = app.buffers.get_mut(path).expect("checked above");
        ed.sync_scroll(text_w.saturating_sub(1), if live { 0 } else { height });
        let touched = ed.take_touched();
        (
            ed.scroll_y,
            ed.scroll_x,
            ed.cursor_line,
            ed.cursor_col,
            touched,
        )
    };

    let (rows, top) = if live {
        live_rows(app, path, text_w, height, scroll_y, cur_line)
    } else {
        let ed = &app.buffers[path];
        let rows = (scroll_y..ed.line_count()).map(Row::Raw).collect();
        (rows, 0)
    };
    if live {
        // Keep the anchor honest for the next frame: the source line at the
        // top of the pane is what `Editor` scrolls by.
        let line = rows.get(top).and_then(Row::line).unwrap_or(0);
        app.buffers.get_mut(path).expect("checked above").scroll_y = line;
    }

    let ed = &app.buffers[path];
    app.preview_len = ed.line_count();

    // Raw rows are one contiguous run — the block the cursor is in — so a
    // single highlight window covers all of them.
    let raw: Vec<usize> = rows
        .iter()
        .skip(top)
        .take(height)
        .filter_map(Row::raw_line)
        .collect();
    let first_line = ed.lines().first().map(String::as_str).unwrap_or("");
    let syntax = app.highlighter.syntax_for_path(path, first_line).clone();
    let highlighted = match (raw.first(), raw.last()) {
        (Some(&a), Some(&b)) => {
            // Resume from the last saved state above the window rather than
            // from line 0. This is the difference between a keystroke deep in
            // a long file taking a third of a second and taking two
            // milliseconds.
            app.highlight_cache.sync(path, &syntax.name, touched);
            app.highlighter.highlight_cached(
                &mut app.highlight_cache,
                ed.lines(),
                &syntax,
                a,
                b + 1 - a,
            )
        }
        _ => Vec::new(),
    };
    let piece_of = |n: usize| raw.first().and_then(|a| highlighted.get(n - a));

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut cursor_row = None;
    for (i, row) in rows.iter().skip(top).take(height).enumerate() {
        let mut spans: Vec<Span> = Vec::new();
        if gutter > 0 {
            // A formatted block wears its first source line and nothing else:
            // its rows do not correspond to source lines one for one, and a
            // made-up number is worse than a blank.
            let (label, style) = match row.line() {
                Some(n) => (
                    format!("{:>w$} ", n + 1, w = gutter - 1),
                    if n == cur_line && focused {
                        pal.text
                    } else {
                        pal.dim
                    },
                ),
                None => (" ".repeat(gutter), pal.dim),
            };
            spans.push(Span::styled(label, style));
        }
        // Built apart from the gutter so a query can be marked in the text
        // without ever landing on a line number.
        let mut content: Vec<Span> = Vec::new();
        match row {
            Row::Raw(n) => {
                if *n == cur_line {
                    cursor_row = Some(i);
                }
                if let Some(pieces) = piece_of(*n) {
                    content.extend(clip_pieces(pieces, scroll_x, text_w, pal));
                }
            }
            Row::Made { content: made, .. } => content.extend(made.spans.iter().cloned()),
        }
        if let Some(q) = query {
            content = mark_query(content, q);
        }
        spans.extend(content);
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).style(pal.text), inner);

    if focused {
        // Place the real terminal cursor, so it blinks where the user expects.
        // It is always in a raw block, so the column arithmetic is the plain
        // one — display width of the text before it, less the horizontal
        // scroll.
        let line = ed.lines().get(cur_line).map(String::as_str).unwrap_or("");
        let before: String = line.chars().take(cur_col).collect();
        let skipped: String = line.chars().take(scroll_x).collect();
        let vis = before.width().saturating_sub(skipped.width());
        let x = inner.x + gutter as u16 + vis as u16;
        let y = match cursor_row {
            Some(r) => inner.y + r as u16,
            None if live => return,
            None => inner.y + (cur_line.saturating_sub(scroll_y)) as u16,
        };
        if x < inner.x + inner.width && y < inner.y + inner.height {
            f.set_cursor_position((x, y));
        }
    }
}

/// One drawn row of a markdown buffer being edited.
enum Row {
    /// A source line, drawn raw: the block the cursor is in.
    Raw(usize),
    /// A rendered line. `line` carries the block's first source line on its
    /// first row and `None` after, so the gutter numbers the block once.
    Made {
        line: Option<usize>,
        content: Line<'static>,
    },
}

impl Row {
    /// The source line to number this row with, if any.
    fn line(&self) -> Option<usize> {
        match self {
            Row::Raw(n) => Some(*n),
            Row::Made { line, .. } => *line,
        }
    }

    fn raw_line(&self) -> Option<usize> {
        match self {
            Row::Raw(n) => Some(*n),
            Row::Made { .. } => None,
        }
    }
}

/// Lay a markdown buffer out for editing: every block formatted except the one
/// the cursor is in, which is shown as it was typed.
///
/// This is the Obsidian trick, and the reason it works is that a block is the
/// unit of both. Formatting is a property of the whole block — a fence, a
/// table, a list — so showing "the line under the cursor" raw and the rest of
/// its block formatted would render half-syntax. The cursor unformats what it
/// is inside, and the moment it leaves, the block comes back.
///
/// Returns the rows and the index of the first one to draw. Rows are built for
/// the whole buffer, then a window is chosen: the drawn height of a block is
/// not known until it is rendered, so there is nothing to scroll by until the
/// layout exists. [`FORMAT_LIMIT`] is what keeps that affordable.
fn live_rows(
    app: &App,
    path: &std::path::Path,
    width: usize,
    height: usize,
    scroll_y: usize,
    cur_line: usize,
) -> (Vec<Row>, usize) {
    let pal = app.palette;
    let ed = &app.buffers[path];
    let mut rows: Vec<Row> = Vec::new();
    let mut top = None;
    let mut cursor_row = 0;

    for b in markdown::blocks(ed.lines()) {
        if b.contains(cur_line) {
            for n in b.start..b.end {
                if n == cur_line {
                    cursor_row = rows.len();
                }
                if n >= scroll_y && top.is_none() {
                    top = Some(rows.len());
                }
                rows.push(Row::Raw(n));
            }
            continue;
        }
        if b.start >= scroll_y && top.is_none() {
            top = Some(rows.len());
        }
        let made =
            markdown::render_block(&ed.lines()[b.start..b.end], width, &pal, &app.highlighter);
        for (i, content) in made.into_iter().enumerate() {
            rows.push(Row::Made {
                line: (i == 0).then_some(b.start),
                content,
            });
        }
    }

    // The anchor is a source line, so it lands wherever that line's block
    // begins; from there the cursor has to be pulled into view in row space.
    let mut top = top.unwrap_or(0).min(rows.len().saturating_sub(1));
    if cursor_row < top {
        top = cursor_row;
    } else if height > 0 && cursor_row >= top + height {
        top = cursor_row + 1 - height;
    }
    (rows, top)
}

/// Apply the horizontal scroll to one highlighted line and clip it to width./// Apply the horizontal scroll to one highlighted line and clip it to width.
///
/// Walks the styled pieces twice over: first discarding `scroll_x` characters
/// from the left, then accumulating until the display width runs out. Both
/// halves have to respect piece boundaries, since a piece is the unit that
/// carries a style.
///
/// Always returns at least one span, so an empty or fully-scrolled line still
/// produces a `Line` and the row numbering stays aligned.
fn clip_pieces<'a>(
    pieces: &[(Style, String)],
    scroll_x: usize,
    width: usize,
    pal: Palette,
) -> Vec<Span<'a>> {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let mut used = 0usize;
    for (style, text) in pieces {
        let chars: Vec<char> = text.chars().collect();
        let mut start = 0;
        if skipped < scroll_x {
            let skip = (scroll_x - skipped).min(chars.len());
            skipped += skip;
            start = skip;
        }
        if start >= chars.len() {
            continue;
        }
        let mut piece = String::new();
        for c in &chars[start..] {
            let w = c.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            used += w;
            piece.push(*c);
        }
        if !piece.is_empty() {
            out.push(Span::styled(piece, *style));
        }
        if used >= width {
            break;
        }
    }
    if out.is_empty() {
        out.push(Span::styled(String::new(), pal.text));
    }
    out
}

// ---- the project map ------------------------------------------------------
