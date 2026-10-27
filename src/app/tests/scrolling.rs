//! Scrolling: by key, by wheel, and by jumping to a line.
//!
//! The rule under all of these is that the view moves the minimum needed to
//! keep the cursor visible, so the text stays still while the cursor moves
//! within the window. A pane that scrolls on every keystroke is a pane that is
//! hard to read.

use super::*;

#[test]
fn the_line_command_puts_the_cursor_on_that_line() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    command(&mut app, "line 4");
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(
        app.active_buffer().unwrap().cursor_line,
        3,
        "counted from 1"
    );
    assert!(app.status.contains("line 4"), "{}", app.status);
}

#[test]
fn a_bare_number_is_a_line_number_too() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    command(&mut app, "3");
    assert_eq!(app.active_buffer().unwrap().cursor_line, 2);
    // And the wordier forms mean the same thing.
    command(&mut app, "go to 2");
    assert_eq!(app.active_buffer().unwrap().cursor_line, 1);
}

#[test]
fn a_line_past_the_end_lands_on_the_last_one() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    let last = app.active_buffer().unwrap().line_count() - 1;
    command(&mut app, "line 900");
    assert_eq!(app.active_buffer().unwrap().cursor_line, last);
}

#[test]
fn the_line_command_says_when_there_is_nothing_to_jump_inside() {
    let (_td, mut app) = fixture();
    select(&mut app, "notes");
    command(&mut app, "line 4");
    assert!(app.status.contains("no file open"), "{}", app.status);
    command(&mut app, "line zero");
    assert!(app.status.contains("not a line number"), "{}", app.status);
}

#[test]
fn a_wheel_notch_moves_exactly_one_line() {
    let (_td, mut app) = fixture();
    select(&mut app, "README.md");
    assert_eq!(app.focus, Focus::Tree, "the note is only being previewed");
    // Draw once so the preview knows how long it is.
    joined(&mut app);

    app.on_scroll(true, 60, 5);
    assert_eq!(app.preview_scroll, 1, "one notch, one line");
    app.on_scroll(false, 60, 5);
    assert_eq!(app.preview_scroll, 0);
    app.on_scroll(false, 60, 5);
    assert_eq!(app.preview_scroll, 0, "and it stops at the top");
}

#[test]
fn the_wheel_moves_the_cursor_in_a_file_being_edited() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Editor);
    joined(&mut app);

    app.on_scroll(true, 60, 5);
    assert_eq!(app.active_buffer().unwrap().cursor_line, 1);
    app.on_scroll(false, 60, 5);
    assert_eq!(app.active_buffer().unwrap().cursor_line, 0);
}

#[test]
fn the_wheel_over_the_tree_moves_the_tree_cursor() {
    let (_td, mut app) = fixture();
    joined(&mut app);
    let (x0, _) = app.last_tree_cols.expect("the tree was drawn");
    assert_eq!(app.selected, 0);

    app.on_scroll(true, x0 + 1, 5);
    assert_eq!(app.selected, 1, "one notch, one row");
    app.on_scroll(false, x0 + 1, 5);
    assert_eq!(app.selected, 0);
}

#[test]
fn one_bar_switches_between_searching_and_commanding_as_you_type() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(!b.is_command(), "plain text searches");
    assert!(!b.results.is_empty(), "and found something");

    // Clear it and lead with the sigil instead.
    for _ in 0.."widget".len() {
        app.on_key(k(KeyCode::Backspace));
    }
    type_str(&mut app, "*copy");
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(b.is_command(), "a leading * is a command");
    assert_eq!(b.command(), "copy");
    assert!(b.results.is_empty(), "a command has nothing to list");
}

#[test]
fn deleting_the_sigil_turns_a_command_back_into_a_search() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "*widget");
    assert!(matches!(&app.mode, Mode::Bar(b) if b.results.is_empty()));

    // Back to the front, which in a command is just after the star, where
    // the cursor is drawn, and take the star off.
    app.on_key(k(KeyCode::Home));
    app.on_key(k(KeyCode::Backspace));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(!b.is_command());
    assert!(!b.results.is_empty(), "the results come straight back");
}

