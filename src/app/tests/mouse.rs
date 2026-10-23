//! The mouse: a click, a drag, a double and a triple click, and the clicks
//! with Shift and Alt, in each place they land.
//!
//! Every test draws a frame first, the way the screen is drawn before anyone
//! can click on it: a click is read against what the last frame put where.

use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const LEFT: MouseButton = MouseButton::Left;

fn mouse(app: &mut App, kind: MouseEventKind, (x, y): (u16, u16), modifiers: KeyModifiers) {
    app.on_mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers,
    });
}

fn click(app: &mut App, at: (u16, u16)) {
    click_with(app, at, KeyModifiers::NONE);
}

fn click_with(app: &mut App, at: (u16, u16), mods: KeyModifiers) {
    mouse(app, MouseEventKind::Down(LEFT), at, mods);
    mouse(app, MouseEventKind::Up(LEFT), at, mods);
}

/// Where `text` first is on a frame `w` by `h`: its column and its row.
fn find(app: &mut App, w: u16, h: u16, text: &str) -> (u16, u16) {
    let rows = screen(app, w, h);
    for (y, row) in rows.iter().enumerate() {
        if let Some(i) = row.find(text) {
            return (row[..i].chars().count() as u16, y as u16);
        }
    }
    panic!("{text:?} is not on screen:\n{}", rows.join("\n"));
}

/// main.py open in the editor: `return utils.load()` is its line 4.
fn editing_main(app: &mut App) {
    select(app, "main.py");
    app.on_key(k(KeyCode::Enter));
}

fn cursor(app: &App) -> (usize, usize) {
    let ed = app.active_buffer().expect("a file open");
    (ed.cursor_line, ed.cursor_col)
}

fn selected(app: &App) -> Option<String> {
    app.active_buffer().and_then(|ed| ed.selected_text())
}

#[test]
fn a_click_puts_the_cursor_where_it_lands() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    let at = find(&mut app, 100, 20, "utils.load");
    click(&mut app, at);
    assert_eq!(cursor(&app), (4, 11));
    assert_eq!(selected(&app), None, "a click selects nothing");
    // Past the end of a line is the end of it.
    click(&mut app, (at.0 + 40, at.1));
    assert_eq!(cursor(&app), (4, 23));
}

#[test]
fn a_drag_selects_what_it_passes_over() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    let at = find(&mut app, 100, 20, "utils.load");
    mouse(&mut app, MouseEventKind::Down(LEFT), at, KeyModifiers::NONE);
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (at.0 + 5, at.1),
        KeyModifiers::NONE,
    );
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (at.0 + 10, at.1),
        KeyModifiers::NONE,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (at.0 + 10, at.1),
        KeyModifiers::NONE,
    );
    assert_eq!(selected(&app).as_deref(), Some("utils.load"));
    // From a cell along (the same one again would be a double click), and
    // up a line: selected the other way.
    let space = (at.0 - 1, at.1);
    mouse(
        &mut app,
        MouseEventKind::Down(LEFT),
        space,
        KeyModifiers::NONE,
    );
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (space.0, at.1 - 1),
        KeyModifiers::NONE,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (space.0, at.1 - 1),
        KeyModifiers::NONE,
    );
    assert_eq!(selected(&app).as_deref(), Some(":\n    return"));
}

#[test]
fn a_double_click_takes_the_word_and_a_triple_the_line() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    let at = find(&mut app, 100, 20, "utils.load");
    let load = (at.0 + 7, at.1);
    click(&mut app, load);
    click(&mut app, load);
    assert_eq!(selected(&app).as_deref(), Some("load"));
    click(&mut app, load);
    // The file's last line, with no line break after it to take.
    assert_eq!(selected(&app).as_deref(), Some("    return utils.load()"));
    // A double click and a drag goes on a word at a time.
    let utils = at;
    click(&mut app, utils);
    mouse(
        &mut app,
        MouseEventKind::Down(LEFT),
        utils,
        KeyModifiers::NONE,
    );
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        load,
        KeyModifiers::NONE,
    );
    mouse(&mut app, MouseEventKind::Up(LEFT), load, KeyModifiers::NONE);
    assert_eq!(selected(&app).as_deref(), Some("utils.load"));
}

#[test]
fn a_click_on_the_line_numbers_takes_the_line() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    let at = find(&mut app, 100, 20, "utils.load");
    // The gutter is just left of the text: the number, then a space.
    let pane = app.last_panes.iter().find(|p| p.pinned.is_none()).unwrap();
    let gutter = pane.text.as_ref().unwrap().area.x - 1;
    click(&mut app, (gutter, at.1 - 1));
    assert_eq!(selected(&app).as_deref(), Some("def main():\n"));
}

#[test]
fn shift_and_a_click_selects_to_there_and_alt_adds_a_cursor() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    let at = find(&mut app, 100, 20, "utils.load");
    click(&mut app, at);
    click_with(&mut app, (at.0 + 5, at.1), KeyModifiers::SHIFT);
    assert_eq!(selected(&app).as_deref(), Some("utils"));
    click_with(&mut app, (at.0 + 10, at.1), KeyModifiers::SHIFT);
    assert_eq!(
        selected(&app).as_deref(),
        Some("utils.load"),
        "from where it began"
    );

    click(&mut app, at);
    click_with(&mut app, (at.0 - 7, at.1 - 4), KeyModifiers::ALT);
    assert_eq!(app.active_buffer().unwrap().others.len(), 1);
    assert!(app.status.contains("2 cursors"), "{}", app.status);
    type_str(&mut app, "X");
    let text = app.active_buffer().unwrap().to_text();
    assert!(text.contains("return Xutils"), "{text}");
}

