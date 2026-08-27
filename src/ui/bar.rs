//! The two single-row strips: the bar, and the status line.
//!
//! Both can sit at the top or the bottom independently — see
//! [`crate::config::Position`] — so neither knows where it is; [`super::draw`]
//! hands each one a row and they fill it.
//!
//! The bar is one field that is two things: a search when it starts with
//! anything, a command when it starts with the sigil. It decides which
//! keystroke by keystroke rather than being opened in a mode, which is why
//! there is only one of it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::parts::split_at_char;

use crate::app::{App, Focus, Mode};

/// The search (`/`) or command (`:`) line.
///
/// Draws its own block cursor as a reversed span, because the terminal has
/// only one real cursor and the editor pane has a stronger claim on it.
pub(super) fn draw_bar(f: &mut Frame, app: &App, area: Rect) {
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
pub(super) fn draw_status(f: &mut Frame, app: &App, area: Rect) {
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
                        "   arrows move | Enter open | 1-3 kinds | / filter | Esc back",
                        pal.dim,
                    ),
                ]);
                f.render_widget(Paragraph::new(line).style(pal.text), area);
                return;
            }
            let hints = match (&app.mode, app.focus) {
                (Mode::Settings(_), _) => "up/down pick | Enter change | Ctrl+S write | Esc close",
                (Mode::Bar(_), _) => "Esc close",
                // The chords, not the bare letters: they are the ones that
                // work from either pane, so a hint that names them is true
                // wherever it is read. Written out in full rather than as
                // `^S` — this line is the first place most people meet these
                // keys, and a caret is a thing you have to already know.
                (_, Focus::Tree) => "Ctrl+/ search | Ctrl+M map | Ctrl+N new | F1 help",
                (_, Focus::Editor) => "Ctrl+S save | Ctrl+Z undo | Ctrl+K cut | Esc back",
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
