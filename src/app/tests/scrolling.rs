//! Scrolling: by key, by wheel, and by jumping to a line.
//!
//! The rule under all of these is that the view moves the minimum needed to
//! keep the cursor visible, so the text stays still while the cursor moves
//! within the window. A pane that scrolls on every keystroke is a pane that is
//! hard to read.

use super::*;

#[test]
fn the_line_command_puts_the_cursor_on_that_line() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    command(&mut app, "line 4");
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(
        app.active_buffer().unwrap().cursor_line,
        3,
        "counted from 1"
    );
    assert!(app.status.contains("line 4"), "{}", app.status);
}

#[test]
fn a_bare_number_is_a_line_number_too() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    command(&mut app, "3");
    assert_eq!(app.active_buffer().unwrap().cursor_line, 2);
    // And the wordier forms mean the same thing.
    command(&mut app, "go to 2");
    assert_eq!(app.active_buffer().unwrap().cursor_line, 1);
}

#[test]
fn a_line_past_the_end_lands_on_the_last_one() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    let last = app.active_buffer().unwrap().line_count() - 1;
    command(&mut app, "line 900");
    assert_eq!(app.active_buffer().unwrap().cursor_line, last);
}

#[test]
fn the_line_command_says_when_there_is_nothing_to_jump_inside() {
    let (_td, mut app) = fixture();
    select(&mut app, "notes");
    command(&mut app, "line 4");
    assert!(app.status.contains("no file open"), "{}", app.status);
    command(&mut app, "line zero");
    assert!(app.status.contains("not a line number"), "{}", app.status);
}

#[test]
fn a_wheel_notch_moves_exactly_one_line() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    assert_eq!(app.focus, Focus::Tree, "the note is only being previewed");
    // Draw once so the preview knows how long it is.
    joined(&mut app);

    app.on_scroll(true, 60);
    assert_eq!(app.preview_scroll, 1, "one notch, one line");
    app.on_scroll(false, 60);
    assert_eq!(app.preview_scroll, 0);
    app.on_scroll(false, 60);
    assert_eq!(app.preview_scroll, 0, "and it stops at the top");
}

#[test]
fn the_wheel_moves_the_cursor_in_a_file_being_edited() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);
    joined(&mut app);

    app.on_scroll(true, 60);
    assert_eq!(app.active_buffer().unwrap().cursor_line, 1);
    app.on_scroll(false, 60);
    assert_eq!(app.active_buffer().unwrap().cursor_line, 0);
}

#[test]
fn the_wheel_over_the_tree_moves_the_tree_cursor() {
    let (_td, mut app) = fixture();
    joined(&mut app);
    let (x0, _) = app.last_tree_cols.expect("the tree was drawn");
    assert_eq!(app.selected, 0);

    app.on_scroll(true, x0 + 1);
    assert_eq!(app.selected, 1, "one notch, one row");
    app.on_scroll(false, x0 + 1);
    assert_eq!(app.selected, 0);
}

#[test]
fn one_bar_switches_between_searching_and_commanding_as_you_type() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(!b.is_command(), "plain text searches");
    assert!(!b.results.is_empty(), "and found something");

    // Clear it and lead with the sigil instead.
    for _ in 0.."widget".len() {
        app.on_key(k(KeyCode::Backspace));
    }
    type_str(&mut app, "*copy");
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(b.is_command(), "a leading * is a command");
    assert_eq!(b.command(), "copy");
    assert!(b.results.is_empty(), "a command has nothing to list");
}

#[test]
fn deleting_the_sigil_turns_a_command_back_into_a_search() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "*widget");
    assert!(matches!(&app.mode, Mode::Bar(b) if b.results.is_empty()));

    // Back to the front and take the star off.
    app.on_key(k(KeyCode::Home));
    app.on_key(k(KeyCode::Delete));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(!b.is_command());
    assert!(!b.results.is_empty(), "the results come straight back");
}

#[test]
fn a_command_typed_into_the_search_bar_runs() {
    let (td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "*copy README.md to notes");
    app.on_key(k(KeyCode::Enter));
    assert!(
        td.path().join("notes/README.md").is_file(),
        "{}",
        app.status
    );
}

