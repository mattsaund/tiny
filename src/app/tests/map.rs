//! The project map.
//!
//! What ends up on it, what the filters take off it, where the cursor starts,
//! and what the detail strip says. The geometry of the lines between the boxes
//! is tested in [`crate::ui::ink`] instead, against the grid rather than
//! against a screenshot.

use super::*;

/// The fixture plus the two files it already points at: `design.md` links
/// to `[[architecture]]` and `main.py` calls `utils.load`, so once those
/// exist the map has real edges to draw.
fn linked_fixture_with(cfg: Config) -> (tempfile::TempDir, App) {
    let (td, mut app) = fixture_with(cfg);
    fs::write(
        td.path().join("notes/architecture.md"),
        "# Architecture\n\nback to [[design]]\n",
    )
    .unwrap();
    fs::write(
        td.path().join("src/utils.py"),
        "def load():\n    return 1\n",
    )
    .unwrap();
    command(&mut app, "reload");
    (td, app)
}

fn linked_fixture() -> (tempfile::TempDir, App) {
    linked_fixture_with(Config::default())
}

#[test]
fn the_bottom_bar_says_only_what_is_not_already_on_screen() {
    let (_td, mut app) = fixture();
    let bar = screen(&mut app, 96, 14).pop().expect("a status line");
    assert!(bar.contains("| Ctrl+L map"), "dividers are pipes:\n{bar}");
    assert!(!bar.contains('\u{b7}'), "{bar}");
    assert!(
        !bar.contains("| ? help"),
        "the status already opens with `? for help`:\n{bar}"
    );
    assert!(
        !bar.contains(": commands"),
        "there is no colon key any more:\n{bar}"
    );
}

#[test]
fn the_map_draws_every_file_in_a_box() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(out.contains("PROJECT MAP"), "{out}");
    // Names sit inside boxes, so the borders are right beside them.
    assert!(
        out.contains("│design.md│"),
        "a box with a name in it:\n{out}"
    );
    assert!(
        out.contains('╭') && out.contains('╯'),
        "box corners:\n{out}"
    );
}

#[test]
fn the_map_joins_linked_files_with_a_line() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(out.contains('─') || out.contains('│'), "lines:\n{out}");
    // No arrowheads: which way a connection runs is the strip's to say, in
    // words. That the routing writes nothing but lines is `ink`'s own test.
    assert!(
        !['◂', '▸', '▴', '▾'].iter().any(|a| out.contains(*a)),
        "a connection is a plain line; which way it runs is in the strip below:\n{out}"
    );
}

#[test]
fn a_code_file_that_calls_another_is_joined_to_it() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    // Only calls: the notes are switched off, so whatever is left on
    // screen is there because of the code.
    app.on_key(ch('1'));
    app.on_key(ch('2'));
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(out.contains("│main.py│"), "{out}");
    assert!(out.contains("│utils.py│"), "{out}");
    assert!(
        out.contains("unconnected"),
        "the notes are unconnected now, so they are filed as such:\n{out}"
    );
}

#[test]
fn the_map_draws_the_cursors_connections_and_nobody_elses() {
    let (td, mut app) = linked_fixture();
    // A file joined to nothing, so there is a selection with no lines.
    fs::write(td.path().join("lonely.md"), "nothing points here\n").unwrap();
    command(&mut app, "reload");
    open_map(&mut app);

    // Every cell a line can occupy. The same boxes are drawn either way, so
    // the difference between two frames is exactly the connections in them.
    let ink = |app: &mut App| -> usize {
        screen(app, 100, 34)
            .join("")
            .chars()
            .filter(|c| "─│┌┐└┘├┤┬┴┼".contains(*c))
            .count()
    };
    let connected = ink(&mut app);

    // Walk to the lonely file. It is under its own heading at the end.
    for _ in 0..12 {
        app.on_key(k(KeyCode::Tab));
        let map = app.project_map.as_ref().expect("the map is open");
        if map
            .selected_node()
            .is_some_and(|n| n.rel.ends_with("lonely.md"))
        {
            break;
        }
    }
    let map = app.project_map.as_ref().expect("the map is open");
    assert!(
        map.selected_node()
            .is_some_and(|n| n.rel.ends_with("lonely.md")),
        "Tab should have reached it"
    );
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(
        out.contains("│lonely.md│"),
        "it is still drawn, under its own heading:\n{out}"
    );
    assert!(
        ink(&mut app) < connected,
        "and nothing is joined to it, so its lines are gone:\n{out}"
    );
}

