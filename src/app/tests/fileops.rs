//! Creating, renaming, deleting, copying and pasting.
//!
//! Including the parts that are supposed to refuse: a path that climbs out of
//! the project, a delete that is not confirmed, a paste that would overwrite.
//! Those are the tests worth having twice.

use super::*;

#[test]
fn n_creates_a_file_in_the_selected_folder_and_selects_it() {
    let (td, mut app) = fixture();
    select(&mut app, "notes");
    app.on_key(ch('n'));
    type_str(&mut app, "today.md");
    app.on_key(k(KeyCode::Enter));
    assert!(td.path().join("notes/today.md").is_file());
    assert_eq!(app.selected_row().unwrap().name, "today.md");
}

#[test]
fn a_nested_name_creates_the_folders_along_the_way() {
    let (td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "journal/2026/aug.md");
    app.on_key(k(KeyCode::Enter));
    assert!(td.path().join("journal/2026/aug.md").is_file());
    assert_eq!(app.selected_row().unwrap().name, "aug.md");
}

#[test]
fn creating_over_an_existing_name_is_refused() {
    let (_td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    assert!(app.status.contains("already exists"), "{}", app.status);
}

#[test]
fn rename_moves_the_file_and_carries_its_open_buffer() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "EDITED");
    app.on_key(k(KeyCode::Esc));

    app.on_key(ch('r'));
    for _ in 0.."main.py".len() {
        app.on_key(k(KeyCode::Backspace));
    }
    type_str(&mut app, "app.py");
    app.on_key(k(KeyCode::Enter));

    assert!(td.path().join("src/app.py").is_file());
    assert!(!td.path().join("src/main.py").exists());
    assert!(app.active_buffer().unwrap().lines()[0].starts_with("EDITED"));
}

#[test]
fn delete_asks_before_removing_and_n_backs_out() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ch('d'));
    assert!(joined(&mut app).contains("Delete README.md?"));
    app.on_key(ch('n'));
    assert!(td.path().join("README.md").exists());
    app.on_key(ch('d'));
    app.on_key(ch('y'));
    assert!(!td.path().join("README.md").exists());
}

#[test]
fn deleting_a_folder_takes_everything_under_it() {
    let (td, mut app) = fixture();
    select(&mut app, "notes");
    app.on_key(ch('d'));
    let out = joined(&mut app);
    assert!(out.contains("Delete folder notes"), "{out}");
    app.on_key(ch('y'));
    assert!(!td.path().join("notes").exists(), "the folder is gone");
    assert!(!td.path().join("notes/design.md").exists());
    assert!(app.status.contains("deleted"), "{}", app.status);
}

#[test]
fn the_delete_command_removes_the_selection_when_given_no_path() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    command(&mut app, "delete");
    assert!(joined(&mut app).contains("Delete README.md?"));
    app.on_key(ch('y'));
    assert!(!td.path().join("README.md").exists());
}

#[test]
fn the_delete_command_takes_a_path_from_the_project_root() {
    let (td, mut app) = fixture();
    // Cursor parked somewhere else entirely: the path is not relative to it.
    select(&mut app, "main.py");
    command(&mut app, "rm notes/design.md");
    app.on_key(ch('y'));
    assert!(!td.path().join("notes/design.md").exists());
    assert!(td.path().join("src/main.py").exists(), "nothing else went");
}

#[test]
fn deleting_the_file_being_edited_hands_the_keyboard_back_to_the_tree() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);

    // In the editor `d` is the letter d and `:` is a colon — Ctrl+P is the
    // way to the command bar from here, and this is why delete needs to be
    // a command and not only a key.
    app.on_key(ctrl('p'));
    type_str(&mut app, "delete");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('y'));
    assert!(!td.path().join("src/main.py").exists());
    assert_eq!(app.focus, Focus::Tree);
    assert!(app.active_buffer().is_none(), "its buffer went with it");
}

#[test]
fn the_delete_command_reports_a_path_that_is_not_there() {
    let (_td, mut app) = fixture();
    command(&mut app, "delete notes/nope.md");
    assert!(app.status.contains("cannot delete"), "{}", app.status);
    assert!(
        matches!(app.mode, Mode::Normal),
        "nothing to confirm, so nothing is armed"
    );
}

#[test]
fn the_delete_command_cannot_escape_the_project() {
    let (_td, mut app) = fixture();
    command(&mut app, "delete ../../etc/hosts");
    assert!(app.status.contains("escapes the project"), "{}", app.status);
}
#[test]
fn copy_reads_like_a_sentence() {
    let (td, mut app) = fixture();
    command(&mut app, "copy README.md to notes");
    assert!(
        td.path().join("notes/README.md").is_file(),
        "naming a folder copies into it"
    );
    assert!(td.path().join("README.md").is_file(), "the original stays");
    assert!(app.status.contains("copied README.md"), "{}", app.status);
}

