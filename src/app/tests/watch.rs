//! Files and folders changed by something other than tiny.
//!
//! Every test here plays the other program: it writes to the temp project
//! directly, then calls [`App::rescan_disk`] the way the event loop does when
//! nothing has been typed for half a second.
//!
//! Note what is *not* mocked — the clock, or the filesystem. The scan compares
//! real modification times and sizes, so a test that passed by accident would
//! have to have written the same bytes.

use super::*;

#[test]
fn a_file_changed_by_another_program_is_picked_up() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    assert!(joined(&mut app).contains("hello widget"), "the old text");
    assert!(
        !app.rescan_disk(),
        "nothing has changed, so there is nothing to redraw"
    );

    fs::write(td.path().join("README.md"), "# Fixture\n\ngoodbye widget\n").unwrap();

    assert!(app.rescan_disk(), "the file moved under us");
    let after = joined(&mut app);
    assert!(after.contains("goodbye widget"), "the new text is drawn");
    assert!(!after.contains("hello widget"), "the old text is gone");
    assert!(after.contains("changed on disk"), "and it says why");
}

#[test]
fn an_unsaved_buffer_is_never_overwritten_by_the_disk() {
    let (td, mut app) = fixture();
    select(&mut app, "README.md");
    app.focus_editor();
    type_str(&mut app, "MINE");
    assert!(app.is_dirty(&td.path().join("README.md")), "unsaved");

    fs::write(
        td.path().join("README.md"),
        "theirs, all of it, at length\n",
    )
    .unwrap();
    assert!(app.rescan_disk(), "the warning is worth a frame");

    let after = joined(&mut app);
    assert!(after.contains("MINE"), "what was typed is still here");
    assert!(!after.contains("theirs"), "the disk did not win");
    assert!(
        after.contains("unsaved copy"),
        "and the reader is told, in {after}"
    );
    assert!(
        !app.rescan_disk(),
        "told once — the stamp moved on even though the buffer did not"
    );
}

#[test]
fn saving_from_tiny_is_not_an_external_change() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    app.focus_editor();
    type_str(&mut app, "x");
    app.save_active();

    // The file's modification time has just moved, and the scan will notice
    // that. What it must not do is call it a change: it reads the file, finds
    // the bytes it already has, and says nothing.
    assert!(!app.rescan_disk(), "our own write is not news");
    assert!(!joined(&mut app).contains("changed on disk"));
}

#[test]
fn a_file_another_program_creates_appears_in_the_tree() {
    let (td, mut app) = fixture();
    assert!(!joined(&mut app).contains("arrived.md"));

    fs::write(td.path().join("arrived.md"), "# New\n").unwrap();

    assert!(app.rescan_disk(), "the folder changed");
    assert!(joined(&mut app).contains("arrived.md"), "and it is listed");
}

