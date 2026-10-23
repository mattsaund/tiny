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
//!  ├─ draw_results       dropped down from the top     tree.rs
//!  └─ two panes, side decided by config
//!      ├─ draw_tree                             tree.rs
//!      └─ draw_preview                         preview.rs
//!           ├─ draw_reading   rendered, no cursor in it
//!           ├─ draw_editor    the file with the real cursor   editor.rs
//!           └─ draw_media     what a picture or a video is
//!  └─ overlays, over whichever window is up
//!      ├─ draw_help                             help.rs
//!      ├─ draw_settings / draw_keybinds         settings.rs
//!      └─ draw_update                           update.rs
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
mod source;
mod tree;
mod update;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use self::bar::{draw_bar, draw_status};
use self::help::draw_help;
use self::map::draw_map;
use self::preview::{draw_pinned, draw_preview};
use self::settings::{draw_keybinds, draw_settings};
use self::source::draw_source;
use self::tree::{draw_results, draw_tree, results_height};
use self::update::draw_update;
use crate::app::MIN_SPLIT_WIDTH;

use crate::app::{App, ConfirmKind, Mode, Window};
use crate::config::Side;

/// Paint one frame. Called once per keypress by the event loop in `main`.
///
/// The vertical layout is assembled rather than hardcoded, because the bar and
/// the status line can each be configured to either end of the window: `order`
/// collects the slots in their final top-to-bottom sequence, then constraints
/// are derived from it. The main area is whatever is left.
///
/// Overlays (help, settings) are drawn against the *full* screen area rather
/// than the main region, so they float above the panes — and over whichever
/// window is up, since every one of them can be opened from all three.
pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Rows: the bar at the top when it is open, the panes, the status line at
    // the bottom. Both used to be settable at either end, which was two
    // settings to describe a window everyone left alone — and a bar that could
    // be anywhere is a bar you have to look for.
    let bar_open = matches!(app.mode, Mode::Bar(_));
    let mut order: Vec<(Slot, u16)> = Vec::new();
    if bar_open {
        order.push((Slot::Bar, bar::bar_height(app)));
    }
    order.push((Slot::Main, 0));
    order.push((Slot::Status, 1));

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

    // Two of the three windows take the whole pane and answer for themselves.
    match app.window {
        Window::Map => draw_map(f, app, main),
        Window::Source => draw_source(f, app, main),
        Window::Main => draw_main(f, app, main),
    }
    draw_overlay(f, app, area);
}

/// What is layered over the window: help, settings, keybinds, and the update
/// question.
fn draw_overlay(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let pal = app.palette;
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
        Mode::Confirm(c) => {
            if let ConfirmKind::Update(version) = &c.kind {
                draw_update(f, app, area, version);
            }
        }
        _ => {}
    }
}

/// The browser and the file, with search results dropped down over them while
/// a search is open.
fn draw_main(f: &mut Frame, app: &mut App, main: ratatui::layout::Rect) {
    // Results drop down from the top of the panes, pushing them down rather
    // than covering them. What you are searching *from* stays on screen — the
    // browser beside you, the file behind the list with the match marked in
    // it — so a search is laid over your place in the project rather than
    // standing in for it.
    let main = match &app.mode {
        Mode::Bar(b) if !b.is_command() => {
            let b = b.clone();
            let h = results_height(main, b.results.len());
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(h), Constraint::Min(3)])
                .split(main);
            draw_results(f, app, rows[0], &b);
            rows[1]
        }
        _ => main,
    };

    // The browser keeps whatever share of the width it was last given —
    // `<` and `>` move it, and `Ctrl+T` takes it away entirely.
    let (side_area, preview_area) = if app.tree_hidden {
        (None, main)
    } else {
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
    // pane it is over. `None` while it is folded away.
    app.last_tree_cols = side_area.map(|a| (a.x, a.x + a.width));

    if let Some(side_area) = side_area {
        draw_tree(f, app, side_area);
    }

    // Two equal panes while split, the live one on whichever side it is. A
    // window that has since become too narrow for two shows the live pane
    // alone, and both come back when it widens again.
    app.last_panes_width = preview_area.width;
    let live_right = app.split.as_ref().map(|s| s.live_right);
    match live_right {
        Some(live_right) if preview_area.width >= MIN_SPLIT_WIDTH => {
            let halves = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
                .split(preview_area);
            let (live, pinned) = if live_right {
                (halves[1], halves[0])
            } else {
                (halves[0], halves[1])
            };
            app.last_pinned_cols = Some((pinned.x, pinned.x + pinned.width));
            // The live pane last, so what it measures (its height, for paging)
            // is what the keys use.
            draw_pinned(f, app, pinned);
            draw_preview(f, app, live);
        }
        _ => {
            app.last_pinned_cols = None;
            draw_preview(f, app, preview_area);
        }
    }
}

/// A horizontal band of the window, used to build the vertical layout in the
/// order the config asks for.
enum Slot {
    Bar,
    Main,
    Status,
}
