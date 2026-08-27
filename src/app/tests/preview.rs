//! What the right pane decides to be, and what it draws.
//!
//! Every kind of thing the cursor can land on: prose that wraps, markdown that
//! renders, code that gets line numbers, a picture that gets described, a
//! folder that gets counted, a binary that says so.

use super::*;

#[test]
fn file_kinds_are_classified_by_extension_then_by_name() {
    let prose: Vec<String> = ["md", "txt", "rst"].iter().map(|s| s.to_string()).collect();
    let k = |p: &str| text_kind(Path::new(p), &prose);
    assert_eq!(k("a.md"), TextKind::Markdown);
    assert_eq!(k("a.MD"), TextKind::Markdown, "case does not matter");
    assert_eq!(k("a.txt"), TextKind::Prose);
    assert_eq!(k("a.rst"), TextKind::Prose);
    assert_eq!(k("a.py"), TextKind::Code);
    assert_eq!(k("a.csv"), TextKind::Code);
    // Extensionless files are code unless they are prose by convention.
    assert_eq!(k("LICENSE"), TextKind::Prose);
    assert_eq!(k("COPYING"), TextKind::Prose);
    assert_eq!(k("Makefile"), TextKind::Code);
    assert_eq!(k("script"), TextKind::Code);
}

#[test]
fn a_text_file_opens_wrapped_and_readable_not_in_the_editor() {
    let (td, mut app) = fixture();
    let long = "a long line of prose that will certainly need to wrap because it keeps going";
    fs::write(td.path().join("notes.txt"), format!("{long}\n")).unwrap();
    command(&mut app, "reload");
    select(&mut app, "notes.txt");

    let out = joined(&mut app);
    assert!(out.contains("READ"), "prose reads first:\n{out}");
    assert!(!out.contains(" 1 "), "no line numbers on prose:\n{out}");
    for line in screen(&mut app, 90, 24) {
        assert!(line.chars().count() == 90);
    }
    // Wrapped, not clipped: the last word of the line still made it.
    assert!(
        out.contains("keeps going"),
        "the tail of the line was lost:\n{out}"
    );
}

#[test]
fn plain_text_opens_straight_into_the_editor() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("notes.txt"), "first line\n").unwrap();
    command(&mut app, "reload");
    select(&mut app, "notes.txt");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor, "no reading step in the way");
    type_str(&mut app, "X");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with('X'));
    app.on_key(ctrl('s'));
    assert_eq!(
        fs::read_to_string(td.path().join("notes.txt")).unwrap(),
        "Xfirst line\n"
    );
}

#[test]
fn right_steps_into_a_file_and_esc_steps_back_out() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Right));
    assert_eq!(app.focus, Focus::Editor, "right steps into the file");
    type_str(&mut app, "x");
    assert!(app.active_buffer().unwrap().dirty, "and lands in the text");

    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Tree);
}

#[test]
fn a_bare_left_moves_the_cursor_instead_of_leaving() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Right));
    app.on_key(k(KeyCode::Right));
    app.on_key(k(KeyCode::Left));
    assert_eq!(
        app.focus,
        Focus::Editor,
        "an unshifted arrow is cursor movement — it has to be"
    );
    assert_eq!(app.active_buffer().unwrap().cursor_col, 0);
}

#[test]
fn the_keybinds_window_lists_every_action_without_columns_colliding() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    app.on_key(k(KeyCode::Enter));
    // Tall enough for the lot: this is about the columns, not scrolling.
    let out = screen(&mut app, 100, 120).join("\n");
    for action in Action::all() {
        let spec = app.keymap.spec(action);
        assert!(
            out.contains(&format!("{}  ", action.name())),
            "{} is missing from the window:\n{out}",
            action.name()
        );
        assert!(
            spec.is_empty() || out.contains(&format!("{spec}  ")),
            "`{spec}` runs into the description beside it:\n{out}"
        );
    }
}

