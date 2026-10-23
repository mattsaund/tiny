//! Two panes side by side: `Ctrl+W`, and `Shift+Left` and `Shift+Right`.
//!
//! What the split promises, each checked on its own: the panes are equal, a
//! new one starts on the file you are in, each keeps its own place in a file
//! they share, the browser opens into the pane with the keyboard, and closing
//! keeps the pane you are in. And what it must not do: take `Shift` with an
//! arrow away from the editor when there is only one pane.

use super::*;

/// The fixture, plus code long enough to scroll. Code, because a note in a
/// pane without the keyboard is a rendered page with no cursor in it.
fn with_long_file() -> (tempfile::TempDir, App) {
    let (td, app) = fixture();
    let lines: String = (1..=80).map(|n| format!("x{n} = {n}\n")).collect();
    fs::write(td.path().join("long.py"), lines).unwrap();
    let mut app = app;
    command(&mut app, "reload");
    (td, app)
}

/// The file names in the pane titles, left to right, read off a frame.
fn titles(app: &mut App) -> Vec<String> {
    let top = screen(app, 120, 30).remove(0);
    let mut found: Vec<(usize, String)> = ["README.md", "design.md", "long.py", "main.py"]
        .iter()
        .flat_map(|name| top.match_indices(name).map(|(at, n)| (at, n.to_string())))
        .collect();
    found.sort();
    found.into_iter().map(|(_, n)| n).collect()
}

#[test]
fn ctrl_w_opens_a_second_pane_on_the_file_you_are_in() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));

    assert!(app.split.is_some());
    assert_eq!(
        titles(&mut app),
        ["README.md", "README.md"],
        "the same file, twice"
    );
    assert!(
        app.status.contains("Shift+Left") && app.status.contains("Ctrl+W"),
        "and says how to use it: {}",
        app.status
    );
}

#[test]
fn the_two_panes_are_the_same_width() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    let top = screen(&mut app, 121, 20).remove(0);
    // Three top-left corners: the browser, then the two panes.
    let corners: Vec<usize> = top
        .char_indices()
        .filter(|(_, c)| *c == '\u{250c}')
        .map(|(i, _)| top[..i].chars().count())
        .collect();
    assert_eq!(corners.len(), 3, "{top}");
    let left = corners[2] - corners[1];
    let right = top.chars().count() - corners[2];
    assert!(left.abs_diff(right) <= 1, "{left} and {right}:\n{top}");
}

#[test]
fn each_pane_keeps_its_own_place_in_a_file_they_share() {
    let (td, mut app) = with_long_file();
    let path = td.path().join("long.py");
    select(&mut app, "long.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));
    for _ in 0..30 {
        app.on_key(k(KeyCode::Down));
    }
    let line = |app: &App| app.buffers[&path].cursor_line;
    assert_eq!(line(&app), 30, "the right pane moved");

    app.on_key(shift(KeyCode::Left));
    assert_eq!(
        line(&app),
        0,
        "the left pane is still where the split began"
    );
    app.on_key(shift(KeyCode::Right));
    assert_eq!(line(&app), 30, "and the right one where it was left");
}

#[test]
fn the_browser_opens_files_into_the_pane_with_the_keyboard() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    select(&mut app, "design.md");
    assert_eq!(
        titles(&mut app),
        ["README.md", "design.md"],
        "the right pane took the new file; the left kept its own"
    );

    app.on_key(shift(KeyCode::Left));
    select(&mut app, "main.py");
    assert_eq!(
        titles(&mut app),
        ["main.py", "design.md"],
        "and now the left one does the browsing"
    );
}

#[test]
fn an_edit_shows_in_both_panes_when_they_share_a_file() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));
    type_str(&mut app, "TYPED ");
    let frame = screen(&mut app, 120, 20).join("\n");
    assert_eq!(
        frame.matches("TYPED").count(),
        2,
        "one buffer, two views:\n{frame}"
    );
}

#[test]
fn ctrl_w_again_keeps_the_pane_you_are_in() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    select(&mut app, "design.md");
    app.on_key(shift(KeyCode::Left));
    app.on_key(ctrl('w'));

    assert!(app.split.is_none());
    assert_eq!(
        titles(&mut app),
        ["README.md"],
        "the left pane, which had the keyboard"
    );
}

#[test]
fn with_one_pane_shift_and_an_arrow_still_move_the_cursor() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::End));
    let at = app.buffers[&td.path().join("README.md")].cursor_col;
    app.on_key(shift(KeyCode::Left));
    assert_eq!(
        app.buffers[&td.path().join("README.md")].cursor_col,
        at - 1,
        "no second pane to go to, so the key does what it always did"
    );
}

#[test]
fn a_window_too_narrow_for_two_panes_says_so() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    screen(&mut app, 60, 20);
    app.on_key(ctrl('w'));
    assert!(app.split.is_none());
    assert!(app.status.contains("too narrow"), "{}", app.status);
}

#[test]
fn ctrl_w_from_git_comes_back_to_the_panes() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('g'));
    app.on_key(ctrl('w'));
    assert_eq!(app.window, Window::Main);
    assert!(app.split.is_some());
}

#[test]
fn the_wheel_over_the_other_pane_scrolls_that_pane() {
    let (td, mut app) = with_long_file();
    let path = td.path().join("long.py");
    select(&mut app, "long.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));
    screen(&mut app, 120, 20);
    let (x0, _) = app.last_pinned_cols.expect("two panes on screen");

    for _ in 0..5 {
        app.on_scroll(true, x0 + 2);
    }
    assert_eq!(
        app.buffers[&path].cursor_line, 0,
        "the pane with the keyboard did not move"
    );
    app.on_key(shift(KeyCode::Left));
    assert_eq!(app.buffers[&path].cursor_line, 5, "the other one did");
}
