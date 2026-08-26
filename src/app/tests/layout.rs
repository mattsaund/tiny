//! Where the panes end up, and what they are painted with.
//!
//! The window is two panes and up to two single-row strips, each placeable at
//! either end, so there are more arrangements than there look to be. These
//! also hold the monochrome brief: with the shipped palette, nothing on screen
//! may name a color — meaning is carried by bold, dim, underline and reverse
//! so tiny inherits the terminal's own theme.

use super::*;

#[test]
fn opens_with_the_project_tree_and_a_preview() {
    let (_td, mut app) = fixture();
    let out = joined(&mut app);
    assert!(out.contains("BROWSER"), "{out}");
    for name in ["notes", "src", "README.md", "logo.png"] {
        assert!(out.contains(name), "tree is missing {name}:\n{out}");
    }
}

#[test]
fn the_tree_can_be_moved_to_the_right_hand_side() {
    let cfg = Config {
        tree_side: Side::Right,
        ..Config::default()
    };
    let (_td, mut app) = fixture_with(cfg);
    select(&mut app, "README.md");
    let rows = screen(&mut app, 90, 24);
    let header = &rows[0];
    let browser_at = header.find("BROWSER").expect("tree pane is drawn");
    let file_at = header.find("README.md").expect("preview pane is drawn");
    assert!(
        browser_at > file_at,
        "the tree should be on the right:\n{header}"
    );
}

#[test]
fn borders_can_be_turned_off_for_a_quieter_screen() {
    let cfg = Config {
        borders: false,
        ..Config::default()
    };
    let (_td, mut app) = fixture_with(cfg);
    let out = joined(&mut app);
    assert!(!out.contains('┌'), "no box corners:\n{out}");
    assert!(!out.contains('│'), "no box sides:\n{out}");
    assert!(out.contains("BROWSER"), "the title still shows:\n{out}");
    assert!(out.contains("README.md"));
}

#[test]
fn tree_markers_can_be_plain_ascii() {
    let cfg = Config {
        markers: Markers::Ascii,
        ..Config::default()
    };
    let (_td, mut app) = fixture_with(cfg);
    let out = joined(&mut app);
    assert!(!out.contains('▾'), "no geometric glyphs:\n{out}");
    assert!(out.contains("- "), "an open folder uses a dash:\n{out}");
}

#[test]
fn the_status_line_can_move_to_the_top() {
    let cfg = Config {
        status_position: Position::Top,
        ..Config::default()
    };
    let (_td, mut app) = fixture_with(cfg);
    let rows = screen(&mut app, 90, 24);
    assert!(rows[0].contains("help"), "status is on row 0:\n{}", rows[0]);
    assert!(rows[1].contains("BROWSER"));
}

#[test]
fn a_narrow_terminal_still_renders() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    for (w, h) in [(30u16, 10u16), (28, 6), (120, 40)] {
        let rows = screen(&mut app, w, h);
        assert_eq!(rows.len(), h as usize);
        assert!(rows.iter().all(|r| r.chars().count() == w as usize));
    }
}
#[test]
fn the_chrome_names_no_colors_at_all() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
    t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();

    // The tree pane holds only chrome — no syntax highlighting to excuse
    // a color. Everything in it should inherit the terminal's palette.
    for y in 0..buf.area.height {
        for x in 0..28 {
            let Some(cell) = buf.cell((x, y)) else {
                continue;
            };
            let allowed = matches!(
                cell.fg,
                Color::Reset | Color::White | Color::DarkGray | Color::Gray | Color::Black
            );
            assert!(
                allowed,
                "chrome at {x},{y} uses {:?}; the design brief is white and black",
                cell.fg
            );
        }
    }
}

#[test]
fn no_emoji_or_pictorial_icons_are_drawn() {
    let (_td, mut app) = fixture();
    select(&mut app, "design.md");
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    let out = joined(&mut app);
    for bad in ['●', '☐', '☑', '🖼', '📁', '📄'] {
        assert!(!out.contains(bad), "found {bad:?} in:\n{out}");
    }
    assert!(
        out.contains(" *"),
        "unsaved work is marked with an asterisk"
    );
}