#[test]
fn ctrl_and_an_arrow_skips_five_lines_in_the_editor() {
    let (td, mut app) = fixture();
    let body: String = (0..20).map(|i| format!("line {i}\n")).collect();
    fs::write(td.path().join("notes/twenty.md"), body).unwrap();
    command(&mut app, "reload");
    select(&mut app, "twenty.md");
    app.on_key(k(KeyCode::Enter));

    app.on_key(ctrl_key(KeyCode::Down));
    assert_eq!(app.active_buffer().unwrap().cursor_line, 5);
    app.on_key(ctrl_key(KeyCode::Down));
    assert_eq!(app.active_buffer().unwrap().cursor_line, 10);
    app.on_key(ctrl_key(KeyCode::Up));
    assert_eq!(app.active_buffer().unwrap().cursor_line, 5);
    // Both ends clamp rather than running off the buffer.
    for _ in 0..10 {
        app.on_key(ctrl_key(KeyCode::Up));
    }
    assert_eq!(app.active_buffer().unwrap().cursor_line, 0);
    for _ in 0..10 {
        app.on_key(ctrl_key(KeyCode::Down));
    }
    assert_eq!(
        app.active_buffer().unwrap().cursor_line,
        19,
        "the last line"
    );
}

#[test]
fn ctrl_and_an_arrow_skips_five_entries_in_the_tree() {
    let (td, mut app) = fixture();
    for i in 0..12 {
        fs::write(td.path().join(format!("f{i:02}.txt")), "x\n").unwrap();
    }
    command(&mut app, "reload");
    app.selected = 0;

    app.on_key(ctrl_key(KeyCode::Down));
    assert_eq!(app.selected, 5);
    app.on_key(ctrl_key(KeyCode::Down));
    assert_eq!(app.selected, 10);
    app.on_key(ctrl_key(KeyCode::Up));
    assert_eq!(app.selected, 5);
    for _ in 0..5 {
        app.on_key(ctrl_key(KeyCode::Up));
    }
    assert_eq!(app.selected, 0, "it stops at the top");
    for _ in 0..20 {
        app.on_key(ctrl_key(KeyCode::Down));
    }
    assert_eq!(app.selected, app.rows.len() - 1, "and at the bottom");
}

#[test]
fn alt_and_an_arrow_jumps_to_an_edge_of_the_text() {
    let (td, mut app) = fixture();
    fs::write(
        td.path().join("notes/edges.md"),
        "first line here\nsecond\nthird\nlast line\n",
    )
    .unwrap();
    command(&mut app, "reload");
    select(&mut app, "edges.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::Down));

    app.on_key(alt(KeyCode::Right));
    let ed = app.active_buffer().unwrap();
    assert_eq!((ed.cursor_line, ed.cursor_col), (1, 6), "end of the line");
    app.on_key(alt(KeyCode::Left));
    let ed = app.active_buffer().unwrap();
    assert_eq!((ed.cursor_line, ed.cursor_col), (1, 0), "start of it");

    app.on_key(alt(KeyCode::Down));
    assert_eq!(app.active_buffer().unwrap().cursor_line, 3, "last line");
    app.on_key(alt(KeyCode::Up));
    assert_eq!(app.active_buffer().unwrap().cursor_line, 0, "first line");
    assert_eq!(app.focus, Focus::Editor, "none of that leaves the file");
}

#[test]
fn a_code_file_still_opens_straight_into_the_editor() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor, "code is for typing into");
    assert!(
        joined(&mut app).contains(" 1 "),
        "and keeps its line numbers"
    );
}

#[test]
fn prose_extensions_are_configurable() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("a.csv"), "x,y\n").unwrap();
    command(&mut app, "reload");
    select(&mut app, "a.csv");
    assert!(joined(&mut app).contains("VIEW"), "csv is code by default");

    command(&mut app, "set prose_extensions md txt csv");
    command(&mut app, "reload");
    select(&mut app, "a.csv");
    assert!(joined(&mut app).contains("READ"), "now it reads as prose");
}
#[test]
fn hovering_a_markdown_file_shows_it_rendered_not_raw() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    let out = joined(&mut app);
    assert!(out.contains("Design Notes"), "{out}");
    assert!(
        !out.contains("# Design Notes"),
        "the '#' is consumed:\n{out}"
    );
    assert!(out.contains("• one"), "list is rendered as bullets:\n{out}");
    assert!(out.contains("READ"));
}

#[test]
fn hovering_a_code_file_shows_source_with_line_numbers() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    let out = joined(&mut app);
    assert!(out.contains("import utils"), "{out}");
    assert!(out.contains(" 1 "), "line numbers are shown:\n{out}");
}

