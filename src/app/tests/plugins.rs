//! Plugins, end to end: a `.lua` file in a folder, and what it does to tiny.
//!
//! The Lua side (loading, errors, the deadline) is tested where it lives, in
//! `crate::plugins`. These are the seams with the app: a plugin's command run
//! from the bar, its edits landing in the buffer as edits, its hooks firing on
//! a real save, and the folder being looked at again on `*reload`.

use super::*;

/// The fixture project, with a plugins folder holding `files`.
fn with_plugins(files: &[(&str, &str)]) -> (tempfile::TempDir, App) {
    let (td, mut app) = fixture();
    let dir = td.path().join(".plugins");
    fs::create_dir_all(&dir).unwrap();
    for (name, source) in files {
        fs::write(dir.join(name), source).unwrap();
    }
    app.plugins_dir = Some(dir);
    app.load_plugins();
    (td, app)
}

const WORDS: &str = r#"
tiny.command("words", function()
  local n = 0
  for _ in tiny.text():gmatch("%S+") do n = n + 1 end
  tiny.status(n .. " words")
end)
"#;

#[test]
fn a_plugins_command_runs_from_the_bar() {
    let (_td, mut app) = with_plugins(&[("words.lua", WORDS)]);
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    command(&mut app, "words");
    assert_eq!(app.status, "4 words", "`#`, Fixture, hello, widget");
}

#[test]
fn a_plugins_edit_is_an_edit_unsaved_and_undoable() {
    let (td, mut app) = with_plugins(&[(
        "upper.lua",
        r#"tiny.command("upper", function() tiny.set_text(tiny.text():upper()) end)"#,
    )]);
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    command(&mut app, "upper");

    let ed = app.active_buffer().unwrap();
    assert!(ed.to_text().starts_with("# FIXTURE"), "{}", ed.to_text());
    assert!(ed.dirty, "changed, not written");
    let on_disk = fs::read_to_string(td.path().join("README.md")).unwrap();
    assert!(on_disk.starts_with("# Fixture"), "the file is untouched");

    app.on_key(ctrl('z'));
    assert!(
        app.active_buffer()
            .unwrap()
            .to_text()
            .starts_with("# Fixture"),
        "one undo takes it back"
    );
}

#[test]
fn a_plugin_hears_a_save_and_a_file_being_opened() {
    let (_td, mut app) = with_plugins(&[(
        "log.lua",
        r#"
        tiny.on("open", function(path) tiny.status("opened " .. path:match("[^/\\]+$")) end)
        tiny.on("save", function(path) tiny.status("saved " .. path:match("[^/\\]+$")) end)
        "#,
    )]);
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.status, "opened main.py");
    type_str(&mut app, "x");
    app.on_key(ctrl('s'));
    assert_eq!(app.status, "saved main.py");
}

#[test]
fn a_plugin_cannot_replace_one_of_tinys_commands() {
    let (td, mut app) = with_plugins(&[(
        "sneaky.lua",
        r#"tiny.command("new", function() tiny.status("mine") end)"#,
    )]);
    command(&mut app, "new made.txt");
    assert!(td.path().join("made.txt").is_file(), "tiny's own *new ran");
    assert_ne!(app.status, "mine");
}

#[test]
fn a_broken_plugin_is_named_on_the_status_line() {
    let (_td, app) = with_plugins(&[("broken.lua", "this is not lua")]);
    assert!(app.status.starts_with("plugin broken: "), "{}", app.status);
}

#[test]
fn a_plugin_dropped_in_is_picked_up_by_reload() {
    let (td, mut app) = with_plugins(&[]);
    assert!(app.plugins.loaded.is_empty());
    fs::write(td.path().join(".plugins/words.lua"), WORDS).unwrap();
    command(&mut app, "reload");
    assert_eq!(app.plugins.loaded, ["words"]);
    assert!(app.status.contains("1 plugin"), "{}", app.status);
}

#[test]
fn plugin_commands_complete_and_are_in_the_help() {
    let (_td, mut app) = with_plugins(&[("words.lua", WORDS)]);
    assert_eq!(completed(&mut app, "wor"), "words");

    app.on_key(ch('?'));
    let out = screen(&mut app, 70, 90).join("\n");
    assert!(out.contains("PLUGINS"), "{out}");
    assert!(out.contains("*words"), "{out}");
}

#[test]
fn a_test_never_loads_the_real_plugins_folder() {
    // `main` is the only thing that points the app at the real folder; an app
    // built any other way has none, so `*reload` loads nothing.
    let (_td, mut app) = fixture();
    assert_eq!(app.plugins_dir, None);
    command(&mut app, "reload");
    assert!(app.plugins.loaded.is_empty());
}

// ---- settings from a plugin ------------------------------------------------

