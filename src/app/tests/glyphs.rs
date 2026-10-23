//! The characters tiny draws with, on every screen.
//!
//! The `glyphs` setting promises that with `ascii` nothing outside ASCII
//! reaches the terminal, so tiny looks right in any font. A promise like that
//! is only as good as the one screen somebody forgot, so this walks every
//! screen tiny has, with content that is ASCII itself, and checks each frame
//! character by character. Anything drawn from a literal instead of from the
//! glyph set fails here, naming the screen and the character.
//!
//! The same walk checks the other set for the two characters tiny's text never
//! uses: the em dash and the dot used as a separator. Hints and messages join
//! their parts with commas, colons and pipes instead.

use super::*;

/// A repository with every kind of thing tiny draws something around: a note
/// with each markdown structure, code, a staged rename (an arrow in the
/// change list) and a name too long for its box on the map (an ellipsis).
fn project(cfg: Config) -> (tempfile::TempDir, App) {
    let td = tempfile::tempdir().unwrap();
    let at = td.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(at)
            .args(args)
            .output()
            .expect("git is installed");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "--quiet", "--initial-branch=main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "core.autocrlf", "false"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(
        at.join("note.md"),
        "# Title\n\n## Section\n\n### Deeper\n\n\
         - one\n  - two\n    - three\n\n\
         > quoted text\n\n\
         | a | b |\n|---|---|\n| 1 | 2 |\n\n\
         ```rust\nfn main() {}\n```\n\n\
         ---\n\n\
         - [ ] task\n- [x] done\n\n\
         See [[a-very-long-file-name-that-will-not-fit]].\n",
    )
    .unwrap();
    fs::write(
        at.join("a-very-long-file-name-that-will-not-fit.md"),
        "back to [[note]]\n",
    )
    .unwrap();
    fs::create_dir_all(at.join("src")).unwrap();
    fs::write(
        at.join("src/main.py"),
        "import utils\n\n\ndef main():\n    pass\n",
    )
    .unwrap();
    fs::write(at.join("old.md"), "to be renamed\n").unwrap();
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "first"]);
    git(&["mv", "old.md", "new.md"]);
    fs::write(at.join("src/main.py"), "import utils\n\nchanged = True\n").unwrap();

    let app = App::new(target(at, None), cfg, None).unwrap();
    (td, app)
}

/// Every screen tiny has, as `(what it is, the frame)`.
///
/// Driven with the keys a person would press, so each screen is reached the
/// way it really is and drawn with whatever state that leaves behind.
fn every_screen(app: &mut App) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let mut shot = |what: &'static str, app: &mut App| {
        out.push((what, screen(app, 110, 40).join("\n")));
    };

    shot("the browser", app);
    select(app, "note.md");
    shot("a note, read", app);
    app.focus_editor();
    shot("a note, edited", app);
    app.on_key(k(KeyCode::Esc));
    select(app, "main.py");
    shot("code", app);

    app.on_key(ch('?'));
    shot("help", app);
    app.on_key(k(KeyCode::Esc));

    type_search(app, "main");
    shot("search results", app);
    app.on_key(k(KeyCode::Esc));
    command_bar(app);
    type_str(app, "copy note.md to src");
    shot("a command, described", app);
    app.on_key(k(KeyCode::Esc));

    settings(app);
    shot("settings", app);
    app.on_key(k(KeyCode::Enter)); // Keybinds is the first button
    shot("keybinds", app);
    app.on_key(k(KeyCode::Esc));
    app.on_key(k(KeyCode::Esc));

    app.on_key(ctrl('l'));
    shot("the project map", app);
    app.on_key(k(KeyCode::Esc));

    app.on_key(ctrl('g'));
    shot("git", app);
    app.on_key(k(KeyCode::Esc));
    out
}

#[test]
fn with_ascii_glyphs_nothing_outside_ascii_reaches_the_screen() {
    let cfg = Config {
        glyphs: GlyphSet::Ascii,
        ..Config::default()
    };
    let (_td, mut app) = project(cfg);
    for (what, frame) in every_screen(&mut app) {
        if let Some((n, line)) = frame.lines().enumerate().find(|(_, l)| !l.is_ascii()) {
            let odd: String = line.chars().filter(|c| !c.is_ascii()).collect();
            panic!("{what}, row {n}, draws {odd:?}:\n{frame}");
        }
    }
}

#[test]
fn with_ascii_glyphs_the_structure_is_still_drawn() {
    // Plain is not the same as bare: the borders, markers and markdown are all
    // still there, only drawn from ASCII.
    let cfg = Config {
        glyphs: GlyphSet::Ascii,
        ..Config::default()
    };
    let (_td, mut app) = project(cfg);
    let frames: Vec<(&str, String)> = every_screen(&mut app);
    let frame = |what: &str| {
        frames
            .iter()
            .find(|(w, _)| *w == what)
            .map(|(_, f)| f.clone())
            .unwrap()
    };

    let browser = frame("the browser");
    assert!(browser.contains("+ BROWSER -"), "a border:\n{browser}");
    assert!(browser.contains("> src"), "a closed folder:\n{browser}");

    let note = frame("a note, read");
    for (glyph, what) in [
        ("=====", "the rule under a first heading"),
        ("* one", "a bullet"),
        ("- two", "a nested bullet"),
        ("| quoted", "a quote's gutter"),
        ("-+-", "a table's header rule"),
        ("| fn main", "a code block's gutter"),
    ] {
        assert!(note.contains(glyph), "{what} ({glyph:?}):\n{note}");
    }

    let map = frame("the project map");
    assert!(map.contains("..."), "a long name cut short:\n{map}");
    let git = frame("git");
    assert!(git.contains("old.md -> new.md"), "a rename:\n{git}");
}

#[test]
fn no_screen_draws_an_em_dash_or_a_dot_separator() {
    let (_td, mut app) = project(Config::default());
    for (what, frame) in every_screen(&mut app) {
        for bad in ['\u{2014}', '\u{00b7}'] {
            assert!(
                !frame.contains(bad),
                "{what} draws {bad:?}, which tiny's text never uses:\n{frame}"
            );
        }
    }
}
