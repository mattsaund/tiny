//! All drawing. The split is deliberate: `app` never touches ratatui types
//! beyond styles, and this module never mutates state except scroll offsets
//! and the media cache, which can only be resolved once the pane size is
//! known.
//!
//! Nothing here picks a colour. Every style comes from the palette, whose
//! defaults name no colour at all — so tiny renders in whatever the terminal
//! is already using, and meaning is carried by weight, underline and reverse.
//!
//! # Reading order
//!
//! [`draw`] is the entry point and the map of everything below it:
//!
//! ```text
//! draw
//!  ├─ bar / status rows, placed by config (either can sit top or bottom)
//!  ├─ draw_graph            when the graph is open — takes the whole screen
//!  └─ two panes, side decided by config
//!      ├─ draw_tree  or  draw_results   (results replace the tree while searching)
//!      └─ draw_preview
//!           ├─ draw_reading   rendered markdown or wrapped prose
//!           ├─ draw_editor    raw source, with the real cursor
//!           └─ draw_media     pictures and poster frames
//!  └─ draw_help / draw_settings    overlays, drawn last so they sit on top
//! ```
//!
//! # `&mut App`, and why
//!
//! Most of these take `&mut App` even though drawing should be a pure read.
//! Three things genuinely cannot be computed until the pane size is known, and
//! the pane size is only known here:
//!
//! - **Scroll offsets.** The key handler that moved a cursor has no idea how
//!   tall the pane is, and the answer changes on every terminal resize.
//! - **`preview_len`.** How far the preview can scroll depends on how many
//!   lines the content produced at *this* width, which means rendering it.
//! - **The media cache.** An image is decoded to fit the pane, so the request
//!   is only well-formed once the pane exists.
//!
//! Everything else must stay read-only. If you find yourself wanting to set
//! application state from a draw function, it almost certainly belongs in
//! `app` instead.
//!
//! # Styling rules
//!
//! Nothing here writes a literal colour. Every style comes from
//! `app.palette`, and the shipped palette names no colour at all — meaning is
//! carried by bold, dim, underline and reverse, so tiny inherits the user's
//! terminal theme. The one exception is syntax highlighting, whose colours
//! come from the syntect theme by design.
//!
//! Widths are measured with `unicode-width`, never `chars().count()`. A CJK
//! character is one character and two cells, and confusing the two is how
//! borders end up misaligned.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Bar, BarKind, Focus, Mode, Preview, Settings, TextKind};
use crate::config::{Config, Palette, Position, Side};
use crate::graph::EdgeKind;
use crate::graphview::GraphView;
use crate::markdown;
use crate::search::HitKind;

/// Paint one frame. Called once per keypress by the event loop in `main`.
///
/// The vertical layout is assembled rather than hardcoded, because the bar and
/// the status line can each be configured to either end of the window: `order`
/// collects the slots in their final top-to-bottom sequence, then constraints
/// are derived from it. The main area is whatever is left.
///
/// Overlays (help, settings) are drawn against the *full* screen area rather
/// than the main region, so they float above the panes.
pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let pal = app.palette;

    // Rows: the bar when open, the panes, and the status line. Both the bar
    // and the status line can sit at either end.
    let bar_open = matches!(app.mode, Mode::Bar(_));
    let bar_h = if bar_open { 1 } else { 0 };
    let mut order: Vec<(Slot, u16)> = Vec::new();
    if bar_open && app.config.search_position == Position::Top {
        order.push((Slot::Bar, bar_h));
    }
    if app.config.status_position == Position::Top {
        order.push((Slot::Status, 1));
    }
    order.push((Slot::Main, 0));
    if bar_open && app.config.search_position == Position::Bottom {
        order.push((Slot::Bar, bar_h));
    }
    if app.config.status_position == Position::Bottom {
        order.push((Slot::Status, 1));
    }

    let constraints: Vec<Constraint> = order
        .iter()
        .map(|(slot, h)| match slot {
            Slot::Main => Constraint::Min(3),
            _ => Constraint::Length(*h),
        })
        .collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut main = area;
    for (i, (slot, _)) in order.iter().enumerate() {
        match slot {
            Slot::Main => main = rows[i],
            Slot::Bar => draw_bar(f, app, rows[i]),
            Slot::Status => draw_status(f, app, rows[i]),
        }
    }

    if app.graph_view.is_some() {
        draw_graph(f, app, main);
        return;
    }

    // A wider list while searching: results are longer than file names.
    let share = if bar_open && matches!(&app.mode, Mode::Bar(b) if b.kind == BarKind::Search) {
        app.config.tree_width.max(0.42)
    } else {
        app.config.tree_width
    };
    let side_w = ((main.width as f32) * share).round() as u16;
    let side_w = side_w.clamp(14, main.width.saturating_sub(12).max(14));

    let (left, right) = match app.config.tree_side {
        Side::Left => (Constraint::Length(side_w), Constraint::Min(10)),
        Side::Right => (Constraint::Min(10), Constraint::Length(side_w)),
    };
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([left, right])
        .split(main);
    let (tree_area, preview_area) = match app.config.tree_side {
        Side::Left => (panes[0], panes[1]),
        Side::Right => (panes[1], panes[0]),
    };

    match &app.mode {
        Mode::Bar(b) if b.kind == BarKind::Search => {
            let b = b.clone();
            draw_results(f, app, tree_area, &b);
        }
        _ => draw_tree(f, app, tree_area),
    }
    draw_preview(f, app, preview_area);

    match &app.mode {
        Mode::Help(scroll) => draw_help(f, area, &pal, *scroll),
        Mode::Settings(s) => {
            let s = s.clone();
            draw_settings(f, app, area, &s);
        }
        _ => {}
    }
}

