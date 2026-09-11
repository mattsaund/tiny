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
    // a gray block once the row was reversed.
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
    // comes out dark against the highlight without a color of its own.
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
    app.on_key(ctrl('/'));
    type_str(&mut app, "widget");

    let out = joined(&mut app);
    assert!(
        out.contains("MATCH"),
        "results need somewhere to go:\n{out}"
    );
    assert!(out.contains("hello widget"), "{out}");
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
fn no_alt_key_does_anything_at_all() {
    // They are gone rather than merely unused: on a Mac they never arrived,
    // and a key that works on one machine and not another is worse than none.
    let (_td, mut app) = fixture();
    for key in [
        alt(KeyCode::Down),
        alt(KeyCode::Up),
        alt(KeyCode::Left),
        alt(KeyCode::Right),
        alt(KeyCode::Char('-')),
        alt(KeyCode::Char('=')),
    ] {
        let before = (app.selected, app.config.tree_width);
        app.on_key(key);
        assert_eq!(
            (app.selected, app.config.tree_width),
            before,
            "{key:?} still does something"
        );
    }
}

#[test]
fn a_capital_letter_still_goes_all_the_way_too() {
    // `I` and `K` are characters, not a modifier, so they were never part of
    // the move off Shift and a terminal cannot mistake them for a selection.
    let (_td, mut app) = fixture();
    app.on_key(ch('K'));
    assert_eq!(app.selected, app.rows.len() - 1);
    app.on_key(ch('I'));
    assert_eq!(app.selected, 0);
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
fn reloading_is_a_command_and_no_longer_a_key() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("appeared.md"), "# new\n").unwrap();
    assert!(!app.rows.iter().any(|r| r.name == "appeared.md"));

    // F5 was the key. Nothing answers to it now, and pressing it must be a
    // no-op rather than quietly doing something else.
    app.on_key(k(KeyCode::F(5)));
    assert!(
        !app.rows.iter().any(|r| r.name == "appeared.md"),
        "F5 is not a key any more"
    );

    command(&mut app, "reload");
    assert!(
        app.rows.iter().any(|r| r.name == "appeared.md"),
        "the command is how the disk is re-read on purpose"
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

// ---- the controls that reach out of the pane you are in --------------------

/// Put the keyboard in a file, with a cursor in real text.
fn editing(app: &mut App) {
    select(app, "README.md");
    app.on_key(k(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Editor, "the editor should have the keys");
}

#[test]
fn making_and_renaming_files_are_commands_and_no_longer_keys() {
    for (key, what) in [(ctrl('n'), "ctrl+n"), (ctrl('r'), "ctrl+r")] {
        let (_td, mut app) = fixture();
        editing(&mut app);
        let before = app.status.clone();
        app.on_key(key);
        assert_eq!(app.status, before, "{what} did something");
        assert!(matches!(app.mode, Mode::Normal), "{what} opened something");
    }
}

#[test]
fn deleting_from_inside_a_file_is_the_command() {
    let (_td, mut app) = fixture();
    editing(&mut app);
    app.on_key(ctrl('d'));
    assert!(
        !matches!(app.mode, Mode::Confirm(_)),
        "ctrl+d is not a key any more"
    );

    command(&mut app, "delete");
    assert!(
        matches!(app.mode, Mode::Confirm(_)),
        "and the command reaches it from here: {}",
        app.status
    );
}

#[test]
fn dotfiles_are_a_browser_key_and_a_setting() {
    let (_td, mut app) = fixture();
    editing(&mut app);
    app.on_key(ctrl('.'));
    assert!(
        !app.config.show_hidden,
        "ctrl+. is gone — no terminal can send it without the keyboard protocol"
    );

    // From a file, the setting is the way; from the browser, one key.
    command(&mut app, "set show_hidden true");
    assert!(app.config.show_hidden);
    app.on_key(k(KeyCode::Esc));
    app.on_key(ch('.'));
    assert!(
        !app.config.show_hidden,
        "and the browser key toggles it back"
    );
}

#[test]
fn a_chord_never_types_its_letter_into_the_file() {
    for key in [ctrl('n'), ctrl('r'), ctrl('d'), ctrl('m'), ctrl('.')] {
        let (_td, mut app) = fixture();
        editing(&mut app);
        let before = app.active_buffer().expect("a buffer").to_text();
        app.on_key(key);
        let after = app.active_buffer().expect("a buffer").to_text();
        assert_eq!(after, before, "a chord put a letter in the file");
    }
}

#[test]
fn ctrl_slash_opens_the_search_and_a_star_turns_it_into_a_command() {
    let (_td, mut app) = fixture();
    editing(&mut app);
    app.on_key(ctrl('/'));
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar should have opened from the editor")
    };
    assert!(!b.is_command(), "it opens as a search");

    app.on_key(ch('*'));
    let Mode::Bar(b) = &app.mode else {
        panic!("bar closed")
    };
    assert!(b.is_command(), "and a star is what turns it into a command");
}

#[test]
fn only_ctrl_with_an_arrow_resizes_the_browser() {
    // Every other arrow belongs to movement, whatever is held with it — see
    // the module docs in `config::keys`. `Ctrl` is the exception and only in
    // this pane, because in the editor those two are word motions.
    let (_td, mut app) = fixture();
    let width = app.config.tree_width;
    for key in [
        k(KeyCode::Left),
        alt(KeyCode::Left),
        ctrl_shift(KeyCode::Left),
        ctrl_alt(KeyCode::Left),
        k(KeyCode::Right),
        alt(KeyCode::Right),
        ctrl_shift(KeyCode::Right),
        ctrl_alt(KeyCode::Right),
    ] {
        app.on_key(key);
        assert_eq!(
            app.config.tree_width, width,
            "an arrow moved the pane edge: {key:?}"
        );
    }
    app.on_key(k(KeyCode::End));
    assert_eq!(
        app.selected,
        app.rows.len() - 1,
        "and End still goes all the way"
    );
}

#[test]
fn ctrl_with_an_arrow_sizes_the_browser_from_the_browser() {
    let (_td, mut app) = fixture();
    let width = app.config.tree_width;
    app.on_key(ctrl_key(KeyCode::Left));
    assert!(app.config.tree_width < width, "narrower");
    app.on_key(ctrl_key(KeyCode::Right));
    assert_eq!(app.config.tree_width, width, "and back");
}

#[test]
fn ctrl_with_an_arrow_is_a_word_motion_once_the_keyboard_is_in_a_file() {
    // The reason the resize is a browser key rather than a global chord: the
    // same two keys have an older job in the editor, and a global binding
    // would take it from them.
    let (_td, mut app) = fixture();
    editing(&mut app);
    let width = app.config.tree_width;
    let start = app.active_buffer().map(|e| e.cursor_col);

    app.on_key(ctrl_key(KeyCode::Right));
    assert_eq!(app.config.tree_width, width, "the pane did not move");
    assert_ne!(
        app.active_buffer().map(|e| e.cursor_col),
        start,
        "the cursor did"
    );
}

#[test]
fn ctrl_and_an_arrow_moves_the_edge_between_the_browser_and_the_file() {
    let (_td, mut app) = fixture();
    let edge = |app: &mut App| {
        screen(app, 90, 24)
            .into_iter()
            .find(|r| r.contains("BROWSER"))
            .and_then(|r| r.find('┐'))
            .expect("the browser is drawn")
    };
    let start = edge(&mut app);

    app.on_key(ctrl_key(KeyCode::Right));
    let wider = edge(&mut app);
    assert!(wider > start, "right widens it: {start} -> {wider}");

    app.on_key(ctrl_key(KeyCode::Left));
    app.on_key(ctrl_key(KeyCode::Left));
    let narrower = edge(&mut app);
    assert!(narrower < start, "left narrows it: {start} -> {narrower}");
}

#[test]
fn narrowing_past_the_end_folds_the_browser_and_widening_brings_it_back() {
    let (_td, mut app) = fixture();
    for _ in 0..20 {
        app.on_key(ctrl_key(KeyCode::Left));
    }
    assert!(app.tree_hidden, "the last step is the fold: {}", app.status);
    assert!(
        !joined(&mut app).contains("BROWSER"),
        "and it is really gone"
    );

    // The width keys belong to the browser, and the browser is not on screen
    // to press them in — so the fold key is what brings it back.
    app.on_key(ctrl(' '));
    assert!(!app.tree_hidden, "and the fold key brings it back");
}

#[test]
fn the_browser_never_grows_past_what_the_config_would_accept() {
    let (_td, mut app) = fixture();
    for _ in 0..40 {
        app.on_key(ctrl_key(KeyCode::Right));
    }
    // The same ceiling `Config::sanitized` enforces. A width the keys could
    // reach but the config would clamp is one that changes by itself on the
    // next restart.
    assert!(
        app.config.tree_width <= 0.60,
        "stopped at {}",
        app.config.tree_width
    );
    assert!(app.config.tree_width > 0.5, "and it got most of the way");
}

// ---- the keys a Mac can actually send --------------------------------------

#[test]
fn home_and_end_reach_the_ends_of_a_line() {
    // The Alt pair beside them is unreachable on a Mac, where Option is not a
    // modifier — and before these were bound, Home and End did nothing at all.
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Tab));
    app.on_key(k(KeyCode::End));
    let line = app.active_buffer().unwrap().lines()[0].chars().count();
    assert_eq!(app.active_buffer().unwrap().cursor_col, line, "the end");
    app.on_key(k(KeyCode::Home));
    assert_eq!(app.active_buffer().unwrap().cursor_col, 0, "and back");
}