#[test]
fn a_command_typed_into_the_search_bar_runs() {
    let (td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "*copy README.md to notes");
    app.on_key(k(KeyCode::Enter));
    assert!(
        td.path().join("notes/README.md").is_file(),
        "{}",
        app.status
    );
}

#[test]
fn ctrl_f_opens_the_bar_empty_and_a_star_makes_it_the_command_line() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('f'));
    let Mode::Bar(b) = &app.mode else {
        panic!("no bar")
    };
    assert!(!b.is_command(), "it opens as a search");
    app.on_key(ch('*'));
    let Mode::Bar(b) = &app.mode else {
        panic!("no bar")
    };
    assert!(b.is_command());
    assert_eq!(b.cursor, 1, "and the cursor is past the star");
}

#[test]
fn there_is_one_key_for_the_bar_not_two() {
    // Ctrl+P used to open it with the star already typed: one key to save
    // one keystroke, and one more row in the keybinds window.
    let (_td, mut app) = fixture();
    app.on_key(ctrl('p'));
    assert!(matches!(app.mode, Mode::Normal), "{:?}", app.mode);
}

#[test]
fn the_bar_shows_which_of_the_two_it_currently_is() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "widget");
    assert!(joined(&mut app).contains(" / "), "a search shows a slash");
    app.on_key(k(KeyCode::Home));
    type_str(&mut app, "*");
    assert!(joined(&mut app).contains(" * "), "a command shows a star");
}

#[test]
fn a_colon_is_just_a_colon_now() {
    let (_td, mut app) = fixture();
    app.on_key(ch(':'));
    assert!(
        matches!(app.mode, Mode::Normal),
        "the bar is reached by / and Ctrl+F; it does not need a third key"
    );
}

#[test]
fn the_bar_shows_the_star_once_not_twice() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "copy");
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" * "))
        .expect("the bar is drawn");
    assert!(bar.contains("* copy"), "{bar}");
    assert!(!bar.contains("**"), "the sigil is the block, not the text");
}

#[test]
fn the_bar_offers_the_completion_in_gray_before_tab() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "cop");
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" * "))
        .expect("the bar is drawn");
    assert!(
        bar.contains("* copy"),
        "the y is offered before Tab is pressed:\n{bar}"
    );

    // And Tab takes up exactly what was offered.
    app.on_key(k(KeyCode::Tab));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.command(), "copy");
}

#[test]
fn the_offer_is_only_made_at_the_end_of_the_line() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "cop");
    app.on_key(k(KeyCode::Home));
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" * "))
        .expect("the bar is drawn");
    assert!(
        !bar.contains("copy"),
        "mid-line it would read as text already there:\n{bar}"
    );
}

#[test]
fn a_search_is_not_offered_a_command_completion() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "cop");
    let bar = screen(&mut app, 90, 24)
        .into_iter()
        .find(|r| r.contains(" / "))
        .expect("the bar is drawn");
    assert!(!bar.contains("copy"), "{bar}");
}

#[test]
fn the_right_arrow_takes_up_the_suggestion_too() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "cop");
    app.on_key(k(KeyCode::Right));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.command(), "copy", "the arrow does what Tab does");
    assert_eq!(b.cursor, b.input.chars().count(), "and lands at the end");
}

#[test]
fn the_right_arrow_still_moves_along_the_text_mid_line() {
    let (_td, mut app) = fixture();
    command_bar(&mut app);
    type_str(&mut app, "cop");
    app.on_key(k(KeyCode::Home));
    app.on_key(k(KeyCode::Right));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.command(), "cop", "nothing was completed");
    assert_eq!(b.cursor, 2, "it just moved, from just after the star");
}

#[test]
fn the_right_arrow_leaves_a_search_alone() {
    let (_td, mut app) = fixture();
    app.on_key(ch('/'));
    type_str(&mut app, "wid");
    app.on_key(k(KeyCode::Right));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert_eq!(b.input, "wid", "a search is never completed for you");
}

#[test]
fn tab_completes_commands_and_setting_names() {
    let (_td, mut app) = fixture();
    assert_eq!(completed(&mut app, "repl"), "replace");
    assert_eq!(completed(&mut app, "set tree_w"), "set tree_width");
}