/// A horizontal band of the window, used to build the vertical layout in the
/// order the config asks for.
enum Slot {
    Bar,
    Main,
    Status,
}

/// A pane frame. With borders off, the title is drawn on its own line and the
/// box is dropped entirely — a quieter screen for people who want one.
///
/// Returns the *inner* rectangle to draw content into, so callers do not have
/// to know which of the two modes is in effect. Every pane goes through here,
/// which is what keeps `borders = false` from needing a second code path in
/// each drawing function.
fn pane<'a>(f: &mut Frame, area: Rect, title: Vec<Span<'a>>, focused: bool, app: &App) -> Rect {
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

/// The project tree. Draws a window of `app.rows` around the cursor.
///
/// Scrolling is resolved here — this is one of the three sanctioned mutations
/// (see the module docs): `tree_scroll` is nudged the minimum needed to keep
/// the selected row on screen, so the list stays still while the cursor moves
/// within the visible region. `last_tree_height` is recorded for the key
/// handler's page-up/page-down.
fn draw_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.palette;
    let focused = app.focus == Focus::Tree && matches!(app.mode, Mode::Normal | Mode::Help(_));
    let title = vec![
        Span::raw(" "),
        Span::styled(
            "PROJECT",
            if focused {
                pal.text.add_modifier(Modifier::BOLD)
            } else {
                pal.dim
            },
        ),
        Span::raw(" "),
    ];
    let inner = pane(f, area, title, focused, app);

    let height = inner.height as usize;
    app.last_tree_height = height;
    if height == 0 || inner.width == 0 {
        return;
    }

    if app.selected < app.tree_scroll {
        app.tree_scroll = app.selected;
    } else if app.selected >= app.tree_scroll + height {
        app.tree_scroll = app.selected + 1 - height;
    }
    app.tree_scroll = app.tree_scroll.min(app.rows.len().saturating_sub(height));

    let (open, closed, leaf) = app.tree_markers();
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (i, row) in app
        .rows
        .iter()
        .enumerate()
        .skip(app.tree_scroll)
        .take(height)
    {
        let marker = if row.is_dir {
            if row.expanded { open } else { closed }
        } else {
            leaf
        };
        let name_style = if row.unreadable {
            pal.dim
        } else if row.is_dir {
            pal.directory
        } else {
            pal.text
        };
        let mut spans = vec![
            Span::styled("  ".repeat(row.depth), pal.dim),
            Span::styled(marker, pal.dim),
            Span::styled(row.name.clone(), name_style),
        ];
        if app.is_dirty(&row.path) {
            spans.push(Span::styled(" *", pal.marker));
        }
        if i == app.selected {
            highlight_row(&mut spans, width, pal, focused);
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Extend a row's styling across the pane so the cursor reads as a row, not
/// just as coloured text.
///
/// Pads to the full width first, then patches the selection style over every
/// span — otherwise a reversed selection would stop at the end of the text and
/// look like a highlighted word rather than a selected line.
///
/// An unfocused pane still shows its cursor, dimmed, so you can see where you
/// will land when focus comes back.
fn highlight_row(spans: &mut Vec<Span>, width: usize, pal: Palette, focused: bool) {
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    let sel = if focused {
        pal.selection
    } else {
        // An unfocused pane still shows where the cursor is, but quietly.
        pal.dim
    };
    for s in spans.iter_mut() {
        *s = Span::styled(s.content.clone(), s.style.patch(sel));
    }
}

// ---- search results -------------------------------------------------------

/// Search results, drawn in the tree's place while the search bar is open.
///
/// Replacing the tree rather than opening a third pane is why `draw` widens
/// the side pane during a search: result lines carry a path, a line number and
/// a snippet, and need considerably more room than a filename.
///
/// Note that scrolling is computed into a local, not written back to `b` — the
/// bar is borrowed immutably here, and the offset is cheap to recompute each
/// frame.
fn draw_results(f: &mut Frame, app: &mut App, area: Rect, b: &Bar) {
    let pal = app.palette;
    let title = vec![
        Span::raw(" "),
        Span::styled(
            format!(
                "{} MATCH{}",
                b.results.len(),
                if b.results.len() == 1 { "" } else { "ES" }
            ),
            pal.text.add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    let inner = pane(f, area, title, true, app);
    let height = inner.height as usize;
    if height == 0 || inner.width == 0 {
        return;
    }

    if b.results.is_empty() {
        let msg = if b.searched {
            "no matches"
        } else {
            "type to search names and contents"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, pal.dim))),
            inner,
        );
        return;
    }

    // Keep the highlighted result on screen.
    let mut scroll = b.scroll;
    if b.selected < scroll {
        scroll = b.selected;
    } else if b.selected >= scroll + height {
        scroll = b.selected + 1 - height;
    }
    scroll = scroll.min(b.results.len().saturating_sub(height));

    let root = app.root().to_path_buf();
    let width = inner.width as usize;
    let query_len = b.input.trim().chars().count();
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (i, hit) in b.results.iter().enumerate().skip(scroll).take(height) {
        let rel = hit
            .path
            .strip_prefix(&root)
            .unwrap_or(&hit.path)
            .to_string_lossy()
            .into_owned();
        let mut spans = match hit.kind {
            HitKind::Name => vec![
                Span::styled("name ", pal.dim),
                Span::styled(rel, pal.text.add_modifier(Modifier::BOLD)),
            ],
            HitKind::Content => vec![Span::styled(format!("{}:{} ", rel, hit.line + 1), pal.dim)],
        };
        if hit.kind == HitKind::Content {
            spans.extend(emphasize(&hit.text, hit.col, query_len, pal));
        }
        if i == b.selected {
            highlight_row(&mut spans, width, pal, true);
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Show a matching line with the matched run standing out.
///
/// Splits at character indices, matching how `search::Hit::col` is reported.
/// The run is drawn reversed rather than coloured, so it stands out in any
/// terminal theme without the palette having to name a highlight colour.
fn emphasize(text: &str, col: usize, len: usize, pal: Palette) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if len == 0 || col >= chars.len() {
        return vec![Span::styled(text.to_string(), pal.text)];
    }
    let end = (col + len).min(chars.len());
    let before: String = chars[..col].iter().collect();
    let hit: String = chars[col..end].iter().collect();
    let after: String = chars[end..].iter().collect();
    let mut out = Vec::new();
    if !before.is_empty() {
        out.push(Span::styled(before, pal.text));
    }
    out.push(Span::styled(hit, pal.text.add_modifier(Modifier::REVERSED)));
    if !after.is_empty() {
        out.push(Span::styled(after, pal.text));
    }
    out
}

// ---- preview pane ---------------------------------------------------------

/// The right-hand pane. Dispatches on `app.preview` to one of the specific
/// drawing functions, and builds the title that names the file and its state.
///
/// The `READ` / `EDIT` / `VIEW` tag encodes the same rule the key handling
/// uses: prose and markdown open rendered and only show raw source once you
/// have deliberately pressed `e`, while code goes straight into the editor.
fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.palette;
    let focused = app.focus == Focus::Editor && matches!(app.mode, Mode::Normal | Mode::Help(_));

    let (name, dirty) = match &app.preview {
        Preview::Buffer { path, .. } => (label(path), app.is_dirty(path)),
        Preview::Media { path, .. } => (label(path), false),
        Preview::Directory { path, .. } => (label(path), false),
        Preview::Binary { path, .. } => (label(path), false),
        Preview::Unreadable(_) => ("error".to_string(), false),
        Preview::Empty => ("nothing selected".to_string(), false),
    };
    let tag = match &app.preview {
        Preview::Buffer { kind, .. } if kind.reads_first() && (!focused || app.read_mode) => "READ",
        Preview::Buffer { .. } if focused => "EDIT",
        Preview::Buffer { .. } => "VIEW",
        Preview::Media { .. } => "VIEW",
        _ => "",
    };

    let mut title = vec![
        Span::raw(" "),
        Span::styled(
            name,
            if focused {
                pal.text.add_modifier(Modifier::BOLD)
            } else {
                pal.dim
            },
        ),
    ];
    if dirty {
        title.push(Span::styled(" *", pal.marker));
    }
    if !tag.is_empty() {
        title.push(Span::styled(format!("  {tag}"), pal.dim));
    }
    title.push(Span::raw(" "));

    let inner = pane(f, area, title, focused, app);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match app.preview.clone() {
        Preview::Buffer { path, kind } => {
            // Rendered while the tree drives the cursor, or while reading;
            // raw source once you are actually editing.
            if kind.reads_first() && (!focused || app.read_mode) {
                draw_reading(f, app, area, inner, &path, kind);
            } else {
                draw_editor(f, app, inner, &path, focused);
            }
        }
        Preview::Media { path, kind, size } => draw_media(f, app, inner, &path, kind, size),
        Preview::Directory { path, entries } => {
            let lines = vec![
                Line::from(Span::styled(label(&path), pal.heading)),
                Line::from(""),
                Line::from(Span::styled(
                    format!("{entries} entr{}", if entries == 1 { "y" } else { "ies" }),
                    pal.text,
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "right to open   n new file   N new folder",
                    pal.dim,
                )),
            ];
            app.preview_len = lines.len();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Preview::Binary { path, size, kind } => {
            let lines = vec![
                Line::from(Span::styled(label(&path), pal.heading)),
                Line::from(""),
                Line::from(Span::styled(
                    format!("{kind}, {}", readable_size(size)),
                    pal.text,
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "not a text file — nothing to edit here",
                    pal.dim,
                )),
            ];
            app.preview_len = lines.len();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Preview::Unreadable(msg) => {
            app.preview_len = 1;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(msg, pal.marker))).wrap(Wrap { trim: true }),
                inner,
            );
        }
        Preview::Empty => {
            app.preview_len = 0;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("nothing selected", pal.dim))),
                inner,
            );
        }
    }
}

