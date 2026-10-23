//! The source-control window, against a real repository.
//!
//! [`crate::git::parse`] tests the awkward text on its own; these tests build
//! an actual repository in a temp directory and drive the window with keys, so
//! what is covered here is the part that could not be faked: that git agrees
//! with the parser, that Enter stages the thing under the cursor, and that the
//! before-and-after columns show the file as it was and as it is.

use super::*;

/// A repository with one commit, one modified file and one untracked one.
///
/// `-c` rather than a config file, so nothing depends on whatever git the
/// machine running the tests happens to have configured.
fn repo() -> (tempfile::TempDir, App) {
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
    // Three settings a machine's own git config could otherwise impose on
    // these tests. `autocrlf` is on by default on Windows and would rewrite
    // every line ending between the working tree and the index, so a diff
    // would be of newlines rather than of words; `safecrlf` would turn the
    // warning about that into an error; and a globally-signed commit needs a
    // key this repository has no business asking for.
    git(&["config", "core.autocrlf", "false"]);
    git(&["config", "core.safecrlf", "false"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(at.join("README.md"), "# Fixture\n\nhello widget\n").unwrap();
    fs::create_dir_all(at.join("src")).unwrap();
    fs::write(at.join("src/main.py"), "import utils\n").unwrap();
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "first"]);

    // One tracked file changed, one file git has never seen.
    fs::write(at.join("README.md"), "# Fixture\n\ngoodbye gadget\n").unwrap();
    fs::write(at.join("scratch.txt"), "notes to self\n").unwrap();

    let app = App::new(target(at, None), Config::default(), None).unwrap();
    (td, app)
}

/// Switch to the source window the way `Ctrl+G` does.
fn source(app: &mut App) {
    app.on_key(ctrl('g'));
    assert_eq!(app.window, Window::Source);
}

/// Put the cursor on a named file in the change list.
fn cursor_on(app: &mut App, name: &str) {
    for _ in 0..20 {
        if app
            .git
            .current()
            .is_some_and(|(c, _)| c.path.ends_with(name))
        {
            return;
        }
        app.on_key(k(KeyCode::Down));
    }
    panic!("never reached {name} in {:?}", app.git.rows);
}

#[test]
fn the_window_lists_what_has_changed_with_gits_own_letters() {
    let (_td, mut app) = repo();
    source(&mut app);

    let out = joined(&mut app);
    assert!(out.contains("SOURCE CONTROL"), "{out}");
    assert!(out.contains("main"), "the branch is named:\n{out}");
    assert!(
        out.contains("UNSTAGED 2"),
        "both changes are listed:\n{out}"
    );
    assert!(
        out.contains("M README.md"),
        "modified, with its letter:\n{out}"
    );
    assert!(out.contains("U scratch.txt"), "untracked too:\n{out}");
}

#[test]
fn enter_stages_the_file_under_the_cursor_and_enter_again_takes_it_back() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");

    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.git.status.staged.len(), 1, "{}", app.status);
    assert_eq!(app.git.status.staged[0].path, "README.md");
    assert!(joined(&mut app).contains("STAGED 1"));

    // Stage a second one, chosen so that "the first file in the list" is a
    // different answer from "the file that was just staged"; otherwise the
    // cursor could be landing in the right place by luck.
    cursor_on(&mut app, "scratch.txt");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.git.status.staged.len(), 2, "{}", app.status);
    assert!(
        app.git
            .current()
            .is_some_and(|(c, s)| c.path == "scratch.txt" && s),
        "the cursor followed the file it staged, rather than falling to the top"
    );

    // Which is what makes Enter a toggle: the same key sends it back.
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.git.status.staged.len(), 1, "{}", app.status);
    assert_eq!(app.git.status.staged[0].path, "README.md");
}

