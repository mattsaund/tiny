//! Getting around: the tree cursor, the folds, and the keys that stand in for
//! the arrows.
//!
//! Two of these keys do two jobs each — right expands or steps in or opens,
//! left closes or goes to the parent — so most of what is here is about the
//! seams between those jobs.

use super::*;

#[test]
fn arrow_keys_move_the_cursor_and_open_folders() {
    let (_td, mut app) = fixture();
    assert_eq!(app.selected, 0, "starts on the project root");
    app.on_key(k(KeyCode::Down));
    assert_eq!(app.selected_row().unwrap().name, "notes");
    app.on_key(k(KeyCode::Right));
    assert!(app.selected_row().unwrap().expanded);
    assert_eq!(app.rows[2].name, "design.md");
    app.on_key(k(KeyCode::Left));
    assert!(!app.selected_row().unwrap().expanded);
}

#[test]
fn enter_opens_and_closes_a_folder_without_moving_the_cursor() {
    let (_td, mut app) = fixture();
    app.on_key(k(KeyCode::Down));
    assert_eq!(app.selected_row().unwrap().name, "notes");

    app.on_key(k(KeyCode::Enter));
    assert!(app.selected_row().unwrap().expanded, "it opened");
    assert_eq!(app.selected_row().unwrap().name, "notes", "and stayed put");

    app.on_key(k(KeyCode::Enter));
    assert!(
        !app.selected_row().unwrap().expanded,
        "the same key shuts it"
    );
    assert_eq!(app.selected_row().unwrap().name, "notes");
}

#[test]
fn right_still_steps_into_a_folder_that_is_already_open() {
    let (_td, mut app) = fixture();
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Right));
    assert!(app.selected_row().unwrap().expanded);
    app.on_key(k(KeyCode::Right));
    assert_eq!(
        app.selected_row().unwrap().name,
        "design.md",
        "right walks inwards where Enter toggles"
    );
}

#[test]
fn the_selected_row_is_highlighted_in_one_piece() {
    let (_td, mut app) = fixture();
    // A nested file, so the row has indent and a marker to the left of the
    // name — the part that used to keep its own dim color and show up as
    // a grey block once the row was reversed.
    select(&mut app, "design.md");
    app.focus = Focus::Tree;

    let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
    t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();

    // Inside the tree pane only — the preview's title carries the same
    // file name, and it is on the same row as the tree's own title.
    let (x0, x1) = app.last_tree_cols.expect("the tree is on screen");
    let row = |y: u16| -> String {
        (x0 + 1..x1 - 1)
            .filter_map(|x| buf.cell((x, y)))
            .map(|c| c.symbol())
            .collect()
    };
    let y = (0..buf.area.height)
        .find(|y| row(*y).contains("design.md"))
        .expect("the selected file is on screen");

    let styles: Vec<_> = (x0 + 1..x1 - 1)
        .filter_map(|x| buf.cell((x, y)))
        .map(|c| (c.fg, c.bg, c.modifier))
        .collect();
    assert!(
        styles.iter().all(|s| *s == styles[0]),
        "the row should be one block; it is drawn as {styles:?}"
    );
}

#[test]
fn no_arrow_in_the_tree_is_drawn_in_the_dim_color() {
    // Open and closed alike. An arrow says what a row is, which is part of
    // reading the row — a second, quieter weight for some of them would read
    // as a distinction that is not there. The fixture has an open root and
    // two closed folders, so both kinds are on screen at once.
    let (_td, mut app) = fixture();
    let (text_fg, dim_fg) = (app.palette.text.fg, app.palette.dim.fg);
    let (open, closed, _) = app.tree_markers();
    let (open, closed) = (open.trim(), closed.trim());

    let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
    t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    // Scoped to the tree, since these glyphs turn up in the map as well.
    let (x0, x1) = app.last_tree_cols.expect("the tree is on screen");

    let (mut opens, mut closeds) = (0, 0);
    for y in 0..buf.area.height {
        for x in x0..x1 {
            let Some(cell) = buf.cell((x, y)) else {
                continue;
            };
            let which = match cell.symbol() {
                s if s == open => &mut opens,
                s if s == closed => &mut closeds,
                _ => continue,
            };
            *which += 1;
            assert_eq!(cell.fg, text_fg.unwrap(), "an arrow at {x},{y}");
            assert_ne!(Some(cell.fg), dim_fg, "an arrow at {x},{y} is chrome");
        }
    }
    assert!(opens > 0, "the open root draws an arrow");
    assert!(closeds > 0, "and the closed folders draw theirs");
}

