//! The file, with the real cursor in it.
//!
//! The one pane you can type into, and the only place a terminal cursor is
//! positioned. Three jobs that have to agree with each other: the gutter of
//! line numbers, the syntax-highlighted text beside it, and the horizontal
//! scroll that keeps the cursor on screen in a long line.
//!
//! # Formatting while you type
//!
//! Markdown is drawn formatted *as it is edited*, see [`live_rows`]. The line
//! the cursor is on stays raw, because you cannot put a cursor in the middle
//! of a heading that has had its `#` taken away and have the arrow keys still
//! mean anything. Every other line renders. The result is that a document
//! looks like itself while you work on it, which is what the reading mode this
//! replaced was for. Lines with text selected, or another cursor on them,
//! stay raw too, for the same reason.
//!
//! # Selections and the other cursors
//!
//! Selected text is drawn in the theme's `selection` style, and every cursor
//! but the main one as a block, since the terminal has only the one cursor of
//! its own to place. A selection that runs on past the end of a line takes
//! one cell after it, for the line break, so a selected empty line shows.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;

use super::parts::{FORMAT_LIMIT, digits, mark_query};

use crate::app::{App, TextKind};
use crate::config::Palette;
use crate::text::editor::Editor;
use crate::text::markdown;
use crate::text::search::Matcher;
use crate::text::tabs;

/// Raw source with line numbers, syntax highlighting, and the real cursor.
///
/// Only the visible window is highlighted, resuming from the saved parser
/// state above it, see `highlight::Highlighter::highlight_cached` for why
/// that is both correct and cheap.
///
/// The cursor is placed with `set_cursor_position` so the terminal's own
/// blinking cursor lands in the right cell, rather than being faked with a
/// reversed span. That means converting the character-indexed cursor column
/// into a *display width* offset, accounting for the horizontal scroll, which
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
    let tab_width = app.config.tab_width;

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
    // held mutably, and asking clears it, so it has to be asked exactly once
    // per frame, whether or not anything ends up being highlighted.
    let (scroll_y, scroll_x, cur_line, cur_col, touched) = {
        let ed = app.buffers.get_mut(path).expect("checked above");
        ed.sync_scroll(
            text_w.saturating_sub(1),
            if live { 0 } else { height },
            tab_width,
        );
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
        let rows_down = app.buffers[path].scroll_rows;
        let marked = marked_lines(&app.buffers[path]);
        live_rows(
            app,
            path,
            text_w,
            height,
            (scroll_y, rows_down),
            cur_line,
            &marked,
        )
    } else {
        // A screenful: on a hundred-thousand-line log, a row for every line
        // below the top of the pane was megabytes a keystroke for nothing.
        let ed = &app.buffers[path];
        let end = ed.line_count().min(scroll_y + height);
        let rows = (scroll_y..end).map(Row::Raw).collect();
        (rows, 0)
    };
    if live {
        // Keep the anchor honest for the next frame: where the top row of the
        // pane is, as a line and how far down its block.
        let (line, down) = rows.get(top).map(Row::anchor).unwrap_or((0, 0));
        let ed = app.buffers.get_mut(path).expect("checked above");
        ed.scroll_y = line;
        ed.scroll_rows = down;
    }

    let ed = &app.buffers[path];
    app.preview_len = ed.line_count();

    // Raw rows are one contiguous run (the block the cursor is in) so a
    // single highlight window covers all of them.
    let raw: Vec<usize> = rows
        .iter()
        .skip(top)
        .take(height)
        .filter_map(Row::raw_line)
        .collect();
    let first_line = ed.lines().first().map(String::as_str).unwrap_or("");
    // Borrowed, not cloned, see `highlight::Highlighter::syntax_for_path`.
    let syntax = app.highlighter.syntax_for_path(path, first_line);
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
                syntax,
                a,
                b + 1 - a,
            )
        }
        _ => Vec::new(),
    };
    let piece_of = |n: usize| raw.first().and_then(|a| highlighted.get(n - a));
    let marks = Marks::of(ed, &raw);

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
                    let pieces = marks.apply(*n, pieces, pal);
                    content.extend(clip_pieces(&pieces, scroll_x, text_w, pal, tab_width));
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
        // one: display width of the text before it, less the horizontal
        // scroll.
        let line = ed.lines().get(cur_line).map(String::as_str).unwrap_or("");
        // Columns, not characters: a tab was drawn as the spaces that reach
        // its stop, so the cursor has to be counted the same way or it blinks
        // somewhere the text is not.
        let vis = tabs::column(line, cur_col, tab_width).saturating_sub(scroll_x);
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
    /// A rendered line: the `nth` drawn from the block that starts at source
    /// line `block`. `line` carries that first line on the block's first row
    /// and `None` after, so the gutter numbers the block once.
    Made {
        line: Option<usize>,
        block: usize,
        nth: usize,
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

    /// Where this row is, as a scroll position: a source line, and how many
    /// rows down that line's block it is.
    fn anchor(&self) -> (usize, usize) {
        match self {
            Row::Raw(n) => (*n, 0),
            Row::Made { block, nth, .. } => (*block, *nth),
        }
    }
}