#[test]
fn enter_on_a_section_takes_the_whole_section() {
    let (_td, mut app) = repo();
    source(&mut app);
    // The header is the row above the first file.
    while !matches!(
        app.git.rows.get(app.git.selected),
        Some(GitRow::Header { .. })
    ) {
        app.on_key(k(KeyCode::Up));
    }
    app.on_key(k(KeyCode::Enter));

    assert_eq!(
        app.git.status.staged.len(),
        2,
        "both at once: {}",
        app.status
    );
    assert!(app.git.status.unstaged.is_empty());
}

#[test]
fn the_two_columns_show_the_file_before_and_after() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");

    let before = app
        .git
        .diff
        .before
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let after = app
        .git
        .diff
        .after
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        before.contains("hello widget"),
        "the committed text: {before}"
    );
    assert!(!before.contains("goodbye gadget"));
    assert!(
        after.contains("goodbye gadget"),
        "and the working copy: {after}"
    );
    assert_eq!(
        app.git.diff.before.len(),
        app.git.diff.after.len(),
        "the sides stay level so the eye can read across"
    );

    let out = joined(&mut app);
    assert!(out.contains("before"), "both halves are drawn:\n{out}");
    assert!(out.contains("after"), "{out}");
}

#[test]
fn a_block_added_on_one_side_is_a_labeled_gap_on_the_other() {
    let (td, mut app) = repo();
    let at = td.path();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(at)
            .args(args)
            .output()
            .expect("git is installed");
    };
    fs::write(at.join("block.py"), "before\nafter\n").unwrap();
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "block"]);
    // Four lines dropped in between the two that were there.
    fs::write(
        at.join("block.py"),
        "before\none\ntwo\nthree\nfour\nafter\n",
    )
    .unwrap();

    let mut app2 = App::new(target(at, None), Config::default(), None).unwrap();
    std::mem::swap(&mut app, &mut app2);
    source(&mut app);
    cursor_on(&mut app, "block.py");

    let out = joined(&mut app);
    // Once, saying what the gap is, and not a mark on every row of it, which
    // read as a column of ragged indent where the file's own text should be.
    assert!(
        out.contains("  4 lines added"),
        "the gap says what it is, lined up with the code:\n{out}"
    );
    assert_eq!(
        out.matches("lines added").count(),
        1,
        "and says it once:\n{out}"
    );
    assert!(!out.contains('~'), "no per-line filler marks left:\n{out}");
    // The file as it was is still all there and still lined up.
    assert!(out.contains("  before"), "{out}");
    assert!(out.contains("  after"), "{out}");
}

#[test]
fn a_gap_on_the_after_side_says_removed_instead() {
    let (td, mut app) = repo();
    let at = td.path();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(at)
            .args(args)
            .output()
            .expect("git is installed");
    };
    fs::write(at.join("block.py"), "keep\none\ntwo\nkeep too\n").unwrap();
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "block"]);
    fs::write(at.join("block.py"), "keep\nkeep too\n").unwrap();

    let mut app2 = App::new(target(at, None), Config::default(), None).unwrap();
    std::mem::swap(&mut app, &mut app2);
    source(&mut app);
    cursor_on(&mut app, "block.py");

    let out = joined(&mut app);
    assert!(out.contains("  2 lines removed"), "{out}");
}

#[test]
fn an_untracked_file_is_all_new_on_the_right() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "scratch.txt");

    assert!(
        app.git.diff.before.iter().all(|l| l.text.is_empty()),
        "git has never seen it, so there is no before"
    );
    assert!(
        app.git
            .diff
            .after
            .iter()
            .any(|l| l.text.contains("notes to self")),
        "and all of it is after"
    );
}

#[test]
fn committing_is_a_command_and_it_clears_the_list() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.git.status.staged.len(), 1);

    command(&mut app, "commit a second commit");
    assert!(app.status.contains("a second commit"), "{}", app.status);
    assert!(app.git.status.staged.is_empty(), "the index is empty again");
}

