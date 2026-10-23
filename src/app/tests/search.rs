//! The bar, as a search and as a completer.
//!
//! Typing, results, stepping through them, the marking of every hit in the
//! preview, and opening one. Completion is here too, because it is the same
//! field: what Tab accepts and what the ghost text shows have to be the same
//! string, and these are what hold them together.

use super::*;

#[test]
fn the_results_drop_down_over_both_panes_without_taking_either_away() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    let rows = screen(&mut app, 90, 24);
    let out = rows.join("\n");

    let hits = rows
        .iter()
        .position(|r| r.contains("MATCH"))
        .expect("the results are drawn");
    assert!(
        rows[hits].trim_end().ends_with('┐'),
        "the list is the width of the window, not of one pane:\n{}",
        rows[hits]
    );
    let browser = rows
        .iter()
        .position(|r| r.contains("BROWSER"))
        .expect("the browser is still there to search from");
    assert!(
        browser > hits,
        "the list comes down from the top, above the panes:\n{out}"
    );
    assert!(
        out.contains("hello widget"),
        "and the file behind still shows the hit:\n{out}"
    );
}

#[test]
fn a_long_result_list_still_leaves_most_of_the_window_to_the_project() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    // One letter, so the list is far longer than the window is tall.
    type_str(&mut app, "e");
    let rows = screen(&mut app, 90, 24);
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(b.results.len() > 8, "a short list proves nothing");

    let bottom = rows
        .iter()
        .position(|r| r.starts_with('└'))
        .expect("the list is a closed box");
    assert!(
        bottom < 9,
        "it stops at a third of the window:\n{}",
        rows.join("\n")
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
    command(&mut app, "reload");
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
fn control_chords_do_not_type_letters_into_the_bar() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "new note.md");
    app.on_key(ctrl('s'));
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed")
    };
    assert_eq!(b.command(), "new note.md", "Ctrl+S must not append an s");
}

#[test]
fn ctrl_s_while_editing_a_setting_does_not_type_an_s_into_it() {
    let (_td, mut app) = fixture();
    settings(&mut app);
    settings_on(&mut app, "tab_width");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('s'));
    let Mode::Settings(s) = &app.mode else {
        panic!("settings closed")
    };
    assert!(
        s.editing.is_none(),
        "the value was taken, as Enter takes it"
    );
    assert_eq!(app.config.tab_width, 4, "and it was 4, not \"4s\"");
    assert!(
        app.status.contains("no config file"),
        "a test has nowhere to write the settings to: {}",
        app.status
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
fn tab_again_goes_round_what_the_candidates_could_be() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("report-one.md"), "").unwrap();
    fs::write(td.path().join("report-two.md"), "").unwrap();
    command(&mut app, "reload");
    command_bar(&mut app);
    type_str(&mut app, "copy rep");
    let line = |app: &App| match &app.mode {
        Mode::Bar(b) => b.input.clone(),
        _ => panic!("the bar closed"),
    };
    app.on_key(k(KeyCode::Tab));
    assert_eq!(line(&app), "*copy report-", "first what they share");
    app.on_key(k(KeyCode::Tab));
    assert_eq!(line(&app), "*copy report-one.md");
    assert!(app.status.starts_with("1 of 2"), "{}", app.status);
    app.on_key(k(KeyCode::Tab));
    assert_eq!(line(&app), "*copy report-two.md");
    app.on_key(k(KeyCode::Tab));
    assert_eq!(line(&app), "*copy report-", "and back to what was typed");
}

#[test]
fn tab_knows_the_newer_verbs_and_the_values_of_a_setting() {
    let (_td, mut app) = fixture();
    assert_eq!(completed(&mut app, "split d"), "split down");
    assert_eq!(completed(&mut app, "close o"), "close others");
    assert_eq!(completed(&mut app, "close t"), "close terminal");
    assert_eq!(completed(&mut app, "set glyphs as"), "set glyphs ascii");
    // `block` and `bar` share no more than the `b`: the first of them.
    assert_eq!(
        completed(&mut app, "set cursor_shape to b"),
        "set cursor_shape to block"
    );
    assert_eq!(
        completed(&mut app, "set cursor_shape to ba"),
        "set cursor_shape to bar"
    );
    assert_eq!(completed(&mut app, "set borders f"), "set borders false");
    assert_eq!(completed(&mut app, "ter"), "terminal");
}

