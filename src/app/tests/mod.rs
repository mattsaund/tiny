//! The fixtures every test here shares, and the tests themselves.
//!
//! One project on a temp directory with one of each thing the panes have to
//! handle — a folder, a markdown note, a source file, a picture that will not
//! decode — plus the helpers that drive an `App` without a terminal: [`screen`]
//! renders a frame into strings, [`select`] puts the cursor on a named row,
//! and [`k`] / [`ch`] / [`ctrl`] build key events.
//!
//! Tests assert on the drawn screen wherever they can. Checking that
//! `app.selected == 3` proves a field changed; checking that the name is on
//! the screen proves the user can see it, which is the thing that was actually
//! promised.

pub(super) use super::*;
pub(super) use crate::app::mode::*;
pub(super) use crate::app::parts::*;
pub(super) use crate::app::preview::*;
pub(super) use crate::config::keys::Action;
pub(super) use crate::config::{Markers, Position, Side};
pub(super) use crossterm::event::KeyEventKind;
pub(super) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(super) use ratatui::Terminal;
pub(super) use ratatui::backend::TestBackend;
pub(super) use ratatui::style::{Color, Modifier};
pub(super) use std::fs;
pub(super) use std::path::{Path, PathBuf};

mod commands;
mod editing;
mod fileops;
mod layout;
mod map;
mod navigation;
mod preview;
mod scrolling;
mod search;
mod settings;
mod single_file;

/// A small project with one of each thing the panes have to handle.
pub(super) fn build(dir: &Path) {
    fs::create_dir_all(dir.join("notes")).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("notes/design.md"),
        "# Design Notes\n\nThe core idea is a split view. See [[architecture]].\n\n- one\n- two\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.py"),
        "import utils\n\n\ndef main():\n    return utils.load()\n",
    )
    .unwrap();
    fs::write(dir.join("README.md"), "# Fixture\n\nhello widget\n").unwrap();
    fs::write(dir.join("logo.png"), [0x89u8, b'P', b'N', b'G', 0, 1, 2, 3]).unwrap();
}

pub(super) fn target(root: &Path, file: Option<PathBuf>) -> project::Target {
    project::Target {
        root: root.to_path_buf(),
        file,
        created: false,
    }
}

pub(super) fn fixture() -> (tempfile::TempDir, App) {
    fixture_with(Config::default())
}

pub(super) fn fixture_with(cfg: Config) -> (tempfile::TempDir, App) {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let app = App::new(target(td.path(), None), cfg, None).unwrap();
    (td, app)
}

pub(super) fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect()
}

pub(super) fn joined(app: &mut App) -> String {
    screen(app, 90, 24).join("\n")
}

/// Text drawn reversed, split by pane: the result list on the left, the
/// preview on the right. Kept apart because both mark their matches, and
/// counting them together would let either one pass for both.
pub(super) fn marked_by_pane(app: &mut App, w: u16, h: u16) -> (String, String) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    // The side pane is `tree_width` of the window; a couple of columns of
    // slack covers its border either way.
    let split = (w as f32 * Config::default().tree_width) as u16 + 2;
    let read = |from: u16, to: u16| -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in from..to {
                match buf.cell((x, y)) {
                    Some(c) if c.modifier.contains(Modifier::REVERSED) => out.push_str(c.symbol()),
                    _ => {}
                }
            }
        }
        out
    };
    (read(0, split), read(split, buf.area.width))
}

/// Just the preview pane's marks.
pub(super) fn marked_in_preview(app: &mut App, w: u16, h: u16) -> String {
    marked_by_pane(app, w, h).1
}

pub(super) fn type_search(app: &mut App, query: &str) {
    app.on_key(ch('/'));
    for c in query.chars() {
        app.on_key(ch(c));
    }
}

pub(super) fn k(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    }
}

pub(super) fn ch(c: char) -> KeyEvent {
    k(KeyCode::Char(c))
}

pub(super) fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::SHIFT,
        ..k(code)
    }
}

pub(super) fn ctrl(c: char) -> KeyEvent {
    ctrl_key(KeyCode::Char(c))
}

pub(super) fn ctrl_key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..k(code)
    }
}

pub(super) fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(ch(c));
    }
}

/// Run a command line through the bar, exactly as a user would.
/// Run a command from wherever the keyboard happens to be.
///
/// Ctrl+P rather than `:`, because in the editor a colon is a colon — the
/// same reason the binding exists at all.
pub(super) fn command(app: &mut App, line: &str) {
    app.on_key(ctrl('p'));
    type_str(app, line);
    app.on_key(k(KeyCode::Enter));
}

/// Type `line` into the command bar, press Tab, and give back what the bar
/// holds afterwards.
pub(super) fn completed(app: &mut App, line: &str) -> String {
    app.on_key(ctrl('p'));
    type_str(app, line);
    app.on_key(k(KeyCode::Tab));
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed")
    };
    let out = b.command().to_string();
    app.on_key(k(KeyCode::Esc));
    out
}

pub(super) fn select(app: &mut App, name: &str) {
    for _ in 0..12 {
        let paths: Vec<PathBuf> = app
            .rows
            .iter()
            .filter(|r| r.is_dir && !r.expanded)
            .map(|r| r.path.clone())
            .collect();
        if paths.is_empty() {
            break;
        }
        for p in paths {
            app.tree.expand(&p);
        }
        app.rows = app.tree.flatten();
    }
    let i = app
        .rows
        .iter()
        .position(|r| r.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no row named {name} in {:?}",
                app.rows.iter().map(|r| &r.name).collect::<Vec<_>>()
            )
        });
    app.selected = i;
    app.sync_preview();
}

/// Open the keybinds window with the cursor on `action`.
pub(super) fn keybinds_on(app: &mut App, action: Action) {
    app.on_key(ch(','));
    app.on_key(k(KeyCode::Enter)); // the Keybinds button is the first row
    let rows = Action::all().position(|a| a == action).unwrap() + KEYBIND_BUTTONS.len();
    for _ in 0..rows {
        app.on_key(k(KeyCode::Down));
    }
}