#[test]
fn ctrl_p_opens_the_bar_with_the_sigil_already_there() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    let Mode::Bar(b) = &app.mode else {
        panic!("no bar")
    };
    assert_eq!(b.input, "*");
    assert!(b.is_command());
    assert_eq!(b.cursor, 1, "and the cursor is past it");
}

#[test]
fn the_bar_shows_which_of_the_two_it_currently_is() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    assert!(joined(&mut app).contains(" / "), "a search shows a slash");
    app.on_key(k(KeyCode::Home));
    type_str(&mut app, "*");
    assert!(joined(&mut app).contains(" * "), "a command shows a star");
}

#[test]
fn a_colon_is_just_a_colon_now() {
    let (_td, mut app) = fixture();
    app.on_key(ch(':'));
    assert!(
        matches!(app.mode, Mode::Normal),
        "the bar is reached by / and Ctrl+P; it does not need a third key"
    );
}

#[test]
fn the_bar_shows_the_star_once_not_twice() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    type_str(&mut app, "copy");
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" * "))
        .expect("the bar is drawn");
    assert!(bar.contains("* copy"), "{bar}");
    assert!(!bar.contains("**"), "the sigil is the block, not the text");
}

#[test]
fn the_bar_offers_the_completion_in_grey_before_tab() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    type_str(&mut app, "cop");
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" * "))
        .expect("the bar is drawn");
    assert!(
        bar.contains("* copy"),
        "the y is offered before Tab is pressed:\n{bar}"
    );

    // And Tab takes up exactly what was offered.
    app.on_key(k(KeyCode::Tab));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.command(), "copy");
}

#[test]
fn the_offer_is_only_made_at_the_end_of_the_line() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    type_str(&mut app, "cop");
    app.on_key(k(KeyCode::Home));
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" * "))
        .expect("the bar is drawn");
    assert!(
        !bar.contains("copy"),
        "mid-line it would read as text already there:\n{bar}"
    );
}

#[test]
fn a_search_is_not_offered_a_command_completion() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "cop");
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" / "))
        .expect("the bar is drawn");
    assert!(!bar.contains("copy"), "{bar}");
}

#[test]
fn the_right_arrow_takes_up_the_suggestion_too() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    type_str(&mut app, "cop");
    app.on_key(k(KeyCode::Right));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.command(), "copy", "the arrow does what Tab does");
    assert_eq!(b.cursor, b.input.chars().count(), "and lands at the end");
}

#[test]
fn the_right_arrow_still_moves_along_the_text_mid_line() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    type_str(&mut app, "cop");
    app.on_key(k(KeyCode::Home));
    app.on_key(k(KeyCode::Right));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.command(), "cop", "nothing was completed");
    assert_eq!(b.cursor, 1, "it just moved");
}

#[test]
fn the_right_arrow_leaves_a_search_alone() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "wid");
    app.on_key(k(KeyCode::Right));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.input, "wid", "a search is never completed for you");
}

#[test]
fn tab_completes_commands_and_setting_names() {
    let (_td, mut app) = fixture();
    assert_eq!(completed(&mut app, "repl"), "replace");
    assert_eq!(completed(&mut app, "set tree_w"), "set tree_width");
}

#[test]
fn ctrl_p_reaches_the_command_bar_from_inside_the_editor() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "z");
    // A bare `:` here would be a character, so the editor needs its own way in.
    app.on_key(ctrl('p'));
    assert!(matches!(app.mode, Mode::Bar(_)));
    type_str(&mut app, "w");
    app.on_key(k(KeyCode::Enter));
    assert!(
        fs::read_to_string(td.path().join("src/main.py"))
            .unwrap()
            .starts_with('z')
    );
}

#[test]
fn a_colon_in_the_editor_is_a_character_not_a_command() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch(':'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(app.active_buffer().unwrap().lines()[0].starts_with(':'));
}

#[test]
fn commands_cover_saving_and_quitting() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "z");
    app.on_key(k(KeyCode::Esc));
    command(&mut app, "w");
    assert!(
        fs::read_to_string(td.path().join("src/main.py"))
            .unwrap()
            .starts_with('z')
    );

    command(&mut app, "q");
    assert!(app.should_quit);
}