#[test]
fn a_commit_with_no_message_is_refused() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));

    command(&mut app, "commit");
    assert!(app.status.contains("needs a message"), "{}", app.status);
    assert_eq!(app.git.status.staged.len(), 1, "and nothing was committed");
}

#[test]
fn the_branch_map_is_drawn_under_the_changes() {
    let (_td, mut app) = repo();
    source(&mut app);
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(out.contains("branches"), "the panel is there:\n{out}");
    assert!(out.contains("first"), "with the commit in it:\n{out}");
}

#[test]
fn tab_leaves_the_window_with_the_file_open() {
    let (td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");

    app.on_key(k(KeyCode::Tab));
    assert_eq!(app.window, Window::Main, "back to the files");
    // The path came back from git, and the one to compare it with was built
    // here: two spellings of the same file on a Mac. See `same_file`.
    let want = td.path().join("README.md");
    let got = app.selected_path().expect("a file is selected");
    assert!(
        same_file(got, &want),
        "on the one that was under the cursor: {got:?} vs {want:?}"
    );
}

#[test]
fn the_buttons_are_a_row_the_arrows_walk_along() {
    let (_td, mut app) = repo();
    source(&mut app);
    // The button row is the first one, so Up from anywhere reaches it.
    for _ in 0..10 {
        app.on_key(k(KeyCode::Up));
    }
    assert_eq!(app.git.rows[app.git.selected], GitRow::Buttons);
    assert_eq!(app.git.button, 0, "starts on the first");

    app.on_key(k(KeyCode::Right));
    assert_eq!(app.git.button, 1, "right walks along it: {}", app.status);
    app.on_key(k(KeyCode::Left));
    assert_eq!(app.git.button, 0, "and left walks back");

    // Left and right do nothing off the button row, where they would otherwise
    // be a second way to move that means nothing.
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Right));
    assert_eq!(app.git.button, 0);
}

/// A repository whose one change is in a file with a grammar behind it.
fn python_repo() -> (tempfile::TempDir, App) {
    let (td, _) = repo();
    let at = td.path().to_path_buf();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&at)
            .args(args)
            .output()
            .expect("git is installed");
    };
    fs::write(
        at.join("src/main.py"),
        "import utils\n\n\ndef main():\n    return utils.load(2)\n",
    )
    .unwrap();
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "python"]);
    fs::write(
        at.join("src/main.py"),
        "import utils\n\n\ndef main():\n    return utils.load(3)\n",
    )
    .unwrap();
    let app = App::new(target(&at, None), Config::default(), None).unwrap();
    (td, app)
}

/// The color the first character of `word` is drawn in, the first time it
/// appears on screen.
///
/// Precise on purpose: counting colored cells anywhere would count the
/// borders, which are colored whatever else is or is not.
fn color_of(app: &mut App, word: &str) -> Color {
    let (w, h) = (110u16, 20u16);
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    for y in 0..h {
        let row: String = (0..w)
            .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
            .collect();
        if let Some(at) = row.find(word) {
            return buf
                .cell((at as u16, y))
                .map(|c| c.fg)
                .unwrap_or(Color::Reset);
        }
    }
    panic!("`{word}` is not on screen");
}

#[test]
fn the_diff_is_syntax_highlighted_on_both_sides() {
    let (_td, mut app) = python_repo();
    source(&mut app);
    cursor_on(&mut app, "main.py");

    // `def` is a keyword in Python and syntect gives it a color of its own.
    // Unhighlighted text is drawn in the theme's `text`, which names no color
    // at all, so a color here is the grammar talking.
    let keyword = color_of(&mut app, "def main");
    assert_ne!(
        keyword,
        Color::Reset,
        "the code in the diff should be syntax highlighted"
    );
    // And the gutter is not what is being measured: it is two columns to the
    // left of the text and keeps its own red and green.
    assert!(
        !matches!(keyword, Color::Red | Color::Green),
        "that is the gutter's color, not the grammar's: {keyword:?}"
    );
}

