//! Typing into a file, and saving it.
//!
//! Including the two things that are easy to get wrong once the editor has the
//! keyboard: a letter must stay a letter, so `q` types a `q` rather than
//! quitting; and markdown must keep its formatting while it is edited, every
//! block but the one the cursor is in.

use super::*;

#[test]
fn enter_on_a_code_file_focuses_the_editor_and_typing_reaches_the_buffer() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);
    type_str(&mut app, "# hi");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with("# hi"));
    assert!(app.active_buffer().unwrap().dirty);
}

#[test]
fn ctrl_s_on_a_folder_saves_everything_unsaved_under_it() {
    let (td, mut app) = fixture();
    // Two dirty files in notes/, one in src/, so the folder has to save
    // its own and leave the neighbour alone.
    fs::write(td.path().join("notes/architecture.md"), "# Arch\n").unwrap();
    command(&mut app, "reload");
    for name in ["design.md", "architecture.md", "main.py"] {
        select(&mut app, name);
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "X");
        app.on_key(k(KeyCode::Esc));
    }
    assert_eq!(app.dirty_buffers().len(), 3);

    select(&mut app, "notes");
    app.on_key(ctrl('s'));
    assert!(app.status.contains("saved 2"), "{}", app.status);
    assert!(
        fs::read_to_string(td.path().join("notes/design.md"))
            .unwrap()
            .starts_with('X')
    );
    let left: Vec<String> = app
        .dirty_buffers()
        .iter()
        .map(|p| display_name(p))
        .collect();
    assert_eq!(left, ["main.py"], "the other folder is untouched");
}

#[test]
fn ctrl_s_on_the_project_folder_saves_the_whole_project() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("notes/architecture.md"), "# Arch\n").unwrap();
    command(&mut app, "reload");
    for name in ["design.md", "main.py", "README.md"] {
        select(&mut app, name);
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "X");
        app.on_key(k(KeyCode::Esc));
    }
    assert_eq!(app.dirty_buffers().len(), 3);

    app.selected = 0;
    assert!(app.selected_row().unwrap().is_dir, "the root row");
    app.on_key(ctrl('s'));
    assert!(app.status.contains("saved 3"), "{}", app.status);
    assert!(app.dirty_buffers().is_empty(), "nothing left unsaved");
    assert!(
        fs::read_to_string(td.path().join("src/main.py"))
            .unwrap()
            .starts_with('X')
    );
}

#[test]
fn ctrl_s_on_a_folder_with_nothing_pending_says_so() {
    let (_td, mut app) = fixture();
    select(&mut app, "notes");
    app.on_key(ctrl('s'));
    assert!(app.status.contains("nothing to save"), "{}", app.status);
}

#[test]
fn ctrl_s_on_a_file_still_saves_just_that_file() {
    let (td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "X");
    app.on_key(k(KeyCode::Esc));
    assert!(app.selected_row().is_some_and(|r| !r.is_dir));
    app.on_key(ctrl('s'));
    assert!(app.status.contains("saved design.md"), "{}", app.status);
    assert!(
        fs::read_to_string(td.path().join("notes/design.md"))
            .unwrap()
            .starts_with('X')
    );
}

#[test]
fn an_unsaved_file_stars_every_folder_above_it() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");

    let src = app.rows.iter().find(|r| r.name == "src").unwrap();
    assert!(
        app.dirty_here_or_below(&src.path),
        "the folder holding the edit is marked"
    );
    assert!(
        app.dirty_here_or_below(app.tree.root_path()),
        "and so is the root, all the way up"
    );

    let notes = app.rows.iter().find(|r| r.name == "notes").unwrap();
    assert!(
        !app.dirty_here_or_below(&notes.path),
        "a sibling folder stays clean"
    );

    // Folding `src` away must not fold the warning away with it.
    let src = src.path.clone();
    app.tree.collapse(&src);
    app.rebuild_rows();
    let row = app.rows.iter().position(|r| r.path == src).unwrap();
    assert!(
        screen(&mut app, 90, 24)[row + 1].contains("src *"),
        "the star survives collapsing the folder"
    );
}

#[test]
fn q_types_a_letter_in_the_editor_instead_of_quitting() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('q'));
    assert!(!app.should_quit, "q must not quit while editing");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with('q'));
}