/// A picture or video poster frame, plus a caption.
///
/// Reserves two rows for the caption and honours the configured
/// `media_height` ceiling, then asks `App::ensure_media` for a preview at that
/// exact size — the cache is keyed on the size, so this both draws and, when
/// the pane has changed shape, triggers the re-decode.
///
/// A failed decode is drawn as its error message rather than an empty pane, so
/// a missing ffmpeg or a corrupt file explains itself.
fn draw_media(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    path: &std::path::Path,
    kind: crate::media::Kind,
    size: u64,
) {
    let pal = app.palette;
    // Leave a row for the caption, and respect the configured ceiling.
    let rows = (inner.height as usize)
        .saturating_sub(2)
        .min(app.config.media_height);
    app.ensure_media(path, kind, inner.width as usize, rows);

    let mut lines: Vec<Line> = Vec::new();
    match app.media.as_ref().map(|c| &c.result) {
        Some(Ok(preview)) => {
            lines.extend(preview.lines.iter().cloned());
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("{}  {}", preview.note, readable_size(size)),
                pal.dim,
            )));
        }
        Some(Err(msg)) => {
            lines.push(Line::from(Span::styled(label(path), pal.heading)));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(msg.clone(), pal.text)));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(readable_size(size), pal.dim)));
        }
        None => {}
    }
    app.preview_len = lines.len();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Rendered markdown, or wrapped prose, with a scroll indicator.