#[test]
fn a_file_with_no_grammar_is_still_drawn() {
    // `scratch.txt` has no syntax behind it, and must still show its text
    // rather than falling over or coming out blank.
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "scratch.txt");
    let out = joined(&mut app);
    assert!(out.contains("notes to self"), "{out}");
}

#[test]
fn right_goes_into_the_diff_and_the_arrows_scroll_both_halves() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    assert_eq!(app.git.focus, GitFocus::List);

    app.on_key(k(KeyCode::Right));
    assert_eq!(app.git.focus, GitFocus::Diff, "{}", app.status);

    // Up and down now scroll rather than moving the cursor, and one offset
    // moves both columns because there is only one offset.
    let row = app.git.selected;
    app.on_key(k(KeyCode::Down));
    assert_eq!(app.git.diff_scroll, 1, "the diff moved");
    assert_eq!(app.git.selected, row, "and the change list did not");
    app.on_key(k(KeyCode::Up));
    assert_eq!(app.git.diff_scroll, 0);

    app.on_key(k(KeyCode::Left));
    assert_eq!(app.git.focus, GitFocus::List, "left comes back");
    app.on_key(k(KeyCode::Down));
    assert_ne!(app.git.selected, row, "and the arrows move the list again");
}

/// Whether the change list's row for `name` has any reversed cell, which is
/// what the selection bar is, in the default theme.
///
/// Looks in the left column only: the diff's own title names the same file, and
/// finding that row instead would be measuring the wrong thing.
fn row_is_highlighted(app: &mut App, name: &str) -> bool {
    let (w, h) = (100u16, 20u16);
    let list = (w as f32 * app.config.tree_width) as u16;
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    for y in 0..h {
        let row: String = (0..list)
            .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
            .collect();
        if row.contains(name) {
            return (0..list).any(|x| {
                buf.cell((x, y))
                    .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
            });
        }
    }
    panic!("`{name}` is not in the change list");
}

#[test]
fn the_change_list_steps_back_when_the_keyboard_goes_into_the_diff() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    assert!(
        row_is_highlighted(&mut app, "README.md"),
        "the cursor's row is highlighted while the list has the keyboard"
    );

    app.on_key(k(KeyCode::Right));
    assert_eq!(app.git.focus, GitFocus::Diff);
    assert!(
        !row_is_highlighted(&mut app, "README.md"),
        "and quietly marked once it does not, the same as the browser"
    );

    app.on_key(k(KeyCode::Left));
    assert!(
        row_is_highlighted(&mut app, "README.md"),
        "and highlighted again on the way back"
    );
}

#[test]
fn the_change_list_steps_back_for_the_message_box_too() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);
    assert_eq!(app.git.focus, GitFocus::Message);
    assert!(
        !row_is_highlighted(&mut app, "README.md"),
        "writing a message is not being in the list"
    );
}

#[test]
fn moving_to_another_file_leaves_the_diff_and_starts_at_its_top() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Right));
    app.on_key(k(KeyCode::Down));
    assert_eq!((app.git.focus, app.git.diff_scroll), (GitFocus::Diff, 1));

    app.on_key(k(KeyCode::Left));
    app.on_key(k(KeyCode::Down));
    assert_eq!(
        app.git.focus,
        GitFocus::List,
        "reading the next one from the list"
    );
    assert_eq!(app.git.diff_scroll, 0, "and from the top of it");
}

/// A repository with a file long enough to scroll through.
fn long_repo() -> (tempfile::TempDir, App) {
    let (td, _) = repo();
    let at = td.path().to_path_buf();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&at)
            .args(args)
            .output()
            .expect("git is installed");
    };
    let before: String = (0..60).map(|n| format!("line {n}\n")).collect();
    fs::write(at.join("long.txt"), &before).unwrap();
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "-m", "long"]);
    fs::write(at.join("long.txt"), before.replace("line 30", "CHANGED")).unwrap();
    let app = App::new(target(&at, None), Config::default(), None).unwrap();
    (td, app)
}