#[test]
fn copy_to_a_name_that_is_not_there_yet_uses_that_name() {
    let (td, mut app) = fixture();
    command(&mut app, "copy README.md to notes/intro.md");
    assert!(td.path().join("notes/intro.md").is_file());
    assert_eq!(
        fs::read_to_string(td.path().join("notes/intro.md")).unwrap(),
        fs::read_to_string(td.path().join("README.md")).unwrap()
    );
}

#[test]
fn copying_a_folder_takes_everything_under_it() {
    let (td, mut app) = fixture();
    fs::create_dir_all(td.path().join("notes/deep/deeper")).unwrap();
    fs::write(td.path().join("notes/deep/deeper/buried.md"), "# buried\n").unwrap();
    command(&mut app, "reload");

    command(&mut app, "copy notes to archive");
    assert!(td.path().join("archive/design.md").is_file());
    assert!(
        td.path().join("archive/deep/deeper/buried.md").is_file(),
        "subfolders come along"
    );
    assert_eq!(
        fs::read_to_string(td.path().join("archive/deep/deeper/buried.md")).unwrap(),
        "# buried\n"
    );
}

#[test]
fn copy_takes_names_with_spaces_without_quoting_them() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("my notes.md"), "# mine\n").unwrap();
    command(&mut app, "reload");

    // `to` is the separator, so both sides can be several words.
    command(&mut app, "copy my notes.md to old work.md");
    assert!(td.path().join("old work.md").is_file(), "{}", app.status);
}

#[test]
fn copy_without_the_word_to_still_works_with_two_names() {
    let (td, mut app) = fixture();
    command(&mut app, "cp README.md notes");
    assert!(
        td.path().join("notes/README.md").is_file(),
        "{}",
        app.status
    );
}

#[test]
fn copy_says_how_to_say_it_when_it_cannot_parse() {
    let (_td, mut app) = fixture();
    command(&mut app, "copy");
    assert!(app.status.contains("say it like"), "{}", app.status);
    command(&mut app, "copy README.md to");
    assert!(app.status.contains("say it like"), "{}", app.status);
}

#[test]
fn copy_never_writes_over_something_that_is_already_there() {
    let (td, mut app) = fixture();
    command(&mut app, "copy notes/design.md to README.md");
    assert!(app.status.contains("already exists"), "{}", app.status);
    assert_eq!(
        fs::read_to_string(td.path().join("README.md")).unwrap(),
        "# Fixture\n\nhello widget\n",
        "the original is untouched"
    );
}

#[test]
fn a_folder_cannot_be_copied_inside_itself() {
    let (_td, mut app) = fixture();
    command(&mut app, "copy notes to notes/backup");
    assert!(app.status.contains("into itself"), "{}", app.status);
}

#[test]
fn copy_cannot_reach_outside_the_project() {
    let (_td, mut app) = fixture();
    command(&mut app, "copy README.md to ../escaped.md");
    assert!(app.status.contains("escapes the project"), "{}", app.status);
}

#[test]
fn ctrl_c_and_ctrl_v_move_a_file_into_another_folder() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('c'));
    assert!(app.status.contains("copied README.md"), "{}", app.status);

    select(&mut app, "notes");
    app.on_key(ctrl('v'));
    assert!(
        td.path().join("notes/README.md").is_file(),
        "{}",
        app.status
    );
    assert!(td.path().join("README.md").is_file(), "a copy, not a move");
}

#[test]
fn pasting_beside_the_original_picks_the_next_free_name() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    app.on_key(ctrl('c'));
    app.on_key(ctrl('v'));
    assert!(td.path().join("README copy.md").is_file(), "{}", app.status);

    select(&mut app, "README.md");
    app.on_key(ctrl('v'));
    assert!(
        td.path().join("README copy 2.md").is_file(),
        "{}",
        app.status
    );
}

#[test]
fn pasting_a_folder_brings_its_contents() {
    let (td, mut app) = fixture();
    select(&mut app, "notes");
    app.on_key(ctrl('c'));
    select(&mut app, "src");
    app.on_key(ctrl('v'));
    assert!(
        td.path().join("src/notes/design.md").is_file(),
        "{}",
        app.status
    );
}

#[test]
fn pasting_with_an_empty_clipboard_says_so() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('v'));
    assert!(app.status.contains("nothing copied"), "{}", app.status);
}