#[test]
fn nothing_is_drawn_on_top_of_a_box() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    let rows = screen(&mut app, 100, 34);
    // Every name on screen is whole, with its own border either side: a
    // line crossing the box would have overwritten one of those cells.
    for name in ["design.md", "architecture.md", "main.py", "utils.py"] {
        let row = rows
            .iter()
            .find(|r| r.contains(name))
            .unwrap_or_else(|| panic!("{name} is missing:\n{}", rows.join("\n")));
        let at = row.find(name).unwrap();
        assert_eq!(
            row[..at].chars().last().unwrap(),
            '│',
            "{name} sits inside its box:\n{row}"
        );
    }
}

/// Everything on screen drawn in one color, row by row.
///
/// The map is the only place tiny uses color at all, so a color is a
/// reliable way to ask "which files did it call outgoing" without knowing
/// where on the grid they landed.
fn drawn_in(app: &mut App, color: Color) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 34)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            match buf.cell((x, y)) {
                Some(c) if c.fg == color => out.push_str(c.symbol()),
                _ => {}
            }
        }
        out.push('\n');
    }
    out
}

/// Put the map's cursor on a named file without walking there.
fn cursor_on(app: &mut App, name: &str) {
    let map = app.project_map.as_mut().expect("the map is open");
    map.selected = map
        .graph
        .nodes
        .iter()
        .position(|n| n.rel.ends_with(name))
        .unwrap_or_else(|| panic!("no node for {name}"));
}

#[test]
fn a_file_this_one_reaches_is_drawn_in_the_outgoing_color() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    // `main.py` calls `utils.load`, and nothing calls `main.py`.
    cursor_on(&mut app, "main.py");

    let red = drawn_in(&mut app, Color::Red);
    assert!(red.contains("utils.py"), "the file it reaches:\n{red}");
    assert!(red.contains("out:"), "and the row that names them:\n{red}");

    let green = drawn_in(&mut app, Color::Green);
    assert!(
        !green.contains("utils.py"),
        "the connection runs one way, so it is not both:\n{green}"
    );
}

#[test]
fn a_file_that_reaches_this_one_is_drawn_in_the_incoming_color() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    cursor_on(&mut app, "utils.py");

    let green = drawn_in(&mut app, Color::Green);
    assert!(
        green.contains("main.py"),
        "the file that reaches it:\n{green}"
    );
    assert!(
        green.contains("in:"),
        "and the row that names them:\n{green}"
    );

    let red = drawn_in(&mut app, Color::Red);
    assert!(!red.contains("main.py"), "not the other way:\n{red}");
}

#[test]
fn a_file_on_neither_side_of_the_cursor_is_left_uncolored() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    cursor_on(&mut app, "main.py");
    for color in [Color::Red, Color::Green] {
        let painted = drawn_in(&mut app, color);
        assert!(
            !painted.contains("design.md"),
            "design.md has nothing to do with main.py:\n{painted}"
        );
    }
}