///
/// Renders from the *buffer* rather than from disk, which is what makes
/// unsaved edits appear in the rendered view the moment you press Esc.
///
/// The full document is rendered every frame and then sliced to the visible
/// window. That is more work than it needs to be, but the render is cheap at
/// note scale and it removes any need to invalidate a cache when the buffer,
/// the width, or the palette changes. `preview_len` is written back so the key
/// handler knows how far it can scroll.
fn draw_reading(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    inner: Rect,
    path: &std::path::Path,
    kind: TextKind,
) {
    let pal = app.palette;
    let Some(ed) = app.buffers.get(path) else {
        return;
    };
    // Render from the buffer, not from disk, so unsaved edits show up the
    // moment you step back out of the editor.
    let source = ed.to_text();
    let width = inner.width as usize;
    let lines = match kind {
        TextKind::Markdown => markdown::render(&source, width, &pal, &app.highlighter),
        // Prose keeps the author's line structure and just wraps what is long.
        _ => markdown::render_plain(&source, width, &pal),
    };
    app.preview_len = lines.len();

    let height = inner.height as usize;
    app.last_edit_height = height;
    let max_scroll = lines.len().saturating_sub(height);
    app.preview_scroll = app.preview_scroll.min(max_scroll);

    let view: Vec<Line> = lines
        .into_iter()
        .skip(app.preview_scroll)
        .take(height)
        .collect();
    f.render_widget(Paragraph::new(view).style(pal.text), inner);

    // Scroll position, on the bottom border so it never covers a line of text.
    // The guard decides whether to draw at all, so `checked_div` would not
    // replace it — a note that fits on screen should show no indicator.
    #[allow(clippy::manual_checked_ops)]
    if max_scroll > 0 && app.config.borders {
        let pct = (app.preview_scroll * 100) / max_scroll;
        let tag = format!(" {pct:>3}% ");
        let w = tag.width() as u16;
        if area.width > w + 2 {
            let spot = Rect {
                x: area.x + area.width - w - 2,
                y: area.y + area.height.saturating_sub(1),
                width: w,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(tag, pal.dim))).alignment(Alignment::Right),
                spot,
            );
        }
    }
}

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
fn draw_editor(f: &mut Frame, app: &mut App, inner: Rect, path: &std::path::Path, focused: bool) {
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

    // Scrolling needs the pane size, so it is resolved here rather than in the
    // key handler, which has no idea how big the window is.
    let (scroll_y, scroll_x, cur_line, cur_col) = {
        let ed = app.buffers.get_mut(path).expect("checked above");
        ed.sync_scroll(text_w.saturating_sub(1), height);
        (ed.scroll_y, ed.scroll_x, ed.cursor_line, ed.cursor_col)
    };
    let ed = &app.buffers[path];
    app.preview_len = ed.line_count();

    let first_line = ed.lines().first().map(String::as_str).unwrap_or("");
    let syntax = app.highlighter.syntax_for_path(path, first_line).clone();
    let highlighted = app
        .highlighter
        .highlight_window(ed.lines(), &syntax, scroll_y, height);

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (i, pieces) in highlighted.into_iter().enumerate() {
        let lineno = scroll_y + i;
        let mut spans: Vec<Span> = Vec::new();
        if gutter > 0 {
            let style = if lineno == cur_line && focused {
                pal.text
            } else {
                pal.dim
            };
            spans.push(Span::styled(
                format!("{:>w$} ", lineno + 1, w = gutter - 1),
                style,
            ));
        }
        spans.extend(clip_pieces(&pieces, scroll_x, text_w, pal));
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).style(pal.text), inner);

    if focused {
        // Place the real terminal cursor, so it blinks where the user expects.
        let line = ed.lines().get(cur_line).map(String::as_str).unwrap_or("");
        let before: String = line.chars().take(cur_col).collect();
        let skipped: String = line.chars().take(scroll_x).collect();
        let vis = before.width().saturating_sub(skipped.width());
        let x = inner.x + gutter as u16 + vis as u16;
        let y = inner.y + (cur_line.saturating_sub(scroll_y)) as u16;
        if x < inner.x + inner.width && y < inner.y + inner.height {
            f.set_cursor_position((x, y));
        }
    }
}

/// Apply the horizontal scroll to one highlighted line and clip it to width.
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
            let w = c.to_string().width();
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

// ---- the graph ------------------------------------------------------------

/// A style's colour, or the terminal's own where the theme names none.
///
/// The canvas widget wants a bare `Color` rather than a `Style`, so this is
/// the one place styles get unwrapped. `Color::Reset` means "whatever the
/// terminal is using", which keeps the default monochrome palette working.
fn colour(style: Style) -> Color {
    style.fg.unwrap_or(Color::Reset)
}

