//! The right pane, and what it decides to be.
//!
//! [`draw_preview`] is a dispatch: it builds the title, then hands the inside
//! of the pane to whichever drawing the current [`Preview`] calls for; the
//! editor for a text file, a rendered page for prose the keyboard has not
//! reached yet, a description for a picture, a line or two of facts for
//! anything else.
//!
//! The rule the `EDIT` / `VIEW` tag encodes: prose and markdown are rendered
//! while the tree has the keyboard and become the editor the moment the pane
//! is focused; code is the editor either way. The tag says which keyboard you
//! have, not whether there is formatting on screen: markdown keeps its
//! formatting while you type into it.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::editor::draw_editor;
use super::parts::{FORMAT_LIMIT, label, mark_query, pane};

use crate::app::{App, Focus, Mode, PaneHit, Preview, TextHits, TextKind};
use crate::text::markdown;
use crate::text::search::Matcher;
use crate::text::tabs;

/// The right-hand pane. Dispatches on `app.preview` to one of the specific
/// drawing functions, and builds the title that names the file and its state.
///
/// The `VIEW` / `EDIT` tag says which keyboard you have: `VIEW` while the
/// browser has it and the file is a picture of itself, `EDIT` once Enter has
/// put it in the file. `EDIT` on markdown is still formatted (see
/// `ui::editor::live_rows`), so the tag is about the keyboard, not about
/// whether there is formatting on screen.
pub(super) fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Editor && matches!(app.mode, Mode::Normal | Mode::Help(_));
    let text = draw_pane(f, app, area, focused);
    app.last_panes.push(PaneHit {
        pinned: None,
        area,
        text,
    });
}

/// A pane without the keyboard, while the main window is split.
///
/// Drawn by lending it the live pane's state for the length of one draw (the
/// preview, its scroll, the highlighting, and the buffer's view when it shows
/// a file) and taking all of it back after. So it is drawn by exactly the code
/// that draws the live pane, looking as that pane would without the keyboard,
/// and nothing about the live pane is touched by it. See `app::split`.
///
/// `again` says whether a pane drawn after this one shows the same file, and
/// so needs to know what was edited in it as well.
pub(super) fn draw_pinned(f: &mut Frame, app: &mut App, id: usize, area: Rect, again: bool) {
    let Some(mut split) = app.split.take() else {
        return;
    };
    let Some((_, pane)) = split.pinned.iter_mut().find(|(p, _)| *p == id) else {
        app.split = Some(split);
        return;
    };
    std::mem::swap(&mut app.preview, &mut pane.preview);
    std::mem::swap(&mut app.preview_scroll, &mut pane.preview_scroll);
    std::mem::swap(&mut app.highlight_cache, &mut pane.highlight);
    let (len, height) = (app.preview_len, app.last_edit_height);

    // The buffer's own view is what the live pane is using; this pane's view
    // goes in for the draw, and the live one comes back after.
    let path = app.preview.path().map(|p| p.to_path_buf());
    let mut lent = None;
    let mut mark = None;
    if let Some(ed) = path.as_ref().and_then(|p| app.buffers.get_mut(p)) {
        mark = ed.touched();
        if let Some(view) = pane.view.take() {
            lent = Some(ed.view());
            ed.set_view(view);
        }
    }

    let text = draw_pane(f, app, area, false);
    app.last_panes.push(PaneHit {
        pinned: Some(id),
        area,
        text,
    });

    if let Some(ed) = path.as_ref().and_then(|p| app.buffers.get_mut(p)) {
        if let Some(own) = lent {
            pane.view = Some(ed.view());
            ed.set_view(own);
        }
        // Drawing took the mark of what changed; a pane drawn after this one
        // showing the same file needs it too.
        if again {
            ed.retouch(mark);
        }
    }
    std::mem::swap(&mut app.highlight_cache, &mut pane.highlight);
    std::mem::swap(&mut app.preview_scroll, &mut pane.preview_scroll);
    std::mem::swap(&mut app.preview, &mut pane.preview);
    (app.preview_len, app.last_edit_height) = (len, height);
    app.split = Some(split);
}

