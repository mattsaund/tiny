//! The `*command` half of the bar, and quitting.
//!
//! One test per command, plus the argument splitting they all share, plus what
//! happens when you try to leave with unsaved work. The screenshot harness
//! lives at the end — it is `#[ignore]`d and exists to render a real frame
//! into the terminal for a human to look at.

use super::*;

#[test]
fn set_changes_a_setting_and_it_takes_effect() {
    let (_td, mut app) = fixture();
    assert_eq!(app.config.tab_width, 4);
    command(&mut app, "set tab_width 2");
    assert_eq!(app.config.tab_width, 2);

    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::Tab));
    assert!(
        app.active_buffer().unwrap().lines()[0].starts_with("  import"),
        "the new tab width is what the editor actually inserts"
    );
}

#[test]
fn set_with_no_value_reports_the_current_one() {
    let (_td, mut app) = fixture();
    command(&mut app, "set tree_side");
    assert_eq!(app.status, "tree_side = left");
}

#[test]
fn set_can_repaint_the_theme_without_a_restart() {
    let (_td, mut app) = fixture();
    command(&mut app, "set theme.heading cyan bold");
    assert_eq!(app.config.theme.heading, "cyan bold");
    assert_eq!(app.palette.heading.fg, Some(Color::Cyan));
}

#[test]
fn set_rejects_nonsense_without_changing_anything() {
    let (_td, mut app) = fixture();
    command(&mut app, "set tab_width banana");
    assert_eq!(app.config.tab_width, 4);
    assert!(app.status.contains("number"), "{}", app.status);

    command(&mut app, "set no_such_setting 1");
    assert!(app.status.contains("unknown setting"), "{}", app.status);
}

#[test]
fn an_unknown_command_says_so() {
    let (_td, mut app) = fixture();
    command(&mut app, "frobnicate");
    assert!(app.status.contains("unknown command"), "{}", app.status);
}

#[test]
fn replace_asks_first_then_rewrites_the_whole_project() {
    let (td, mut app) = fixture();
    fs::write(td.path().join("notes/more.md"), "widget widget\n").unwrap();
    command(&mut app, "reload");

    command(&mut app, "replace widget gadget");
    assert!(matches!(app.mode, Mode::Confirm(_)), "it asks first");
    let out = joined(&mut app);
    assert!(
        out.contains("3 occurrences"),
        "counts them up front:\n{out}"
    );
    assert!(out.contains("2 files"), "{out}");

    app.on_key(ch('n'));
    assert!(
        fs::read_to_string(td.path().join("README.md"))
            .unwrap()
            .contains("widget"),
        "answering no changes nothing"
    );

    command(&mut app, "replace widget gadget");
    app.on_key(ch('y'));
    assert!(
        fs::read_to_string(td.path().join("README.md"))
            .unwrap()
            .contains("gadget"),
        "answering yes rewrites it"
    );
    assert_eq!(
        fs::read_to_string(td.path().join("notes/more.md")).unwrap(),
        "gadget gadget\n"
    );
    assert!(app.status.contains("replaced 3"), "{}", app.status);
}

#[test]
fn replace_needs_both_halves_and_reports_a_miss() {
    let (_td, mut app) = fixture();
    command(&mut app, "replace onlyone");
    assert!(app.status.contains(":replace"), "{}", app.status);
    command(&mut app, "replace absent-word x");
    assert!(app.status.contains("not in this project"), "{}", app.status);
}

#[test]
fn replace_takes_quoted_strings_with_spaces_in_them() {
    let (td, mut app) = fixture();
    command(&mut app, "replace \"hello widget\" \"goodbye gadget\"");
    app.on_key(ch('y'));
    assert!(
        fs::read_to_string(td.path().join("README.md"))
            .unwrap()
            .contains("goodbye gadget"),
        "quoted arguments keep their spaces"
    );
}
#[test]
fn quitting_with_unsaved_work_asks_first() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(k(KeyCode::Esc));

    app.on_key(ch('q'));
    assert!(!app.should_quit);
    assert!(joined(&mut app).contains("Discard unsaved changes"));
    app.on_key(ch('n'));
    assert!(!app.should_quit);
    app.on_key(ch('q'));
    app.on_key(ch('y'));
    assert!(app.should_quit);
}

