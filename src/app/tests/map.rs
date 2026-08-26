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
    assert!(bar.contains("| m map |"), "dividers are pipes:\n{bar}");
    assert!(!bar.contains('·'), "{bar}");
    assert!(
        !bar.contains("? help"),
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
    app.on_key(ch('m'));
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
    app.on_key(ch('m'));
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(out.contains('─') || out.contains('│'), "lines:\n{out}");
    assert!(
        ['◂', '▸', '▴', '▾'].iter().any(|a| out.contains(*a)),
        "an arrowhead says which way it runs:\n{out}"
    );
}

#[test]
fn a_code_file_that_calls_another_is_joined_to_it() {
    let (_td, mut app) = linked_fixture();
    app.on_key(ch('m'));
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
    app.on_key(ch('m'));

    let arrows = |app: &mut App| -> usize {
        screen(app, 100, 34)
            .join("")
            .chars()
            .filter(|c| "◂▸▴▾".contains(*c))
            .count()
    };
    assert!(
        arrows(&mut app) > 0,
        "the map opens on a connected file, so it opens with lines"
    );

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
    assert_eq!(
        arrows(&mut app),
        0,
        "and nothing is joined to it, so nothing is drawn:\n{out}"
    );
}

#[test]
fn nothing_is_drawn_on_top_of_a_box() {
    let (_td, mut app) = linked_fixture();
    app.on_key(ch('m'));
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

#[test]
fn the_map_can_be_drawn_without_box_characters() {
    let cfg = Config {
        markers: Markers::Ascii,
        ..Config::default()
    };
    let (_td, mut app) = linked_fixture_with(cfg);
    app.on_key(ch('m'));
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(!out.contains('╭'), "no box drawing at all:\n{out}");
    assert!(
        out.contains("|design.md|"),
        "boxes are made of pipes:\n{out}"
    );
    assert!(out.contains('+'), "and corners of plus signs:\n{out}");
}
