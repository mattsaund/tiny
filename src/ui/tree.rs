//! The browser pane, and the result list that drops down in front of it.
//!
//! Two scrolling lists with one highlighted row each, which is why they share
//! [`super::parts::keep_visible`] and [`super::parts::highlight_row`] and
//! differ only in what a row says. They no longer share a rectangle: the tree
//! keeps its pane and the results drop down above both panes, pushing them
//! down, so a search never costs you sight of where you are.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::parts::{highlight_row, keep_visible, mark_query, pane};

use crate::app::{App, Bar, Focus, Mode};
use crate::text::search::{HitKind, Matcher};

/// The project tree. Draws a window of `app.rows` around the cursor.
///
/// Scrolling is resolved here — this is one of the three sanctioned mutations
/// (see the module docs): `tree_scroll` is nudged the minimum needed to keep
/// the selected row on screen, so the list stays still while the cursor moves
/// within the visible region. `last_tree_height` is recorded for the key
/// handler's page-up/page-down.
pub(super) fn draw_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.palette;
    let focused = app.focus == Focus::Tree && matches!(app.mode, Mode::Normal | Mode::Help(_));
    let title = vec![
        Span::raw(" "),
        Span::styled(
            "BROWSER",
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
        // Every arrow is drawn in the text color. An arrow says what a row
        // *is* — you are inside here, this is shut — which is part of reading
        // the row rather than chrome around it, and arrows in two weights
        // read as two kinds of importance that do not exist. The selected row
        // inverts as one piece (see [`highlight_row`]), so its arrow comes
        // out dark against the highlight without needing a colour of its own.
        // The indent is the same weight for the same reason: it is spaces, and
        // the row is one thing.
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
            Span::styled("  ".repeat(row.depth), pal.text),
            Span::styled(marker, pal.text),
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

/// The most of the panes a dropped-down result list is allowed to take.
///
/// A third, so what you are searching *from* stays on screen. The list is a
/// way of aiming at a place in the project; one that fills the window is one
/// you are reading instead.
const RESULTS_SHARE: u16 = 3;

/// How tall the result list wants to be inside `area`.
///
/// As tall as the list itself, up to that share of the window, and never so
/// short there is no room for a row between the borders. Lives here rather
/// than inside [`draw_results`] because the answer decides the layout, and the
/// layout has to be settled before anything is drawn into it.
pub(super) fn results_height(area: Rect, hits: usize) -> u16 {
    let wanted = hits.max(1).saturating_add(2).min(u16::MAX as usize) as u16;
    wanted
        .min((area.height / RESULTS_SHARE).max(3))
        .min(area.height)
}

/// Search results, dropped down from the top of the panes.
///
/// A window of its own rather than a pane in place of the browser: the tree
/// stays beside you and the file keeps showing the highlighted hit with the
/// match marked in it, so stepping through results moves you around the
/// project without taking the project off screen.
///
/// It pushes the panes down instead of covering them, which costs a few rows
/// and buys their titles and their top lines — a list that hides the thing it
/// is pointing at is answering the wrong half of the question.
///
/// Full width for the same reason it used to want the tree's place: a result
/// line carries a path, a line number and a snippet, and there is no
/// filename-width version of that worth reading.
///
/// Note that scrolling is computed into a local, not written back to `b` — the
/// bar is borrowed immutably here, and the offset is cheap to recompute each
/// frame.
pub(super) fn draw_results(f: &mut Frame, app: &mut App, area: Rect, b: &Bar) {
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
    let query = Matcher::new(&b.input);
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
            let text = vec![Span::styled(hit.text.clone(), pal.text)];
            match &query {
                Some(q) => spans.extend(mark_query(text, q)),
                None => spans.extend(text),
            }
        }
        if i == b.selected {
            highlight_row(&mut spans, width, pal, true);
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