#[test]
fn a_page_key_pages_the_diff_from_either_half() {
    let (_td, mut app) = long_repo();
    source(&mut app);
    cursor_on(&mut app, "long.txt");
    // Drawn once, so the diff has a height to page by.
    joined(&mut app);
    let page = app.git.diff_height.saturating_sub(1).max(1);

    app.on_key(k(KeyCode::PageDown));
    assert_eq!(app.git.diff_scroll, page, "{}", app.status);
    // The chord beside it, for a keyboard with no Page Down.
    app.on_key(ctrl('u'));
    assert_eq!(app.git.diff_scroll, 0, "and back up again");

    app.on_key(k(KeyCode::Right));
    assert_eq!(app.git.focus, GitFocus::Diff);
    app.on_key(ctrl('d'));
    assert_eq!(app.git.diff_scroll, page, "the same from inside the diff");
}

#[test]
fn a_page_key_pages_the_message_box_too() {
    // `long_repo` committed everything else, so `long.txt` is the one change.
    let (_td, mut app) = long_repo();
    source(&mut app);
    cursor_on(&mut app, "long.txt");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);
    for n in 0..10 {
        type_str(&mut app, &format!("line {n}"));
        app.on_key(k(KeyCode::Enter));
    }
    let editor = app.git.message.as_ref().unwrap();
    assert_eq!(editor.cursor_line, 10, "ten lines in");

    app.on_key(ctrl('u'));
    let page = app.git.diff_height.saturating_sub(1).max(1);
    assert_eq!(
        app.git.message.as_ref().unwrap().cursor_line,
        10usize.saturating_sub(page),
        "a screen back: the editor's own key, working here too"
    );
    assert!(
        !app.git.message.as_ref().unwrap().to_text().contains('u'),
        "and not a letter typed"
    );
}

#[test]
fn the_angle_brackets_size_the_change_list_without_folding_it() {
    let (_td, mut app) = repo();
    source(&mut app);
    let start = app.config.tree_width;

    app.on_key(ch('>'));
    assert!(app.config.tree_width > start, "wider: {}", app.status);
    app.on_key(ch('<'));
    assert!((app.config.tree_width - start).abs() < 0.001, "and back");

    // Past the end it stops rather than folding: a source window with no
    // change list has nothing in it.
    for _ in 0..20 {
        app.on_key(ch('<'));
    }
    assert!(!app.tree_hidden, "the list is still there: {}", app.status);
    assert!(app.config.tree_width >= 0.10);
    assert!(joined(&mut app).contains("UNSTAGED"), "and still drawn");
}

#[test]
fn a_project_that_is_not_a_repository_says_so_rather_than_looking_broken() {
    let (_td, mut app) = fixture();
    app.on_key(ctrl('g'));
    assert_eq!(app.window, Window::Source);

    let out = joined(&mut app);
    assert!(
        out.to_lowercase().contains("not a git repository"),
        "it says why there is nothing:\n{out}"
    );
}

/// Put the cursor on the commit button and press it.
fn press_commit(app: &mut App) {
    for _ in 0..10 {
        app.on_key(k(KeyCode::Up));
    }
    for _ in 0..GIT_BUTTONS.len() {
        app.on_key(k(KeyCode::Right));
    }
    app.on_key(k(KeyCode::Enter));
}

