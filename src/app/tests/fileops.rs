//! Creating, renaming, deleting, copying and pasting.
//!
//! Including the parts that are supposed to refuse: a path that climbs out of
//! the project, a delete that is not confirmed, a paste that would overwrite.
//! Those are the tests worth having twice.

use super::*;

#[test]
fn new_creates_a_file_in_the_selected_folder_and_selects_it() {
    let (td, mut app) = fixture();
    select(&mut app, "notes");
    command(&mut app, "new today.md");
    assert!(td.path().join("notes/today.md").is_file());
    assert_eq!(app.selected_row().unwrap().name, "today.md");
}

#[test]
fn mkdir_makes_a_folder_where_new_makes_a_file() {
    let (td, mut app) = fixture();
    command(&mut app, "new archive");
    assert!(
        td.path().join("archive").is_file(),
        "*new makes a file whatever the name looks like"
    );
    command(&mut app, "mkdir boxes");
    assert!(td.path().join("boxes").is_dir(), "and *mkdir a folder");
}

#[test]
fn a_nested_name_creates_the_folders_along_the_way() {
    let (td, mut app) = fixture();
    command(&mut app, "new journal/2026/aug.md");
    assert!(td.path().join("journal/2026/aug.md").is_file());
    assert_eq!(app.selected_row().unwrap().name, "aug.md");
}

#[test]
fn creating_over_an_existing_name_is_refused() {
    let (_td, mut app) = fixture();
    command(&mut app, "new README.md");
    assert!(app.status.contains("already exists"), "{}", app.status);
}

#[test]
fn rename_moves_the_file_and_carries_its_open_buffer() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "EDITED");
    app.on_key(k(KeyCode::Esc));

    // One argument renames whatever the cursor is on, in place.
    command(&mut app, "rename app.py");

    assert!(td.path().join("src/app.py").is_file());
    assert!(!td.path().join("src/main.py").exists());
    assert!(app.active_buffer().unwrap().lines()[0].starts_with("EDITED"));
}

#[test]
fn rename_with_two_paths_moves_between_folders() {
    let (td, mut app) = fixture();
    command(&mut app, "rename src/main.py to notes/main.py");
    assert!(td.path().join("notes/main.py").is_file(), "{}", app.status);
    assert!(!td.path().join("src/main.py").exists());
}

#[test]
fn renaming_onto_a_name_that_exists_is_refused() {
    let (td, mut app) = fixture();
    command(&mut app, "rename README.md to logo.png");
    assert!(app.status.contains("already exists"), "{}", app.status);
    assert!(td.path().join("README.md").exists(), "nothing moved");
}

#[test]
fn a_bare_new_name_renames_in_place_and_a_path_moves() {
    let (td, mut app) = fixture();
    // No separator: the file stays in the folder it was in.
    command(&mut app, "rename src/main.py to app.py");
    assert!(td.path().join("src/app.py").is_file(), "{}", app.status);

    // A separator means a path from the project root.
    command(&mut app, "rename src/app.py to notes/app.py");
    assert!(td.path().join("notes/app.py").is_file(), "{}", app.status);
}

#[test]
fn delete_asks_before_removing_and_n_backs_out() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    command(&mut app, "delete");
    assert!(joined(&mut app).contains("Delete README.md?"));
    app.on_key(ch('n'));
    assert!(td.path().join("README.md").exists());
    command(&mut app, "delete");
    app.on_key(ch('y'));
    assert!(!td.path().join("README.md").exists());
}

#[test]
fn deleting_a_folder_takes_everything_under_it() {
    let (td, mut app) = fixture();
    select(&mut app, "notes");
    command(&mut app, "delete");
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

    // In the editor `d` is the letter d and `/` is a slash; Ctrl+F is the
    // way to the bar from here, and a star makes it the command line. This is
    // why delete needs to be a command and not only a key.
    app.on_key(ctrl('f'));
    type_str(&mut app, "*delete");
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
    assert!(
        app.status.contains("no file or folder called"),
        "{}",
        app.status
    );
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

#[test]
fn renaming_a_folder_carries_every_open_buffer_inside_it() {
    // The data loss this guards against: edits in a file inside a renamed
    // folder used to stay under the old path, where saving them failed.
    let (td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "UNSAVED ");
    app.on_key(k(KeyCode::Esc));
    command(&mut app, "rename notes to journal");

    let moved = td.path().join("journal/design.md");
    let ed = app
        .buffers
        .get(&moved)
        .expect("the buffer moved with its folder");
    assert!(
        ed.dirty && ed.to_text().starts_with("UNSAVED"),
        "edits and all"
    );
    assert_eq!(ed.path, moved, "and saves to where the file is now");
    assert!(!app.buffers.contains_key(&td.path().join("notes/design.md")));

    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('s'));
    let on_disk = fs::read_to_string(&moved).unwrap();
    assert!(on_disk.starts_with("UNSAVED"), "{}", app.status);
}

