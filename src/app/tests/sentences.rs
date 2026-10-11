//! Commands said as sentences, end to end: typed into the bar, planned,
//! described under it, and carried out on a real project.

use super::*;

/// The fixture, plus a few files deeper down for names to be found by.
fn project() -> (tempfile::TempDir, App) {
    let (td, mut app) = fixture();
    let r = td.path();
    fs::create_dir_all(r.join("work/drafts")).unwrap();
    fs::write(
        r.join("work/drafts/temp.txt"),
        "hi there, this is high. hi!\n",
    )
    .unwrap();
    fs::write(
        r.join("src/utils.py"),
        "def load():\n    return 1\n\n\ndef save():\n    return 2\n",
    )
    .unwrap();
    fs::write(
        r.join("notes/guide.md"),
        "# Guide\n\n## Install\n\nrun it\n\n## Usage\n\nwidget here\n",
    )
    .unwrap();
    command(&mut app, "reload");
    (td, app)
}

/// What the line under the bar says for a command typed but not yet run.
fn previewed(app: &mut App, line: &str) -> String {
    command_bar(app);
    type_str(app, line);
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed");
    };
    let said = match &b.plan {
        Some(Ok(s)) => s.clone(),
        Some(Err(e)) => format!("ERR {e}"),
        None => String::new(),
    };
    app.on_key(k(KeyCode::Esc));
    said
}

/// The open file and the line the cursor is on, counted from 1.
fn at(app: &App) -> (String, usize) {
    let ed = app.active_buffer().expect("a file is open");
    (
        ed.path.file_name().unwrap().to_string_lossy().into_owned(),
        ed.cursor_line + 1,
    )
}

// ---- the notes this came from ---------------------------------------------

#[test]
fn copy_finds_a_file_and_a_folder_by_name_wherever_they_are() {
    let (td, mut app) = project();
    command(&mut app, "copy temp.txt to notes");
    assert!(td.path().join("notes/temp.txt").is_file(), "{}", app.status);
    assert!(
        td.path().join("work/drafts/temp.txt").is_file(),
        "a copy, not a move"
    );
}

#[test]
fn the_bar_says_what_enter_will_do_before_it_is_pressed() {
    let (td, mut app) = project();
    assert_eq!(
        previewed(&mut app, "copy temp.txt to notes"),
        "copies work/drafts/temp.txt to notes/temp.txt"
    );
    assert!(
        !td.path().join("notes/temp.txt").exists(),
        "describing it did not do it"
    );
    assert!(
        previewed(&mut app, "replace hi with hello").starts_with("replaces 2 occurrences of `hi`"),
        "whole words only: `this` and `high` are not counted"
    );
    assert!(previewed(&mut app, "delete nothing-here.md").starts_with("ERR no file or folder"));
}

#[test]
fn goto_line_goes_to_a_line_of_the_open_file_or_a_named_one() {
    let (_td, mut app) = project();
    command(&mut app, "goto line 4 in main.py");
    assert_eq!(at(&app), ("main.py".into(), 4));
    assert_eq!(app.focus, Focus::Editor, "ready to type there");
    command(&mut app, "go to 2");
    assert_eq!(
        at(&app),
        ("main.py".into(), 2),
        "the open file, with no `in`"
    );
}

#[test]
fn goto_function_finds_the_definition() {
    let (_td, mut app) = project();
    command(&mut app, "goto function save");
    assert_eq!(at(&app), ("utils.py".into(), 5));
    command(&mut app, "goto function main in main.py");
    assert_eq!(at(&app), ("main.py".into(), 4));
}

#[test]
fn goto_word_looks_through_the_project_and_again_goes_on_to_the_next() {
    let (_td, mut app) = project();
    command(&mut app, "goto word widget");
    let first = at(&app);
    assert!(app.status.contains("1 of 2"), "{}", app.status);
    command(&mut app, "goto word widget");
    let second = at(&app);
    assert_ne!(first, second, "the same command goes on to the next one");
    assert!(app.status.contains("2 of 2"), "{}", app.status);
}

#[test]
fn goto_word_in_a_file_looks_only_there() {
    let (_td, mut app) = project();
    command(&mut app, "goto word widget in guide.md");
    assert_eq!(at(&app), ("guide.md".into(), 9));
}

#[test]
fn goto_word_skips_the_word_inside_longer_ones() {
    let (_td, mut app) = project();
    command(&mut app, "goto word his in temp.txt");
    assert!(
        app.status.contains("nothing in temp.txt matches"),
        "`this` is not the word `his`: {}",
        app.status
    );
}

#[test]
fn goto_file_and_folder_find_them_by_name() {
    let (td, mut app) = project();
    command(&mut app, "goto file guide.md");
    assert_eq!(at(&app).0, "guide.md");
    command(&mut app, "goto folder drafts");
    assert!(
        same_file(app.selected_path().unwrap(), &td.path().join("work/drafts")),
        "{:?}",
        app.selected_path()
    );
    command(&mut app, "open readme");
    assert_eq!(at(&app).0, "README.md", "a name without its extension");
}

#[test]
fn goto_heading_finds_a_section_of_a_note() {
    let (_td, mut app) = project();
    command(&mut app, "goto heading usage");
    assert_eq!(at(&app), ("guide.md".into(), 7));
}