#[test]
fn the_arrow_on_the_selected_row_inverts_with_the_rest_of_it() {
    // Hovering a folder reverses the whole row in one piece, so the arrow
    // comes out dark against the highlight without a colour of its own.
    let (_td, mut app) = fixture();
    let (open, ..) = app.tree_markers();
    let open = open.trim().to_string();

    let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
    t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let (x0, x1) = app.last_tree_cols.expect("the tree is on screen");
    let cell = (0..buf.area.height)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .find_map(|(x, y)| buf.cell((x, y)).filter(|c| c.symbol() == open))
        .expect("the root folder is selected and open");
    assert!(
        cell.modifier.contains(Modifier::REVERSED),
        "the arrow is inverted along with its row"
    );
}

#[test]
fn ctrl_space_folds_the_tree_away_and_brings_it_back() {
    let (_td, mut app) = fixture();
    assert!(joined(&mut app).contains("BROWSER"));

    app.on_key(ctrl(' '));
    let out = joined(&mut app);
    assert!(!out.contains("BROWSER"), "the pane is gone:\n{out}");
    assert!(
        !out.contains("┐┌"),
        "and the file has the whole width:\n{out}"
    );
    assert_eq!(app.focus, Focus::Editor, "keys cannot go to an unseen pane");

    app.on_key(ctrl(' '));
    assert!(joined(&mut app).contains("BROWSER"));
    assert_eq!(app.focus, Focus::Tree, "the keyboard comes back with it");
}

#[test]
fn folding_the_tree_while_editing_leaves_you_editing() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));

    app.on_key(ctrl(' '));
    assert!(app.tree_hidden);
    assert_eq!(app.focus, Focus::Editor);
    app.on_key(ctrl(' '));
    assert_eq!(
        app.focus,
        Focus::Editor,
        "you were typing before and still are"
    );
    type_str(&mut app, "X");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with('X'));
}

#[test]
fn esc_brings_the_tree_back_and_lands_on_it() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl(' '));

    app.on_key(k(KeyCode::Esc));
    assert!(
        !app.tree_hidden,
        "Esc means the tree, so the tree comes back"
    );
    assert_eq!(app.focus, Focus::Tree);
}

#[test]
fn search_still_gets_a_pane_while_the_tree_is_folded_away() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl(' '));
    app.on_key(ctrl('f'));
    type_str(&mut app, "widget");

    let out = joined(&mut app);
    assert!(
        out.contains("MATCH"),
        "results need somewhere to go:\n{out}"
    );
    assert!(out.contains("hello widget"), "{out}");
}

#[test]
fn n_makes_a_file_when_the_name_has_an_extension() {
    let (td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "todo.txt");
    app.on_key(k(KeyCode::Enter));
    assert!(td.path().join("todo.txt").is_file(), "{}", app.status);
}

#[test]
fn n_makes_a_folder_when_the_name_has_none() {
    let (td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "archive");
    app.on_key(k(KeyCode::Enter));
    assert!(td.path().join("archive").is_dir(), "{}", app.status);
}

#[test]
fn a_trailing_slash_makes_a_folder_whatever_the_name_looks_like() {
    let (td, mut app) = fixture();
    app.on_key(ch('n'));
    type_str(&mut app, "site.v2/");
    app.on_key(k(KeyCode::Enter));
    assert!(td.path().join("site.v2").is_dir(), "{}", app.status);
}

#[test]
fn the_new_command_still_makes_a_file_with_no_extension() {
    let (td, mut app) = fixture();
    command(&mut app, "new LICENSE");
    assert!(
        td.path().join("LICENSE").is_file(),
        "the explicit form is how you get an extensionless file: {}",
        app.status
    );
}

#[test]
fn shift_is_not_needed_for_anything_that_makes_a_thing() {
    let (_td, mut app) = fixture();
    app.on_key(ch('N'));
    assert!(
        matches!(app.mode, Mode::Normal),
        "capital N does nothing now; n covers both"
    );
}