#[test]
fn hovering_a_picture_describes_it_rather_than_drawing_it() {
    let (_td, mut app) = fixture();
    select(&mut app, "logo.png");
    assert!(matches!(app.preview, Preview::Media { .. }));
    let out = joined(&mut app);
    assert!(out.contains("logo.png"), "{out}");
    assert!(
        out.contains("PNG image"),
        "the pane says what it is:\n{out}"
    );
    assert!(
        !out.contains('\u{2580}'),
        "nothing is drawn into the terminal:\n{out}"
    );
}

#[test]
fn a_real_picture_reports_its_resolution_and_its_size() {
    let (td, mut app) = fixture();
    // A PNG header and nothing else: tiny reads the size out of the header
    // and never decodes the pixels, so there are none to write.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&320u32.to_be_bytes());
    png.extend_from_slice(&240u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    fs::write(td.path().join("real.png"), png).unwrap();
    command(&mut app, "reload");
    select(&mut app, "real.png");
    let out = joined(&mut app);
    assert!(
        out.contains("320 \u{d7} 240"),
        "the resolution shows:\n{out}"
    );
    assert!(
        out.contains("KB") || out.contains("B"),
        "and a size:\n{out}"
    );
    assert!(out.contains("Enter opens it"), "and the way out:\n{out}");
}

#[test]
fn a_picture_that_will_not_open_still_previews() {
    // The fixture's png is deliberately corrupt. It has no resolution to
    // report, so the pane says why instead of showing an empty field.
    let (_td, mut app) = fixture();
    select(&mut app, "logo.png");
    let Preview::Media { info, .. } = &app.preview else {
        panic!("expected a media preview");
    };
    assert_eq!(info.dimensions, None);
    assert!(info.note.is_some(), "a reason is given");
    let out = joined(&mut app);
    assert!(
        out.contains("logo.png"),
        "and the name is still there:\n{out}"
    );
}

#[test]
fn opening_a_picture_leaves_the_keyboard_on_the_tree() {
    // Enter hands the file to the desktop; there is nothing in the pane to
    // type into, so the cursor stays where it can still be moved.
    let (_td, mut app) = fixture();
    select(&mut app, "logo.png");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Tree);
}

#[test]
fn hovering_a_folder_summarises_it() {
    let (_td, mut app) = fixture();
    select(&mut app, "notes");
    assert!(joined(&mut app).contains("1 entry"));
}

/// A markdown file with `sections` headings in it, which is roughly six source
/// lines each.
fn long_note(dir: &Path, name: &str, sections: usize) -> usize {
    let mut doc = String::new();
    for i in 0..sections {
        doc.push_str(&format!(
            "## Section {i}\n\nSome prose in section {i}.\n\n- a bullet\n\n"
        ));
    }
    fs::write(dir.join(name), &doc).unwrap();
    doc.lines().count()
}

#[test]
fn a_short_note_previews_formatted() {
    let (td, mut app) = fixture();
    long_note(td.path(), "short.md", 10);
    command(&mut app, "reload");
    select(&mut app, "short.md");
    let out = joined(&mut app);
    assert!(out.contains("Section 0"), "{out}");
    assert!(
        !out.contains("## Section 0"),
        "the hashes are formatting, not text:\n{out}"
    );
}

#[test]
fn a_note_too_long_to_format_previews_as_its_own_source() {
    // Formatting is linear in the length of the file and redone every frame,
    // so past the ceiling the pane shows the file as it is — which is also
    // exactly what pressing Enter would show.
    let (td, mut app) = fixture();
    let lines = long_note(td.path(), "huge.md", 900);
    assert!(
        lines > 4000,
        "the fixture has to clear the ceiling: {lines}"
    );
    command(&mut app, "reload");
    select(&mut app, "huge.md");
    let out = joined(&mut app);
    assert!(
        out.contains("## Section 0"),
        "the source is shown as written:\n{out}"
    );
}

#[test]
fn a_note_too_long_to_format_still_scrolls_to_the_end() {
    // The cheap path only builds the rows on screen, so the length it reports
    // has to be right without producing any of them.
    let (td, mut app) = fixture();
    long_note(td.path(), "huge.md", 900);
    command(&mut app, "reload");
    select(&mut app, "huge.md");
    joined(&mut app); // so the wheel knows where the panes are
    // The wheel is what scrolls a preview the keyboard has not reached, and
    // the far right of the window is over it rather than over the tree.
    for _ in 0..app.preview_len {
        app.on_scroll(true, 88);
    }
    let out = joined(&mut app);
    assert!(
        out.contains("Section 899"),
        "the last section is reachable:\n{out}"
    );
}