#[test]
fn help_opens_and_any_key_closes_it() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    // Tall enough for the whole thing; the short-terminal case scrolls,
    // and has a test of its own.
    let out = screen(&mut app, 90, 70).join("\n");
    assert!(out.contains("Keys and commands"), "{out}");
    assert!(out.contains("search"), "{out}");
    assert!(out.contains("the project map"), "{out}");
    app.on_key(ch('x'));
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn help_lists_the_commands_as_well_as_the_keys() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    let out = screen(&mut app, 90, 70).join("\n");
    assert!(out.contains("*copy a to b"), "commands are in it:\n{out}");
    assert!(out.contains("*line 42"), "{out}");
    assert!(out.contains("*replace old new"), "{out}");
    assert!(
        // Keys read as they would be written in the config, which is what
        // the keybinds window shows too.
        out.contains("ctrl+s"),
        "and the keys are still there:\n{out}"
    );
}

#[test]
fn the_help_window_shows_a_rebinding_not_the_shipped_key() {
    let (_td, mut app) = fixture();
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('z'));
    app.on_key(k(KeyCode::Esc));
    app.on_key(k(KeyCode::Esc));

    app.on_key(ch('?'));
    let out = screen(&mut app, 90, 70).join("\n");
    let row = out
        .lines()
        .find(|l| l.contains("move"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(row.contains('z'), "the help follows the keyboard:\n{row}");
    assert!(
        !row.contains("down k"),
        "and not the key that used to do it:\n{row}"
    );
}

#[test]
fn an_unbound_action_leaves_the_help_row_empty_rather_than_lying() {
    let (_td, mut app) = fixture();
    // Ctrl+N is taken off `new` by giving it to another action in the same
    // context — `new` is what the help row for making a file reads.
    keybinds_on(&mut app, Action::Rename);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ctrl('n'));
    app.on_key(k(KeyCode::Esc));
    app.on_key(k(KeyCode::Esc));

    app.on_key(ch('?'));
    let out = screen(&mut app, 90, 70).join("\n");
    let row = out
        .lines()
        .find(|l| l.contains("dot makes a file"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        !row.contains("ctrl+n"),
        "nothing reaches it, so nothing is offered:\n{row}"
    );
}

#[test]
fn a_wide_window_puts_keys_and_commands_side_by_side() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    let rows = screen(&mut app, 130, 46);
    // Whichever command happens to line up with it — the point is that a
    // key and a command share a row.
    assert!(
        rows.iter()
            .any(|r| r.contains("open or close") && r.contains('*')),
        "two columns on one row:\n{}",
        rows.join("\n")
    );
}

#[test]
fn a_narrow_window_stacks_them_into_one_column() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    let rows = screen(&mut app, 70, 70);
    let out = rows.join("\n");
    assert!(
        !rows
            .iter()
            .any(|r| r.contains("open or close") && r.contains('*')),
        "no room for two:\n{out}"
    );
    assert!(out.contains("open or close"), "{out}");
    assert!(
        out.contains("*copy a to b"),
        "both are still listed:\n{out}"
    );
}

#[test]
fn help_scrolls_on_a_terminal_too_short_for_it() {
    let (_td, mut app) = fixture();
    app.on_key(ch('?'));
    let top = screen(&mut app, 80, 16).join("\n");
    assert!(top.contains("MOVING"), "starts at the top:\n{top}");

    for _ in 0..12 {
        app.on_key(k(KeyCode::Down));
    }
    assert!(
        matches!(app.mode, Mode::Help(_)),
        "arrows scroll, not close"
    );
    let lower = screen(&mut app, 80, 16).join("\n");
    assert_ne!(top, lower, "the view moved");

    // The point of scrolling: the end of the list is reachable at all.
    app.on_key(k(KeyCode::End));
    let bottom = screen(&mut app, 80, 16).join("\n");
    assert!(
        bottom.contains("quit"),
        "the last entry is reachable:\n{bottom}"
    );
    assert!(
        !bottom.contains("MOVING"),
        "and the top has scrolled off:\n{bottom}"
    );

    // Anything that is not a scroll key still puts it away.
    app.on_key(ch('z'));
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn dot_toggles_hidden_files() {
    let (td, mut app) = fixture();
    fs::write(td.path().join(".secret"), "x").unwrap();
    command(&mut app, "reload");
    assert!(!app.rows.iter().any(|r| r.name == ".secret"));
    app.on_key(ch('.'));
    assert!(app.rows.iter().any(|r| r.name == ".secret"));
}

#[test]
fn refresh_keeps_buffers_that_have_unsaved_work() {
    let (_td, mut app) = fixture();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "KEEP");
    app.on_key(k(KeyCode::Esc));
    command(&mut app, "reload");
    select(&mut app, "main.py");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with("KEEP"));
}

