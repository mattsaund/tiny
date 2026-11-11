//! The scrollbars: drawn only while there is more than fits, and moved with
//! the mouse, a press on the track or a drag of the thumb.

use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const W: u16 = 90;
const H: u16 = 24;

fn mouse(app: &mut App, kind: MouseEventKind, (x, y): (u16, u16)) {
    app.on_mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

fn press(app: &mut App, at: (u16, u16)) {
    mouse(app, MouseEventKind::Down(MouseButton::Left), at);
}

fn drag(app: &mut App, at: (u16, u16)) {
    mouse(app, MouseEventKind::Drag(MouseButton::Left), at);
}

fn release(app: &mut App, at: (u16, u16)) {
    mouse(app, MouseEventKind::Up(MouseButton::Left), at);
}

/// The fixture with more than fits: sixty notes in the browser, and three
/// long files, as code, as a note and as prose.
fn long_project() -> (tempfile::TempDir, App) {
    let (td, mut app) = fixture();
    for i in 0..60 {
        fs::write(td.path().join(format!("note{i:02}.md")), "x\n").unwrap();
    }
    let lines = |each: &dyn Fn(usize) -> String| (1..=300).map(each).collect::<String>();
    fs::write(
        td.path().join("long.rs"),
        lines(&|i| format!("// line {i}\n")),
    )
    .unwrap();
    fs::write(
        td.path().join("long.md"),
        lines(&|i| format!("Paragraph {i}.\n\n")),
    )
    .unwrap();
    command(&mut app, "reload");
    (td, app)
}

/// The bar the last frame drew for what `which` picks.
fn bar(app: &mut App, which: fn(&Scrolls) -> bool) -> ScrollHit {
    screen(app, W, H);
    app.last_scrollbars
        .iter()
        .find(|h| which(&h.scrolls))
        .cloned()
        .unwrap_or_else(|| panic!("no such bar in {:?}", app.last_scrollbars))
}

/// The bottom of a bar's track, and its thumb's first row.
fn bottom(hit: &ScrollHit) -> (u16, u16) {
    (hit.x, hit.top + hit.track - 1)
}

fn thumb_top(hit: &ScrollHit) -> (u16, u16) {
    let (start, _) = hit.scroll.thumb(hit.track).unwrap();
    (hit.x, hit.top + start)
}

#[test]
fn a_bar_is_drawn_only_while_there_is_more_than_fits() {
    let (_td, mut app) = fixture();
    let rows = screen(&mut app, W, H);
    assert!(app.last_scrollbars.is_empty(), "{:?}", app.last_scrollbars);
    assert!(
        !rows.join("\n").contains('█'),
        "nothing to scroll, no thumb"
    );

    let (_td, mut app) = long_project();
    let hit = bar(&mut app, |s| *s == Scrolls::Browser);
    let rows = screen(&mut app, W, H);
    let (start, len) = hit.scroll.thumb(hit.track).unwrap();
    for y in hit.top..hit.top + hit.track {
        let cell = rows[usize::from(y)]
            .chars()
            .nth(usize::from(hit.x))
            .unwrap();
        let on = (hit.top + start..hit.top + start + len).contains(&y);
        assert_eq!(
            cell == '█',
            on,
            "row {y}: the thumb, and the border round it"
        );
    }
}

#[test]
fn a_press_on_the_browsers_bar_brings_the_list_there_and_the_cursor_along() {
    let (_td, mut app) = long_project();
    let hit = bar(&mut app, |s| *s == Scrolls::Browser);
    press(&mut app, bottom(&hit));
    release(&mut app, bottom(&hit));
    let end = app.rows.len() - hit.scroll.shown;
    assert_eq!(app.tree_scroll, end, "the list's end");
    assert!(
        (end..app.rows.len()).contains(&app.selected),
        "the cursor came into view: {}",
        app.selected
    );
    // And it stays there, the cursor being in view.
    screen(&mut app, W, H);
    assert_eq!(app.tree_scroll, end);
}

#[test]
fn the_thumb_is_taken_where_it_is_pressed_and_follows_a_drag() {
    let (_td, mut app) = long_project();
    let hit = bar(&mut app, |s| *s == Scrolls::Browser);
    let (x, y) = thumb_top(&hit);
    press(&mut app, (x, y));
    assert_eq!(app.tree_scroll, 0, "taking hold of it does not move it");
    drag(&mut app, (x, y + 5));
    let down = app.tree_scroll;
    assert!(down > 0, "it moved with the pointer");
    let (start, _) = Scroll {
        at: down,
        ..hit.scroll
    }
    .thumb(hit.track)
    .unwrap();
    assert_eq!(start, 5, "the thumb is where the pointer took it");
    // Wherever the pointer goes, even off the bar, the thumb follows.
    drag(&mut app, (x + 20, y + 200));
    assert_eq!(app.tree_scroll, app.rows.len() - hit.scroll.shown);
    drag(&mut app, (x, 0));
    assert_eq!(app.tree_scroll, 0);
    release(&mut app, (x, 0));
    // Let go of, a drag is nobody's.
    drag(&mut app, (x, y + 5));
    assert_eq!(app.tree_scroll, 0);
}

#[test]
fn the_editors_bar_moves_the_view_and_keeps_the_cursor_in_it() {
    let (_td, mut app) = long_project();
    select(&mut app, "long.rs");
    app.on_key(k(KeyCode::Enter));
    let hit = bar(&mut app, |s| *s == Scrolls::Lines(None));
    press(&mut app, bottom(&hit));
    release(&mut app, bottom(&hit));
    let ed = app.active_buffer().unwrap();
    assert_eq!(ed.scroll_y, 300 - hit.scroll.shown, "the file's end");
    assert!(ed.cursor_line >= ed.scroll_y, "the cursor came along");
    assert!(joined(&mut app).contains("// line 300"));
    // From the end, a press at the top goes back to the start.
    let hit = bar(&mut app, |s| *s == Scrolls::Lines(None));
    press(&mut app, (hit.x, hit.top));
    release(&mut app, (hit.x, hit.top));
    assert_eq!(app.active_buffer().unwrap().scroll_y, 0);
}

#[test]
fn a_note_being_edited_scrolls_by_its_rows_as_drawn() {
    let (_td, mut app) = long_project();
    select(&mut app, "long.md");
    app.on_key(k(KeyCode::Enter));
    let hit = bar(&mut app, |s| matches!(s, Scrolls::Blocks(_)));
    press(&mut app, bottom(&hit));
    release(&mut app, bottom(&hit));
    let line = app.active_buffer().unwrap().cursor_line;
    assert!(line > 500, "near the end of 600 lines: {line}");
    assert!(joined(&mut app).contains("Paragraph 300."));
}

#[test]
fn a_page_being_read_scrolls_by_its_own_rows() {
    let (_td, mut app) = long_project();
    select(&mut app, "long.md");
    let hit = bar(&mut app, |s| *s == Scrolls::Page(None));
    press(&mut app, bottom(&hit));
    release(&mut app, bottom(&hit));
    assert_eq!(app.preview_scroll, app.preview_len - hit.scroll.shown);
    assert_eq!(app.focus, Focus::Tree, "reading it does not go into it");
    assert!(joined(&mut app).contains("Paragraph 300."));
}

#[test]
fn the_browsers_edge_is_still_taken_beside_its_bar() {
    let (_td, mut app) = long_project();
    let hit = bar(&mut app, |s| *s == Scrolls::Browser);
    let width = app.config.tree_width;
    // The view's border, beside the bar, is the line between them.
    let y = hit.top + 2;
    press(&mut app, (hit.x + 1, y));
    drag(&mut app, (hit.x + 6, y));
    release(&mut app, (hit.x + 6, y));
    assert!(app.config.tree_width > width, "the browser is wider");
    assert_eq!(app.tree_scroll, 0, "and nothing scrolled");
}

#[test]
fn without_borders_the_thumb_is_over_the_last_column() {
    let (td, mut app) = fixture_with(Config {
        borders: false,
        ..Config::default()
    });
    for i in 0..60 {
        fs::write(td.path().join(format!("note{i:02}.md")), "x\n").unwrap();
    }
    command(&mut app, "reload");
    let hit = bar(&mut app, |s| *s == Scrolls::Browser);
    let inner = app.last_tree_inner.unwrap();
    assert_eq!(hit.x, inner.right() - 1);
    press(&mut app, bottom(&hit));
    assert!(app.tree_scroll > 0);
}
