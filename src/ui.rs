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
//!  ├─ draw_map              when the map is open — takes the whole screen
//!  └─ two panes, side decided by config
//!      ├─ draw_tree  or  draw_results   (results replace the tree while searching)
//!      └─ draw_preview
//!           ├─ draw_reading   rendered, for a preview with no cursor in it
//!           ├─ draw_editor    the file with the real cursor in it
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

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, BUTTONS, Bar, Focus, KEYBIND_BUTTONS, Keybinds, Mode, Preview, Settings, TextKind,
};
use crate::config::{Config, Markers, Palette, Position, Side};
use crate::graph::EdgeKind;
use crate::keys::{Action, Keymap};
use crate::markdown;
use crate::projectmap::{self, Placed, ProjectMap};
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

    if app.project_map.is_some() {
        draw_map(f, app, main);
        return;
    }

    let searching = bar_open && matches!(&app.mode, Mode::Bar(b) if !b.is_command());

    // The side pane is where search results are listed, so a search brings it
    // back for as long as the bar is open — hiding the tree must not take the
    // results with it.
    let (side_area, preview_area) = if app.tree_hidden && !searching {
        (None, main)
    } else {
        // Results take the tree's place at exactly the tree's width, so the
        // screen does not lurch sideways the moment you start typing.
        let side_w = ((main.width as f32) * app.config.tree_width).round() as u16;
        let side_w = side_w.clamp(14, main.width.saturating_sub(12).max(14));

        let (left, right) = match app.config.tree_side {
            Side::Left => (Constraint::Length(side_w), Constraint::Min(10)),
            Side::Right => (Constraint::Min(10), Constraint::Length(side_w)),
        };
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([left, right])
            .split(main);
        match app.config.tree_side {
            Side::Left => (Some(panes[0]), panes[1]),
            Side::Right => (Some(panes[1]), panes[0]),
        }
    };

    // Tell `app` where the tree ended up, so a mouse wheel can work out which
    // pane it is over. `None` while it is folded away or replaced by results.
    app.last_tree_cols = side_area
        .filter(|_| !searching)
        .map(|a| (a.x, a.x + a.width));

    if let Some(side_area) = side_area {
        match &app.mode {
            Mode::Bar(b) if !b.is_command() => {
                let b = b.clone();
                draw_results(f, app, side_area, &b);
            }
            _ => draw_tree(f, app, side_area),
        }
    }
    draw_preview(f, app, preview_area);

    match &app.mode {
        Mode::Help(scroll) => draw_help(f, app, area, &pal, *scroll),
        Mode::Settings(s) => {
            let s = s.clone();
            draw_settings(f, app, area, &s);
        }
        Mode::Keybinds(kb) => {
            let kb = kb.clone();
            draw_keybinds(f, app, area, &kb);
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
        // An open folder's marker is drawn in the text colour, not the dim
        // one: it is the only glyph that says "you are inside here", so it
        // reads as part of the name rather than as chrome.
        let (marker, marker_style) = if row.is_dir {
            if row.expanded {
                (open, pal.text)
            } else {
                (closed, pal.dim)
            }
        } else {
            (leaf, pal.dim)
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
            Span::styled(marker, marker_style),
            Span::styled(row.name.clone(), name_style),
        ];
        if app.dirty_here_or_below(&row.path) {
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

    // Keep the highlighted result on screen, from the top until it has to
    // move. Recomputed rather than remembered: the list is rebuilt on every
    // keystroke anyway, so there is no offset worth carrying between frames.
    let scroll = keep_visible(b.selected, height, b.results.len());

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
/// uses: prose and markdown open rendered and only take the keyboard once you
/// have deliberately pressed Enter, while code goes straight into the editor.
/// `EDIT` on markdown is still formatted — see [`live_rows`] — so the tag says
/// which keyboard you have, not whether there is any formatting on screen.
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
            // Rendered while the tree still has the keyboard — the preview is
            // a picture of the file then, with no cursor in it. The moment it
            // is focused it becomes the editor, markdown formatting and all.
            if kind.reads_first() && !focused {
                draw_reading(f, app, area, inner, &path, kind);
            } else {
                draw_editor(f, app, inner, &path, kind, focused);
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
                Line::from(Span::styled("right to open   n new", pal.dim)),
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
/// Only ever drawn for a preview the keyboard has not reached yet: focus the
/// pane and it becomes the editor. Nothing here has a cursor, which is exactly
/// why markdown can be rendered whole — there is no block to leave raw.
///
/// Renders from the *buffer* rather than from disk, which is what makes
/// unsaved edits show up here the moment you step back out to the tree.
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
fn draw_editor(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    path: &std::path::Path,
    kind: TextKind,
    focused: bool,
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
    let live = kind == TextKind::Markdown && app.buffers[path].line_count() <= LIVE_LIMIT;

    // Scrolling needs the pane size, so it is resolved here rather than in the
    // key handler, which has no idea how big the window is. Live preview does
    // its own vertical scrolling in drawn rows rather than source lines, since
    // the two stop being the same thing once a block is formatted.
    let (scroll_y, scroll_x, cur_line, cur_col) = {
        let ed = app.buffers.get_mut(path).expect("checked above");
        ed.sync_scroll(text_w.saturating_sub(1), if live { 0 } else { height });
        (ed.scroll_y, ed.scroll_x, ed.cursor_line, ed.cursor_col)
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
        (Some(&a), Some(&b)) => app
            .highlighter
            .highlight_window(ed.lines(), &syntax, a, b + 1 - a),
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
        match row {
            Row::Raw(n) => {
                if *n == cur_line {
                    cursor_row = Some(i);
                }
                match piece_of(*n) {
                    Some(pieces) => spans.extend(clip_pieces(pieces, scroll_x, text_w, pal)),
                    None => spans.push(Span::raw("")),
                }
            }
            Row::Made { content, .. } => spans.extend(content.spans.iter().cloned()),
        }
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

/// Markdown longer than this is edited raw.
///
/// Live preview re-renders every block outside the cursor's on each keystroke,
/// which is linear in the file. That is nothing for a note and noticeable for
/// a generated document, so past this length the editor stops formatting
/// rather than getting slow.
const LIVE_LIMIT: usize = 4000;

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
/// layout exists. [`LIVE_LIMIT`] is what keeps that affordable.
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

// ---- the project map ------------------------------------------------------

/// The project map: every file as a box, every connection as a line, and a
/// detail strip underneath.
///
/// Drawn by composing an [`Ink`] grid rather than by emitting spans directly.
/// Lines go down first and boxes on top, so a connection running past a file
/// never writes over its name, and two lines crossing resolve into a junction
/// without either of them having to know the other was there.
///
/// What is drawn is decided in three steps: `place` works out which boxes fit
/// and where, `route` draws the line between each pair of them, and then the
/// boxes are stamped over the top.
fn draw_map(f: &mut Frame, app: &mut App, area: Rect) {
    if app.project_map.is_none() {
        return;
    }
    let pal = app.palette;
    let markers = app.config.markers;

    // The picture, then a strip describing whatever the cursor is on.
    let detail_h = if area.height > 14 { 6 } else { 0 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(detail_h)])
        .split(area);

    // The border is fixed, so the pane inside it can be measured before the
    // block that will carry the titles exists — and the titles need the
    // layout, which needs the size.
    let bordered = || Block::default().borders(Borders::ALL);
    let inner = bordered().inner(rows[0]);
    if inner.width < 8 || inner.height < 3 {
        f.render_widget(bordered().border_style(pal.border_focus), rows[0]);
        return;
    }

    // Two borrows, in order: laying the picture out writes `pos` back, then
    // the frame around it is read-only. The one-way rule bends exactly as far
    // as `last_tree_cols` already bends it.
    let placement = app
        .project_map
        .as_mut()
        .expect("checked above")
        .place(inner.width, inner.height);
    let view = app.project_map.as_ref().expect("checked above");
    let selected = view.selected;
    let near = view.neighbours(selected);

    let mut block = bordered()
        .border_style(pal.border_focus)
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("PROJECT MAP", pal.text.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", view.summary()), pal.dim),
            Span::raw(" "),
        ]));
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
    if placement.offscreen > 0 {
        // A map cut off at the edge must not read as a finished picture. The
        // view follows the cursor, so moving it is what reaches the rest.
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {} more — arrow to reach them ", placement.offscreen),
                pal.dim,
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, rows[0]);

    // Index into the placed boxes by node, so link routing can find both ends
    // fast. A node with no entry is scrolled off the view.
    let mut slot_of: HashMap<usize, usize> = HashMap::new();
    for (n, p) in placement.boxes.iter().enumerate() {
        slot_of.insert(p.node, n);
    }

    let glyphs = Glyphs::for_markers(markers);
    let mut ink = Ink::new(inner.width as usize, inner.height as usize);

    // Lines first, so a box always sits on top of whatever runs past it. One
    // line per pair of files, however many symbols they share — the parallel
    // bundles were most of what made this unreadable.
    for l in view.links() {
        let (Some(&a), Some(&b)) = (slot_of.get(&l.a), slot_of.get(&l.b)) else {
            continue;
        };
        let hot = l.a == selected || l.b == selected;
        let (a, b) = (&placement.boxes[a], &placement.boxes[b]);
        // Routing twice for a mutual link retraces the same path and lands an
        // arrowhead on each end, which is exactly what "both ways" looks like.
        if l.a_to_b {
            route(&mut ink, a, b, hot, &glyphs);
        }
        if l.b_to_a {
            route(&mut ink, b, a, hot, &glyphs);
        }
    }

    for p in &placement.boxes {
        let style = if p.node == selected {
            InkStyle::Selected
        } else if near.contains(&p.node) {
            InkStyle::Near
        } else {
            InkStyle::Far
        };
        ink.draw_box(
            p,
            &projectmap::label_of(&view.graph.nodes[p.node].name),
            style,
            &glyphs,
        );
    }

    for g in &placement.folders {
        let files = if g.files == 1 { "file" } else { "files" };
        ink.write(
            0,
            g.row as i32,
            &format!("{}  {} {files}", g.label, g.files),
            InkStyle::Folder,
        );
    }
    f.render_widget(Paragraph::new(ink.render(&glyphs, &pal)), inner);

    if detail_h > 0 {
        draw_map_detail(f, app, view, rows[1]);
    }
}

/// How brightly one cell of the graph is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InkStyle {
    /// A folder heading. Not part of the picture — it names the group of
    /// boxes under it — so it is written into the grid rather than over the
    /// finished row, and a line running past it keeps the cells it had.
    Folder,
    /// Everything not connected to the cursor.
    Far,
    /// The file under the cursor, and the lines reaching it.
    Near,
    /// The file under the cursor itself.
    Selected,
}

/// The characters the graph is drawn out of.
///
/// Two sets, chosen by the `markers` setting, so a terminal with no box-drawing
/// characters gets a picture made of `+`, `-` and `|` rather than a screen of
/// replacement glyphs. Boxes use rounded corners where they exist, which keeps
/// them visually separate from the sharp corners a line turns with.
struct Glyphs {
    /// Indexed by the direction bits a line leaves a cell by.
    lines: [char; 16],
    corners: [char; 4],
    /// Left, right, up, down — drawn where a line enters the box it points at.
    arrows: [char; 4],
    horizontal: char,
    vertical: char,
}

impl Glyphs {
    fn for_markers(markers: Markers) -> Self {
        match markers {
            Markers::Arrows => Self {
                lines: [
                    ' ', '│', '│', '│', '─', '┘', '┐', '┤', '─', '└', '┌', '├', '─', '┴', '┬', '┼',
                ],
                corners: ['╭', '╮', '╰', '╯'],
                arrows: ['◂', '▸', '▴', '▾'],
                horizontal: '─',
                vertical: '│',
            },
            Markers::Ascii => Self {
                lines: [
                    ' ', '|', '|', '|', '-', '+', '+', '+', '-', '+', '+', '+', '-', '+', '+', '+',
                ],
                corners: ['+', '+', '+', '+'],
                arrows: ['<', '>', '^', 'v'],
                horizontal: '-',
                vertical: '|',
            },
        }
    }
}

// Which way a line leaves a cell. Kept as bits so two lines crossing produce
// the right junction on their own, without anyone having to notice they met.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// The character grid the graph is composed on before it becomes a widget.
///
/// Two layers: lines, held as direction bits so crossings merge themselves,
/// and text, which is the boxes and always wins. Composing into a grid rather
/// than emitting spans as we go is what lets an edge be drawn without knowing
/// what else has already been drawn where it lands.
struct Ink {
    w: usize,
    h: usize,
    bits: Vec<u8>,
    /// Set where a line touches the file under the cursor, so it stays legible
    /// where dimmer lines cross it.
    hot: Vec<bool>,
    text: Vec<Option<(char, InkStyle)>>,
}

impl Ink {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![0; w * h],
            hot: vec![false; w * h],
            text: vec![None; w * h],
        }
    }

    /// Bounds-checked index. Everything else goes through this, so a route
    /// that runs off the edge is clipped instead of panicking.
    fn at(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return None;
        }
        Some(y as usize * self.w + x as usize)
    }

    fn mark(&mut self, x: i32, y: i32, dirs: u8, hot: bool) {
        if let Some(i) = self.at(x, y) {
            self.bits[i] |= dirs;
            self.hot[i] |= hot;
        }
    }

    fn put(&mut self, x: i32, y: i32, ch: char, style: InkStyle) {
        if let Some(i) = self.at(x, y) {
            self.text[i] = Some((ch, style));
        }
    }

    /// A run of horizontal line. Endpoints only get the bit pointing along the
    /// run, so a corner formed with a vertical run turns properly.
    fn hline(&mut self, y: i32, x1: i32, x2: i32, hot: bool) {
        let (a, b) = (x1.min(x2), x1.max(x2));
        for x in a..=b {
            let mut dirs = 0;
            if x > a {
                dirs |= LEFT;
            }
            if x < b {
                dirs |= RIGHT;
            }
            self.mark(x, y, dirs, hot);
        }
    }

    fn vline(&mut self, x: i32, y1: i32, y2: i32, hot: bool) {
        let (a, b) = (y1.min(y2), y1.max(y2));
        for y in a..=b {
            let mut dirs = 0;
            if y > a {
                dirs |= UP;
            }
            if y < b {
                dirs |= DOWN;
            }
            self.mark(x, y, dirs, hot);
        }
    }

    fn draw_box(&mut self, p: &Placed, name: &str, style: InkStyle, g: &Glyphs) {
        let (x0, y0) = (p.col as i32, p.row as i32);
        let x1 = x0 + p.width as i32 - 1;
        let y1 = y0 + Placed::HEIGHT as i32 - 1;
        self.put(x0, y0, g.corners[0], style);
        self.put(x1, y0, g.corners[1], style);
        self.put(x0, y1, g.corners[2], style);
        self.put(x1, y1, g.corners[3], style);
        for x in x0 + 1..x1 {
            self.put(x, y0, g.horizontal, style);
            self.put(x, y1, g.horizontal, style);
        }
        self.put(x0, y0 + 1, g.vertical, style);
        self.put(x1, y0 + 1, g.vertical, style);
        for (i, c) in name.chars().enumerate() {
            self.put(x0 + 1 + i as i32, y0 + 1, c, style);
        }
    }

    /// Write a run of text starting at a cell, clipped at the right edge.
    fn write(&mut self, x: i32, y: i32, text: &str, style: InkStyle) {
        for (i, c) in text.chars().enumerate() {
            self.put(x + i as i32, y, c, style);
        }
    }

    /// Turn the grid into one `Line` per row, merging neighbouring cells that
    /// share a style so a row is a handful of spans rather than one per column.
    fn render(&self, g: &Glyphs, pal: &Palette) -> Vec<Line<'static>> {
        let style_of = |s: InkStyle| match s {
            InkStyle::Selected => pal.text.add_modifier(Modifier::REVERSED),
            InkStyle::Near => pal.text.add_modifier(Modifier::BOLD),
            InkStyle::Far => pal.text,
            InkStyle::Folder => pal.heading,
        };
        let mut out = Vec::with_capacity(self.h);
        for y in 0..self.h {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_style: Option<Style> = None;
            for x in 0..self.w {
                let i = y * self.w + x;
                let (ch, style) = match self.text[i] {
                    Some((ch, s)) => (ch, style_of(s)),
                    None if self.bits[i] != 0 => (
                        g.lines[self.bits[i] as usize],
                        if self.hot[i] { pal.text } else { pal.dim },
                    ),
                    None => (' ', pal.dim),
                };
                if run_style != Some(style) {
                    if !run.is_empty() {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            run_style.unwrap_or(pal.dim),
                        ));
                    }
                    run_style = Some(style);
                }
                run.push(ch);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, run_style.unwrap_or(pal.dim)));
            }
            out.push(Line::from(spans));
        }
        out
    }
}

