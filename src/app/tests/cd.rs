//! `*cd`, another folder as the project, and the folded start the desktop's
//! launchers ask for with `--hide-browser`.

use super::*;
use crate::files::project::canonical;

/// The fixture, opened the way the command line opens a folder: by the path
/// the disk resolves it to, which `*cd` lands on too. A Mac's temporary
/// folders are under /var, which is a link to /private/var, and a file
/// reached by both would be two files to tiny.
fn project() -> (tempfile::TempDir, App) {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let root = canonical(td.path()).unwrap();
    let app = App::new(target(&root, None), Config::default(), None).unwrap();
    (td, app)
}

/// What the line under the bar says for a command typed but not yet run.
fn previewed(app: &mut App, line: &str) -> String {
    command_bar(app);
    type_str(app, line);
    let Mode::Bar(b) = &app.mode else {
        panic!("the bar closed");
    };
    let said = match &b.plan {
        Some(Ok(s)) => s.clone(),
        Some(Err(e)) => format!("ERR {e}"),
        None => String::new(),
    };
    app.on_key(k(KeyCode::Esc));
    said
}

/// The names of the rows the browser shows, under its root.
fn names(app: &App) -> Vec<String> {
    app.rows.iter().skip(1).map(|r| r.name.clone()).collect()
}

#[test]
fn cd_opens_a_folder_in_the_project_and_cd_dash_comes_back() {
    let (td, mut app) = project();
    let src = canonical(&td.path().join("src")).unwrap();
    assert!(previewed(&mut app, "cd src").starts_with("opens "));
    assert!(previewed(&mut app, "cd src").ends_with(" as the project"));
    command(&mut app, "cd src");
    assert_eq!(app.root(), src, "{}", app.status);
    assert_eq!(names(&app), ["main.py"]);
    assert!(app.status.starts_with("now in "), "{}", app.status);
    assert_eq!((app.focus, app.entered), (Focus::Tree, true));
    assert_eq!(app.selected, 0);
    // And back the way it came, by `..` and by `-`.
    command(&mut app, "cd ..");
    assert!(same_file(app.root(), td.path()), "{}", app.status);
    command(&mut app, "cd -");
    assert_eq!(app.root(), src, "back to the one before");
    command(&mut app, "project ..");
    assert!(
        same_file(app.root(), td.path()),
        "the other way of saying it"
    );
    command(&mut app, "open project src");
    assert_eq!(app.root(), src, "and the third");
}

#[test]
fn cd_will_not_open_a_file_a_folder_that_is_not_there_or_the_project_itself() {
    let (td, mut app) = project();
    let before = app.root().to_path_buf();
    assert!(previewed(&mut app, "cd README.md").contains("is a file"));
    assert!(previewed(&mut app, "cd nowhere").contains("there is no folder nowhere"));
    assert!(previewed(&mut app, "cd .").contains("is the project already"));
    assert!(previewed(&mut app, "cd -").contains("no project before"));
    command(&mut app, "cd nowhere");
    assert_eq!(app.root(), before, "nothing moved");
    assert!(td.path().join("notes").is_dir());
}

#[test]
fn a_full_path_goes_anywhere_on_the_disk() {
    let (_td, mut app) = project();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::write(elsewhere.path().join("todo.md"), "- milk\n").unwrap();
    let path = elsewhere.path().to_string_lossy().into_owned();
    command(&mut app, &format!("cd \"{path}\""));
    assert!(same_file(app.root(), elsewhere.path()), "{}", app.status);
    assert_eq!(names(&app), ["todo.md"]);
}

#[test]
fn files_being_edited_stay_open_with_their_changes() {
    let (td, mut app) = project();
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "# mine\n");
    command(&mut app, "cd notes");
    assert!(same_file(app.root(), &td.path().join("notes")));
    let dirty: Vec<String> = app
        .dirty_buffers()
        .iter()
        .map(|p| display_name(p))
        .collect();
    assert_eq!(dirty, ["main.py"], "nothing typed was dropped");
    // And it is still there to go back to.
    command(&mut app, "cd ..");
    select(&mut app, "main.py");
    assert!(app.active_buffer().unwrap().lines()[0].starts_with("# mine"));
}

#[test]
fn cd_brings_the_browser_back_wherever_it_was_typed() {
    let (_td, mut app) = project();
    app.on_key(ctrl('t'));
    assert!(app.tree_hidden);
    app.on_key(ctrl('g'));
    command(&mut app, "cd src");
    assert!(!app.tree_hidden, "the browser is back");
    assert_eq!(app.window, Window::Main, "and in the main window");
    assert!(joined(&mut app).contains("main.py"));
}

#[test]
fn tab_finishes_a_folder_for_cd_and_offers_no_files() {
    let (_td, mut app) = project();
    assert_eq!(
        completed(&mut app, "cd sr"),
        format!("cd src{}", std::path::MAIN_SEPARATOR)
    );
    assert_eq!(
        completed(&mut app, "cd READ"),
        "cd READ",
        "README.md is a file"
    );
    assert_eq!(
        completed(&mut app, "cd .."),
        format!("cd ..{}", std::path::MAIN_SEPARATOR)
    );
    assert_eq!(
        completed(&mut app, "open project no"),
        format!("open project notes{}", std::path::MAIN_SEPARATOR)
    );
}

#[test]
fn the_desktop_start_hides_the_browser_and_says_how_to_get_it_back() {
    let (_td, mut app) = project();
    app.hide_browser_at_start();
    assert!(app.tree_hidden);
    assert_eq!(app.status, "Ctrl+T for the browser, ? for help");
    let screen = joined(&mut app);
    assert!(
        screen.contains("Ctrl+T for the browser   *new for a file   *cd for another folder"),
        "{screen}"
    );
    assert!(!screen.contains("BROWSER"), "{screen}");
    app.on_key(ctrl('t'));
    assert!(!app.tree_hidden, "Ctrl+T brings it back");
    assert_eq!((app.focus, app.entered), (Focus::Tree, true));
}

#[test]
fn a_file_from_the_desktop_opens_across_the_window_as_it_always_did() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let file = td.path().join("README.md");
    let mut app = App::new(target(td.path(), Some(file)), Config::default(), None).unwrap();
    let status = app.status.clone();
    app.hide_browser_at_start();
    assert!(app.tree_hidden);
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(app.status, status, "nothing else to say");
}

#[test]
fn a_warning_is_not_replaced_by_the_desktop_start() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let mut app = App::new(
        target(td.path(), None),
        Config::default(),
        Some("tiny.conf: line 3 is not a setting".into()),
    )
    .unwrap();
    app.hide_browser_at_start();
    assert!(app.tree_hidden);
    assert_eq!(app.status, "tiny.conf: line 3 is not a setting");
}

/// The same folder by another path, through a link, the way a Mac's
/// temporary folders are: still the project, not somewhere to go.
#[cfg(unix)]
#[test]
fn the_project_is_the_project_whatever_path_it_was_opened_by() {
    let td = tempfile::tempdir().unwrap();
    build(&td.path().join("real"));
    std::os::unix::fs::symlink(td.path().join("real"), td.path().join("link")).unwrap();
    let mut app = App::new(
        target(&td.path().join("link"), None),
        Config::default(),
        None,
    )
    .unwrap();
    assert!(previewed(&mut app, "cd .").contains("is the project already"));
    assert!(previewed(&mut app, "cd ../real").contains("is the project already"));
}
