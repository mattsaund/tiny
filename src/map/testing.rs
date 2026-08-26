//! Fixtures shared by the map's own tests.
//!
//! [`view`](super::view) and [`layout`](super::layout) are two halves of one
//! `ProjectMap`, so their tests want the same small project to look at: two
//! notes that link to each other, two source files that call across, and one
//! file nothing reaches. Building it in one place keeps the two halves
//! agreeing about what they are describing.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::keys::Action;
use crate::map::graph;
use crate::map::view::ProjectMap;

use crossterm::event::{KeyEventKind, KeyModifiers};
use std::fs;

pub(crate) fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Two notes that link to each other, two code files that call across,
/// and one file connected to nothing.
pub(crate) fn fixture() -> (tempfile::TempDir, ProjectMap) {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "notes/design.md", "see [[architecture]]\n");
    write(td.path(), "notes/architecture.md", "back to [[design]]\n");
    write(td.path(), "src/utils.py", "def load():\n    return 1\n");
    write(td.path(), "src/main.py", "import utils\nutils.load()\n");
    write(td.path(), "notes/alone.md", "nothing points here\n");
    let view = ProjectMap::build(td.path(), &graph::Options::default());
    (td, view)
}

pub(crate) fn k(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    }
}

pub(crate) fn ch(c: char) -> KeyEvent {
    k(KeyCode::Char(c))
}

/// What the shipped keyboard makes of a key. These tests are about the
/// view, not the bindings.
pub(crate) fn act(key: KeyEvent) -> Option<Action> {
    crate::config::keys::Keymap::default().find(crate::config::keys::Context::Map, &key)
}

pub(crate) fn rel_of(view: &ProjectMap, i: usize) -> &str {
    &view.graph.nodes[i].rel
}

pub(crate) fn visible_names(view: &ProjectMap) -> Vec<&str> {
    view.visible_indices()
        .into_iter()
        .map(|i| rel_of(view, i))
        .collect()
}