/// Draw the connection from one box to another as two turns of a line.
///
/// Orthogonal on purpose. A character grid has no diagonals worth the name —
/// a diagonal has to be faked out of dots, and reads as a smear rather than as
/// a diagram. Right angles are what anyone drawing this on paper would use, and
/// they land on cell boundaries exactly.
///
/// Which way the line leaves depends on where the other box is: side to side
/// when there is clear air between them, top to bottom when one is above the
/// other. Either way it goes out, across the middle, and back in, so the turn
/// happens in the gutter between boxes rather than over one of them.
fn route(ink: &mut Ink, a: &Placed, b: &Placed, hot: bool, g: &Glyphs) {
    let (arrow, ax, ay, bx, by, vertical) = if b.col > a.right() {
        // Clear air to the right.
        (
            g.arrows[1],
            a.right() as i32,
            a.middle() as i32,
            b.col as i32 - 1,
            b.middle() as i32,
            false,
        )
    } else if a.col > b.right() {
        (
            g.arrows[0],
            a.col as i32 - 1,
            a.middle() as i32,
            b.right() as i32,
            b.middle() as i32,
            false,
        )
    } else if b.row > a.row {
        // Overlapping columns: go under.
        (
            g.arrows[3],
            a.centre() as i32,
            a.bottom() as i32,
            b.centre() as i32,
            b.row as i32 - 1,
            true,
        )
    } else if a.row > b.row {
        (
            g.arrows[2],
            a.centre() as i32,
            a.row as i32 - 1,
            b.centre() as i32,
            b.bottom() as i32,
            true,
        )
    } else {
        return; // the same slot: nothing to draw between them
    };

    if vertical {
        let mid = (ay + by) / 2;
        ink.vline(ax, ay, mid, hot);
        ink.hline(mid, ax, bx, hot);
        ink.vline(bx, mid, by, hot);
    } else {
        let mid = (ax + bx) / 2;
        ink.hline(ay, ax, mid, hot);
        ink.vline(mid, ay, by, hot);
        ink.hline(by, mid, bx, hot);
    }
    // The arrowhead says which way the connection runs, which is the whole
    // difference between "calls" and "is called by".
    if let Some(i) = ink.at(bx, by) {
        ink.text[i] = Some((arrow, if hot { InkStyle::Near } else { InkStyle::Far }));
        ink.hot[i] |= hot;
    }
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

/// Human-readable name for an edge kind. Also labels the `1`-`3` toggles, so
/// the words on screen match the words in the help and the README.
fn kind_word(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Wikilink => "wikilink",
        EdgeKind::Link => "link",
        EdgeKind::Call => "call",
    }
}