#[test]
fn ijkl_move_the_tree_cursor_like_the_arrows() {
    let (_td, mut app) = fixture();
    app.on_key(ch('k'));
    assert_eq!(app.selected_row().unwrap().name, "notes", "k is down");
    app.on_key(ch('l'));
    assert!(app.selected_row().unwrap().expanded, "l is right");
    app.on_key(ch('j'));
    assert!(!app.selected_row().unwrap().expanded, "j is left");
    app.on_key(ch('i'));
    assert_eq!(app.selected, 0, "i is up");
}

#[test]
fn shift_i_and_k_go_all_the_way_like_shift_and_an_arrow() {
    let (_td, mut app) = fixture();
    app.on_key(ch('K'));
    assert_eq!(app.selected, app.rows.len() - 1);
    app.on_key(ch('I'));
    assert_eq!(app.selected, 0);
}

#[test]
fn ijkl_scroll_something_with_no_text_in_it() {
    let (_td, mut app) = fixture();
    // A picture has no buffer and no cursor, so the letters that stand in
    // for arrows still move the view. In a text file they are letters.
    // Folding the tree away is what hands the keyboard to a pane like
    // this one \u{2014} Enter on a picture opens it outside tiny instead.
    select(&mut app, "logo.png");
    app.on_key(ctrl(' '));
    assert_eq!(app.focus, Focus::Editor);
    joined(&mut app);
    app.on_key(ch('k'));
    assert_eq!(app.preview_scroll, 1);
    app.on_key(ch('i'));
    assert_eq!(app.preview_scroll, 0);
}

#[test]
fn a_letter_in_the_editor_is_still_just_a_letter() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "ijkl");
    assert!(
        app.active_buffer().unwrap().lines()[0].starts_with("ijkl"),
        "typing is typing"
    );
}

#[test]
fn a_letter_reaches_a_note_the_moment_it_is_opened() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Enter));
    // `i` and `e` used to mean things here. With no reading mode between
    // the tree and the text, a letter is a letter.
    type_str(&mut app, "ie");
    assert!(
        app.active_buffer().unwrap().lines()[0].starts_with("ie"),
        "{:?}",
        app.active_buffer().unwrap().lines()[0]
    );
}

#[test]
fn shift_and_an_arrow_goes_to_the_first_or_last_entry() {
    let (_td, mut app) = fixture();
    app.on_key(shift(KeyCode::Down));
    assert_eq!(app.selected, app.rows.len() - 1, "all the way down");
    app.on_key(shift(KeyCode::Up));
    assert_eq!(app.selected, 0, "and all the way back");
}

#[test]
fn the_old_letter_keys_for_those_are_gone() {
    let (_td, mut app) = fixture();
    app.on_key(ch('G'));
    assert_eq!(app.selected, 0, "G no longer jumps to the end");
    app.on_key(ch('R'));
    assert!(
        matches!(app.mode, Mode::Normal),
        "R is *reload now, not a key"
    );
}

#[test]
fn f5_and_the_reload_command_both_re_read_the_project() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("appeared.md"), "# new\n").unwrap();
    assert!(!app.rows.iter().any(|r| r.name == "appeared.md"));
    app.on_key(k(KeyCode::F(5)));
    assert!(app.rows.iter().any(|r| r.name == "appeared.md"), "the key");

    fs::write(td.path().join("later.md"), "# later\n").unwrap();
    command(&mut app, "reload");
    assert!(
        app.rows.iter().any(|r| r.name == "later.md"),
        "and the command"
    );
}

#[test]
fn the_reload_command_does_the_same() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("appeared.md"), "# new\n").unwrap();
    command(&mut app, "reload");
    assert!(app.rows.iter().any(|r| r.name == "appeared.md"));
}

#[test]
fn the_cursor_cannot_run_off_either_end() {
    let (_td, mut app) = fixture();
    for _ in 0..50 {
        app.on_key(k(KeyCode::Up));
    }
    assert_eq!(app.selected, 0);
    for _ in 0..50 {
        app.on_key(k(KeyCode::Down));
    }
    assert_eq!(app.selected, app.rows.len() - 1);
}
