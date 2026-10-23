//! The terminal under the panes: the screen the shell drew, painted cell by
//! cell, see [`crate::shell`].
//!
//! The emulator keeps every cell's character, colors and attributes, and
//! each becomes a ratatui cell of the same, so a program's colors come out
//! as it chose them. They are brought down to what tiny's own terminal can
//! show along with everything else, at the end of the frame.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::parts::pane;
use crate::app::{App, Focus, Mode};

/// Draw the terminal into `area`, and tell the shell how big that is.
pub(super) fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    app.last_term_area = Some(area);
    let focused = app.focus == Focus::Terminal && matches!(app.mode, Mode::Normal);
    let pal = app.palette;
    let name = app
        .term
        .as_ref()
        .and_then(|s| s.title())
        .unwrap_or("terminal")
        .to_string();
    let title = vec![
        Span::raw(" "),
        Span::styled(
            name,
            if focused {
                pal.text.add_modifier(Modifier::BOLD)
            } else {
                pal.dim
            },
        ),
        Span::raw(" "),
    ];
    let inner = pane(f, area, title, focused, app);
    let Some(shell) = app.term.as_mut() else {
        return;
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    shell.resize(inner.height, inner.width);
    let screen = shell.screen();
    let lines: Vec<Line> = (0..inner.height)
        .map(|row| Line::from(row_spans(screen, row, inner.width)))
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
    if focused && !screen.hide_cursor() && screen.scrollback() == 0 {
        let (row, col) = screen.cursor_position();
        if row < inner.height && col < inner.width {
            f.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
}

/// One row of the screen, as spans of one style each.
fn row_spans(screen: &vt100::Screen, row: u16, width: u16) -> Vec<Span<'static>> {
    let mut spans: Vec<Span> = Vec::new();
    let mut text = String::new();
    let mut style = Style::default();
    let mut col = 0;
    while col < width {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        let here = cell_style(cell);
        if here != style && !text.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut text), style));
        }
        style = here;
        if cell.has_contents() {
            text.push_str(cell.contents());
        } else {
            text.push(' ');
        }
        // A wide character takes the next cell too, which holds nothing of
        // its own.
        col += if cell.is_wide() { 2 } else { 1 };
    }
    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }
    spans
}

/// A cell's colors and attributes, as a style.
fn cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default()
        .fg(color(cell.fgcolor()))
        .bg(color(cell.bgcolor()));
    for (on, m) in [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(m);
        }
    }
    style
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