#[test]
fn a_drag_past_the_bottom_goes_on_down_the_file() {
    let (td, mut app) = fixture();
    let lines: String = (0..100).map(|n| format!("line {n}\n")).collect();
    fs::write(td.path().join("long.txt"), lines).unwrap();
    command(&mut app, "reload");
    select(&mut app, "long.txt");
    app.on_key(k(KeyCode::Enter));
    let at = find(&mut app, 80, 12, "line 0");
    mouse(&mut app, MouseEventKind::Down(LEFT), at, KeyModifiers::NONE);
    for _ in 0..20 {
        // Below the pane, on the status line: a line further each time, the
        // view following.
        mouse(
            &mut app,
            MouseEventKind::Drag(LEFT),
            (at.0, 11),
            KeyModifiers::NONE,
        );
        screen(&mut app, 80, 12);
    }
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (at.0, 11),
        KeyModifiers::NONE,
    );
    let (line, _) = cursor(&app);
    assert!(line > 15, "dragged on to line {line}");
    assert!(selected(&app).is_some_and(|s| s.starts_with("line 0\nline 1\n")));
}

#[test]
fn a_click_in_the_browser_picks_a_row_and_a_second_opens_it() {
    let (_td, mut app) = fixture();
    let at = find(&mut app, 100, 20, "README.md");
    click(&mut app, at);
    assert_eq!(app.selected_row().unwrap().name, "README.md");
    assert_eq!(app.focus, Focus::Tree);
    click(&mut app, at);
    assert_eq!(app.focus, Focus::Editor, "the second click opens it");
    // From the editor, a click back in the browser puts the keyboard there.
    let notes = find(&mut app, 100, 20, "notes");
    click(&mut app, notes);
    assert_eq!(app.focus, Focus::Tree);
    assert_eq!(app.selected_row().unwrap().name, "notes");
}

#[test]
fn a_folder_opens_and_closes_with_a_second_click() {
    let (_td, mut app) = fixture();
    let at = find(&mut app, 100, 20, "src");
    click(&mut app, at);
    click(&mut app, at);
    let frame = screen(&mut app, 100, 20).join("\n");
    assert!(frame.contains("main.py"), "open:\n{frame}");
    click(&mut app, at);
    let frame = screen(&mut app, 100, 20).join("\n");
    assert!(!frame.contains("main.py"), "and shut again:\n{frame}");
}

#[test]
fn a_click_in_another_pane_gives_it_the_keyboard() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    app.on_key(ctrl('w'));
    screen(&mut app, 120, 30);
    let live = app.split.as_ref().unwrap().live;
    let other = app
        .last_panes
        .iter()
        .find(|p| p.pinned.is_some())
        .cloned()
        .expect("a second pane on screen");
    let text = other.text.expect("drawn as lines");
    click(&mut app, (text.area.x + 2, text.area.y + 4));
    let split = app.split.as_ref().unwrap();
    assert_eq!(Some(split.live), other.pinned);
    assert_ne!(split.live, live);
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(cursor(&app), (4, 2), "and the cursor where it landed");
}

#[test]
fn a_click_on_a_search_result_goes_there() {
    let (_td, mut app) = fixture();
    type_search(&mut app, "widget");
    let at = find(&mut app, 100, 20, "hello widget");
    click(&mut app, at);
    assert!(matches!(app.mode, Mode::Normal), "the bar went");
    assert!(app.preview.path().unwrap().ends_with("README.md"));
    assert_eq!(cursor(&app).0, 2, "on the line it found");
}

#[test]
fn a_click_on_the_bar_keeps_it_and_one_elsewhere_puts_it_away() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('f'));
    screen(&mut app, 100, 20);
    let bar = app.last_bar_area.expect("the bar on screen");
    click(&mut app, (bar.x + 3, bar.y));
    assert!(matches!(app.mode, Mode::Bar(_)));
    let at = find(&mut app, 100, 20, "README.md");
    click(&mut app, at);
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(
        app.selected_row().unwrap().name,
        "README.md",
        "and the click landed"
    );
}

#[test]
fn a_click_puts_the_help_away() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    screen(&mut app, 100, 40);
    click(&mut app, (5, 5));
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn a_click_on_the_map_picks_a_box_and_a_second_opens_it() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('l'));
    screen(&mut app, 120, 40);
    let selected = app.project_map.as_ref().unwrap().selected;
    let &(area, node) = app
        .last_map_boxes
        .iter()
        .find(|(_, n)| *n != selected)
        .expect("more than one box");
    let at = (area.x + 1, area.y + 1);
    click(&mut app, at);
    assert_eq!(app.project_map.as_ref().unwrap().selected, node);
    click(&mut app, at);
    assert_eq!(app.window, Window::Main, "the second click opens it");
}