#[test]
fn slash_does_not_open_search_while_typing_code() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('/'));
    assert!(
        matches!(app.mode, Mode::Normal),
        "it is a comment, not a key"
    );
    assert!(app.active_buffer().unwrap().lines()[0].starts_with('/'));
}

#[test]
fn ctrl_s_writes_the_file_to_disk() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(ctrl('s'));
    assert!(!app.active_buffer().unwrap().dirty);
    let on_disk = fs::read_to_string(td.path().join("src/main.py")).unwrap();
    assert!(on_disk.starts_with("ximport utils"), "{on_disk:?}");
}

#[test]
fn unsaved_edits_survive_navigating_away_and_back() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "MARKER");
    app.on_key(k(KeyCode::Esc));
    select(&mut app, "README.md");
    select(&mut app, "main.py");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with("MARKER"));
}

#[test]
fn markdown_previews_rendered_and_opens_into_the_editor() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    let out = joined(&mut app);
    assert!(
        !out.contains("# Design Notes"),
        "unfocused, it is a picture of the file:\n{out}"
    );
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);
    let out = joined(&mut app);
    // The cursor starts on the first line, so that block — and only that
    // block — shows the hashes it was typed with.
    assert!(out.contains("# Design Notes"), "raw source shows:\n{out}");
    assert!(out.contains("EDIT"));
}

#[test]
fn editing_markdown_formats_every_block_but_the_cursor_s() {
    let (td, mut app) = fixture();
    fs::write(
        td.path().join("notes/live.md"),
        "# Title\n\nsome **bold** words\n\n- one\n- two\n",
    )
    .unwrap();
    command(&mut app, "reload");
    select(&mut app, "live.md");
    app.on_key(k(KeyCode::Enter));

    // Cursor on line 0: the heading is raw, everything else is formatted.
    let out = joined(&mut app);
    assert!(out.contains("# Title"), "the cursor's block is raw:\n{out}");
    assert!(
        !out.contains("**bold**"),
        "the paragraph is formatted:\n{out}"
    );
    assert!(out.contains("• one"), "and so is the list:\n{out}");

    // Down to the paragraph: it unformats, the heading formats again.
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Down));
    let out = joined(&mut app);
    assert!(out.contains("**bold**"), "now the paragraph is raw:\n{out}");
    assert!(!out.contains("# Title"), "and the heading is not:\n{out}");

    // Into the list: the whole list unformats, not just the line under
    // the cursor — half a list is not a list.
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Down));
    let out = joined(&mut app);
    assert!(out.contains("- one"), "{out}");
    assert!(
        out.contains("- two"),
        "the whole block, not one line:\n{out}"
    );
    assert!(!out.contains("• "), "no bullets left:\n{out}");
}

#[test]
fn a_fenced_block_unformats_whole_and_typing_still_lands() {
    let (td, mut app) = fixture();
    fs::write(
        td.path().join("notes/fence.md"),
        "intro\n\n```python\nx = 1\n\ny = 2\n```\n\nafter\n",
    )
    .unwrap();
    command(&mut app, "reload");
    select(&mut app, "fence.md");
    app.on_key(k(KeyCode::Enter));
    for _ in 0..3 {
        app.on_key(k(KeyCode::Down));
    }
    let out = joined(&mut app);
    assert!(out.contains("```python"), "the fence shows:\n{out}");
    assert!(out.contains("y = 2"), "past a blank line inside it:\n{out}");

    // The cursor is real: what is typed goes where the cursor is drawn.
    type_str(&mut app, "z");
    assert_eq!(app.active_buffer().unwrap().lines()[3], "zx = 1");
}

#[test]
fn a_long_markdown_file_is_edited_raw() {
    let (td, mut app) = fixture();
    let mut body = String::from("# Title\n\n");
    for i in 0..5000 {
        body.push_str(&format!("line {i}\n"));
    }
    fs::write(td.path().join("notes/huge.md"), body).unwrap();
    command(&mut app, "reload");
    select(&mut app, "huge.md");
    app.on_key(k(KeyCode::Enter));
    let out = joined(&mut app);
    assert!(
        out.contains("# Title"),
        "past the limit the hashes stay put, formatted or not:\n{out}"
    );
}
