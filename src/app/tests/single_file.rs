//! `tiny example.py` — one file, opened on its own.
//!
//! The tree folds away and the editor takes the keyboard, because naming a
//! file is asking to edit it. The exceptions are what these are mostly about:
//! a picture has nothing to type into, and a folder is not a file.

use super::*;

#[test]
fn naming_a_file_opens_it_alone_ready_to_type_into() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let file = td.path().join("src/main.py");
    let mut app = App::new(
        target(td.path(), Some(file.clone())),
        Config::default(),
        None,
    )
    .unwrap();

    assert_eq!(app.selected_row().unwrap().path, file);
    assert_eq!(app.focus, Focus::Editor, "it is ready to type into");
    assert!(app.tree_hidden, "and nothing else is in the way of it");
    let out = joined(&mut app);
    assert!(out.contains("main.py"), "{out}");
    assert!(out.contains("import utils"), "{out}");
    assert!(!out.contains("BROWSER"), "the tree is folded away:\n{out}");

    // Typing lands in the file without pressing anything first.
    type_str(&mut app, "X");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with('X'));
}

#[test]
fn the_project_is_one_key_away_from_a_file_opened_on_its_own() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let mut app = App::new(
        target(td.path(), Some(td.path().join("src/main.py"))),
        Config::default(),
        None,
    )
    .unwrap();

    app.on_key(ctrl(' '));
    let out = joined(&mut app);
    assert!(out.contains("BROWSER"), "the tree comes back:\n{out}");
    assert!(out.contains("README.md"), "with the folder in it:\n{out}");
}

#[test]
fn naming_a_picture_keeps_the_tree_since_there_is_nothing_to_type() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let app = App::new(
        target(td.path(), Some(td.path().join("logo.png"))),
        Config::default(),
        None,
    )
    .unwrap();

    assert!(!app.tree_hidden, "a picture takes no keyboard");
    assert_eq!(app.focus, Focus::Tree);
}

#[test]
fn a_new_project_opens_empty_and_says_so() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("fresh");
    fs::create_dir(&root).unwrap();
    let mut app = App::new(
        project::Target {
            root: root.clone(),
            file: None,
            created: true,
        },
        Config::default(),
        None,
    )
    .unwrap();

    assert_eq!(app.rows.len(), 1, "just the root row");
    assert_eq!(app.focus, Focus::Tree, "nothing has been made to type into");
    assert!(app.status.contains("new project"), "{}", app.status);
    joined(&mut app);
}

#[test]
fn naming_a_markdown_file_opens_it_in_the_editor() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let file = td.path().join("notes/design.md");
    let mut app = App::new(target(td.path(), Some(file)), Config::default(), None).unwrap();
    assert_eq!(app.focus, Focus::Editor);
    // Formatted, but with a cursor in it — the heading under the cursor
    // wears its hashes and the rest of the note does not.
    let out = joined(&mut app);
    assert!(out.contains("# Design Notes"), "{out}");
}