#[test]
fn a_file_another_program_deletes_leaves_the_tree() {
    let (td, mut app) = fixture();
    select(&mut app, "design.md");
    assert!(joined(&mut app).contains("design.md"));

    fs::remove_file(td.path().join("notes/design.md")).unwrap();

    assert!(app.rescan_disk(), "the folder changed");
    assert!(
        !app.rows.iter().any(|r| r.name == "design.md"),
        "the row is gone from {:?}",
        app.rows.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    assert!(
        !app.buffers.contains_key(&td.path().join("notes/design.md")),
        "and so is the buffer, so the name is free if the file comes back"
    );
    assert!(
        joined(&mut app).contains("gone from disk"),
        "and the reader is told rather than left wondering"
    );
}

#[test]
fn losing_the_file_you_are_editing_takes_the_keyboard_out_of_it() {
    let (td, mut app) = fixture();
    select(&mut app, "design.md");
    app.focus_editor();
    assert_eq!(app.focus, Focus::Editor);

    fs::remove_file(td.path().join("notes/design.md")).unwrap();
    assert!(app.rescan_disk());

    assert_eq!(
        app.focus,
        Focus::Tree,
        "the cursor fell onto another file; typing must not land in it"
    );
}

#[test]
fn a_deleted_file_that_comes_back_is_read_again_rather_than_remembered() {
    let (td, mut app) = fixture();
    let path = td.path().join("README.md");
    select(&mut app, "README.md");
    assert!(joined(&mut app).contains("hello widget"));

    fs::remove_file(&path).unwrap();
    app.rescan_disk();
    fs::write(&path, "# Fixture\n\nsecond life\n").unwrap();
    app.rescan_disk();

    select(&mut app, "README.md");
    let after = joined(&mut app);
    assert!(after.contains("second life"), "read afresh, in {after}");
    assert!(!after.contains("hello widget"), "not the old buffer");
}

#[test]
fn a_reload_leaves_the_cursor_where_the_reader_left_it() {
    let (td, mut app) = fixture();
    let path = td.path().join("src/main.py");
    select(&mut app, "main.py");
    app.focus_editor();
    for _ in 0..3 {
        app.on_key(k(KeyCode::Down));
    }
    let (line, col) = {
        let ed = app.buffers.get(&path).unwrap();
        (ed.cursor_line, ed.cursor_col)
    };
    assert_eq!(line, 3, "three lines down");

    fs::write(
        &path,
        "import utils\n\n\ndef main():\n    return utils.load()\n\n\ndef other():\n    pass\n",
    )
    .unwrap();
    assert!(app.rescan_disk());

    let ed = app.buffers.get(&path).unwrap();
    assert_eq!((ed.cursor_line, ed.cursor_col), (line, col), "still there");
    assert_eq!(ed.line_count(), 9, "and the new lines arrived");
}

#[test]
fn a_reload_clamps_a_cursor_the_file_no_longer_reaches() {
    let (td, mut app) = fixture();
    let path = td.path().join("src/main.py");
    select(&mut app, "main.py");
    app.focus_editor();
    for _ in 0..4 {
        app.on_key(k(KeyCode::Down));
    }
    app.on_key(k(KeyCode::End));

    fs::write(&path, "one\n").unwrap();
    assert!(app.rescan_disk());

    let ed = app.buffers.get(&path).unwrap();
    assert_eq!(ed.cursor_line, 0, "the file has one line now");
    assert!(ed.cursor_col <= 3, "and the cursor is inside it");
}

#[test]
fn an_edit_elsewhere_does_not_move_the_file_being_read() {
    let (td, mut app) = fixture();
    select(&mut app, "design.md");
    app.preview_scroll = 2;

    // Something lands in the project, but not in the file on screen.
    fs::write(td.path().join("unrelated.md"), "# Elsewhere\n").unwrap();
    assert!(app.rescan_disk(), "the tree changed");

    assert_eq!(app.preview_scroll, 2, "the reader kept their place");
    assert_eq!(
        app.selected_path(),
        Some(td.path().join("notes/design.md").as_path()),
        "and the cursor did not move"
    );
}

#[test]
fn turning_auto_reload_off_leaves_the_disk_alone() {
    let (td, mut app) = fixture();
    app.config.auto_reload = false;
    select(&mut app, "README.md");

    fs::write(td.path().join("README.md"), "# Fixture\n\nchanged\n").unwrap();
    assert!(!app.rescan_disk(), "not asked to look");
    assert!(joined(&mut app).contains("hello widget"), "so it did not");

    // F5 still works, which is the whole point of being able to turn it off.
    app.on_key(k(KeyCode::F(5)));
    assert!(joined(&mut app).contains("changed"), "asked, and it looked");
}

#[test]
fn a_file_that_stops_being_text_is_let_go_of() {
    let (td, mut app) = fixture();
    let path = td.path().join("README.md");
    select(&mut app, "README.md");

    fs::write(&path, [0x89u8, b'P', b'N', b'G', 0, 1, 2, 3, 4, 5]).unwrap();
    assert!(app.rescan_disk());

    assert!(
        !app.buffers.contains_key(&path),
        "a buffer holding text that is not the file's is worse than none"
    );
}