#[test]
fn the_commit_button_opens_a_message_box_over_both_columns() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter)); // stage it, so there is something to commit
    press_commit(&mut app);

    assert_eq!(app.git.focus, GitFocus::Message, "{}", app.status);
    // The first press opens the box and does nothing else; it must not try to
    // commit the empty message it has just made.
    assert!(
        app.status.contains("write the message"),
        "one press opens, it does not send: {}",
        app.status
    );
    assert_eq!(
        app.git.status.staged.len(),
        1,
        "still staged, not committed"
    );
    let out = joined(&mut app);
    assert!(out.contains("commit message"), "the box is drawn:\n{out}");
    assert!(
        out.contains("1 staged file"),
        "and says what it will take:\n{out}"
    );
    assert!(
        !out.contains("before"),
        "over both diff columns, not beside them:\n{out}"
    );
}

#[test]
fn the_message_box_types_and_the_button_sends_it() {
    let (td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);

    // A summary, a blank line, and a body: the shape git expects.
    type_str(&mut app, "tidy the README");
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "the widget was a gadget all along");
    assert!(
        joined(&mut app).contains("tidy the README"),
        "{}",
        app.status
    );

    // Esc puts the box down without losing a word of it, and the button sends
    // it, which is the two-press shape the window promises.
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.git.focus, GitFocus::List);
    assert!(app.git.message.is_some(), "the message is kept");
    press_commit(&mut app);

    assert!(
        app.git.status.staged.is_empty(),
        "committed: {}",
        app.status
    );
    assert!(app.git.message.is_none(), "and the box is put away");
    let log = std::process::Command::new("git")
        .current_dir(td.path())
        .args(["log", "-1", "--pretty=format:%s%n%n%b"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&log.stdout);
    assert!(log.contains("tidy the README"), "{log}");
    assert!(log.contains("gadget all along"), "the body went too: {log}");
}

#[test]
fn ctrl_s_commits_from_inside_the_box() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);
    type_str(&mut app, "straight from the box");

    app.on_key(ctrl('s'));
    assert!(
        app.git.status.staged.is_empty(),
        "committed: {}",
        app.status
    );
    assert!(
        app.status.contains("straight from the box"),
        "{}",
        app.status
    );
}

#[test]
fn the_box_does_not_open_with_nothing_staged() {
    let (_td, mut app) = repo();
    source(&mut app);
    press_commit(&mut app);
    assert_eq!(app.git.focus, GitFocus::List);
    assert!(app.git.message.is_none());
    assert!(app.status.contains("nothing staged"), "{}", app.status);
}

#[test]
fn a_letter_in_the_message_is_a_letter_and_not_a_key() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);

    // `r` refreshes the window from the list; in the box it is the letter r.
    type_str(&mut app, "refactor");
    let message = app.git.message.as_ref().unwrap().to_text();
    // The buffer keeps the trailing newline a text file ends with, which is
    // also what git wants at the end of a message.
    assert_eq!(message, "refactor\n", "{}", app.status);
}

/// The repository, with sixty more files git has never seen: a change list
/// far longer than the window it is drawn in.
fn crowded_repo() -> (tempfile::TempDir, App) {
    let (td, _) = repo();
    for n in 0..60 {
        fs::write(td.path().join(format!("new-{n:02}.txt")), "x\n").unwrap();
    }
    let app = App::new(target(td.path(), None), Config::default(), None).unwrap();
    (td, app)
}

/// The change list as drawn, a row of text per screen row, and the text of
/// the rows drawn as the cursor.
fn change_list(app: &mut App, w: u16, h: u16) -> (Vec<String>, String) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let (x0, x1) = app.git.list_cols.expect("the list was drawn");
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

/// The name of the file the cursor is on.
fn current_name(app: &App) -> String {
    app.git.current().expect("on a file").0.path.clone()
}

#[test]
fn a_change_list_longer_than_the_window_scrolls_to_the_cursor() {
    let (_td, mut app) = crowded_repo();
    source(&mut app);
    let (rows, _) = change_list(&mut app, 100, 24);
    assert!(
        rows.iter().any(|r| r.contains(" more ")),
        "what is out of sight is counted on the border:\n{}",
        rows.join("\n")
    );

    for _ in 0..50 {
        app.on_key(k(KeyCode::Down));
    }
    let name = current_name(&app);
    let (rows, cursor) = change_list(&mut app, 100, 24);
    assert!(
        cursor.contains(&name),
        "the cursor's row is on screen, as the cursor: {cursor:?}\n{}",
        rows.join("\n")
    );
    assert!(
        !rows.iter().any(|r| r.contains("fetch")),
        "and the top of the list has scrolled away to make room:\n{}",
        rows.join("\n")
    );
}

