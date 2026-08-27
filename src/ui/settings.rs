//! The two editing overlays: settings, and key bindings.
//!
//! Both are the same shape — a scrolling list of name-and-value rows, one of
//! them selected, with the selected value editable in place — so they share
//! [`super::parts::keep_visible`], [`super::parts::highlight_row`] and
//! [`super::parts::split_at_char`] and differ only in where the rows come
//! from. Settings reads [`crate::config::Config::settings_index`]; keybinds
//! reads the keymap.
//!
//! Neither writes anything. A change takes effect on screen immediately and is
//! only persisted by an explicit `Ctrl+S`, so a setting can be tried and
//! backed out of without the file ever changing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::parts::{centred, highlight_row, keep_visible, split_at_char};

use crate::app::{App, BUTTONS, KEYBIND_BUTTONS, Keybinds, Settings};
use crate::config::Config;
use crate::config::keys::Action;

/// The keybinds window: every action, what it does, and the keys that reach it.
///
/// Grouped by context with a heading for each, because the same key means
/// different things in different panes and a flat list of sixty rows would
/// hide that. A binding that has been changed is drawn brightly, so what you
/// have done to the shipped keyboard is visible at a glance.
pub(super) fn draw_keybinds(f: &mut Frame, app: &App, area: Rect, kb: &Keybinds) {
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
                " Enter change · Delete restore · Ctrl+S write tiny.conf · Esc back "
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

/// The settings overlay: every key from `Config::settings_index`, its current
/// value, and its description.
///
/// A row being edited swaps the description for an inline text field with its
/// own drawn cursor. Values are read live from `app.config`, so a change takes
/// effect on the screen behind the overlay immediately — `Ctrl+S` is what
/// makes it persist to `tiny.conf`.
pub(super) fn draw_settings(f: &mut Frame, app: &App, area: Rect, s: &Settings) {
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
            " Enter change · Ctrl+S write tiny.conf · Esc close ",
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