/// Lay a markdown buffer out for editing: every block formatted except the one
/// the cursor is in, which is shown as it was typed.
///
/// This is the Obsidian trick, and the reason it works is that a block is the
/// unit of both. Formatting is a property of the whole block (a fence, a
/// table, a list) so showing "the line under the cursor" raw and the rest of
/// its block formatted would render half-syntax. The cursor unformats what it
/// is inside, and the moment it leaves, the block comes back.
///
/// Returns the rows and the index of the first one to draw. Rows are built for
/// the whole buffer, then a window is chosen: the drawn height of a block is
/// not known until it is rendered, so there is nothing to scroll by until the
/// layout exists. [`FORMAT_LIMIT`] is what keeps that affordable.
///
/// The window is anchored where the last frame left it: a source line and a
/// number of rows down its block (`scroll_rows`), since a pane can begin part
/// way down a block of many rows. Anchored on the line alone, a pane that
/// began mid-block had no line to name, fell back to the top of the note, and
/// the next frame pulled the cursor to the bottom row, so Up scrolled the
/// text instead of moving the cursor.
fn live_rows(
    app: &App,
    path: &std::path::Path,
    width: usize,
    height: usize,
    (scroll_y, scroll_rows): (usize, usize),
    cur_line: usize,
    marked: &[(usize, usize)],
) -> (Vec<Row>, usize) {
    let ed = &app.buffers[path];
    let mut rows: Vec<Row> = Vec::new();
    let mut top = None;
    let mut cursor_row = 0;
    let labels = markdown::link_labels(ed.lines());

    for b in markdown::blocks(ed.lines()) {
        let touches = |&(first, last): &(usize, usize)| first < b.end && last >= b.start;
        if b.contains(cur_line) || marked.iter().any(touches) {
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
        let made = block_rows(
            app,
            &ed.lines()[b.start..b.end],
            width,
            &labels,
            b.start == 0,
        );
        if b.start >= scroll_y && top.is_none() {
            // Part way down this block, if that is where the pane began.
            let down = if b.start == scroll_y { scroll_rows } else { 0 };
            top = Some(rows.len() + down.min(made.len().saturating_sub(1)));
        }
        for (i, content) in made.into_iter().enumerate() {
            rows.push(Row::Made {
                line: (i == 0).then_some(b.start),
                block: b.start,
                nth: i,
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

/// The runs of lines, first and last, that a selection runs over or another
/// cursor is on: in a note, the blocks that have to stay raw for the marks
/// to be drawn on them.
fn marked_lines(ed: &Editor) -> Vec<(usize, usize)> {
    if ed.anchor.is_none() && ed.others.is_empty() {
        return Vec::new();
    }
    ed.cursors()
        .into_iter()
        .map(|(pos, sel)| sel.map_or((pos.line, pos.line), |(a, b)| (a.line, b.line)))
        .collect()
}

/// What is selected on the lines being drawn, and where the cursors other
/// than the terminal's own are on them, see the module docs.
struct Marks {
    lines: HashMap<usize, LineMarks>,
}

/// The marks on one line.
#[derive(Default)]
struct LineMarks {
    /// The ranges of characters selected, end not included, an end one past
    /// the line's last character taking in its line break.
    selected: Vec<(usize, usize)>,
    /// The columns other cursors are at.
    carets: Vec<usize>,
}

impl Marks {
    /// The marks on `shown`, the raw lines about to be drawn, first to last.
    fn of(ed: &Editor, shown: &[usize]) -> Self {
        let mut lines: HashMap<usize, LineMarks> = HashMap::new();
        let (Some(&first), Some(&last)) = (shown.first(), shown.last()) else {
            return Self { lines };
        };
        if ed.anchor.is_none() && ed.others.is_empty() {
            return Self { lines };
        }
        let main = ed.pos();
        for (pos, sel) in ed.cursors() {
            if pos != main && (first..=last).contains(&pos.line) {
                lines.entry(pos.line).or_default().carets.push(pos.col);
            }
            let Some((a, b)) = sel else {
                continue;
            };
            for n in a.line.max(first)..=b.line.min(last) {
                let from = if n == a.line { a.col } else { 0 };
                let to = if n == b.line {
                    b.col
                } else {
                    ed.lines()[n].chars().count() + 1
                };
                if to > from {
                    lines.entry(n).or_default().selected.push((from, to));
                }
            }
        }
        Self { lines }
    }

    /// Line `n`'s highlighted `pieces`, with its marks drawn on them.
    fn apply(&self, n: usize, pieces: &[(Style, String)], pal: Palette) -> Vec<(Style, String)> {
        let Some(LineMarks { selected, carets }) = self.lines.get(&n) else {
            return pieces.to_vec();
        };
        let len: usize = pieces.iter().map(|(_, t)| t.chars().count()).sum();
        // One cell past the end, when a cursor or a line break needs it.
        let past = selected.iter().any(|&(_, to)| to > len) || carets.contains(&len);
        let end = (pal.text, " ".to_string());
        let mut out: Vec<(Style, String)> = Vec::new();
        let mut i = 0;
        for (style, text) in pieces.iter().chain(past.then_some(&end)) {
            for c in text.chars() {
                let chosen = selected.iter().any(|&(a, b)| (a..b).contains(&i));
                let st = match (chosen, carets.contains(&i)) {
                    (false, false) => *style,
                    (true, false) => style.patch(pal.selection),
                    (false, true) => style.add_modifier(Modifier::REVERSED),
                    // A cursor inside a selection drawn reversed is the gap
                    // in it; in one drawn some other way, it is reversed.
                    (true, true) if pal.selection.add_modifier.contains(Modifier::REVERSED) => {
                        *style
                    }
                    (true, true) => style.patch(pal.selection).add_modifier(Modifier::REVERSED),
                };
                match out.last_mut() {
                    Some((last, text)) if *last == st => text.push(c),
                    _ => out.push((st, c.to_string())),
                }
                i += 1;
            }
        }
        out
    }
}

/// One block of a note, formatted, from the cache when it has not changed.
///
/// Every frame lays the whole note out, because the height of a block is not
/// known until it is formatted. Formatting each block again, its code fences
/// highlighted from scratch, is what made a keystroke in a long note slow; a
/// block not being typed in is the same text at the same width as the frame
/// before, and so the same rows.
///
/// The note's link labels are part of what a block is drawn from, so they
/// are part of the key: a definition added further down turns `[spec][1]`
/// into a link.
fn block_rows(
    app: &App,
    lines: &[String],
    width: usize,
    labels: &[(String, String)],
    top: bool,
) -> Vec<Line<'static>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.hash(&mut h);
    width.hash(&mut h);
    labels.hash(&mut h);
    top.hash(&mut h);
    let key = h.finish();
    if let Some(made) = app.block_rows.borrow().get(&key) {
        return made.clone();
    }
    let made = markdown::render_block(lines, width, &app.palette, &app.highlighter, labels, top);
    let mut cache = app.block_rows.borrow_mut();
    // Kept from growing without end as text is typed and deleted: whatever
    // is on screen is back in a frame.
    if cache.len() >= 4096 {
        cache.clear();
    }
    cache.insert(key, made.clone());
    made
}

/// One line's styled pieces, as much of them as fits between column
/// `scroll_x` and `scroll_x + width`.
///
/// Tabs become spaces as they are reached, each from the column the text
/// before it got to, since a tab's width is where it sits. Everything is
/// measured in columns, the way `scroll_x` and the cursor are, a grapheme
/// cluster at a time the way ratatui draws: a wide character cut by the left
/// edge shows as the spaces of its visible half, so nothing after it shifts,
/// and one that would cross the right edge is left off.
pub(super) fn clip_pieces<'a>(
    pieces: &[(Style, String)],
    scroll_x: usize,
    width: usize,
    pal: Palette,
    tab_width: usize,
) -> Vec<Span<'a>> {
    let end = scroll_x + width;
    let mut out = Vec::new();
    let mut col = 0usize;
    'pieces: for (style, text) in pieces {
        let text = tabs::expand(text, tab_width, col);
        let mut shown = String::new();
        for g in text.graphemes(true) {
            let at = col;
            col += tabs::cluster_width(g);
            if col <= scroll_x {
                continue;
            }
            if col > end {
                if !shown.is_empty() {
                    out.push(Span::styled(shown, *style));
                }
                break 'pieces;
            }
            if at < scroll_x {
                shown.extend(std::iter::repeat_n(' ', col - scroll_x));
            } else {
                shown.push_str(g);
            }
        }
        if !shown.is_empty() {
            out.push(Span::styled(shown, *style));
        }
    }
    if out.is_empty() {
        out.push(Span::styled(String::new(), pal.text));
    }
    out
}
