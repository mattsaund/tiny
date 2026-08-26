//! The bar, as a search and as a completer.
//!
//! Typing, results, stepping through them, the marking of every hit in the
//! preview, and opening one. Completion is here too, because it is the same
//! field: what Tab accepts and what the ghost text shows have to be the same
//! string, and these are what hold them together.

use super::*;

#[test]
fn the_results_pane_is_the_width_of_the_tree_it_replaces() {
    let (_td, mut app) = fixture();
    let tree_row = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains("BROWSER"))
        .expect("the tree is drawn");
    let tree_edge = tree_row.find("┐").expect("the pane ends somewhere");

    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    let hits_row = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains("MATCH"))
        .expect("the results are drawn");
    assert_eq!(
        hits_row.find("┐"),
        Some(tree_edge),
        "the screen must not lurch sideways when you start typing"
    );
}

#[test]
fn slash_opens_the_search_bar_and_typing_finds_matches() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    assert!(matches!(app.mode, Mode::Bar(_)));
    type_str(&mut app, "widget");

    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(!b.results.is_empty(), "found nothing");
    assert!(b.results.iter().any(|h| h.text.contains("hello widget")));

    let out = joined(&mut app);
    assert!(out.contains("MATCH"), "{out}");
    assert!(out.contains("hello widget"), "{out}");
}

#[test]
fn the_search_bar_shows_the_query_as_it_is_typed() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "design");
    let rows = screen(&mut app, 90, 24);
    assert!(
        rows[0].contains("design"),
        "the bar is on top:\n{}",
        rows[0]
    );
    assert!(rows[0].trim_start().starts_with('/'));
}

#[test]
fn enter_jumps_to_the_selected_match_and_puts_the_cursor_on_it() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "utils.load");
    app.on_key(k(KeyCode::Enter));

    assert!(matches!(app.mode, Mode::Normal), "the bar closes");
    assert_eq!(app.selected_row().unwrap().name, "main.py");
    assert_eq!(app.focus, Focus::Editor);
    let ed = app.active_buffer().unwrap();
    assert_eq!(ed.cursor_line, 4, "landed on the matching line");
    assert_eq!(ed.cursor_col, 11, "and on the matching column");
}

#[test]
fn moving_through_results_previews_each_one() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "e");

    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    let paths: Vec<PathBuf> = b.results.iter().map(|h| h.path.clone()).collect();
    assert_eq!(
        app.selected_path().unwrap(),
        paths[0],
        "starts on the first"
    );

    // Walk down to the first result in a different file.
    let other = paths
        .iter()
        .position(|p| *p != paths[0])
        .expect("fixture should match in more than one file");
    for _ in 0..other {
        app.on_key(k(KeyCode::Down));
    }
    assert!(matches!(app.mode, Mode::Bar(_)), "still searching");
    assert_eq!(
        app.selected_path().unwrap(),
        paths[other],
        "the preview follows the highlighted result"
    );
}

#[test]
fn a_search_marks_the_word_where_it_lives_in_the_preview() {
    let (_td, mut app) = fixture();
    type_search(&mut app, "widget");
    let marked = marked_in_preview(&mut app, 100, 24);
    assert!(
        marked.contains("widget"),
        "stepping through results should show where each hit is, got {marked:?}"
    );
}

#[test]
fn the_result_list_marks_its_matches_too() {
    let (_td, mut app) = fixture();
    type_search(&mut app, "widget");
    let (results, _) = marked_by_pane(&mut app, 100, 24);
    assert!(
        results.contains("widget"),
        "the snippet in the list points at the word as well, got {results:?}"
    );
}

#[test]
fn closing_the_search_leaves_the_preview_unmarked() {
    let (_td, mut app) = fixture();
    type_search(&mut app, "widget");
    app.on_key(k(KeyCode::Esc));
    let marked = marked_in_preview(&mut app, 100, 24);
    assert!(
        !marked.contains("widget"),
        "the marking belongs to the search, not to the file: {marked:?}"
    );
}

#[test]
fn marking_a_code_file_never_lands_on_a_line_number() {
    let (td, mut app) = fixture();
    // Line 3 holds a literal 3, so the gutter and the text both offer one
    // to mark and only the text's should be taken.
    fs::write(td.path().join("count.py"), "a = 1\nb = 2\nc = 3\n").unwrap();
    app.on_key(k(KeyCode::F(5)));
    type_search(&mut app, "3");

    let marked = marked_in_preview(&mut app, 100, 24);
    assert_eq!(
        marked, "3",
        "exactly the one in the source, not the one in the gutter"
    );
}