/// The strip under the map: what the cursor is on, what it connects to, and
/// which edge kinds are switched on.
fn draw_map_detail(f: &mut Frame, app: &App, view: &ProjectMap, area: Rect) {
    let pal = app.palette;
    let Some(node) = view.selected_node() else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("  nothing selected", pal.dim))),
            area,
        );
        return;
    };
    let (out, incoming) = view.connections(view.selected);

    // Which edge kinds are switched on, as the 1-3 keys see them.
    let toggles: Vec<Span> = [EdgeKind::Wikilink, EdgeKind::Link, EdgeKind::Call]
        .iter()
        .enumerate()
        .flat_map(|(i, k)| {
            let on = view.kinds[crate::projectmap::kind_index(*k)];
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
    // One bar, so the sigil is a readout rather than a mode you chose: it
    // follows what has been typed, and changes under you the moment the line
    // starts or stops with a `*`.
    let command = b.is_command();
    let sigil = if command { " * " } else { " / " };
    // The sigil is drawn as its own block, so the typed `*` is not shown a
    // second time — and the cursor has to shift back past it to match.
    let (text, cursor) = if command {
        (b.command(), b.cursor.saturating_sub(1))
    } else {
        (b.input.as_str(), b.cursor)
    };
    let (before, after) = split_at_char(text, cursor);

    // What Tab would fill in, shown in grey ahead of the cursor. Only at the
    // end of the line: in the middle of one it would read as text that is
    // already there rather than as an offer.
    let ghost = if command && b.cursor == b.input.chars().count() {
        crate::app::completion_for(b, app.tree.root_path(), app.config.show_hidden)
            .unwrap_or_default()
    } else {
        String::new()
    };

    let hint = if command {
        "  Tab or → complete · Enter run · Esc close"
    } else if b.results.is_empty() {
        "  names and contents · * for a command · Esc close"
    } else {
        "  up/down pick · Enter jump · Esc close"
    };
    // The real cursor lives in the editor, so the bar draws its own — on the
    // next character of the line, or on the first character being offered when
    // there is nothing left of the line. Sitting it on a blank instead would
    // push the offer along by a space and break the word in half.
    let (cursor_char, ghost) = match (after.chars().next(), ghost.chars().next()) {
        (Some(c), _) => (c.to_string(), ghost),
        (None, Some(c)) => (c.to_string(), ghost.chars().skip(1).collect()),
        (None, None) => (" ".to_string(), String::new()),
    };
    let line = Line::from(vec![
        Span::styled(sigil, pal.text.add_modifier(Modifier::REVERSED)),
        Span::styled(before, pal.text),
        Span::styled(cursor_char, pal.text.add_modifier(Modifier::REVERSED)),
        Span::styled(after.chars().skip(1).collect::<String>(), pal.text),
        Span::styled(ghost, pal.dim),
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
                Span::styled("   Enter confirm | Esc cancel", pal.dim),
            ])
        }
        Mode::Confirm(c) => Line::from(vec![
            Span::styled(" ! ", pal.marker.add_modifier(Modifier::REVERSED)),
            Span::styled(format!(" {}", c.message), pal.marker),
        ]),
        _ => {
            if app.project_map.is_some() {
                let line = Line::from(vec![
                    Span::styled(format!(" {}", app.status), pal.text),
                    Span::styled(
                        "   arrows move | Enter open | 1-3 kinds | o unconnected | / filter | Esc back",
                        pal.dim,
                    ),
                ]);
                f.render_widget(Paragraph::new(line).style(pal.text), area);
                return;
            }
            let hints = match (&app.mode, app.focus) {
                (Mode::Settings(_), _) => "up/down pick | Enter change | ^S write | Esc close",
                (Mode::Bar(_), _) => "Esc close",
                (_, Focus::Tree) => "/ search | m map | n new | q quit",
                (_, Focus::Editor) => "^S save | ^Z undo | ^K cut line | Esc back",
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
        Some(ed) if app.focus == Focus::Editor => {
            format!("{}:{} ", ed.cursor_line + 1, ed.cursor_col + 1)
        }
        _ => format!("{}/{} ", app.selected + 1, app.rows.len()),
    }
}

// ---- overlays -------------------------------------------------------------

/// The keybinds window: every action, what it does, and the keys that reach it.
///
/// Grouped by context with a heading for each, because the same key means
/// different things in different panes and a flat list of sixty rows would
/// hide that. A binding that has been changed is drawn brightly, so what you
/// have done to the shipped keyboard is visible at a glance.
fn draw_keybinds(f: &mut Frame, app: &App, area: Rect, kb: &Keybinds) {
    let pal = app.palette;
    let actions: Vec<Action> = Action::all().collect();
    // A heading appears wherever the context changes.
    let mut rows: Vec<BindRow> = KEYBIND_BUTTONS.iter().map(|l| BindRow::Button(l)).collect();
    let mut context = None;
    for (i, action) in actions.iter().enumerate() {
        if context != Some(action.context()) {
            context = Some(action.context());
            rows.push(BindRow::Heading(action.context().title()));
        }
        rows.push(BindRow::Action(i, *action));
    }

    let popup = centred(area, 76, area.height.saturating_sub(4));
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Keybinds ",
            pal.text.add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(
            if kb.capturing {
                " press the key you want it on "
            } else {
                " Enter change · Delete restore · ^S write tiny.conf · Esc back "
            },
            pal.dim,
        )));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    let height = inner.height as usize;
    if height == 0 {
        return;
    }

    // The cursor counts buttons and actions, not headings, so the line it is
    // on has to be looked up rather than indexed.
    let cursor_row = rows
        .iter()
        .position(|r| match r {
            BindRow::Button(_) => kb.selected < KEYBIND_BUTTONS.len(),
            BindRow::Action(i, _) => *i + KEYBIND_BUTTONS.len() == kb.selected,
            BindRow::Heading(_) => false,
        })
        .unwrap_or(0);
    let scroll = keep_visible(cursor_row, height, rows.len());

    // Both columns are measured, not written down: an action with a long name
    // or three keys on it widens the column instead of running into the one
    // after it. A capture in progress shows `…`, which is never the widest.
    let name_w = actions.iter().map(|a| a.name().width()).max().unwrap_or(0);
    let keys_w = actions
        .iter()
        .map(|a| app.keymap.spec(*a).width())
        .max()
        .unwrap_or(0);

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (n, row) in rows.iter().enumerate().skip(scroll).take(height) {
        let selected = n == cursor_row;
        let mut spans = match row {
            BindRow::Heading(title) => vec![Span::styled(title.to_string(), pal.heading)],
            BindRow::Button(label) => vec![Span::styled(
                format!("[ {label} ]"),
                pal.text.add_modifier(Modifier::BOLD),
            )],
            BindRow::Action(_, action) => {
                let changed = app.config.keys.contains_key(action.name());
                let keys = if selected && kb.capturing {
                    "…".to_string()
                } else {
                    app.keymap.spec(*action)
                };
                vec![
                    Span::styled(format!("  {:<w$}  ", action.name(), w = name_w), pal.dim),
                    Span::styled(
                        format!("{keys:<keys_w$}  "),
                        if changed {
                            pal.text.add_modifier(Modifier::BOLD)
                        } else {
                            pal.text
                        },
                    ),
                    Span::styled(action.describe().to_string(), pal.dim),
                ]
            }
        };
        if selected {
            highlight_row(&mut spans, width, pal, true);
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// One line of the keybinds window. Headings are drawn but never landed on.
enum BindRow {
    Heading(&'static str),
    Button(&'static str),
    /// The action, and its place in the cursor's numbering.
    Action(usize, Action),
}

/// The first row to draw so that `selected` is on screen, scrolling no further
/// than it has to.
///
/// Used by the lists that are rebuilt from scratch each frame — search results,
/// the settings, the keybinds. The tree keeps an offset of its own instead,
/// because there the cursor is the thing being moved around a list that stays
/// put, and remembering where you were reading matters.
fn keep_visible(selected: usize, height: usize, total: usize) -> usize {
    selected
        .saturating_sub(height.saturating_sub(1))
        .min(total.saturating_sub(height))
}

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

/// One row of the keys half of `?`.
///
/// Descriptions are labels, not sentences. The window has to hold every
/// binding at once, and its width is set by the longest of them, so a phrase
/// here costs a column on every other row. The README is where the prose
/// lives; this is the thing you glance at.
enum KeyRow {
    Heading(&'static str),
    /// A gap between sections.
    Blank,
    /// The keys are looked up in the live keymap when the window is drawn, so
    /// a rebinding shows here as well as in the keybinds window. Several
    /// actions on one row read as one idea — "move the cursor" is up and down.
    Bound(&'static [Action], &'static str),
    /// A row whose keys cannot be rebound, and so cannot change.
    Fixed(&'static str, &'static str),
}

use KeyRow::{Blank, Bound, Fixed, Heading};

const KEYS: &[KeyRow] = &[
    Heading("TREE"),
    Bound(&[Action::TreeUp, Action::TreeDown], "move"),
    Bound(
        &[Action::TreeJumpUp, Action::TreeJumpDown],
        "five at a time",
    ),
    Bound(&[Action::TreeOpen], "open or close"),
    Bound(&[Action::TreeInto, Action::TreeOut], "in | out"),
    // Kept apart from `TreeLast`: together their key lists are wide enough to
    // stretch the whole column, and width is what the window is short of.
    Bound(&[Action::TreeFirst], "top"),
    Bound(&[Action::TreeLast], "bottom"),
    Bound(&[Action::TreeNew], "new — dot makes a file"),
    Bound(&[Action::TreeRename], "rename"),
    Bound(&[Action::TreeCopy, Action::TreePaste], "copy | paste"),
    Bound(&[Action::Save], "save — a folder saves all"),
    Bound(&[Action::TreeDelete], "delete"),
    Bound(
        &[Action::TreeHidden, Action::TreeRefresh],
        "dotfiles | re-read",
    ),
    Bound(&[Action::ToggleTreePane], "fold the tree away"),
    Blank,
    Heading("PROJECT MAP"),
    Bound(&[Action::TreeMap], "open the map"),
    Bound(&[Action::MapUp, Action::MapDown], "nearest, up | down"),
    Bound(
        &[Action::MapLeft, Action::MapRight],
        "nearest, left | right",
    ),
    Bound(&[Action::MapOpen], "open it"),
    Bound(
        &[Action::MapWikilinks, Action::MapLinks, Action::MapCalls],
        "wikilinks | links | calls",
    ),
    Bound(
        &[Action::MapOrphans, Action::MapReload],
        "orphans | rebuild",
    ),
    Blank,
    Heading("THE BAR"),
    Bound(&[Action::TreeBar], "search"),
    Fixed("*", "star first = a command"),
    Bound(&[Action::Bar, Action::CommandBar], "same, from the editor"),
    Bound(&[Action::TreeSettings], "settings and keybinds"),
    Blank,
    Heading("PREVIEW"),
    Bound(&[Action::ViewUp, Action::ViewDown], "scroll a picture"),
    Fixed("wheel", "one line a notch"),
    Blank,
    Heading("EDITOR"),
    Bound(&[Action::EditorBack], "back to the tree"),
    Bound(&[Action::Save], "save"),
    Bound(&[Action::EditorUndo, Action::EditorRedo], "undo | redo"),
    Bound(&[Action::EditorDeleteLine], "delete the line"),
    Bound(
        &[Action::EditorWordLeft, Action::EditorWordRight],
        "by word",
    ),
    // One action per row: paired, their key lists are wide enough to set the
    // field for the whole table, and width is what this window is short of.
    Bound(
        &[Action::EditorJumpUp, Action::EditorJumpDown],
        "five lines",
    ),
    Bound(&[Action::EditorLineStart], "start of the line"),
    Bound(&[Action::EditorLineEnd], "end of the line"),
    Bound(&[Action::EditorDocStart], "first line"),
    Bound(&[Action::EditorDocEnd], "last line"),
    Blank,
    Bound(&[Action::TreeQuit, Action::Quit], "quit"),
];

/// The keys half of `?`, resolved against what the keys actually do now.
fn key_rows(keymap: &Keymap) -> Vec<(String, String)> {
    KEYS.iter()
        .map(|row| match row {
            Heading(title) => (String::new(), (*title).to_string()),
            Blank => (String::new(), String::new()),
            Fixed(keys, desc) => ((*keys).to_string(), (*desc).to_string()),
            Bound(actions, desc) => {
                // An action bound to nothing contributes nothing, rather than
                // an empty gap in the middle of the row.
                let keys: Vec<String> = actions
                    .iter()
                    .map(|a| keymap.spec(*a))
                    .filter(|s| !s.is_empty())
                    .collect();
                (keys.join("  "), (*desc).to_string())
            }
        })
        .collect()
}

/// The right half of the `?` window: every command, in the form you type it.
///
/// Written with the star, because that is what reaches them — there is one bar
/// and this is how you tell it you meant a command. Anything added to
/// `App::run_command` belongs here, and in `complete_command`'s list too.
///
/// One line per command, not per variation: the quoting rule for `*replace`
/// and the dotted theme keys `*set` accepts are in the README rather than
/// here, where they would have widened the window for everyone.
const COMMANDS: &[(&str, &str)] = &[
    ("", "FILES"),
    ("*copy a to b", "a file or a folder"),
    ("*delete path", "bare = the cursor's"),
    ("*new notes/x.md", "a file"),
    ("*mkdir notes", "a folder"),
    ("", ""),
    ("", "MOVING"),
    ("*line 42", "jump — *42 works too"),
    ("*map", "the project map"),
    ("*reload", "re-read from disk"),
    ("", ""),
    ("", "CHANGING THINGS"),
    ("*replace old new", "across every file"),
    ("*set tab_width 2", "change a setting"),
    ("*config", "settings and keybinds"),
    ("", ""),
    ("", "LEAVING"),
    ("*w  *q  *wq", "save | quit | both"),
    ("*help", "this window"),
];

/// The keymap overlay. Scrollable, because the full list does not fit a short
/// terminal — the footer says which of the two situations you are in.
fn draw_help(f: &mut Frame, app: &App, area: Rect, pal: &Palette, scroll: usize) {
    // Keys on the left, commands on the right, so `?` answers both halves of
    // "how do I do this" at once. Every width here is measured from the tables
    // rather than written down, so adding a row with a long description
    // widens the window instead of quietly losing the end of the line.
    // The keys are read out of the live keymap, so a rebinding shows here and
    // not only in the window that made it.
    let keys = key_rows(&app.keymap);
    let commands: Vec<(String, String)> = COMMANDS
        .iter()
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect();
    let (keys_field, keys_w) = (field_width(&keys), natural_width(&keys));
    let (cmds_field, cmds_w) = (field_width(&commands), natural_width(&commands));

    // One column: the two tables run together with a blank line between, which
    // makes one wider field for both and so has to be measured on the joined
    // list rather than on either half.
    let stacked: Vec<(String, String)> = keys
        .iter()
        .cloned()
        .chain(std::iter::once((String::new(), String::new())))
        .chain(commands.iter().cloned())
        .collect();

    // Two columns need both of them whole, plus the border and a gutter.
    let two_up = (keys_w + cmds_w + 5) as u16 <= area.width.saturating_sub(2);
    let rows = if two_up {
        keys.len().max(commands.len())
    } else {
        stacked.len()
    };
    let width = if two_up {
        (keys_w + cmds_w + 5) as u16
    } else {
        (natural_width(&stacked) + 2) as u16
    };
    let popup = centred(area, width, rows as u16 + 2);
    let scrolls = (popup.height as usize).saturating_sub(2) < rows;

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Keys and commands ",
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
    let scroll = scroll.min(rows.saturating_sub(height));
    if two_up {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(keys_w as u16 + 2), Constraint::Min(0)])
            .split(inner);
        f.render_widget(
            help_column(&keys, scroll, height, keys_field, pal),
            halves[0],
        );
        f.render_widget(
            help_column(&commands, scroll, height, cmds_field, pal),
            halves[1],
        );
    } else {
        let field = field_width(&stacked);
        f.render_widget(help_column(&stacked, scroll, height, field, pal), inner);
    }
}

/// How wide the first field has to be for every row of `rows` to line up.
fn field_width(rows: &[(String, String)]) -> usize {
    rows.iter()
        .filter(|(keys, _)| !keys.is_empty())
        .map(|(keys, _)| keys.width())
        .max()
        .unwrap_or(0)
}

/// How wide the column has to be for no row of it to be cut off. Headings and
/// spacers span the whole column, so they count too.
fn natural_width(rows: &[(String, String)]) -> usize {
    let field = field_width(rows);
    rows.iter()
        .map(|(keys, desc)| {
            if keys.is_empty() {
                desc.width()
            } else {
                field + 2 + desc.width()
            }
        })
        .max()
        .unwrap_or(0)
}

/// One column of the `?` window, already scrolled and clipped to `height`.
fn help_column(
    rows: &[(String, String)],
    scroll: usize,
    height: usize,
    field: usize,
    pal: &Palette,
) -> Paragraph<'static> {
    let lines: Vec<Line> = rows
        .iter()
        .skip(scroll)
        .take(height)
        .map(|(keys, desc)| {
            if keys.is_empty() {
                Line::from(Span::styled(desc.clone(), pal.heading))
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("{keys:<field$}  "),
                        pal.text.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(desc.clone(), pal.dim),
                ])
            }
        })
        .collect();
    Paragraph::new(lines)
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
    let rows = BUTTONS.len() + index.len();
    let popup = centred(area, 80, rows as u16 + 4);
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

    let scroll = keep_visible(s.selected, height, rows);

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);

    // The buttons first, drawn as buttons rather than as settings with no
    // value: bracketed, so it is clear they do something rather than hold
    // something.
    for (i, label) in BUTTONS.iter().enumerate().skip(scroll).take(height) {
        let mut spans = vec![Span::styled(
            format!("[ {label} ]"),
            pal.text.add_modifier(Modifier::BOLD),
        )];
        if i == s.selected {
            highlight_row(&mut spans, width, pal, true);
        }
        lines.push(Line::from(spans));
    }

    for (row, (key, desc)) in index
        .iter()
        .enumerate()
        .map(|(n, r)| (n + BUTTONS.len(), r))
        .skip(scroll.saturating_sub(BUTTONS.len()))
        .take(height.saturating_sub(lines.len()))
    {
        let i = row;
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