#[test]
fn ctrl_f_reaches_the_command_line_from_inside_the_editor() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "z");
    // A bare `/` here would be a character, so the editor needs its own way in.
    app.on_key(ctrl('f'));
    assert!(matches!(app.mode, Mode::Bar(_)));
    type_str(&mut app, "*set tab_width 7");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.config.tab_width, 7, "the command ran: {}", app.status);
    assert!(
        fs::read_to_string(td.path().join("src/main.py"))
            .unwrap()
            .starts_with("import"),
        "and the z is still unsaved in the buffer, not on disk"
    );
}

#[test]
fn a_colon_in_the_editor_is_a_character_not_a_command() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch(':'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(app.active_buffer().unwrap().lines()[0].starts_with(':'));
}

#[test]
fn saving_and_quitting_are_keys_and_not_commands() {
    let (td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "z");
    app.on_key(k(KeyCode::Esc));

    // `*w` and `*q` were a second way to do what two chords already did.
    command(&mut app, "w");
    assert!(app.status.contains("not a command"), "{}", app.status);
    assert!(
        !fs::read_to_string(td.path().join("src/main.py"))
            .unwrap()
            .starts_with('z'),
        "and it did not quietly save anyway"
    );

    app.on_key(ctrl('s'));
    assert!(
        fs::read_to_string(td.path().join("src/main.py"))
            .unwrap()
            .starts_with('z')
    );

    command(&mut app, "q");
    assert!(!app.should_quit, "nor quit");
    app.on_key(ctrl('q'));
    assert!(app.should_quit);
}

/// The fixture, plus a folder of three hundred files: far more rows than any
/// window shows at once.
fn with_a_crowded_folder() -> (tempfile::TempDir, App) {
    let (td, mut app) = fixture();
    let many = td.path().join("many");
    fs::create_dir(&many).unwrap();
    for n in 0..300 {
        fs::write(many.join(format!("item-{n:03}.txt")), "").unwrap();
    }
    command(&mut app, "reload");
    (td, app)
}

/// The browser as drawn, a row of text per screen row, and the text of the
/// row drawn as the cursor.
///
/// Read off the browser's own columns, because the preview's title names the
/// selected file too, and a name found there would prove nothing about the
/// list.
fn browser(app: &mut App, w: u16, h: u16) -> (Vec<String>, String) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let (x0, x1) = app.last_tree_cols.expect("the browser was drawn");
    let buf = t.backend().buffer();
    let mut rows = Vec::new();
    let mut cursor = String::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in x0..x1 {
            let cell = &buf[(x, y)];
            row.push_str(cell.symbol());
            if cell.modifier.contains(Modifier::REVERSED) {
                cursor.push_str(cell.symbol());
            }
        }
        rows.push(row);
    }
    (rows, cursor)
}

/// The name of the row the cursor is on.
fn selected_name(app: &App) -> String {
    app.rows[app.selected].name.clone()
}

/// That the cursor's row is on screen, and is the one drawn as the cursor.
fn cursor_shown(app: &mut App, w: u16, h: u16) {
    let name = selected_name(app);
    let (rows, cursor) = browser(app, w, h);
    assert!(
        cursor.contains(&name),
        "{name} should be on screen as the cursor, got {cursor:?}:\n{}",
        rows.join("\n")
    );
}

#[test]
fn a_crowded_folder_scrolls_to_keep_the_cursor_on_screen() {
    let (_td, mut app) = with_a_crowded_folder();
    select(&mut app, "item-000.txt");
    cursor_shown(&mut app, 100, 30);

    for _ in 0..250 {
        app.on_key(k(KeyCode::Down));
    }
    assert_eq!(selected_name(&app), "item-250.txt");
    cursor_shown(&mut app, 100, 30);
    let (rows, _) = browser(&mut app, 100, 30);
    assert!(
        !rows.iter().any(|r| r.contains("item-000.txt")),
        "the top of the folder has scrolled away:\n{}",
        rows.join("\n")
    );

    // And back up, all the way.
    for _ in 0..250 {
        app.on_key(k(KeyCode::Up));
    }
    assert_eq!(selected_name(&app), "item-000.txt");
    cursor_shown(&mut app, 100, 30);
}