#[test]
fn split_args_keeps_quoted_runs_together() {
    assert_eq!(split_args("set tab_width 2"), ["set", "tab_width", "2"]);
    assert_eq!(
        split_args(r#"replace "old thing" "new thing""#),
        ["replace", "old thing", "new thing"]
    );
    assert_eq!(split_args("  spaced   out  "), ["spaced", "out"]);
    assert!(split_args("").is_empty());
    // An empty quoted string is a real argument, not nothing.
    assert_eq!(split_args(r#"replace "x" """#), ["replace", "x", ""]);
}

#[test]
fn safe_join_rejects_paths_outside_the_root() {
    let root = Path::new("/project");
    assert!(safe_join(root, "notes/ok.md", root).is_ok());
    assert!(safe_join(root, "../outside.md", root).is_err());
    assert!(safe_join(root, "a/../../outside.md", root).is_err());
    assert!(safe_join(root, "/etc/passwd", root).is_err());
    assert_eq!(
        safe_join(root, "a/../b.md", root).unwrap(),
        PathBuf::from("/project/b.md")
    );
}

/// The arrow a screenshot sentinel names, for `TINY_SHOT_KEYS`.
fn arrow(c: char) -> Option<KeyCode> {
    match c {
        '↑' => Some(KeyCode::Up),
        '↓' => Some(KeyCode::Down),
        '←' => Some(KeyCode::Left),
        '→' => Some(KeyCode::Right),
        _ => None,
    }
}

/// Not an assertion — a way to look at the panes.
/// `TINY_SHOT=/path cargo test screenshot -- --ignored --nocapture`
///
/// `TINY_SHOT_KEYS` sentinels: `^x` Ctrl+X, `^→` Ctrl+Right, `^⇧→`
/// Ctrl+Shift+Right, `⇧→` Shift+Right, `~x` Alt+X, `⏎` Enter, `⎋` Esc, `\t`
/// Tab, and the four arrows themselves.
#[test]
#[ignore]
fn screenshot() {
    let dir = std::env::var("TINY_SHOT").expect("set TINY_SHOT");
    let dir = std::path::Path::new(&dir)
        .canonicalize()
        .expect("TINY_SHOT must name a real directory");
    let dir = dir.to_string_lossy().into_owned();
    let file = std::env::var("TINY_SHOT_FILE").ok();
    let mut app = App::new(target(Path::new(&dir), None), Config::default(), None).unwrap();
    if let Some(f) = file {
        select(&mut app, &f);
    }
    if let Ok(keys) = std::env::var("TINY_SHOT_KEYS") {
        let mut chars = keys.chars();
        while let Some(c) = chars.next() {
            match c {
                // `^b` sends Ctrl+B, so a shot can be taken of anything a
                // key can reach.
                // `^x` is Ctrl+X, and `^` before an arrow — or before a
                // `⇧` and an arrow — is that chord with the modifiers on it.
                '^' => match chars.next() {
                    Some('⇧') => {
                        if let Some(code) = chars.next().and_then(arrow) {
                            app.on_key(ctrl_shift(code));
                        }
                    }
                    Some(c) if arrow(c).is_some() => {
                        app.on_key(ctrl_key(arrow(c).expect("just checked")))
                    }
                    Some(c) => app.on_key(ctrl(c)),
                    None => {}
                },
                // `~x` is Alt+X, so a shot can be taken of the pane widths.
                '~' => {
                    if let Some(c) = chars.next() {
                        app.on_key(alt(KeyCode::Char(c)));
                    }
                }
                '\n' | '⏎' => app.on_key(k(KeyCode::Enter)),
                '\t' => app.on_key(k(KeyCode::Tab)),
                '⎋' => app.on_key(k(KeyCode::Esc)),
                // Named as well as literal, because a shell strips a
                // trailing newline out of the variable this arrives in.
                c if arrow(c).is_some() => app.on_key(k(arrow(c).expect("just checked"))),
                '⇧' => {
                    // Anything unrecognised is left alone rather than
                    // quietly becoming an arrow: a shot that sends the
                    // wrong key is worse than one that sends none.
                    if let Some(code) = chars.next().and_then(arrow) {
                        app.on_key(shift(code));
                    }
                }
                _ => app.on_key(ch(c)),
            }
        }
    }
    let w: u16 = std::env::var("TINY_SHOT_W")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let h: u16 = std::env::var("TINY_SHOT_H")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    for row in screen(&mut app, w, h) {
        println!("{row}");
    }
}