#[test]
fn what_was_copied_follows_its_folder_when_it_is_renamed() {
    let (td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(ctrl('c'));
    command(&mut app, "rename notes to journal");
    assert_eq!(
        app.clipboard.as_deref(),
        Some(td.path().join("journal/design.md").as_path())
    );
}

#[test]
fn a_replace_across_the_project_changes_an_unsaved_file_in_its_buffer() {
    let (td, mut app) = fixture();
    let readme = td.path().join("README.md");
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "draft ");
    app.on_key(k(KeyCode::Esc));

    command(&mut app, "replace widget with gadget");
    app.on_key(ch('y'));
    let ed = &app.buffers[&readme];
    assert!(
        ed.to_text().contains("hello gadget") && ed.to_text().starts_with("draft"),
        "replaced in the copy on screen, edits kept: {}",
        ed.to_text()
    );
    assert!(ed.dirty, "and still unsaved");
    assert!(
        fs::read_to_string(&readme)
            .unwrap()
            .contains("hello widget"),
        "the disk copy is not written under the buffer"
    );
    assert!(app.status.contains("not saved yet"), "{}", app.status);
}

#[test]
fn reload_keeps_the_cursor_where_it_was_in_a_clean_file() {
    let (td, mut app) = fixture();
    let lines: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    fs::write(td.path().join("long.txt"), &lines).unwrap();
    command(&mut app, "reload");
    select(&mut app, "long.txt");
    app.on_key(k(KeyCode::Enter));
    for _ in 0..20 {
        app.on_key(k(KeyCode::Down));
    }
    fs::write(
        td.path().join("long.txt"),
        lines.replace("line 1\n", "first\n"),
    )
    .unwrap();
    command(&mut app, "reload");
    let ed = &app.buffers[&td.path().join("long.txt")];
    assert!(ed.to_text().starts_with("first"), "the new text");
    assert_eq!(ed.cursor_line, 20, "at the same place in it");
}

#[test]
fn a_rename_is_not_refused_because_the_name_is_used_somewhere_else() {
    // README.md is at the top of the project; renaming a note in a folder to
    // that name puts it in its own folder, where nothing is in the way.
    let (td, mut app) = fixture();
    command(&mut app, "rename notes/design.md to README.md");
    assert!(td.path().join("notes/README.md").exists(), "{}", app.status);
    assert!(
        td.path().join("README.md").exists(),
        "and the other is untouched"
    );
}

#[test]
fn copying_into_a_folder_that_has_one_of_that_name_is_refused() {
    let (td, mut app) = fixture();
    fs::create_dir_all(td.path().join("archive/notes")).unwrap();
    command(&mut app, "reload");
    command(&mut app, "copy notes to archive");
    assert!(app.status.contains("already exists"), "{}", app.status);
    assert!(
        !td.path().join("archive/notes/notes").exists(),
        "rather than a copy nested inside the one already there"
    );
}

#[cfg(unix)]
#[test]
fn copying_a_folder_keeps_the_links_inside_it() {
    let (td, mut app) = fixture();
    let venv = td.path().join("venv");
    fs::create_dir_all(venv.join("lib")).unwrap();
    fs::write(venv.join("lib/site.py"), "x = 1\n").unwrap();
    std::os::unix::fs::symlink("lib", venv.join("lib64")).unwrap();
    std::os::unix::fs::symlink("nowhere", venv.join("dangling")).unwrap();
    command(&mut app, "reload");
    command(&mut app, "copy venv to venv2");
    let copy = td.path().join("venv2");
    assert!(copy.join("lib/site.py").exists(), "{}", app.status);
    assert_eq!(fs::read_link(copy.join("lib64")).unwrap(), Path::new("lib"));
    assert_eq!(
        fs::read_link(copy.join("dangling")).unwrap(),
        Path::new("nowhere"),
        "a link to nothing is copied as the link it is"
    );
}

#[test]
fn a_note_named_like_the_new_name_without_its_extension_is_not_in_the_way() {
    let (td, mut app) = fixture();
    fs::create_dir_all(td.path().join("docs")).unwrap();
    fs::write(td.path().join("docs/archive.md"), "old\n").unwrap();
    command(&mut app, "reload");
    command(&mut app, "copy notes to archive");
    assert!(
        td.path().join("archive/design.md").exists(),
        "archive is a new name, not docs/archive.md: {}",
        app.status
    );
}