#[test]
fn tab_reads_the_same_verbs_the_command_does() {
    let (_td, mut app) = fixture();
    // Spellings the reading knows, which Tab once did not.
    assert_eq!(
        completed(&mut app, "dup README.md to sr"),
        format!("dup README.md to src{}", std::path::MAIN_SEPARATOR)
    );
    assert_eq!(completed(&mut app, "destroy READ"), "destroy README.md");
    // `go to` is `goto`, so the kind of place completes after it.
    assert_eq!(completed(&mut app, "go to func"), "go to function");
    assert_eq!(completed(&mut app, "take me to head"), "take me to heading");
    // `move to` is going somewhere rather than moving a file, so it is not
    // offered the `to` a move wants next.
    assert_eq!(completed(&mut app, "move to "), "move to ");
    // `mkdir` has already said what it makes.
    assert_eq!(completed(&mut app, "mkdir fol"), "mkdir fol");
    assert_eq!(completed(&mut app, "new fol"), "new folder");
}

#[test]
fn tab_reads_past_a_polite_start_and_into_an_open_quote() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("my notes.md"), "").unwrap();
    command(&mut app, "reload");
    assert_eq!(
        completed(&mut app, "please go to func"),
        "please go to function"
    );
    assert_eq!(
        completed(&mut app, "can you copy READ"),
        "can you copy README.md"
    );
    // Inside a quote a space is part of the name, so what is offered is the
    // rest of the name, not the `to` that would follow a finished one.
    assert_eq!(completed(&mut app, "copy \"my "), "copy \"my notes.md");
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
    command(&mut app, "delete");
    assert!(app.status.contains("cannot delete"));
    command(&mut app, "rename anything.md");
    assert!(app.status.contains("cannot rename"), "{}", app.status);
}

#[test]
fn a_typed_path_cannot_escape_the_project() {
    let (td, mut app) = fixture();
    command(&mut app, "new ../escaped.md");
    assert!(app.status.contains("escapes"), "{}", app.status);
    assert!(!td.path().parent().unwrap().join("escaped.md").exists());
}

#[test]
fn a_search_from_inside_a_file_lists_that_files_hits_first() {
    let (td, mut app) = fixture();
    // Three files hold the word. `zzz.md` sorts last in the walk, so if its
    // hits come out on top it is because the search was started from it and
    // not because of where it sits on disk.
    fs::write(td.path().join("aaa.md"), "the marker is here\n").unwrap();
    fs::write(td.path().join("mmm.md"), "the marker again\n").unwrap();
    fs::write(td.path().join("zzz.md"), "one marker\nand another marker\n").unwrap();
    command(&mut app, "reload");

    // From the browser, nothing is favored: the walk's order stands.
    type_search(&mut app, "marker");
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(
        !b.results[0].path.ends_with("zzz.md"),
        "nothing should be favored from the browser"
    );
    let from_tree = b.results.len();
    app.on_key(k(KeyCode::Esc));

    // From inside zzz.md, its own hits come out on top, and every other hit
    // is still in the list, further down.
    select(&mut app, "zzz.md");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);
    // The chord, not the bare slash: in a file a slash is a slash.
    app.on_key(ctrl('f'));
    type_str(&mut app, "marker");
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.results.len(), from_tree, "nothing was filtered out");
    assert!(
        b.results[0].path.ends_with("zzz.md") && b.results[1].path.ends_with("zzz.md"),
        "both of its hits come first, got {:?}",
        b.results.iter().map(|h| &h.text).collect::<Vec<_>>()
    );
    assert!(
        b.results[2..].iter().all(|h| !h.path.ends_with("zzz.md")),
        "and only its hits"
    );
}

#[test]
fn stepping_through_results_does_not_re_order_them_underneath_you() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("aaa.md"), "the marker is here\n").unwrap();
    fs::write(td.path().join("zzz.md"), "one marker\n").unwrap();
    command(&mut app, "reload");

    select(&mut app, "zzz.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('f'));
    type_str(&mut app, "marke");
    // Down moves onto a hit in another file, which opens that file in the
    // preview. The next keystroke must still favor the file the search was
    // started from, not whichever one the cursor has landed on.
    app.on_key(k(KeyCode::Down));
    app.on_key(ch('r'));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(
        b.results[0].path.ends_with("zzz.md"),
        "the order should not follow the cursor, got {:?}",
        b.results.iter().map(|h| &h.text).collect::<Vec<_>>()
    );
}

#[test]
fn moving_along_the_query_keeps_the_hit_that_was_chosen() {
    let (td, mut app) = fixture();
    for name in ["a.md", "b.md", "c.md"] {
        fs::write(td.path().join(name), "a foo here\n").unwrap();
    }
    command(&mut app, "reload");
    type_search(&mut app, "foo");
    app.settle();
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Home));
    app.on_key(k(KeyCode::Left));
    app.settle();
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed")
    };
    assert_eq!(
        b.selected, 2,
        "the cursor moved along the text; the search did not change"
    );
}

#[test]
fn home_in_a_command_stops_after_the_star() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "reload");
    app.on_key(k(KeyCode::Home));
    app.on_key(ch('x'));
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed")
    };
    assert!(b.is_command(), "still a command: {:?}", b.input);
    assert_eq!(b.command(), "xreload");
}