#[test]
fn a_plugin_can_set_the_glyphs_when_it_loads() {
    // The way a plugin that sets up tiny's look would do it: at the top of
    // its file, not inside a command anyone has to run.
    let (_td, mut app) = with_plugins(&[("plain.lua", r#"tiny.set("glyphs", "ascii")"#)]);
    assert_eq!(app.config.glyphs, GlyphSet::Ascii);
    let out = joined(&mut app);
    assert!(
        out.contains("+ BROWSER -"),
        "and the screen follows:\n{out}"
    );
}

#[test]
fn a_plugin_setting_takes_numbers_and_booleans_as_lua_writes_them() {
    let (_td, app) = with_plugins(&[(
        "prefs.lua",
        r#"
tiny.set("tab_width", 2)
tiny.set("line_numbers", false)
"#,
    )]);
    assert_eq!(app.config.tab_width, 2);
    assert!(!app.config.line_numbers);
}

#[test]
fn a_misspelled_setting_is_an_error_at_the_line_that_wrote_it() {
    let (_td, app) = with_plugins(&[("typo.lua", r#"tiny.set("glyps", "ascii")"#)]);
    assert!(app.plugins.loaded.is_empty(), "the plugin did not load");
    assert!(
        app.plugins.errors[0].contains("glyps"),
        "and says which setting: {:?}",
        app.plugins.errors
    );
    assert_eq!(app.config.glyphs, GlyphSet::Auto, "nothing changed");
}

#[test]
fn a_plugin_that_fails_to_load_asks_for_nothing() {
    // It set something, and then broke: half of what it meant to do is not
    // worth doing, the same rule as for a command that fails part way.
    let (_td, app) = with_plugins(&[(
        "broken.lua",
        r#"
tiny.set("tab_width", 8)
error("and then this")
"#,
    )]);
    assert_eq!(app.config.tab_width, Config::default().tab_width);
}

#[test]
fn what_a_plugin_says_while_it_loads_is_shown() {
    // Asked for at load time, so before this it was cleared away unseen by
    // the first call into any plugin.
    let (_td, app) = with_plugins(&[("hello.lua", r#"tiny.status("hello from a plugin")"#)]);
    assert_eq!(app.status, "hello from a plugin");
}

#[test]
fn a_plugin_command_can_change_a_setting_too() {
    let (_td, mut app) = with_plugins(&[(
        "toggle.lua",
        r#"tiny.command("plain", function() tiny.set("glyphs", "ascii") end)"#,
    )]);
    assert_eq!(app.config.glyphs, GlyphSet::Auto);
    command(&mut app, "plain");
    assert_eq!(app.config.glyphs, GlyphSet::Ascii);
}

#[test]
fn a_hook_that_opens_a_file_does_not_hear_its_own_open() {
    // Every open opens another: without a guard this recursed until the
    // stack ran out, taking the terminal down with it.
    let (_td, mut app) = with_plugins(&[(
        "follow.lua",
        r#"tiny.on("open", function(path) tiny.open("src/main.py") end)"#,
    )]);
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(
        app.selected_path().and_then(|p| p.file_name()),
        Some(std::ffi::OsStr::new("main.py")),
        "the hook ran once, and its open happened: {}",
        app.status
    );
}

#[test]
fn a_save_hook_works_on_the_file_being_saved_not_the_one_on_screen() {
    let (td, mut app) = with_plugins(&[(
        "stamp.lua",
        r##"
        tiny.on("save", function(path)
          if path:match("%.py$") then tiny.set_text(tiny.text() .. "# saved\n") end
        end)
        "##,
    )]);
    // Edit the code, then look at the note and save the whole project from
    // the root row: the hook hears about main.py while README.md is on
    // screen.
    select(&mut app, "main.py");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(k(KeyCode::Esc));
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "y");
    app.on_key(k(KeyCode::Esc));
    app.on_key(k(KeyCode::Home));
    app.on_key(ctrl('s'));

    let code = &app.buffers[&td.path().join("src/main.py")];
    assert!(
        code.to_text().ends_with("# saved\n"),
        "the hook's edit is in main.py: {:?}",
        code.to_text()
    );
    let note = &app.buffers[&td.path().join("README.md")];
    assert!(
        !note.to_text().contains("# saved"),
        "and not in the note on screen: {:?}",
        note.to_text()
    );
}

#[test]
fn a_formatter_with_nothing_to_change_leaves_the_file_saved() {
    let (_td, mut app) = with_plugins(&[(
        "same.lua",
        r#"tiny.on("save", function(path) tiny.set_text(tiny.text()) end)"#,
    )]);
    select(&mut app, "README.md");
    app.on_key(k(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(ctrl('s'));
    assert!(
        !app.active_buffer().unwrap().dirty,
        "the hook handed the text back unchanged, which is not an edit"
    );
}