#[test]
fn ctrl_home_and_ctrl_end_reach_the_ends_of_a_file() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Tab));
    app.on_key(ctrl_key(KeyCode::End));
    let last = app.active_buffer().unwrap().line_count() - 1;
    assert_eq!(app.active_buffer().unwrap().cursor_line, last, "last line");
    app.on_key(ctrl_key(KeyCode::Home));
    assert_eq!(app.active_buffer().unwrap().cursor_line, 0, "first line");
}

#[test]
fn home_and_end_reach_the_ends_of_the_browser_list() {
    let (_td, mut app) = fixture();
    app.on_key(k(KeyCode::End));
    assert_eq!(app.selected, app.rows.len() - 1, "last row");
    app.on_key(k(KeyCode::Home));
    assert_eq!(app.selected, 0, "first row");
}

#[test]
fn no_key_is_bound_to_alt_at_all() {
    // On macOS Terminal, Option is not a modifier: an `alt+` binding never
    // arrives at all. There are none left, and this is what keeps it that way.
    use crate::config::keys::{Action, Keymap};
    let keymap = Keymap::default();
    for action in Action::all() {
        let keys = keymap.spec(action);
        assert!(
            !keys.split_whitespace().any(|k| k.starts_with("alt+")),
            "{} is bound to an Alt key a Mac cannot send: {keys}",
            action.name()
        );
    }
}