/// The graph, drawn on a braille canvas with a detail strip underneath.
///
/// Braille gives four times the vertical resolution and twice the horizontal
/// resolution of a text cell, which is what makes diagonal edges readable.
///
/// # Two things are harder than they look
///
/// **Fitting the viewport.** Labels stick out sideways from their dot, so the
/// bounding box has to be widened by half the widest label or names get
/// clipped at the edge. That width is only knowable in canvas units after a
/// first [`fit_bounds`] pass, hence the two rounds: fit, measure, widen, fit
/// again.
///
/// **Label collision.** Labels are placed in priority order — the selected
/// file first, then its neighbours, then everything else — and each one claims
/// a `(row, start, end)` span. A label whose span overlaps something already
/// placed is simply dropped. That is why the cursor's own label is always
/// legible however dense the graph gets.
///
/// Drawing is layered: edges, then dots, then labels, so a name is never
/// threaded onto the line running under it.
fn draw_graph(f: &mut Frame, app: &App, area: Rect) {
    let Some(view) = app.graph_view.as_ref() else {
        return;
    };
    let pal = app.palette;

    // The picture, then a strip describing whatever the cursor is on.
    let detail_h = if area.height > 14 { 6 } else { 0 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(detail_h)])
        .split(area);

    let title = vec![
        Span::raw(" "),
        Span::styled("GRAPH", pal.text.add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", view.summary()), pal.dim),
        Span::raw(" "),
    ];
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(title));
    if view.filtering || !view.filter.is_empty() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(" / ", pal.text.add_modifier(Modifier::REVERSED)),
            Span::styled(view.filter.clone(), pal.text),
            Span::styled(if view.filtering { "_ " } else { " " }, pal.dim),
        ]));
    } else if !view.show_orphans && view.orphan_count() > 0 {
        // Say that `o` has something to show, rather than leaving those files
        // silently missing.
        block = block.title_bottom(Line::from(Span::styled(
            format!(" o: {} unconnected ", view.orphan_count()),
            pal.dim,
        )));
    }
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    if inner.width < 4 || inner.height < 3 {
        return;
    }

    let selected = view.selected;
    let near = view.neighbours(selected);
    let visible = view.visible_indices();
    let label_all = view.labels_all || visible.len() <= 30;

    // Labels stick out sideways from the dot they belong to, so the viewport
    // has to make room for the widest one or it gets clipped at the edge.
    let (x0, x1, y0, y1) = fit_bounds(view.bounds(), inner.width, inner.height);
    let unit_x = (x1 - x0) / inner.width.max(1) as f64;
    let widest = visible
        .iter()
        .map(|i| view.graph.nodes[*i].name.chars().count())
        .max()
        .unwrap_or(0);
    let margin = (widest as f64 / 2.0 + 1.0) * unit_x;
    let (x0, x1, y0, y1) = fit_bounds(
        (x0 - margin, x1 + margin, y0, y1),
        inner.width,
        inner.height,
    );
    let unit_x = (x1 - x0) / inner.width.max(1) as f64;
    let unit_y = (y1 - y0) / inner.height.max(1) as f64;

    // Work out which labels fit before drawing any: the selected file gets
    // the space first, then its neighbours, then whatever is left over.
    let mut order: Vec<usize> = Vec::new();
    if view.node_visible(selected) {
        order.push(selected);
    }
    order.extend(visible.iter().copied().filter(|i| near.contains(i)));
    if label_all {
        order.extend(
            visible
                .iter()
                .copied()
                .filter(|i| *i != selected && !near.contains(i)),
        );
    }

    let mut taken: Vec<(i32, i32, i32)> = Vec::new();
    let mut labels: Vec<(f64, f64, String, Style)> = Vec::new();
    for i in order {
        let node = &view.graph.nodes[i];
        // Padded on both sides so the label clears the edge lines running
        // underneath it instead of being threaded onto them.
        let label = format!(" {} ", node.name);
        let w = label.chars().count() as i32;
        let (px, py) = view.pos[i];
        let col = ((px - x0) / unit_x).round() as i32;
        let row = ((y1 - py) / unit_y).round() as i32 + 1;
        let (a, b) = (col - w / 2, col - w / 2 + w);
        if taken.iter().any(|(r, s, e)| *r == row && a < *e && *s < b) {
            continue; // something already has this space
        }
        taken.push((row, a, b));
        let style = if i == selected {
            pal.text.add_modifier(Modifier::REVERSED)
        } else if near.contains(&i) {
            pal.text
        } else {
            pal.dim
        };
        labels.push((px - (w as f64 / 2.0) * unit_x, py - unit_y, label, style));
    }

    let dim = colour(pal.dim);
    let fg = colour(pal.text);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([x0, x1])
        .y_bounds([y0, y1])
        .paint(move |ctx| {
            for e in &view.graph.edges {
                if !view.edge_visible(e) {
                    continue;
                }
                let touches = e.from == selected || e.to == selected;
                let (ax, ay) = view.pos[e.from];
                let (bx, by) = view.pos[e.to];
                ctx.draw(&CanvasLine {
                    x1: ax,
                    y1: ay,
                    x2: bx,
                    y2: by,
                    color: if touches { fg } else { dim },
                });
            }
            ctx.layer();

            // Every visible file gets a dot, so an unlabelled one still reads
            // as something rather than as the end of a line.
            let dots: Vec<(f64, f64)> = visible.iter().map(|i| view.pos[*i]).collect();
            ctx.draw(&Points {
                coords: &dots,
                color: dim,
            });
            ctx.layer();

            for (x, y, label, style) in &labels {
                ctx.print(*x, *y, Line::from(Span::styled(label.clone(), *style)));
            }
        });
    f.render_widget(canvas, inner);

    if detail_h > 0 {
        draw_graph_detail(f, app, view, rows[1]);
    }
}

