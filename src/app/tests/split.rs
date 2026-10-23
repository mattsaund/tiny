//! The panes: `Ctrl+W`, `*split`, `*split down` and `*close`.
//!
//! What the panes promise, each checked on its own: they share the space
//! equally, a new one starts on the file you are in, each keeps its own
//! place in a file they share, the browser opens into the pane with the
//! keyboard, and closing hands the keyboard to the pane before. And what
//! they must not do: take `Shift` with an arrow away from the editor, where
//! it selects.

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
    let mut found: Vec<(usize, String)> =
        ["README.md", "INTRO.md", "design.md", "long.py", "main.py"]
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
        app.status.contains("Ctrl+W"),
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

    app.on_key(ctrl('w'));
    assert_eq!(
        line(&app),
        0,
        "the left pane is still where the split began"
    );
    app.on_key(ctrl('w'));
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

    app.on_key(ctrl('w'));
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
fn closing_a_pane_hands_the_keyboard_to_the_one_before() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    select(&mut app, "design.md");
    command(&mut app, "close");

    assert!(app.split.is_none());
    assert_eq!(
        titles(&mut app),
        ["README.md"],
        "the left pane, which was before it"
    );
    command(&mut app, "close");
    assert!(app.status.contains("only one"), "{}", app.status);
}

#[test]
fn shift_and_an_arrow_select_whatever_the_panes() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));
    app.on_key(k(KeyCode::End));
    let at = app.buffers[&td.path().join("README.md")].cursor_col;
    app.on_key(shift(KeyCode::Left));
    let ed = &app.buffers[&td.path().join("README.md")];
    assert_eq!(ed.cursor_col, at - 1, "the cursor moved");
    assert!(ed.has_selection(), "selecting as it went");
}

#[test]
fn panes_go_side_by_side_and_stacked_in_any_mix() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    screen(&mut app, 160, 40);
    command(&mut app, "split");
    screen(&mut app, 160, 40);
    command(&mut app, "split down");
    screen(&mut app, 160, 40);
    assert_eq!(app.pane_count(), 3);
    let live = app.last_live_area;
    let mut pinned: Vec<_> = app.last_pinned.iter().map(|(_, r)| *r).collect();
    pinned.sort_by_key(|r| (r.x, r.y));
    let (left, above) = (pinned[0], pinned[1]);
    assert_eq!(
        left.height,
        live.height + above.height,
        "the first, full height"
    );
    assert_eq!(above.x, live.x, "the other two, one above the other");
    assert!(above.y < live.y, "the new one below, with the keyboard");

    // Ctrl+W goes round all three, in reading order.
    let mut seen = vec![live];
    for _ in 0..3 {
        app.on_key(ctrl('w'));
        screen(&mut app, 160, 40);
        seen.push(app.last_live_area);
    }
    assert_eq!(seen[0], seen[3], "and back to the start");
    assert_eq!(seen[1], left, "from the last to the first");
    assert_eq!(seen[2], above);
}

#[test]
fn three_side_by_side_are_thirds() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('t'));
    select(&mut app, "README.md");
    screen(&mut app, 150, 30);
    command(&mut app, "split");
    screen(&mut app, 150, 30);
    command(&mut app, "split");
    screen(&mut app, 150, 30);
    let mut widths: Vec<u16> = app.last_pinned.iter().map(|(_, r)| r.width).collect();
    widths.push(app.last_live_area.width);
    assert_eq!(widths, [50, 50, 50], "{widths:?}");
    command(&mut app, "close others");
    assert!(app.split.is_none());
    assert_eq!(app.pane_count(), 1);
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
    let (_, pinned) = *app.last_pinned.first().expect("two panes on screen");

    for _ in 0..5 {
        app.on_scroll(true, pinned.x + 2, pinned.y + 2);
    }
    assert_eq!(
        app.buffers[&path].cursor_line, 0,
        "the pane with the keyboard did not move"
    );
    app.on_key(ctrl('w'));
    assert_eq!(app.buffers[&path].cursor_line, 5, "the other one did");
}

#[test]
fn the_other_pane_follows_its_file_through_a_rename() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    select(&mut app, "design.md");
    command(&mut app, "rename README.md to INTRO.md");
    // The browser goes to the renamed file, as a rename always does, and the
    // left pane went with its file.
    assert_eq!(
        titles(&mut app),
        ["INTRO.md", "INTRO.md"],
        "the left pane shows the file under its new name"
    );
    app.on_key(ctrl('w'));
    assert!(
        app.active_buffer().is_some(),
        "and taking it up gives the keyboard a file to type into"
    );
}

#[test]
fn the_other_pane_says_when_its_file_is_deleted() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    select(&mut app, "design.md");
    command(&mut app, "delete README.md");
    app.on_key(ch('y'));
    let frame = screen(&mut app, 120, 20).join("\n");
    assert!(frame.contains("README.md is gone"), "{frame}");
}

#[test]
fn ctrl_w_from_another_window_brings_you_back_to_the_panes() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('w'));
    let live = app.split.as_ref().unwrap().live;
    app.on_key(ctrl('g'));
    app.on_key(ctrl('w'));
    assert_eq!(app.window, Window::Main, "Ctrl+W brings you to them");
    assert_eq!(
        app.split.as_ref().unwrap().live,
        live,
        "the panes were not on screen, so the keyboard stays where it was"
    );
}

#[test]
fn moving_to_the_other_pane_ends_a_run_of_typing() {
    let (td, mut app) = with_long_file();
    let path = td.path().join("long.py");
    select(&mut app, "long.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));
    for _ in 0..30 {
        app.on_key(k(KeyCode::Down));
    }
    type_str(&mut app, "foo");
    app.on_key(ctrl('w'));
    type_str(&mut app, "bar");
    app.on_key(ctrl('z'));
    let text = app.buffers[&path].to_text();
    assert!(!text.contains("bar"), "the second pane's typing is undone");
    assert!(text.contains("foo"), "and the first pane's is not: {text}");
}

#[test]
fn the_other_pane_reads_its_file_again_after_a_rename() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("notes.txt"), "plain words\n").unwrap();
    command(&mut app, "reload");
    select(&mut app, "notes.txt");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('w'));
    // The keyboard goes back to the browser, and the live pane on to another
    // file, leaving notes.txt in the pane set aside.
    app.on_key(k(KeyCode::Esc));
    select(&mut app, "README.md");
    command(&mut app, "rename notes.txt to notes.rs");
    let pinned = &app.split.as_ref().expect("still two panes").pinned[0]
        .1
        .preview;
    assert!(
        matches!(pinned, Preview::Buffer { path, kind: TextKind::Code } if path.ends_with("notes.rs")),
        "{pinned:?}"
    );
}