#[test]
fn home_and_end_reach_both_ends_of_the_change_list() {
    let (_td, mut app) = crowded_repo();
    source(&mut app);
    change_list(&mut app, 100, 24);

    app.on_key(k(KeyCode::End));
    assert_eq!(app.git.selected, app.git.rows.len() - 1);
    let name = current_name(&app);
    let (rows, cursor) = change_list(&mut app, 100, 24);
    assert!(cursor.contains(&name), "{cursor:?}\n{}", rows.join("\n"));
    assert!(
        !rows.iter().any(|r| r.contains(" more ")),
        "nothing is left below the last row:\n{}",
        rows.join("\n")
    );

    app.on_key(k(KeyCode::Home));
    assert_eq!(app.git.selected, 0, "the buttons");
    let (rows, _) = change_list(&mut app, 100, 24);
    assert!(
        rows.iter().any(|r| r.contains("fetch")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn the_wheel_over_the_change_list_moves_its_cursor() {
    let (_td, mut app) = crowded_repo();
    source(&mut app);
    change_list(&mut app, 100, 24);
    let (x0, _) = app.git.list_cols.unwrap();
    let before = app.git.selected;

    for _ in 0..40 {
        app.on_scroll(true, x0 + 2);
    }
    assert_eq!(app.git.selected, before + 40, "one notch, one row");
    let name = current_name(&app);
    let (_, cursor) = change_list(&mut app, 100, 24);
    assert!(cursor.contains(&name), "and the list follows: {cursor:?}");
}

#[test]
fn the_wheel_over_the_diff_scrolls_the_diff() {
    let (_td, mut app) = long_repo();
    source(&mut app);
    cursor_on(&mut app, "long.txt");
    change_list(&mut app, 100, 30);
    let (_, x1) = app.git.list_cols.unwrap();
    let selected = app.git.selected;

    app.on_scroll(true, x1 + 5);
    app.on_scroll(true, x1 + 5);
    assert_eq!(app.git.diff_scroll, 2);
    assert_eq!(app.git.selected, selected, "the list stays where it was");
}

#[test]
fn the_wheel_in_the_git_window_leaves_the_browser_alone() {
    let (_td, mut app) = repo();
    joined(&mut app);
    let (x0, _) = app.last_tree_cols.expect("the browser was drawn");
    let selected = app.selected;
    source(&mut app);
    joined(&mut app);

    // Where the browser was is where the change list is now.
    app.on_scroll(true, x0 + 1);
    assert_eq!(app.selected, selected, "the browser is not on screen");
}

#[test]
fn a_commit_message_longer_than_its_box_scrolls() {
    let (_td, mut app) = long_repo();
    source(&mut app);
    cursor_on(&mut app, "long.txt");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);
    type_str(&mut app, "summary");
    for n in 0..40 {
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, &format!("body-{n:02}"));
    }
    let out = screen(&mut app, 100, 24).join("\n");
    assert!(out.contains("body-39"), "the line being typed:\n{out}");
    assert!(
        !out.contains("body-00"),
        "the top has scrolled away:\n{out}"
    );

    // And across: the end of a line wider than the box is where typing is.
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, &format!("{}END", "x".repeat(150)));
    let out = screen(&mut app, 100, 24).join("\n");
    assert!(out.contains("xEND"), "{out}");
}