#[test]
fn a_search_with_no_matches_says_so() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "zzzznothing");
    assert!(joined(&mut app).contains("no matches"));
}

#[test]
fn control_chords_do_not_type_letters_into_the_search_bar() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "wid");
    app.on_key(ctrl('x'));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.input, "wid", "Ctrl+X must not append an x");
}

#[test]
fn ctrl_q_quits_from_inside_the_search_bar() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    app.on_key(ctrl('q'));
    assert!(app.should_quit, "Ctrl+Q means quit everywhere");
}

#[test]
fn control_chords_do_not_type_letters_into_a_prompt() {
    let (_td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "note");
    app.on_key(ctrl('s'));
    let Mode::Prompt(p) = &app.mode else {
        panic!("prompt closed")
    };
    assert_eq!(p.input, "note", "Ctrl+S must not append an s");
}

#[test]
fn ctrl_s_while_editing_a_setting_does_not_type_an_s_into_it() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    for _ in 0..4 {
        app.on_key(k(KeyCode::Down));
    }
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('s'));
    let Mode::Settings(s) = &app.mode else {
        panic!("settings closed")
    };
    assert_eq!(
        s.editing.as_deref(),
        Some("4"),
        "the field still holds tab_width's value, not \"4s\""
    );
}

#[test]
fn escape_closes_the_search_bar() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    app.on_key(k(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(joined(&mut app).contains("BROWSER"), "the tree is back");
}
#[test]
fn tab_completes_a_command_name() {
    let (_td, mut app) = fixture();
    assert_eq!(completed(&mut app, "cop"), "copy");
    assert_eq!(completed(&mut app, "del"), "delete");
}

#[test]
fn tab_completes_a_path_argument() {
    let (_td, mut app) = fixture();
    assert_eq!(completed(&mut app, "copy REA"), "copy README.md");
    assert_eq!(
        completed(&mut app, "delete notes/des"),
        "delete notes/design.md"
    );
}

#[test]
fn tab_completes_a_folder_with_a_separator_so_it_can_carry_on() {
    let (_td, mut app) = fixture();
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(completed(&mut app, "copy not"), format!("copy notes{sep}"));
    assert_eq!(
        completed(&mut app, &format!("copy notes{sep}")),
        format!("copy notes{sep}design.md"),
    );
}

#[test]
fn tab_fills_in_the_word_to_in_the_middle_of_a_copy() {
    let (_td, mut app) = fixture();
    assert_eq!(completed(&mut app, "copy README.md "), "copy README.md to");
    assert_eq!(completed(&mut app, "copy README.md t"), "copy README.md to");
}

#[test]
fn tab_completes_the_destination_after_to() {
    let (_td, mut app) = fixture();
    assert_eq!(
        completed(&mut app, "copy README.md to sr"),
        format!("copy README.md to src{}", std::path::MAIN_SEPARATOR)
    );
}

#[test]
fn tab_stops_where_the_candidates_stop_agreeing() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("report-one.md"), "").unwrap();
    fs::write(td.path().join("report-two.md"), "").unwrap();
    command(&mut app, "reload");
    // Two matches, so it fills in only what they share.
    assert_eq!(completed(&mut app, "copy rep"), "copy report-");
}

#[test]
fn completion_leaves_dotfiles_alone_unless_asked_for() {
    let (td, mut app) = fixture();
    fs::write(td.path().join(".secret.md"), "").unwrap();
    command(&mut app, "reload");

    assert_eq!(
        completed(&mut app, "delete .sec"),
        "delete .secret.md",
        "a leading dot is someone asking for one"
    );
    // Without the dot it is not a candidate at all, so the visible entries
    // are all that is left and they share no prefix to fill in.
    assert_eq!(completed(&mut app, "delete "), "delete ");
}

#[test]
fn the_project_root_cannot_be_deleted_or_renamed() {
    let (_td, mut app) = fixture();
    app.selected = 0;
    app.on_key(ch('d'));
    assert!(app.status.contains("cannot delete"));
    app.on_key(ch('r'));
    assert!(app.status.contains("cannot rename"));
}

#[test]
fn a_typed_path_cannot_escape_the_project() {
    let (td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "../escaped.md");
    app.on_key(k(KeyCode::Enter));
    assert!(app.status.contains("escapes"), "{}", app.status);
    assert!(!td.path().parent().unwrap().join("escaped.md").exists());
}
