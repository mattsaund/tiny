//! All drawing. The split is deliberate: `app` never touches ratatui types
//! beyond styles, and this module never mutates state except scroll offsets
//! and the media cache, which can only be resolved once the pane size is
//! known.
//!
//! Nothing here picks a colour. Every style comes from the palette, whose
//! defaults name no colour at all — so tiny renders in whatever the terminal
//! is already using, and meaning is carried by weight, underline and reverse.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Bar, BarKind, Focus, Mode, Preview, Settings};
use crate::config::{Config, Palette, Position, Side};
use crate::markdown;
use crate::search::HitKind;

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
        Mode::Help => draw_help(f, area, &pal),
        Mode::Settings(s) => {
            let s = s.clone();
            draw_settings(f, app, area, &s);
        }
        _ => {}
    }
}

enum Slot {
    Bar,
    Main,
    Status,
}

/// A pane frame. With borders off, the title is drawn on its own line and the
/// box is dropped entirely — a quieter screen for people who want one.
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

fn draw_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.palette;
    let focused = app.focus == Focus::Tree && matches!(app.mode, Mode::Normal | Mode::Help);
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

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.palette;
    let focused = app.focus == Focus::Editor && matches!(app.mode, Mode::Normal | Mode::Help);

    let (name, dirty) = match &app.preview {
        Preview::Buffer { path, .. } => (label(path), app.is_dirty(path)),
        Preview::Media { path, .. } => (label(path), false),
        Preview::Directory { path, .. } => (label(path), false),
        Preview::Binary { path, .. } => (label(path), false),
        Preview::Unreadable(_) => ("error".to_string(), false),
        Preview::Empty => ("nothing selected".to_string(), false),
    };
    let tag = match &app.preview {
        Preview::Buffer { markdown: true, .. } if !focused || app.read_mode => "READ",
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
        Preview::Buffer { path, markdown } => {
            // Rendered while the tree drives the cursor, or while reading;
            // raw source once you are actually editing.
            if markdown && (!focused || app.read_mode) {
                draw_markdown(f, app, area, inner, &path);
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

fn draw_markdown(f: &mut Frame, app: &mut App, area: Rect, inner: Rect, path: &std::path::Path) {
    let pal = app.palette;
    let Some(ed) = app.buffers.get(path) else {
        return;
    };
    // Render from the buffer, not from disk, so unsaved edits show up the
    // moment you step back out of the editor.
    let source = ed.to_text();
    let lines = markdown::render(&source, inner.width as usize, &pal, &app.highlighter);
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

// ---- the bar --------------------------------------------------------------

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
            let hints = match (&app.mode, app.focus) {
                (Mode::Settings(_), _) => "up/down pick · Enter change · ^S write · Esc close",
                (Mode::Bar(_), _) => "Esc close",
                (_, Focus::Tree) => "/ search · : commands · n new · d delete · ? help · q quit",
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

fn position_readout(app: &App) -> String {
    match app.active_buffer() {
        Some(ed) if app.focus == Focus::Editor && !app.read_mode => {
            format!("{}:{} ", ed.cursor_line + 1, ed.cursor_col + 1)
        }
        _ => format!("{}/{} ", app.selected + 1, app.rows.len()),
    }
}

// ---- overlays -------------------------------------------------------------

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

fn draw_help(f: &mut Frame, area: Rect, pal: &Palette) {
    let popup = centred(area, 58, HELP.len() as u16 + 2);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Keys ",
            pal.text.add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(" any key to close ", pal.dim)));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lines: Vec<Line> = HELP
        .iter()
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

fn label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn digits(n: usize) -> usize {
    let mut n = n.max(1);
    let mut d = 0;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}

fn split_at_char(s: &str, ci: usize) -> (String, String) {
    let b = s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b);
    (s[..b].to_string(), s[b..].to_string())
}

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