#[test]
fn page_keys_and_the_ends_keep_the_cursor_on_screen_in_a_crowded_folder() {
    let (_td, mut app) = with_a_crowded_folder();
    select(&mut app, "item-000.txt");
    browser(&mut app, 100, 30);

    for _ in 0..3 {
        app.on_key(k(KeyCode::PageDown));
        cursor_shown(&mut app, 100, 30);
    }
    assert!(app.selected > 60, "three pages down, not three rows");

    app.on_key(k(KeyCode::End));
    assert_eq!(app.selected, app.rows.len() - 1, "the very last row");
    cursor_shown(&mut app, 100, 30);

    app.on_key(k(KeyCode::PageUp));
    cursor_shown(&mut app, 100, 30);

    app.on_key(k(KeyCode::Home));
    assert_eq!(app.selected, 0);
    cursor_shown(&mut app, 100, 30);
}

#[test]
fn the_wheel_through_a_crowded_folder_keeps_the_cursor_on_screen() {
    let (_td, mut app) = with_a_crowded_folder();
    select(&mut app, "item-000.txt");
    browser(&mut app, 100, 30);
    let (x0, _) = app.last_tree_cols.unwrap();

    for _ in 0..120 {
        app.on_scroll(true, x0 + 1, 5);
    }
    assert_eq!(selected_name(&app), "item-120.txt", "one notch, one row");
    cursor_shown(&mut app, 100, 30);
}

#[test]
fn a_shorter_window_still_shows_the_cursor() {
    let (_td, mut app) = with_a_crowded_folder();
    select(&mut app, "item-200.txt");
    cursor_shown(&mut app, 100, 40);
    // The terminal is made smaller with the cursor near the bottom of what
    // was on screen.
    cursor_shown(&mut app, 100, 10);
    cursor_shown(&mut app, 100, 6);
}

#[test]
fn folding_a_crowded_folder_from_deep_inside_it_leaves_no_gap() {
    let (_td, mut app) = with_a_crowded_folder();
    select(&mut app, "item-280.txt");
    cursor_shown(&mut app, 100, 30);

    // Left goes up to the folder, and left again folds it.
    app.on_key(k(KeyCode::Left));
    assert_eq!(selected_name(&app), "many");
    app.on_key(k(KeyCode::Left));
    assert!(!app.rows[app.selected].expanded, "folded");
    cursor_shown(&mut app, 100, 30);
    let (rows, _) = browser(&mut app, 100, 30);
    assert!(
        rows.iter().any(|r| r.contains("README.md")),
        "the whole short list is back on screen, from the top:\n{}",
        rows.join("\n")
    );
}

#[test]
fn the_wheel_scrolls_the_help_and_stops_at_its_end() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    let top = screen(&mut app, 90, 14).join("\n");
    for _ in 0..500 {
        app.on_scroll(true, 10, 5);
    }
    let end = screen(&mut app, 90, 14).join("\n");
    assert_ne!(top, end, "the wheel moved the help");
    let Mode::Help(scroll) = app.mode else {
        panic!("the help closed");
    };
    assert!(scroll < 500, "held to the end of the list, not {scroll}");

    // So the first notch back up moves it, rather than paying back the
    // notches spent past the end.
    app.on_scroll(false, 10, 5);
    assert_ne!(screen(&mut app, 90, 14).join("\n"), end);
}

#[test]
fn far_down_a_long_file_colors_are_guessed_then_read_in_behind() {
    let (td, mut app) = fixture();
    let code: String = (0..120)
        .map(|n| format!("fn f{n}() {{\n    g();\n}}\n\n"))
        .collect();
    fs::write(td.path().join("long.rs"), code).unwrap();
    command(&mut app, "reload");
    select(&mut app, "long.rs");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl_key(KeyCode::End));
    let first = screen(&mut app, 100, 30);
    assert!(app.colors_behind, "the first frame is drawn from a guess");
    // What the event loop does while nothing else is happening.
    app.reading_colors = true;
    let mut frames = 0;
    while app.colors_behind {
        assert_eq!(screen(&mut app, 100, 30), first, "only colors change");
        frames += 1;
        assert!(frames <= 480, "the exact colors never came");
    }
    assert!(!app.highlight_cache.behind());
}
