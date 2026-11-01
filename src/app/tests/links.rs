//! Following a link with the mouse: a click on one drawn formatted, Ctrl and
//! a click on one in text being edited, and where each kind goes.

use super::*;
use crate::text::markdown::Target;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn click_with(app: &mut App, (x, y): (u16, u16), modifiers: KeyModifiers) {
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.on_mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers,
        });
    }
}

/// Where the last frame drew the link going to `target`.
fn link_to(app: &mut App, w: u16, h: u16, target: &crate::text::markdown::Target) -> (u16, u16) {
    screen(app, w, h);
    let hit = app
        .last_links
        .iter()
        .find(|l| l.target == *target)
        .unwrap_or_else(|| panic!("no link to {target:?} drawn: {:?}", app.last_links));
    (hit.area.x, hit.area.y)
}

fn open_note(app: &mut App, td: &tempfile::TempDir, name: &str, text: &str) {
    fs::write(td.path().join(name), text).unwrap();
    command(app, "reload");
    select(app, name);
}

#[test]
fn a_click_on_a_link_in_a_note_being_read_opens_what_it_names() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("notes/architecture.md"), "# Architecture\n").unwrap();
    // Read, not edited: the browser keeps the keyboard.
    open_note(
        &mut app,
        &td,
        "start.md",
        "See the [design](notes/design.md) first.\n",
    );
    assert_eq!(app.focus, Focus::Tree);
    let at = link_to(&mut app, 100, 20, &Target::Path("notes/design.md".into()));
    click_with(&mut app, at, KeyModifiers::NONE);
    assert!(
        app.preview
            .path()
            .is_some_and(|p| p.ends_with("notes/design.md")),
        "{:?} {}",
        app.preview.path(),
        app.status
    );
    // And a wikilink in that one, to the note of that name.
    let at = link_to(&mut app, 100, 20, &Target::Wiki("architecture".into()));
    click_with(&mut app, at, KeyModifiers::NONE);
    assert!(
        app.preview
            .path()
            .is_some_and(|p| p.ends_with("notes/architecture.md")),
        "{}",
        app.status
    );
}

#[test]
fn a_link_up_a_folder_opens_the_file_there() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("top.md"), "# Top\n").unwrap();
    fs::write(
        td.path().join("notes/up.md"),
        "Back to [the top](../top.md).\n",
    )
    .unwrap();
    command(&mut app, "reload");
    select(&mut app, "up.md");
    let at = link_to(&mut app, 100, 20, &Target::Path("../top.md".into()));
    click_with(&mut app, at, KeyModifiers::NONE);
    let opened = app.preview.path().map(Path::to_path_buf);
    assert!(
        opened.as_ref().is_some_and(|p| p.ends_with("top.md")
            && !p.ends_with("notes/top.md")
            && !p.components().any(|c| c == std::path::Component::ParentDir)),
        "{opened:?} {}",
        app.status
    );
    assert!(!app.config.show_hidden, "a `..` is not a hidden folder");
}

#[test]
fn a_web_address_opens_in_the_browser() {
    let (td, mut app) = fixture();
    open_note(
        &mut app,
        &td,
        "links.md",
        "Docs at <https://example.com/docs> and more.\n",
    );
    crate::files::media::opened::take();
    let at = link_to(
        &mut app,
        100,
        20,
        &Target::Web("https://example.com/docs".into()),
    );
    click_with(&mut app, at, KeyModifiers::NONE);
    assert_eq!(
        crate::files::media::opened::take(),
        [std::path::PathBuf::from("https://example.com/docs")]
    );
    assert!(app.status.contains("opened"), "{}", app.status);
}

#[test]
fn in_text_being_edited_a_click_places_the_cursor_and_ctrl_follows() {
    let (td, mut app) = fixture();
    open_note(
        &mut app,
        &td,
        "tool.py",
        "# see https://example.com/tool here\nx = 1\n",
    );
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);
    crate::files::media::opened::take();
    let web = Target::Web("https://example.com/tool".into());
    let at = link_to(&mut app, 100, 20, &web);
    click_with(&mut app, (at.0 + 3, at.1), KeyModifiers::NONE);
    assert!(
        crate::files::media::opened::take().is_empty(),
        "a plain click edits"
    );
    let ed = app.active_buffer().unwrap();
    assert_eq!(ed.cursor_line, 0, "the cursor went there instead");

    let at = link_to(&mut app, 100, 20, &web);
    click_with(&mut app, at, KeyModifiers::CONTROL);
    assert_eq!(
        crate::files::media::opened::take().len(),
        1,
        "Ctrl and a click follows"
    );
}

#[test]
fn a_link_to_a_heading_opens_the_note_there() {
    let (td, mut app) = fixture();
    fs::write(
        td.path().join("guide.md"),
        "# Guide\n\nintro\n\n## Getting Started\n\nsteps\n",
    )
    .unwrap();
    open_note(
        &mut app,
        &td,
        "start.md",
        "Read [how](guide.md#getting-started).\n",
    );
    let at = link_to(
        &mut app,
        100,
        20,
        &Target::Path("guide.md#getting-started".into()),
    );
    click_with(&mut app, at, KeyModifiers::NONE);
    let ed = app.active_buffer().expect("the guide is open");
    assert!(ed.path.ends_with("guide.md"));
    assert_eq!(ed.cursor_line, 4, "{}", app.status);
}

#[test]
fn a_link_to_nothing_says_so() {
    let (td, mut app) = fixture();
    open_note(
        &mut app,
        &td,
        "start.md",
        "A [[ghost]] and a [lost](gone.md) link.\n",
    );
    let at = link_to(&mut app, 100, 20, &Target::Wiki("ghost".into()));
    click_with(&mut app, at, KeyModifiers::NONE);
    assert!(app.status.contains("[[ghost]]"), "{}", app.status);
    assert!(
        app.preview.path().is_some_and(|p| p.ends_with("start.md")),
        "nothing else opened"
    );
}
