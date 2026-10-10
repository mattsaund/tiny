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
//!
//! # A name and a value, and nothing else
//!
//! Every row is the name you would write in `tiny.conf` and what it is set
//! to. The hint that says a word more is drawn only beside the row under the
//! cursor: thirty hints on screen at once is a wall to read past to find the
//! one you wanted, and one at a time is the one you are looking at.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::parts::{centered, highlight_row, keep_visible, split_at_char};

use crate::app::{App, BUTTONS, KEYBIND_BUTTONS, Keybinds, Settings};
use crate::config::Config;
use crate::config::keys::Action;

/// The keybinds window: every action, and the keys that reach it.
///
/// Grouped with a heading for each pane, because the same key means different
/// things in different panes and a flat list would hide that. The movements
/// that work in several panes are one group of their own at the top — they
/// belong to no single pane, and listing them under the first one that happens
/// to have them would be a lie about where they work.
///
/// A binding that has been changed is drawn brightly, so what you have done to
/// the shipped keyboard is visible at a glance.
pub(super) fn draw_keybinds(f: &mut Frame, app: &App, area: Rect, kb: &Keybinds) {
    let pal = app.palette;
    let actions: Vec<Action> = Action::all().collect();
    // A heading appears wherever the group changes.
    let mut rows: Vec<BindRow> = KEYBIND_BUTTONS.iter().map(|l| BindRow::Button(l)).collect();
    let mut group = None;
    for (i, action) in actions.iter().enumerate() {
        if group != Some(action.group()) {
            group = Some(action.group());
            rows.push(BindRow::Heading(action.group()));
        }
        rows.push(BindRow::Action(i, *action));
    }

    // Both columns are measured, not written down: an action with a long name
    // or three keys on it widens the column instead of running into the one
    // after it. A capture in progress shows `…`, which is never the widest.
    let name_w = actions.iter().map(|a| a.name().width()).max().unwrap_or(0);
    let keys_w = actions
        .iter()
        .map(|a| app.keymap.spec(*a).width())
        .max()
        .unwrap_or(0);
    let hint_w = actions
        .iter()
        .map(|a| a.describe().width())
        .max()
        .unwrap_or(0);
    let want = 2 + name_w + 2 + keys_w + 2 + hint_w + 2;

    // As tall as the list, and no taller: a window padded out with empty
    // rows is one more thing to look past.
    let popup = centered(
        area,
        want as u16,
        (rows.len() as u16 + 2).min(area.height.saturating_sub(4)),
    );
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
                " press a key "
            } else {
                " Enter change · Delete reset · Ctrl+S save · Esc back "
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
                let mut spans = vec![
                    Span::styled(format!("  {:<w$}  ", action.name(), w = name_w), pal.dim),
                    Span::styled(
                        format!("{keys:<keys_w$}"),
                        if changed {
                            pal.text.add_modifier(Modifier::BOLD)
                        } else {
                            pal.text
                        },
                    ),
                ];
                if selected && !action.describe().is_empty() {
                    spans.push(Span::styled(format!("  {}", action.describe()), pal.dim));
                }
                spans
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

/// The widest a value is drawn in the settings list. A longer one — a list of
/// a dozen extensions — is cut with `…`, and shown whole once it is being
/// edited. Cut, it can no longer run on into the hint beside it, which is
/// what made a long list read as one sentence with its description.
const VALUE_WIDTH: usize = 24;

/// The settings overlay: every key from `Config::settings_index` and its
/// current value, with a hint beside the one under the cursor.
///
/// A row being edited swaps the value for an inline text field with its own
/// drawn cursor. Values are read live from `app.config`, so a change takes
/// effect on the screen behind the overlay immediately — `Ctrl+S` is what
/// makes it persist to `tiny.conf`.
pub(super) fn draw_settings(f: &mut Frame, app: &App, area: Rect, s: &Settings) {
    let pal = app.palette;
    let index = Config::settings_index();
    let rows = BUTTONS.len() + index.len();
    let name_w = index.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let hint_w = index.iter().map(|(_, h)| h.width()).max().unwrap_or(0);
    let want = name_w + 2 + VALUE_WIDTH + 2 + hint_w + 2;
    let popup = centered(area, want as u16, rows as u16 + 2);
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(Span::styled(
            " Settings ",
            pal.text.add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(
            " Enter change · Ctrl+S save · Esc close ",
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

    for (row, (key, hint)) in index
        .iter()
        .enumerate()
        .map(|(n, r)| (n + BUTTONS.len(), r))
        .skip(scroll.saturating_sub(BUTTONS.len()))
        .take(height.saturating_sub(lines.len()))
    {
        let i = row;
        let selected = i == s.selected;
        let editing = selected && s.editing.is_some();
        let value = match (&s.editing, editing) {
            (Some(buf), true) => buf.clone(),
            _ => app.config.get(key).unwrap_or_default(),
        };
        let mut spans = vec![Span::styled(
            format!("{key:<name_w$}  "),
            pal.text.add_modifier(Modifier::BOLD),
        )];
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
            spans.push(Span::styled(clip(&value, VALUE_WIDTH), pal.text));
            if selected && !hint.is_empty() {
                spans.push(Span::styled(format!("  {hint}"), pal.dim));
            }
            if selected {
                highlight_row(&mut spans, width, pal, true);
            }
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- small helpers --------------------------------------------------------

/// `text` in exactly `width` cells: padded when short, cut with `…` when long.
fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return format!("{text:<width$}");
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + c.to_string().width() >= width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    format!("{out:<width$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_value_is_cut_to_its_column_and_says_so() {
        let cut = clip("md markdown mdown mkd txt text rst org", 12);
        assert_eq!(cut.width(), 12);
        assert!(cut.trim_end().ends_with('…'), "{cut:?}");
        assert_eq!(clip("left", 6), "left  ", "a short one is padded");
        assert_eq!(
            clip("exactly", 7),
            "exactly",
            "and one that fits is left alone"
        );
    }
}
