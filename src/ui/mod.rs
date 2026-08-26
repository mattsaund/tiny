//! All drawing, one file per pane.
//!
//! The split from `app` is deliberate: `app` never touches ratatui types
//! beyond styles, and nothing in here mutates state except the three things
//! listed below, which cannot be resolved until the pane size is known.
//!
//! # Where each pane lives
//!
//! [`draw`] is the entry point, and the only thing in this file. It decides
//! the layout and hands each rectangle to the module that fills it:
//!
//! ```text
//! draw                                        (here)
//!  ├─ draw_bar / draw_status                   bar.rs
//!  ├─ draw_map            the whole screen     map.rs   → ink.rs
//!  └─ two panes, side decided by config
//!      ├─ draw_tree  or  draw_results          tree.rs
//!      └─ draw_preview                         preview.rs
//!           ├─ draw_reading   rendered, no cursor in it
//!           ├─ draw_editor    the file with the real cursor   editor.rs
//!           └─ draw_media     what a picture or a video is
//!  └─ draw_help                                help.rs
//!  └─ draw_settings / draw_keybinds             settings.rs
//! ```
//!
//! [`parts`] holds what they all share: the border, the selected row, the
//! marking of a search hit, the small text helpers. A pane that disagrees with
//! another about how something looks has usually stopped calling through it.
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
//! - **The highlight cache.** Which lines the parser has to reach is decided
//!   by which lines are on screen — see [`crate::text::highlight::Resume`]. It
//!   is filled in by the parse that had to happen anyway.
//!
//! Everything else must stay read-only. If you find yourself wanting to set
//! application state from a draw function, it almost certainly belongs in
//! `app` instead.
//!
//! # Styling rules
//!
//! Nothing here writes a literal color. Every style comes from `app.palette`,
//! and the shipped palette names no color at all — meaning is carried by bold,
//! dim, underline and reverse, so tiny inherits the user's terminal theme. The
//! one exception is syntax highlighting, whose colors come from the syntect
//! theme by design.
//!
//! Widths are measured with `unicode-width`, never `chars().count()`. A CJK
//! character is one character and two cells, and confusing the two is how
//! borders end up misaligned.

mod bar;
mod editor;
mod help;
mod ink;
mod map;
mod parts;
mod preview;
mod settings;
mod tree;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use self::bar::{draw_bar, draw_status};
use self::help::draw_help;
use self::map::draw_map;
use self::preview::draw_preview;
use self::settings::{draw_keybinds, draw_settings};
use self::tree::{draw_results, draw_tree};

use crate::app::{App, Mode};
use crate::config::{Position, Side};

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