#[test]
fn the_two_directions_can_be_recolored_from_the_config() {
    let (_td, mut app) = linked_fixture();
    command(&mut app, "set theme.map_out #ff00aa");
    command(&mut app, "set theme.map_in #00ccff");
    open_map(&mut app);
    cursor_on(&mut app, "utils.py");

    let hex = drawn_in(&mut app, Color::Rgb(0x00, 0xcc, 0xff));
    assert!(hex.contains("main.py"), "the incoming color took:\n{hex}");
    assert!(
        drawn_in(&mut app, Color::Green).trim().is_empty(),
        "and the shipped green is gone"
    );

    cursor_on(&mut app, "main.py");
    let hex = drawn_in(&mut app, Color::Rgb(0xff, 0x00, 0xaa));
    assert!(
        hex.contains("utils.py"),
        "and so did the outgoing one:\n{hex}"
    );
}

#[test]
fn the_map_can_be_drawn_without_box_characters() {
    let cfg = Config {
        glyphs: GlyphSet::Ascii,
        ..Config::default()
    };
    let (_td, mut app) = linked_fixture_with(cfg);
    open_map(&mut app);
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(!out.contains('╭'), "no box drawing at all:\n{out}");
    assert!(
        out.contains("|design.md|"),
        "boxes are made of pipes:\n{out}"
    );
    assert!(out.contains('+'), "and corners of plus signs:\n{out}");
}

// ---- the bar, over the map -------------------------------------------------
//
// The map has no search of its own: narrowing it is the same field, in the
// same place, that searches the project from every other window.

#[test]
fn slash_over_the_map_opens_the_one_bar() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    app.on_key(ch('/'));

    assert!(
        matches!(app.mode, Mode::Bar(_)),
        "the bar, not a box of its own"
    );
    let out = joined(&mut app);
    assert!(out.contains("narrowing the map"), "and it says so:\n{out}");
}

#[test]
fn typing_in_the_bar_narrows_the_map_as_it_goes() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    app.on_key(ch('/'));
    type_str(&mut app, "src");

    let view = app.project_map.as_ref().unwrap();
    assert_eq!(view.filter(), "src");
    let shown: Vec<&str> = view
        .visible_indices()
        .iter()
        .map(|i| view.graph.nodes[*i].rel.as_str())
        .collect();
    assert!(!shown.is_empty(), "something is still drawn");
    assert!(
        shown.iter().all(|r| r.contains("src")),
        "only what matches: {shown:?}"
    );
}

#[test]
fn enter_keeps_the_filter_and_escape_takes_it_off() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    app.on_key(ch('/'));
    type_str(&mut app, "src");
    app.on_key(k(KeyCode::Enter));

    assert!(matches!(app.mode, Mode::Normal), "the bar is put away");
    assert_eq!(
        app.project_map.as_ref().unwrap().filter(),
        "src",
        "the filter stays"
    );
    assert_eq!(app.window, Window::Map, "and the map is still the window");

    app.on_key(ch('/'));
    type_str(&mut app, "notes");
    app.on_key(k(KeyCode::Esc));
    assert!(
        app.project_map.as_ref().unwrap().filter().is_empty(),
        "escape clears it"
    );
}

#[test]
fn a_command_still_runs_from_over_the_map() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    app.on_key(ch('/'));
    // A star turns the same field into the command line, whatever window it
    // is drawn over: the filter is not what a starred line means.
    type_str(&mut app, "*new fromthemap.md");
    app.on_key(k(KeyCode::Enter));

    assert!(
        app.project_map.as_ref().unwrap().filter().is_empty(),
        "a command never filtered anything"
    );
    assert!(joined(&mut app).contains("fromthemap.md") || app.window == Window::Main);
}

// ---- the window keys ------------------------------------------------------

#[test]
fn the_window_keys_are_letters_every_terminal_sends() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('l'));
    assert_eq!(app.window, Window::Map, "Ctrl+L is the map");
    app.on_key(ctrl('l'));
    assert_eq!(app.window, Window::Main, "and the same key puts it away");

    app.on_key(ctrl('g'));
    assert_eq!(app.window, Window::Source, "Ctrl+G is git");
    app.on_key(ctrl('l'));
    assert_eq!(app.window, Window::Map, "one window straight to the other");
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.window, Window::Main, "Esc comes back from either");

    // Ctrl with a digit is not in a terminal's legacy encoding (`Ctrl+3`
    // arrives as Esc on most of them) so it is not bound to anything.
    app.on_key(ctrl('3'));
    assert_eq!(app.window, Window::Main);
}

