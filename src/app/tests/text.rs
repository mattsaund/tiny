//! Selecting, copying and pasting in a file, more cursors, recording keys,
//! and patterns: typing as 0.6.0 does it, through the keys.

use super::*;

/// main.py, open in the editor. Its path, to read the buffer back by.
fn editing_main(app: &mut App) -> PathBuf {
    select(app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.active_buffer().expect("main.py is text").path.clone()
}

fn text(app: &App, path: &Path) -> String {
    app.buffers[path].to_text()
}

fn alt_shift(code: KeyCode) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::ALT | KeyModifiers::SHIFT,
        ..k(code)
    }
}

#[test]
fn shift_selects_and_the_clipboard_keys_copy_and_paste() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    for _ in 0..6 {
        app.on_key(shift(KeyCode::Right));
    }
    app.on_key(ctrl('c'));
    assert_eq!(app.status, "copied 6 characters");
    app.on_key(k(KeyCode::End));
    app.on_key(ch(' '));
    app.on_key(ctrl('v'));
    assert!(text(&app, &path).starts_with("import utils import\n"));
}

#[test]
fn with_nothing_selected_cut_takes_the_line_and_paste_puts_it_above() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    app.on_key(ctrl('x'));
    assert_eq!(app.status, "cut 1 line");
    assert!(text(&app, &path).starts_with("\n\ndef main():"));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Down));
    app.on_key(ctrl('v'));
    assert!(
        text(&app, &path).starts_with("\n\nimport utils\ndef main():"),
        "{}",
        text(&app, &path)
    );
}

#[test]
fn ctrl_a_and_typing_replace_everything_and_one_undo_brings_it_back() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    let before = text(&app, &path);
    app.on_key(ctrl('a'));
    type_str(&mut app, "x");
    assert_eq!(text(&app, &path), "x\n");
    app.on_key(ctrl('z'));
    assert_eq!(text(&app, &path), before);
    assert!(
        app.active_buffer().unwrap().has_selection(),
        "and it is selected again"
    );
}

#[test]
fn esc_lets_go_of_a_selection_before_it_leaves_the_file() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    app.on_key(shift(KeyCode::Down));
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Editor, "still in the file");
    assert!(!app.active_buffer().unwrap().has_selection());
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Tree, "and then out of it");
}

#[test]
fn ctrl_shift_and_an_arrow_select_a_word_at_a_time() {
    let (_td, mut app) = fixture();
    editing_main(&mut app);
    app.on_key(ctrl_shift(KeyCode::Right));
    let ed = app.active_buffer().unwrap();
    assert_eq!(ed.selected_text().as_deref(), Some("import "));
}

#[test]
fn alt_n_puts_a_cursor_on_every_match_and_typing_goes_in_at_each() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    for _ in 0..8 {
        app.on_key(k(KeyCode::Right));
    }
    app.on_key(alt(KeyCode::Char('n')));
    app.on_key(alt(KeyCode::Char('n')));
    type_str(&mut app, "lib");
    assert_eq!(
        text(&app, &path),
        "import lib\n\n\ndef main():\n    return lib.load()\n"
    );
    let frame = screen(&mut app, 100, 20).join("\n");
    assert!(
        frame.contains("2 cursors"),
        "the status line counts them:\n{frame}"
    );
    app.on_key(k(KeyCode::Esc));
    assert!(
        app.active_buffer().unwrap().others.is_empty(),
        "Esc lets go"
    );
}

#[test]
fn alt_shift_and_an_arrow_add_a_cursor_below() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    app.on_key(alt_shift(KeyCode::Down));
    type_str(&mut app, "#");
    assert!(text(&app, &path).starts_with("#import utils\n#\n"));
}

#[test]
fn a_recording_plays_back_where_the_cursor_is_now() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("list.txt"), "a\nb\nc\nd\n").unwrap();
    command(&mut app, "reload");
    select(&mut app, "list.txt");
    app.on_key(k(KeyCode::Enter));
    let path = td.path().join("list.txt");

    app.on_key(ctrl('r'));
    assert!(app.is_recording());
    type_str(&mut app, "- ");
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Home));
    app.on_key(ctrl('r'));
    assert!(app.status.starts_with("recorded 4 keys"), "{}", app.status);

    app.on_key(ctrl('p'));
    assert_eq!(text(&app, &path), "- a\n- b\nc\nd\n");
    command(&mut app, "play 2");
    assert_eq!(text(&app, &path), "- a\n- b\n- c\n- d\n");
}

#[test]
fn a_paste_from_the_terminal_goes_in_as_it_was() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    app.on_paste("one\n  two\n".into());
    assert!(
        text(&app, &path).starts_with("one\n  two\nimport utils"),
        "no indent added, and no line lost"
    );
}

#[test]
fn whole_lines_pasted_back_by_the_terminal_still_go_above() {
    let (_td, mut app) = fixture();
    let path = editing_main(&mut app);
    app.on_key(ctrl('c'));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::End));
    app.on_paste("import utils\n".into());
    assert!(
        text(&app, &path).starts_with("import utils\n\n\nimport utils\ndef main():"),
        "{}",
        text(&app, &path)
    );
}

#[test]
fn a_search_between_slashes_is_a_pattern() {
    let (_td, mut app) = fixture();
    type_search(&mut app, "/def \\w+/");
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed");
    };
    assert_eq!(b.results.len(), 1, "{:?}", b.results);
    assert!(b.results[0].path.ends_with("main.py"));
    assert_eq!(b.results[0].line, 3);
}

#[test]
fn a_pattern_that_does_not_read_says_why() {
    let (_td, mut app) = fixture();
    type_search(&mut app, "/(oops/");
    let frame = screen(&mut app, 100, 20).join("\n");
    assert!(frame.contains("not a pattern"), "{frame}");
}

#[test]
fn a_replace_between_slashes_uses_what_it_captured() {
    let (td, mut app) = fixture();
    command(&mut app, "replace /(\\w+)\\.load/ with $1.read in main.py");
    let path = td.path().join("src/main.py");
    assert!(
        text(&app, &path).contains("return utils.read()"),
        "{}",
        app.status
    );
}

#[test]
fn the_cursor_s_shape_is_a_setting() {
    use crate::config::CursorShape;
    let (_td, mut app) = fixture();
    command(&mut app, "set cursor_shape bar");
    assert_eq!(app.config.cursor_shape, CursorShape::Bar);
    command(&mut app, "set cursor_blink false");
    assert!(!app.config.cursor_blink);
    command(&mut app, "set cursor_shape triangle");
    assert!(app.status.contains("must be"), "{}", app.status);
    assert_eq!(app.config.cursor_shape, CursorShape::Bar, "left as it was");
}

#[test]
fn copying_on_a_picture_copies_the_file() {
    let (_td, mut app) = fixture();
    select(&mut app, "logo.png");
    app.focus = Focus::Editor;
    app.on_key(ctrl('c'));
    assert!(app.status.starts_with("copied logo.png"), "{}", app.status);
}