/// Run git in `dir` for a test, failing loudly.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_refresh_leaves_a_message_being_written_with_the_keyboard() {
    // A push finishing refreshes the window. It used to put the keyboard
    // back in the list, so the next Enter typed into the message staged a
    // file instead.
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);
    type_str(&mut app, "half a summary");
    app.refresh_git();
    assert_eq!(app.git.focus, GitFocus::Message);
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "more");
    let text = app.git.message.as_ref().unwrap().to_text();
    assert!(text.contains("half a summary\nmore"), "{text:?}");
}

#[test]
fn a_refresh_leaves_the_cursor_on_the_buttons() {
    let (_td, mut app) = repo();
    source(&mut app);
    for _ in 0..10 {
        app.on_key(k(KeyCode::Up));
    }
    assert_eq!(app.git.rows[app.git.selected], GitRow::Buttons);
    app.refresh_git();
    assert_eq!(app.git.rows[app.git.selected], GitRow::Buttons);
}

#[test]
fn right_goes_back_into_a_message_that_was_put_down() {
    let (_td, mut app) = repo();
    source(&mut app);
    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    press_commit(&mut app);
    type_str(&mut app, "first");
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.git.focus, GitFocus::List);
    let out = joined(&mut app);
    assert!(out.contains("commit button sends it"), "{out}");

    cursor_on(&mut app, "README.md");
    app.on_key(k(KeyCode::Right));
    assert_eq!(app.git.focus, GitFocus::Message, "{}", app.status);
    type_str(&mut app, " and more");
    let text = app.git.message.as_ref().unwrap().to_text();
    assert_eq!(text.trim_end(), "first and more");
}

#[test]
fn sync_pulls_and_then_pushes() {
    let (td, mut app) = repo();
    let remote = tempfile::tempdir().unwrap();
    git_in(
        remote.path(),
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let url = remote.path().to_string_lossy().into_owned();
    git_in(td.path(), &["remote", "add", "origin", &url]);
    git_in(td.path(), &["push", "--quiet", "-u", "origin", "main"]);
    // Everything committed: a pull with a rebase refuses a tree with changes
    // in it, and then, rightly, nothing is pushed.
    git_in(td.path(), &["add", "--all"]);
    git_in(td.path(), &["commit", "--quiet", "-m", "made here"]);

    source(&mut app);
    for _ in 0..10 {
        app.on_key(k(KeyCode::Up));
    }
    let sync = GIT_BUTTONS
        .iter()
        .position(|b| b.label() == "sync")
        .unwrap();
    for _ in 0..sync {
        app.on_key(k(KeyCode::Right));
    }
    app.on_key(k(KeyCode::Enter));
    for _ in 0..500 {
        if app.poll_git_job() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.git.job.is_none(), "the job finished: {}", app.status);
    let log = git_in(remote.path(), &["log", "--oneline", "main"]);
    assert!(
        log.contains("made here"),
        "the commit reached the remote: {log}, {}",
        app.status
    );
}

#[test]
fn a_gap_cut_off_by_the_top_of_the_pane_still_says_how_long_it_is() {
    let (td, _) = repo();
    let at = td.path();
    let start: String = (0..10).map(|n| format!("line {n}\n")).collect();
    fs::write(at.join("grows.txt"), &start).unwrap();
    git_in(at, &["add", "--all"]);
    git_in(at, &["commit", "--quiet", "-m", "grows"]);
    let added: String = (0..40).map(|n| format!("new {n}\n")).collect();
    fs::write(
        at.join("grows.txt"),
        start.replacen("line 5\n", &format!("line 5\n{added}"), 1),
    )
    .unwrap();
    let mut app = App::new(target(at, None), Config::default(), None).unwrap();
    source(&mut app);
    cursor_on(&mut app, "grows.txt");
    joined(&mut app);

    // Into the middle of the forty added lines.
    app.git.diff_scroll = 20;
    let out = joined(&mut app);
    assert!(
        out.contains("40 lines added"),
        "the left column says what the gap is, even though it began above:\n{out}"
    );
}
