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
    Bound(&[Action::MapReload], "rebuild"),
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