/// Expand the graph's bounding box to match the pane's shape, so the layout
/// is not stretched. Braille sub-cells are roughly square, and there are two
/// across and four down per character cell.
///
/// Always grows the box, never shrinks it, so nothing that was visible is
/// pushed off screen. The target ratio is `(w * 2) / (h * 4)` — the pane's
/// dimensions in braille sub-cells rather than in characters.
fn fit_bounds(b: (f64, f64, f64, f64), w: u16, h: u16) -> (f64, f64, f64, f64) {
    let (mut x0, mut x1, mut y0, mut y1) = b;
    let want = (w as f64 * 2.0) / (h as f64 * 4.0);
    let (dx, dy) = ((x1 - x0).max(1e-6), (y1 - y0).max(1e-6));
    if dx / dy < want {
        let target = dy * want;
        let pad = (target - dx) / 2.0;
        x0 -= pad;
        x1 += pad;
    } else {
        let target = dx / want;
        let pad = (target - dy) / 2.0;
        y0 -= pad;
        y1 += pad;
    }
    (x0, x1, y0, y1)
}

/// Human-readable name for a node kind, for the detail strip.
fn node_word(kind: crate::graph::NodeKind) -> &'static str {
    use crate::graph::NodeKind;
    match kind {
        NodeKind::Note => "note",
        NodeKind::Prose => "prose",
        NodeKind::Code => "code",
        NodeKind::Other => "file",
    }
}

/// Human-readable name for an edge kind. Also labels the `1`-`4` toggles, so
/// the words on screen match the words in the help and the README.
fn kind_word(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Wikilink => "wikilink",
        EdgeKind::Link => "link",
        EdgeKind::Import => "import",
        EdgeKind::Call => "call",
    }
}

/// The strip under the graph: what the cursor is on, what it connects to, and
/// which edge kinds are switched on.
///
/// Everything is capped — four connections each way, six defined symbols —
/// with a `+n more` tail, because this has a fixed six rows and a hub file can
/// have dozens of edges.
///
/// Only call edges show their label, since that label is the symbol being
/// called and is the actual information. For an import or a link the label
/// just repeats the filename already shown.
fn draw_graph_detail(f: &mut Frame, app: &App, view: &GraphView, area: Rect) {
    let pal = app.palette;
    let Some(node) = view.selected_node() else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("  nothing selected", pal.dim))),
            area,
        );
        return;
    };
    let (out, incoming) = view.connections(view.selected);

    // Which edge kinds are switched on, as the 1-4 keys see them.
    let toggles: Vec<Span> = [
        EdgeKind::Wikilink,
        EdgeKind::Link,
        EdgeKind::Import,
        EdgeKind::Call,
    ]
    .iter()
    .enumerate()
    .flat_map(|(i, k)| {
        let on = view.kinds[crate::graphview::kind_index(*k)];
        [
            Span::styled(
                format!("{}:{} ", i + 1, kind_word(*k)),
                if on { pal.text } else { pal.dim },
            ),
            Span::raw(""),
        ]
    })
    .collect();

    let summarise = |edges: &[&crate::graph::Edge], outgoing: bool| {
        if edges.is_empty() {
            return vec![Span::styled("none", pal.dim)];
        }
        let mut spans = Vec::new();
        for e in edges.iter().take(4) {
            let other = if outgoing { e.to } else { e.from };
            spans.push(Span::styled(view.graph.nodes[other].name.clone(), pal.text));
            // The symbol is the point of a call edge. For an import or a link
            // the label just repeats the file name, so it is left out.
            if e.kind == EdgeKind::Call {
                let times = if e.count > 1 {
                    format!(":{} x{}", e.label, e.count)
                } else {
                    format!(":{}", e.label)
                };
                spans.push(Span::styled(times, pal.dim));
            }
            spans.push(Span::raw(" "));
        }
        if edges.len() > 4 {
            spans.push(Span::styled(format!("+{} more", edges.len() - 4), pal.dim));
        }
        spans
    };

    let mut lines = vec![
        Line::from(vec![
            Span::raw("  "),
            Span::styled(node.rel.clone(), pal.text.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", node_word(node.kind)), pal.dim),
        ]),
        Line::from(
            std::iter::once(Span::styled(format!("  out {:<3} ", out.len()), pal.dim))
                .chain(summarise(&out, true))
                .collect::<Vec<_>>(),
        ),
        Line::from(
            std::iter::once(Span::styled(
                format!("  in  {:<3} ", incoming.len()),
                pal.dim,
            ))
            .chain(summarise(&incoming, false))
            .collect::<Vec<_>>(),
        ),
    ];
    if !node.defines.is_empty() {
        // A file can define dozens of things; the first few say enough.
        let shown = node.defines.len().min(6);
        let mut text = node.defines[..shown].join(", ");
        if node.defines.len() > shown {
            text.push_str(&format!(", +{}", node.defines.len() - shown));
        }
        lines.push(Line::from(vec![
            Span::styled("  defines ", pal.dim),
            Span::styled(text, pal.dim),
        ]));
    }
    // The toggles, with a note on which languages calls can be followed in —
    // right beside the switch it explains.
    let mut footer: Vec<Span> = std::iter::once(Span::raw("  ")).chain(toggles).collect();
    footer.push(Span::styled(
        format!("  calls traced in {}", view.languages().join(", ")),
        pal.dim,
    ));
    lines.push(Line::from(footer));

    f.render_widget(Paragraph::new(lines), area);
}