#[test]
fn a_bare_name_goes_to_whatever_answers_to_it() {
    let (_td, mut app) = project();
    command(&mut app, "goto load");
    assert_eq!(at(&app), ("utils.py".into(), 1), "a function");
    command(&mut app, "guide.md");
    assert_eq!(at(&app).0, "guide.md", "a file, with no verb at all");
}

// ---- replace -------------------------------------------------------------

#[test]
fn replace_in_a_file_changes_only_that_file_and_undoes() {
    let (td, mut app) = project();
    command(&mut app, "replace hi with hello in temp.txt");
    let path = td.path().join("work/drafts/temp.txt");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "hello there, this is high. hello!\n",
        "saved, since it had no edits of its own"
    );
    assert!(app.status.contains("2 occurrences"), "{}", app.status);
    assert_eq!(app.focus, Focus::Editor, "open, so Ctrl+Z is right there");
    app.on_key(ctrl('z'));
    assert_eq!(
        app.active_buffer().unwrap().to_text(),
        "hi there, this is high. hi!\n",
        "one undo puts it all back"
    );
}

#[test]
fn replace_with_no_in_asks_then_does_the_whole_project() {
    let (td, mut app) = project();
    command(&mut app, "replace widget with gadget");
    assert!(matches!(app.mode, Mode::Confirm(_)), "it asks first");
    assert!(joined(&mut app).contains("2 occurrences"));
    app.on_key(ch('y'));
    assert!(
        fs::read_to_string(td.path().join("README.md"))
            .unwrap()
            .contains("gadget")
    );
    assert!(
        fs::read_to_string(td.path().join("notes/guide.md"))
            .unwrap()
            .contains("gadget here")
    );
}

// ---- names ---------------------------------------------------------------

#[test]
fn two_things_with_one_name_are_listed_not_guessed() {
    let (td, mut app) = project();
    fs::write(td.path().join("notes/temp.txt"), "another\n").unwrap();
    command(&mut app, "reload");
    command(&mut app, "delete temp.txt");
    assert!(
        app.status.contains("2 things are called temp.txt"),
        "{}",
        app.status
    );
    assert!(app.status.contains("notes/temp.txt"), "{}", app.status);
    assert!(matches!(app.mode, Mode::Normal), "nothing was armed");
    command(&mut app, "delete drafts/temp.txt");
    assert!(
        matches!(app.mode, Mode::Confirm(_)),
        "a little more of the path says which"
    );
}

#[test]
fn move_puts_a_file_into_a_folder_named_anywhere() {
    let (td, mut app) = project();
    command(&mut app, "move temp.txt into notes");
    assert!(td.path().join("notes/temp.txt").is_file(), "{}", app.status);
    assert!(!td.path().join("work/drafts/temp.txt").exists());
}

#[test]
fn rename_with_a_bare_name_keeps_the_file_where_it_is() {
    let (td, mut app) = project();
    command(&mut app, "rename temp.txt to notes");
    assert!(
        td.path().join("work/drafts/notes").is_file(),
        "a rename, even though a folder called notes exists: {}",
        app.status
    );
}

#[test]
fn new_makes_a_folder_inside_one_named_anywhere() {
    let (td, mut app) = project();
    command(&mut app, "make a new folder called ideas in drafts");
    assert!(
        td.path().join("work/drafts/ideas").is_dir(),
        "{}",
        app.status
    );
}

#[test]
fn every_way_in_still_cannot_leave_the_project() {
    let (td, mut app) = project();
    for line in [
        "delete ../../etc/hosts",
        "copy README.md to ../outside.md",
        "new ../escaped.md",
    ] {
        command(&mut app, line);
        assert!(app.status.contains("escapes"), "{line}: {}", app.status);
    }
    assert!(!td.path().parent().unwrap().join("outside.md").exists());
}

#[test]
fn windows_open_by_name() {
    let (_td, mut app) = project();
    command(&mut app, "open the map");
    assert_eq!(app.window, Window::Map);
    command(&mut app, "show help");
    assert!(matches!(app.mode, Mode::Help(_)));
}

#[test]
fn tab_walks_through_a_sentence() {
    let (_td, mut app) = project();
    assert_eq!(completed(&mut app, "goto fun"), "goto function");
    assert_eq!(completed(&mut app, "replace hi "), "replace hi with");
    assert_eq!(
        completed(&mut app, "replace hi with hello "),
        "replace hi with hello in"
    );
    // A name from anywhere in the project, once the bar has listed them.
    command_bar(&mut app);
    type_str(&mut app, "delete temp");
    app.on_key(k(KeyCode::Tab));
    let Mode::Bar(b) = &app.mode else { panic!() };
    assert_eq!(b.command(), "delete temp.txt");
}

#[test]
fn a_verb_half_typed_is_not_looked_for_across_the_project() {
    let (_td, mut app) = project();
    assert!(
        previewed(&mut app, "cop").contains("not a command"),
        "on the way to `copy`"
    );
    assert_eq!(
        previewed(&mut app, "guide.md"),
        "opens notes/guide.md",
        "a name still answers"
    );
}