#[test]
fn the_hint_names_the_window_keys_the_keymap_has() {
    let (_td, mut app) = fixture();
    let bar = screen(&mut app, 96, 14).pop().expect("a status line");
    assert!(bar.contains("Ctrl+G git | Ctrl+L map"), "{bar}");

    app.config.keys.insert("map".into(), "ctrl+o".into());
    app.apply_config();
    let bar = screen(&mut app, 96, 14).pop().expect("a status line");
    assert!(bar.contains("Ctrl+O map"), "a rebinding shows:\n{bar}");
}

#[test]
fn a_narrow_window_drops_hints_from_the_end_and_keeps_the_first() {
    let (_td, mut app) = fixture();
    // Too narrow for all three of the browser's hints, wide enough for two.
    let bar = screen(&mut app, 55, 14).pop().expect("a status line");
    assert!(bar.contains("/ search | Ctrl+G git"), "{bar}");
    assert!(!bar.contains("Ctrl+L"), "no half hint either: {bar}");
}

#[test]
fn a_lone_key_is_named_as_it_is_typed() {
    use crate::config::keys::Action;
    let (_td, app) = fixture();
    // `R` would be Shift and r, which does nothing.
    assert_eq!(app.key_name(Action::Refresh), "r");
    assert_eq!(app.key_name(Action::Save), "Ctrl+S");
    assert_eq!(app.key_name(Action::Back), "Esc");
    assert_eq!(app.key_name(Action::EditorCursorUp), "Alt+Shift+Up");
}

#[test]
fn a_rebinding_survives_the_terminal_being_asked() {
    let (_td, mut app) = fixture();
    app.config.keys.insert("map".into(), "ctrl+o".into());
    app.apply_config();
    // What `main` does once it knows the terminal is Windows'.
    app.set_keyboard(Keyboard::Windows);

    app.on_key(ctrl('o'));
    assert_eq!(
        app.window,
        Window::Map,
        "what the user asked for still wins"
    );
}

#[test]
fn r_reads_the_project_again_and_keeps_how_it_was_being_looked_at() {
    let (_td, mut app) = linked_fixture();
    open_map(&mut app);
    app.on_key(ch('3'));
    app.on_key(ch('/'));
    type_str(&mut app, "notes");
    app.on_key(k(KeyCode::Enter));
    let before = app.project_map.as_ref().unwrap();
    let (kinds, filter) = (before.kinds, before.filter().to_string());
    let on = before.selected_node().map(|n| n.rel.clone());

    app.on_key(ch('r'));
    let after = app.project_map.as_ref().unwrap();
    assert_eq!(after.kinds, kinds, "the kinds switched off stay off");
    assert_eq!(after.filter(), filter, "the filter stays");
    assert_eq!(
        after.selected_node().map(|n| n.rel.clone()),
        on,
        "and the cursor"
    );
}

#[test]
fn files_that_share_a_name_are_told_apart_in_the_detail_strip() {
    let (td, mut app) = fixture();
    for dir in ["app", "ui"] {
        fs::create_dir_all(td.path().join(dir)).unwrap();
        fs::write(td.path().join(dir).join("mod.md"), "back to [[README]]\n").unwrap();
    }
    command(&mut app, "reload");
    open_map(&mut app);
    let view = app.project_map.as_ref().unwrap();
    let names: Vec<&str> = (0..view.graph.nodes.len())
        .filter(|i| view.graph.nodes[*i].name == "mod.md")
        .map(|i| view.short_name(i))
        .collect();
    assert_eq!(names, ["app/mod.md", "ui/mod.md"]);
}
