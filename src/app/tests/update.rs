//! The update question.
//!
//! Nothing here asks GitHub: the check's answer is put straight into
//! `update_found`, the way [`App::poll_update`] would on finding one. What is
//! tested is the part a person meets: when the question appears, what it
//! says, and what each answer does.

use super::*;

/// A newer tiny than any that will ever really exist, already found.
fn found(app: &mut App) {
    app.update_found = Some("9.9.9".into());
}

fn is_asking(app: &App) -> bool {
    matches!(&app.mode, Mode::Confirm(c) if matches!(c.kind, ConfirmKind::Update(_)))
}

#[test]
fn the_question_waits_until_you_are_in_the_browser() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    found(&mut app);

    // Put while you were typing, the next letter (a `y`) would answer it.
    app.poll_update();
    assert!(!is_asking(&app), "not while a file has the keyboard");

    app.on_key(k(KeyCode::Esc));
    assert!(app.poll_update(), "the screen has something new on it");
    assert!(is_asking(&app), "and now it asks");
}

#[test]
fn the_question_is_a_window_in_the_words_it_was_asked_for() {
    let (_td, mut app) = fixture();
    found(&mut app);
    app.poll_update();
    let out = joined(&mut app);
    assert!(
        out.contains("tiny has been updated to version 9.9.9,"),
        "{out}"
    );
    assert!(out.contains("would you like to update now?"), "{out}");
    assert!(out.contains("y yes"), "{out}");
}

#[test]
fn yes_quits_and_leaves_the_version_to_be_installed() {
    let (_td, mut app) = fixture();
    found(&mut app);
    app.poll_update();
    app.on_key(ch('y'));
    assert!(app.should_quit, "tiny leaves, so the installer can run");
    assert_eq!(app.update_on_exit.as_deref(), Some("9.9.9"));
}

#[test]
fn yes_with_unsaved_work_asks_about_it_first() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(k(KeyCode::Esc));
    found(&mut app);
    app.poll_update();
    app.on_key(ch('y'));

    assert!(!app.should_quit, "not with an unsaved file");
    let Mode::Confirm(c) = &app.mode else {
        panic!("expected the save question, got {:?}", app.mode)
    };
    assert!(c.message.contains("before updating"), "{}", c.message);

    // Taking that back takes the update back with it: a later Ctrl+Q is a
    // quit, not an install.
    app.on_key(k(KeyCode::Esc));
    assert!(!app.should_quit);
    assert_eq!(app.update_on_exit, None);
}

#[test]
fn saving_on_the_way_out_still_updates() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(k(KeyCode::Esc));
    found(&mut app);
    app.poll_update();
    app.on_key(ch('y'));
    app.on_key(ch('y'));

    assert!(app.should_quit);
    assert_eq!(app.update_on_exit.as_deref(), Some("9.9.9"));
    let saved = fs::read_to_string(td.path().join("src/main.py")).unwrap();
    assert!(saved.starts_with('x'), "the work was written first");
}

#[test]
fn no_keeps_this_version_and_does_not_ask_again() {
    let (_td, mut app) = fixture();
    found(&mut app);
    app.poll_update();
    app.on_key(ch('n'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(!app.should_quit);
    assert_eq!(app.update_on_exit, None);
    assert!(app.status.contains("*update"), "{}", app.status);

    app.poll_update();
    assert!(!is_asking(&app), "once a run is enough");
}

#[test]
fn any_other_key_leaves_the_question_standing() {
    let (_td, mut app) = fixture();
    found(&mut app);
    app.poll_update();
    let before = app.selected;
    app.on_key(ch('k'));
    assert!(is_asking(&app), "a stray key does not answer it");
    assert_eq!(app.selected, before, "or move anything behind it");
}

#[test]
fn the_update_command_puts_the_question_from_anywhere() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    found(&mut app);
    command(&mut app, "update");
    assert!(is_asking(&app), "asked for, so asked, even from a file");
}
