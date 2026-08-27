//! The help overlay: every key and every command, on one screen.
//!
//! The table is generated from the live [`Keymap`], never from a written list,
//! so a rebound key shows its new binding here and a new action cannot be
//! added without appearing. That is the whole point of the indirection in
//! [`crate::config::keys`]: there is one source of truth for what a key does
//! and this is a view of it.
//!
//! Laid out in two columns when the window is wide enough for both and one
//! when it is not, which is why the column widths are measured rather than
//! fixed.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::parts::centred;

use crate::app::App;
use crate::config::Palette;
use crate::config::keys::{Action, Keymap};

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
    /// Like [`KeyRow::Bound`], but the keys are folded together rather than
    /// listed one action at a time — see [`merge`]. For the rows where four
    /// actions are one gesture, and writing `ctrl+` four times says nothing
    /// the first one did not.
    Merged(&'static [Action], &'static str),
    /// A row whose keys cannot be rebound, and so cannot change.
    Fixed(&'static str, &'static str),
}

use KeyRow::{Blank, Bound, Fixed, Heading, Merged};

/// The keys half of `?`, in the order someone learning tiny needs them.
///
/// Moving first, because that is what you do before anything else; then the
/// things you do to a file; then the windows that open over the top; then how
/// to leave. Every command in the second half is a chord, which is the point
/// of them — see [`crate::config::keys::Context::Global`].
const KEYS: &[KeyRow] = &[
    Heading("MOVING"),
    Merged(
        &[
            Action::TreeUp,
            Action::TreeDown,
            Action::TreeOut,
            Action::TreeInto,
        ],
        "move",
    ),
    Merged(
        &[
            Action::TreeJumpUp,
            Action::TreeJumpDown,
            Action::EditorWordLeft,
            Action::EditorWordRight,
        ],
        "five at a time, or a word",
    ),
    Merged(
        &[
            Action::TreeFirst,
            Action::TreeLast,
            Action::EditorLineStart,
            Action::EditorLineEnd,
        ],
        "to the ends",
    ),
    Bound(&[Action::TreeOpen], "open or close, or edit"),
    Bound(&[Action::TreePreview], "hand the keyboard over"),
    Bound(&[Action::EditorBack], "back — or quit, from the browser"),
    Blank,
    Heading("FILES"),
    Bound(&[Action::New], "new — a dot makes a file"),
    Bound(&[Action::Rename], "rename"),
    Bound(&[Action::Delete], "delete"),
    Bound(&[Action::Save], "save — on a folder, all of it"),
    Bound(&[Action::Copy, Action::Paste], "copy | paste"),
    Bound(&[Action::Hidden], "show dotfiles"),
    Bound(&[Action::Refresh], "re-read from disk"),
    Blank,
    Heading("WINDOWS"),
    Bound(&[Action::Bar], "search — star first, a command"),
    Bound(&[Action::Map], "the project map"),
    Bound(&[Action::Settings], "settings and keybinds"),
    Bound(&[Action::Help], "this window"),
    Blank,
    Heading("THE BROWSER"),
    // Listed rather than merged: two single-character keys folded under one
    // modifier come out as `alt+- =`, which reads as one key and a stray.
    Bound(
        &[Action::PaneNarrower, Action::PaneWider],
        "narrower | wider",
    ),
    Bound(&[Action::ToggleTreePane], "fold it away, and back"),
    Fixed("wheel", "one line a notch"),
    Blank,
    Heading("EDITING"),
    Bound(&[Action::EditorUndo, Action::EditorRedo], "undo | redo"),
    Bound(&[Action::EditorDeleteLine], "delete the line"),
    Bound(
        &[Action::EditorDocStart, Action::EditorDocEnd],
        "first | last line",
    ),
    Blank,
    Bound(&[Action::Quit], "quit"),
];

/// The keys of several actions, folded into one field.
///
/// Two rules, and both are about the same thing: four keys that are one
/// gesture should read as one gesture. A modifier every key shares is written
/// once at the front, so the row for "five at a time" says `ctrl+up down left
/// right` rather than repeating `ctrl+` four times. And named keys come before
/// letters, so the arrows arrive as a group and the `i k j l` that stand in for
/// them arrive as another.
///
/// Everything is read out of the live keymap, so a rebinding shows here. A
/// rebinding that breaks the shared modifier simply falls back to a plain list,
/// which is longer but never wrong.
fn merge(keymap: &Keymap, actions: &[Action]) -> String {
    // Group by the modifier prefix — everything up to and including the last
    // `+` — keeping the order the groups first appeared in.
    let mut groups: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    for spec in actions.iter().flat_map(|a| keymap.keys(*a)) {
        let written = spec.to_string();
        let cut = written.rfind('+').map_or(0, |i| i + 1);
        let (mods, base) = written.split_at(cut);
        let slot = match groups.iter().position(|(m, ..)| m == mods) {
            Some(i) => &mut groups[i],
            None => {
                groups.push((mods.to_string(), Vec::new(), Vec::new()));
                groups.last_mut().expect("just pushed")
            }
        };
        // A single character is a letter standing in for an arrow; anything
        // longer is a named key. They read better apart than interleaved.
        if base.chars().count() == 1 {
            slot.2.push(base.to_string());
        } else {
            slot.1.push(base.to_string());
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (mods, named, letters) in groups {
        for half in [named, letters] {
            if !half.is_empty() {
                out.push(format!("{mods}{}", half.join(" ")));
            }
        }
    }
    out.join("  ")
}

/// The keys half of `?`, resolved against what the keys actually do now.
fn key_rows(keymap: &Keymap) -> Vec<(String, String)> {
    KEYS.iter()
        .map(|row| match row {
            Heading(title) => (String::new(), (*title).to_string()),
            Blank => (String::new(), String::new()),
            Fixed(keys, desc) => ((*keys).to_string(), (*desc).to_string()),
            Merged(actions, desc) => (merge(keymap, actions), (*desc).to_string()),
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
pub(super) fn draw_help(f: &mut Frame, app: &App, area: Rect, pal: &Palette, scroll: usize) {
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