#[test]
#[cfg(unix)]
fn a_file_named_through_a_symlink_still_opens() {
    // The shape of the macOS problem, reproducible anywhere with a symlink:
    // the project root has been resolved and the path being opened has not, so
    // the two are the same file spelled differently. Windows has the same
    // problem with the `\\?\` prefix `canonicalize` adds there.
    let (td, _) = fixture();
    let real = td.path().canonicalize().unwrap();
    let link = td.path().parent().unwrap().join(format!(
        "link-{}",
        real.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&real, &link).unwrap();

    // Rooted at the resolved path, the way `project::resolve` leaves it.
    let mut app = App::new(target(&real, None), Config::default(), None).unwrap();
    app.open_path(&link.join("README.md"));

    let got = app.selected_path().expect("something is selected");
    assert!(
        got.ends_with("README.md"),
        "opened through the link: {} — {}",
        got.display(),
        app.status
    );
    let _ = fs::remove_file(&link);
}

#[test]
#[cfg(unix)]
fn a_resolved_path_opens_in_a_project_rooted_at_a_symlink() {
    // The macOS case exactly, the mirror of the test above: the project root is
    // a path through a symlink — a Mac's temp directory is under `/var`, which
    // is `/private/var` — and the path being opened is the resolved one, which
    // is what git reports. Neither side matches the other until *both* are
    // resolved. Windows does the same with 8.3 names like `RUNNER~1`.
    let (td, _) = fixture();
    let real = td.path().canonicalize().unwrap();
    let link = td.path().parent().unwrap().join(format!(
        "rooted-{}",
        real.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let mut app = App::new(target(&link, None), Config::default(), None).unwrap();
    app.open_path(&real.join("README.md"));

    let got = app.selected_path().map(Path::to_path_buf);
    let _ = fs::remove_file(&link);
    assert_eq!(
        got,
        Some(link.join("README.md")),
        "opened, and spelled the way the tree spells it: {}",
        app.status
    );
}

#[test]
fn f1_reaches_the_help_window_from_inside_the_editor() {
    let (_td, mut app) = fixture();
    editing(&mut app);
    app.on_key(k(KeyCode::F(1)));
    assert!(matches!(app.mode, Mode::Help(_)), "F1 is help");
}

#[test]
fn the_settings_are_a_command_and_no_longer_a_key() {
    let (_td, mut app) = fixture();
    editing(&mut app);
    app.on_key(ctrl(','));
    assert!(
        !matches!(app.mode, Mode::Settings(_)),
        "Ctrl+, is not a key any more"
    );

    command(&mut app, "config");
    assert!(
        matches!(app.mode, Mode::Settings(_)),
        "status was: {}",
        app.status
    );
}

#[test]
fn a_comma_in_the_browser_is_not_a_key_either() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    assert!(
        !matches!(app.mode, Mode::Settings(_)),
        "the bare letter went with the chord"
    );
}

#[test]
fn the_windows_are_ctrl_and_a_number() {
    let (_td, mut app) = fixture();
    editing(&mut app);

    app.on_key(ctrl('3'));
    assert_eq!(app.window, Window::Map, "the map, from inside a file");
    app.on_key(ctrl('2'));
    assert_eq!(app.window, Window::Source, "and straight on to source");
    app.on_key(ctrl('1'));
    assert_eq!(app.window, Window::Main, "and back to the files");
}

#[test]
fn esc_leaves_a_window_for_the_main_one() {
    let (_td, mut app) = fixture();
    for window in [Window::Map, Window::Source] {
        app.on_key(match window {
            Window::Map => ctrl('3'),
            _ => ctrl('2'),
        });
        assert_eq!(app.window, window);
        app.on_key(k(KeyCode::Esc));
        assert_eq!(app.window, Window::Main, "esc came back from {window:?}");
    }
}

#[test]
fn the_map_is_rebuilt_every_time_it_is_switched_to() {
    let (td, mut app) = fixture();
    open_map(&mut app);
    let before = app.project_map.as_ref().expect("built").graph.nodes.len();

    app.on_key(ctrl('1'));
    fs::write(td.path().join("late.md"), "# late\n").unwrap();
    open_map(&mut app);

    let after = app.project_map.as_ref().expect("built").graph.nodes.len();
    assert_eq!(after, before + 1, "the new file is on the map");
}