/// One pane, with or without the keyboard. Returns where its lines were
/// drawn, when it showed a file's lines, for the mouse.
pub(super) fn draw_pane(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    focused: bool,
) -> Option<TextHits> {
    let pal = app.palette;

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
        return None;
    }

    // Prepared once per frame, before the pane borrows `app` mutably. This is
    // what makes stepping through search results show where each hit sits in
    // the file, and not only which file it is in.
    let query = app.live_query().and_then(Matcher::new);

    match app.preview.clone() {
        Preview::Buffer { path, kind } => {
            // Rendered while the tree still has the keyboard: the preview is
            // a picture of the file then, with no cursor in it. The moment it
            // is focused it becomes the editor, markdown formatting and all.
            if kind.reads_first() && !focused {
                draw_reading(f, app, area, inner, &path, kind, query.as_ref());
            } else {
                return draw_editor(f, app, inner, &path, kind, focused, query.as_ref());
            }
        }
        Preview::Media {
            path,
            kind,
            size,
            info,
        } => draw_media(f, app, inner, &path, kind, size, &info),
        Preview::Directory { path, entries } => {
            let lines = vec![
                Line::from(Span::styled(label(&path), pal.heading)),
                Line::from(""),
                Line::from(Span::styled(
                    format!("{entries} entr{}", if entries == 1 { "y" } else { "ies" }),
                    pal.text,
                )),
                Line::from(""),
                // The command rather than a key: it works wherever you happen
                // to be when you read it.
                Line::from(Span::styled("right to open   *new for a file", pal.dim)),
            ];
            app.preview_len = lines.len();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Preview::Binary { path, size, kind } => {
            let lines = vec![
                Line::from(Span::styled(label(&path), pal.heading)),
                Line::from(""),
                Line::from(Span::styled(
                    format!("{kind}, {}", exact_size(size)),
                    pal.text,
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "not a text file, nothing to edit here",
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
    None
}

/// What a picture, a video, a PDF or a sound *is*, in words.
///
/// tiny stopped drawing media into the terminal (see the [`crate::files::media`]
/// module docs for why) so this pane is a description and an invitation:
/// the facts `media::probe` could read, then the one key that opens the file
/// in the viewer the desktop already has for it.
///
/// Facts that could not be read are simply absent rather than shown as
/// `unknown`, and `info.note` says why underneath. A video on a machine
/// without ffprobe therefore reads as a name, a format and a size, plus a line
/// naming the tool that would fill in the rest.
fn draw_media(
    f: &mut Frame,
    app: &mut App,
    inner: Rect,
    path: &std::path::Path,
    kind: crate::files::media::Kind,
    size: u64,
    info: &crate::files::media::Info,
) {
    let pal = app.palette;
    let mut lines = vec![
        Line::from(Span::styled(label(path), pal.heading)),
        Line::from(""),
        Line::from(Span::styled(
            format!("{} {}", crate::files::media::format_name(path), kind.noun()),
            pal.text,
        )),
    ];
    for fact in [info.resolution(pal.glyphs.times), info.runtime()]
        .into_iter()
        .flatten()
    {
        lines.push(Line::from(Span::styled(fact, pal.text)));
    }
    lines.push(Line::from(Span::styled(exact_size(size), pal.text)));
    if let Some(note) = &info.note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(note.clone(), pal.marker)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("Enter opens it in your {}", opener_name(kind)),
        pal.dim,
    )));

    app.preview_len = lines.len();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// What to call the program Enter will hand the file to.
///
/// Deliberately vague: "your image viewer", not a program name. Which one it
/// actually is depends on the desktop's own association, and tiny does not
/// know it and should not claim to.
fn opener_name(kind: crate::files::media::Kind) -> &'static str {
    use crate::files::media::Kind;
    match kind {
        Kind::Video => "video player",
        Kind::Pdf => "PDF viewer",
        Kind::Audio => "music player",
        Kind::Image | Kind::Other => "image viewer",
    }
}

/// Rendered markdown, or wrapped prose, with a scroll indicator.
///
/// Only ever drawn for a preview the keyboard has not reached yet: focus the
/// pane and it becomes the editor. Nothing here has a cursor, which is exactly
/// why markdown can be rendered whole; there is no block to leave raw.
///
/// Renders from the *buffer* rather than from disk, which is what makes
/// unsaved edits show up here the moment you step back out to the tree.
///
/// # Why the whole document, every frame
///
/// Formatting a document is linear in its length, and this redoes all of it on
/// every keypress, then throws away everything outside the visible window. The
/// alternative is a cache, which would need invalidating whenever the buffer,
/// the width or the palette changed: three things that change for different
/// reasons. At note scale the render is a fraction of a millisecond, so the
/// cache would be complexity bought with nothing.
///
/// [`FORMAT_LIMIT`] is what keeps that true. Past it the document is shown as
/// its own source, one row per line, and only the rows on screen are built,
/// which is both cheap and exactly what the editor would show for the same
/// file.
///
/// `preview_len` is written back so the key handler knows how far it can
/// scroll.
fn draw_reading(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    inner: Rect,
    path: &std::path::Path,
    kind: TextKind,
    query: Option<&Matcher>,
) {
    let pal = app.palette;
    let tab_width = app.config.tab_width;
    let Some(ed) = app.buffers.get(path) else {
        return;
    };
    let width = inner.width as usize;
    let height = inner.height as usize;
    app.last_edit_height = height;

    // Past the ceiling, formatting the whole file on every frame is what would
    // make a keystroke slow, so the file is shown as itself and only the rows
    // on screen are built. One source line is one row, which is what makes the
    // length knowable without producing any of them.
    let (lines, scroll) = if ed.line_count() > FORMAT_LIMIT {
        app.preview_len = ed.line_count();
        let scroll = app
            .preview_scroll
            .min(ed.line_count().saturating_sub(height));
        let rows = ed
            .lines()
            .iter()
            .skip(scroll)
            .take(height)
            .map(|l| {
                let l = tabs::expand(l, tab_width, 0);
                Line::from(Span::styled(clip_to(&l, width), pal.text))
            })
            .collect();
        (rows, scroll)
    } else {
        // Render from the buffer, not from disk, so unsaved edits show up the
        // moment you step back out of the editor.
        let source = ed.to_text();
        let rows = match kind {
            TextKind::Markdown => markdown::render(&source, width, &pal, &app.highlighter),
            // Prose keeps the author's line structure and wraps what is long.
            _ => markdown::render_plain(&source, width, &pal, tab_width),
        };
        app.preview_len = rows.len();
        let scroll = app.preview_scroll.min(rows.len().saturating_sub(height));
        (
            rows.into_iter()
                .skip(scroll)
                .take(height)
                .collect::<Vec<_>>(),
            scroll,
        )
    };
    app.preview_scroll = scroll;
    let max_scroll = app.preview_len.saturating_sub(height);

    // Marked after slicing, so the cost is per visible row rather than per
    // line of the document.
    let view: Vec<Line> = lines
        .into_iter()
        .map(|line| match query {
            Some(q) => Line::from(mark_query(line.spans, q)),
            None => line,
        })
        .collect();
    f.render_widget(Paragraph::new(view).style(pal.text), inner);

    // Scroll position, on the bottom border so it never covers a line of text.
    // The guard decides whether to draw at all, so `checked_div` would not
    // replace it: a note that fits on screen should show no indicator.
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

/// One source line, cut to the pane's width.
///
/// Only used for a document too long to format, where the alternative to
/// clipping is wrapping, and wrapping would mean measuring every line in the
/// file to know how many rows it takes, which is the cost this path exists to
/// avoid.
fn clip_to(line: &str, width: usize) -> String {
    let mut out = String::with_capacity(width);
    let mut used = 0;
    for c in line.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// A file's size the way the status line says it, so the two never round the
/// same file two ways.
fn exact_size(bytes: u64) -> String {
    crate::files::size::human(crate::files::size::Size {
        bytes,
        complete: true,
    })
}