// ---- the bar --------------------------------------------------------------

/// The search (`/`) or command (`:`) line.
///
/// Draws its own block cursor as a reversed span, because the terminal has
/// only one real cursor and the editor pane has a stronger claim on it.
fn draw_bar(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Bar(b) = &app.mode else { return };
    let pal = app.palette;
    let sigil = match b.kind {
        BarKind::Search => " / ",
        BarKind::Command => " : ",
    };
    let (before, after) = split_at_char(&b.input, b.cursor);
    let hint = match b.kind {
        BarKind::Search if b.results.is_empty() => "  names and contents · Esc close",
        BarKind::Search => "  up/down pick · Enter jump · Esc close",
        BarKind::Command => "  Tab complete · Enter run · Esc close",
    };
    let line = Line::from(vec![
        Span::styled(sigil, pal.text.add_modifier(Modifier::REVERSED)),
        Span::styled(before, pal.text),
        // The real cursor lives in the editor, so the bar draws its own.
        Span::styled(
            after
                .chars()
                .next()
                .map(String::from)
                .unwrap_or_else(|| " ".into()),
            pal.text.add_modifier(Modifier::REVERSED),
        ),
        Span::styled(after.chars().skip(1).collect::<String>(), pal.text),
        Span::styled(hint, pal.dim),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// ---- status line ----------------------------------------------------------

/// The status line: prompts, confirmations, or the ordinary message plus
/// context-sensitive hints.
///
/// Hints are dropped before the position readout when the window is too narrow
/// for both, so the line degrades gracefully instead of wrapping or being
/// clipped mid-word.
fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let pal = app.palette;
    let line = match &app.mode {
        Mode::Prompt(p) => {
            let (before, after) = split_at_char(&p.input, p.cursor);
            Line::from(vec![
                Span::styled(format!(" {}: ", p.label), pal.heading),
                Span::styled(before, pal.text),
                Span::styled(
                    after
                        .chars()
                        .next()
                        .map(String::from)
                        .unwrap_or_else(|| " ".into()),
                    pal.text.add_modifier(Modifier::REVERSED),
                ),
                Span::styled(after.chars().skip(1).collect::<String>(), pal.text),
                Span::styled("   Enter confirm · Esc cancel", pal.dim),
            ])
        }
        Mode::Confirm(c) => Line::from(vec![
            Span::styled(" ! ", pal.marker.add_modifier(Modifier::REVERSED)),
            Span::styled(format!(" {}", c.message), pal.marker),
        ]),
        _ => {
            if app.graph_view.is_some() {
                let line = Line::from(vec![
                    Span::styled(format!(" {}", app.status), pal.text),
                    Span::styled(
                        "   arrows move · Enter open · 1-4 kinds · o orphans · / filter · Esc back",
                        pal.dim,
                    ),
                ]);
                f.render_widget(Paragraph::new(line).style(pal.text), area);
                return;
            }
            let hints = match (&app.mode, app.focus) {
                (Mode::Settings(_), _) => "up/down pick · Enter change · ^S write · Esc close",
                (Mode::Bar(_), _) => "Esc close",
                (_, Focus::Tree) => "/ search · w web · : commands · n new · ? help · q quit",
                (_, Focus::Editor) if app.read_mode => "up/down scroll · e edit · Esc back",
                (_, Focus::Editor) => "^S save · ^Z undo · ^K cut line · Esc back",
            };
            let pos = position_readout(app);
            let room = area.width as usize;
            let mut spans = vec![
                Span::styled(format!(" {}", app.status), pal.text),
                Span::raw("   "),
            ];
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            if used + hints.width() + pos.width() + 3 < room {
                spans.push(Span::styled(hints, pal.dim));
            }
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            if used + pos.width() + 1 < room {
                spans.push(Span::raw(" ".repeat(room - used - pos.width() - 1)));
                spans.push(Span::styled(pos, pal.dim));
            }
            Line::from(spans)
        }
    };
    f.render_widget(Paragraph::new(line).style(app.palette.text), area);
}

/// The right-hand end of the status line: `line:col` while editing, otherwise
/// the tree cursor's position in the row list.
fn position_readout(app: &App) -> String {
    match app.active_buffer() {
        Some(ed) if app.focus == Focus::Editor && !app.read_mode => {
            format!("{}:{} ", ed.cursor_line + 1, ed.cursor_col + 1)
        }
        _ => format!("{}/{} ", app.selected + 1, app.rows.len()),
    }
}

// ---- overlays -------------------------------------------------------------

/// A centred rectangle for an overlay, shrunk to fit if the window is smaller
/// than the requested size.
fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// The keymap shown by `?`, as `(keys, description)` pairs.
///
/// An empty `keys` makes the row a section heading, and a pair of empty
/// strings is a spacer. This is the user-facing source of truth for the
/// bindings — if you add a key in `app`, add it here too, and to the table in
/// the README.
const HELP: &[(&str, &str)] = &[
    ("", "TREE"),
    ("up down  k j", "move the cursor"),
    ("right  Enter", "open a folder, or edit a file"),
    ("left", "close a folder, or jump to its parent"),
    ("g  G", "first / last entry"),
    ("n  N", "new file / new folder"),
    ("r", "rename"),
    ("d", "delete (asks first)"),
    (".", "show or hide dotfiles"),
    ("R  F5", "re-read the project from disk"),
    ("", ""),
    ("", "GRAPH"),
    ("w", "draw the project as a graph"),
    ("", "  notes link by [[wikilink]], code by import and call"),
    ("arrows", "move to the nearest file that way"),
    ("Enter", "open the file the cursor is on"),
    ("1 2 3 4", "wikilinks / links / imports / calls"),
    ("o  L  r", "orphans · all labels · lay out again"),
    ("", ""),
    ("", "SEARCH AND COMMANDS"),
    ("/", "search names and contents"),
    (":", "commands — :set :replace :config"),
    (",  F2", "the settings area"),
    ("", ""),
    ("", "PREVIEW"),
    ("up down", "scroll a note or a picture"),
    ("e", "switch to raw editing"),
    ("Esc", "back to the tree"),
    ("", ""),
    ("", "EDITOR"),
    ("Ctrl+S", "save"),
    ("Ctrl+Z  Ctrl+Y", "undo / redo"),
    ("Ctrl+K", "delete the current line"),
    ("Ctrl+left right", "move by word"),
    ("", ""),
    ("q  Ctrl+Q", "quit"),
];

/// The keymap overlay. Scrollable, because the full list does not fit a short
/// terminal — the footer says which of the two situations you are in.
fn draw_help(f: &mut Frame, area: Rect, pal: &Palette, scroll: usize) {
    let popup = centred(area, 62, HELP.len() as u16 + 2);
    // Short terminals cannot show the whole keymap at once.
    let scrolls = (popup.height as usize).saturating_sub(2) < HELP.len();
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Keys ",
            pal.text.add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(
            if scrolls {
                " up/down · any other key closes "
            } else {
                " any key to close "
            },
            pal.dim,
        )));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let height = inner.height as usize;
    let scroll = scroll.min(HELP.len().saturating_sub(height));
    let lines: Vec<Line> = HELP
        .iter()
        .skip(scroll)
        .take(height)
        .map(|(keys, desc)| {
            if keys.is_empty() {
                Line::from(Span::styled(desc.to_string(), pal.heading))
            } else {
                Line::from(vec![
                    Span::styled(format!("{keys:<18}"), pal.text.add_modifier(Modifier::BOLD)),
                    Span::styled(desc.to_string(), pal.dim),
                ])
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// The settings overlay: every key from `Config::settings_index`, its current
/// value, and its description.
///
/// A row being edited swaps the description for an inline text field with its
/// own drawn cursor. Values are read live from `app.config`, so a change takes
/// effect on the screen behind the overlay immediately — `Ctrl+S` is what
/// makes it persist to `tiny.conf`.
fn draw_settings(f: &mut Frame, app: &App, area: Rect, s: &Settings) {
    let pal = app.palette;
    let index = Config::settings_index();
    let popup = centred(area, 80, index.len() as u16 + 4);
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Settings ",
            pal.text.add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(
            " Enter change · ^S write tiny.conf · Esc close ",
            pal.dim,
        )));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    let height = inner.height as usize;
    if height == 0 {
        return;
    }

    let mut scroll = s.scroll;
    if s.selected < scroll {
        scroll = s.selected;
    } else if s.selected >= scroll + height {
        scroll = s.selected + 1 - height;
    }
    scroll = scroll.min(index.len().saturating_sub(height));

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (i, (key, desc)) in index.iter().enumerate().skip(scroll).take(height) {
        let editing = i == s.selected && s.editing.is_some();
        let value = match (&s.editing, editing) {
            (Some(buf), true) => buf.clone(),
            _ => app.config.get(key).unwrap_or_default(),
        };
        let mut spans = vec![
            Span::styled(format!("{key:<20}"), pal.text.add_modifier(Modifier::BOLD)),
            Span::raw(" "),
        ];
        if editing {
            let (before, after) = split_at_char(&value, s.cursor);
            spans.push(Span::styled(before, pal.text));
            spans.push(Span::styled(
                after
                    .chars()
                    .next()
                    .map(String::from)
                    .unwrap_or_else(|| " ".into()),
                pal.text.add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::styled(
                after.chars().skip(1).collect::<String>(),
                pal.text,
            ));
        } else {
            spans.push(Span::styled(format!("{value:<18}"), pal.text));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(desc.to_string(), pal.dim));
        }
        if i == s.selected && !editing {
            highlight_row(&mut spans, width, pal, true);
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- small helpers --------------------------------------------------------

/// Filename for a pane title, falling back to the whole path for anything
/// without one.
fn label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Decimal digit count, used to size the line-number gutter so it is exactly
/// wide enough for the longest number in the file.
fn digits(n: usize) -> usize {
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
fn split_at_char(s: &str, ci: usize) -> (String, String) {
    let b = s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b);
    (s[..b].to_string(), s[b..].to_string())
}

/// Byte count as `B` / `KB` / `MB` / …, with one decimal place above a
/// kilobyte. Exact bytes below that, since "0.4 KB" tells you less than "412 B".
fn readable_size(bytes: u64) -> String {
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
    fn emphasize_marks_only_the_matched_run() {
        let pal = Palette::default();
        let spans = emphasize("the widget plan", 4, 6, pal);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(text, "the widget plan", "no characters are lost");
        let marked: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(marked, ["widget"]);
    }

    #[test]
    fn emphasize_survives_an_out_of_range_column() {
        let pal = Palette::default();
        let spans = emphasize("short", 99, 4, pal);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(text, "short");
    }
}
