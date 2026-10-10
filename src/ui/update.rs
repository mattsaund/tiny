//! The update question: a newer tiny is out, and do you want it now.
//!
//! Every other yes-or-no in tiny is asked on the status line, because it is
//! about something you just did and you are already looking there. This one
//! arrives on its own, about nothing you did, so it gets a window in the middle
//! of the screen — the only place sure to be seen.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::parts::centered;

use crate::app::App;

/// Draw the question for `version`, centered over whatever is on screen.
pub(super) fn draw_update(f: &mut Frame, app: &App, area: Rect, version: &str) {
    let pal = app.palette;
    let lines = [
        format!("tiny has been updated to version {version},"),
        "would you like to update now?".to_string(),
    ];
    let text_w = lines.iter().map(|l| l.width()).max().unwrap_or(0);
    // Two cells of air on either side of the text, and the border.
    let popup = centered(area, text_w as u16 + 6, 7);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Update ",
            pal.text.add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut body: Vec<Line> = vec![Line::from("")];
    body.extend(
        lines
            .iter()
            .map(|l| Line::from(Span::styled(format!("  {l}"), pal.text))),
    );
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("y", pal.text.add_modifier(Modifier::BOLD)),
        Span::styled(" yes   ", pal.dim),
        Span::styled("n", pal.text.add_modifier(Modifier::BOLD)),
        Span::styled(" no", pal.dim),
    ]));
    f.render_widget(Paragraph::new(body), inner);
}
