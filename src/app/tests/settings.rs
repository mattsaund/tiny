//! The settings area and the keybinds window.
//!
//! Changing a value, seeing it take effect without a restart, writing it out,
//! and resetting it. The keybinds half also covers what a rebinding must not
//! break: a chord is not a movement, and an action cannot be left unreachable.

use super::*;

#[test]
fn the_settings_area_lists_every_setting_with_its_value() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    assert!(matches!(app.mode, Mode::Settings(_)));
    let out = screen(&mut app, 100, 34).join("\n");
    assert!(out.contains("Settings"), "{out}");
    assert!(out.contains("tab_width"), "{out}");
    assert!(out.contains("tree_side"), "{out}");
    assert!(out.contains("left"), "values are shown:\n{out}");
}
#[test]
fn a_chord_is_not_a_movement_in_a_list() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    app.on_key(ctrl('k'));
    let Mode::Settings(s) = &app.mode else {
        panic!("settings closed")
    };
    assert_eq!(s.selected, 0, "^K is a chord, not the letter k");
}

#[test]
fn the_settings_area_opens_on_its_two_buttons() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    let out = screen(&mut app, 96, 40).join("\n");
    assert!(out.contains("[ Keybinds ]"), "{out}");
    assert!(out.contains("[ Reset settings ]"), "{out}");
    assert!(
        out.contains("tab_width"),
        "the settings are still there:\n{out}"
    );
}

#[test]
fn the_keybinds_button_opens_the_keybinds_window() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    app.on_key(k(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Keybinds(_)));
    let out = screen(&mut app, 96, 40).join("\n");
    assert!(out.contains("Keybinds"), "{out}");
    assert!(out.contains("tree.down"), "every action is listed:\n{out}");
    assert!(
        out.contains("ctrl+s"),
        "with the keys that reach it:\n{out}"
    );
}

#[test]
fn esc_from_the_keybinds_window_goes_back_to_the_settings() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::Esc));
    assert!(
        matches!(app.mode, Mode::Settings(_)),
        "back where it was opened from, not out to the tree"
    );
}

#[test]
fn a_key_can_be_rebound_and_the_tree_answers_to_it() {
    let (_td, mut app) = fixture();
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    assert!(matches!(&app.mode, Mode::Keybinds(kb) if kb.capturing));
    app.on_key(ch('z'));

    assert_eq!(
        app.config.keys.get("tree.down").map(String::as_str),
        Some("z")
    );
    app.on_key(k(KeyCode::Esc));
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.selected, 0);
    app.on_key(ch('z'));
    assert_eq!(app.selected, 1, "z moves down now");
    app.on_key(ch('k'));
    assert_eq!(app.selected, 1, "and k does not");
}

#[test]
fn rebinding_takes_the_key_off_whatever_had_it() {
    let (_td, mut app) = fixture();
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('n')); // n was tree.new

    assert!(app.status.contains("taken from tree.new"), "{}", app.status);
    assert_eq!(
        app.keymap.spec(Action::TreeNew),
        "",
        "one key does one thing, and the window says so"
    );
}

#[test]
fn delete_puts_one_binding_back() {
    let (_td, mut app) = fixture();
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('z'));
    assert!(app.config.keys.contains_key("tree.down"));

    app.on_key(k(KeyCode::Delete));
    assert!(
        !app.config.keys.contains_key("tree.down"),
        "the override is gone, not set back to the same value"
    );
    assert_eq!(app.keymap.spec(Action::TreeDown), "down k");
}

#[test]
fn binding_a_key_back_to_its_default_drops_the_override() {
    let (_td, mut app) = fixture();
    keybinds_on(&mut app, Action::TreeRename);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('r'));
    assert!(
        app.config.keys.is_empty(),
        "the config only holds what actually changed"
    );
}

#[test]
fn resetting_the_keybinds_asks_first_and_then_restores_them() {
    let (_td, mut app) = fixture();
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('z'));

    // Back up to the reset button.
    app.on_key(ch('I'));
    app.on_key(k(KeyCode::Enter));
    assert!(
        joined(&mut app).contains("back to what tiny ships with"),
        "asks"
    );
    app.on_key(ch('n'));
    assert!(app.config.keys.contains_key("tree.down"), "n backed out");

    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('y'));
    assert!(app.config.keys.is_empty(), "y put them all back");
    assert!(
        matches!(app.mode, Mode::Keybinds(_)),
        "and left the window open"
    );
}

#[test]
fn resetting_the_settings_asks_first_and_then_restores_them() {
    let (_td, mut app) = fixture();
    command(&mut app, "set tab_width 7");
    assert_eq!(app.config.tab_width, 7);

    app.on_key(ch(','));
    app.on_key(k(KeyCode::Down)); // the reset button
    app.on_key(k(KeyCode::Enter));
    assert!(
        joined(&mut app).contains("Reset 1 setting"),
        "{}",
        app.status
    );
    app.on_key(ch('n'));
    assert_eq!(app.config.tab_width, 7, "n backed out");

    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('y'));
    assert_eq!(app.config.tab_width, 4, "back to the shipped value");
    assert!(matches!(app.mode, Mode::Settings(_)), "and stayed open");
}

#[test]
fn the_two_resets_do_not_touch_each_other() {
    let (_td, mut app) = fixture();
    command(&mut app, "set tab_width 7");
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('z'));

    // Reset the keys: the setting survives.
    app.on_key(ch('I'));
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('y'));
    assert!(app.config.keys.is_empty());
    assert_eq!(app.config.tab_width, 7, "a setting is not a keybinding");

    // And the other way round.
    keybinds_on(&mut app, Action::TreeDown);
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('z'));
    app.on_key(k(KeyCode::Esc));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Enter));
    app.on_key(ch('y'));
    assert_eq!(app.config.tab_width, 4);
    assert!(
        app.config.keys.contains_key("tree.down"),
        "a keybinding is not a setting"
    );
}

#[test]
fn resetting_when_nothing_has_changed_says_so() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    app.on_key(k(KeyCode::Down));
    app.on_key(k(KeyCode::Enter));
    assert!(app.status.contains("already"), "{}", app.status);
    assert!(matches!(app.mode, Mode::Settings(_)), "nothing to confirm");
}

#[test]
fn a_rebinding_from_the_config_file_is_what_the_keys_do() {
    let td = tempfile::tempdir().unwrap();
    build(td.path());
    let mut cfg = Config::default();
    cfg.keys.insert("tree.down".into(), "z".into());
    let mut app = App::new(target(td.path(), None), cfg, None).unwrap();

    app.on_key(ch('z'));
    assert_eq!(app.selected, 1, "the file said z, so z it is");
}

#[test]
fn a_setting_can_be_changed_from_the_settings_area() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    // Past the two buttons, then to tab_width, second in the index.
    for _ in 0..3 {
        app.on_key(k(KeyCode::Down));
    }
    app.on_key(k(KeyCode::Enter));

    // The field is prefilled with the current value.
    app.on_key(k(KeyCode::Backspace));
    type_str(&mut app, "7");
    app.on_key(k(KeyCode::Enter));
    assert_eq!(app.config.tab_width, 7, "status was: {}", app.status);
}

#[test]
fn escape_while_editing_a_setting_leaves_it_alone() {
    let (_td, mut app) = fixture();
    app.on_key(ch(','));
    // Onto tab_width, the same row the test above changes.
    for _ in 0..3 {
        app.on_key(k(KeyCode::Down));
    }
    app.on_key(k(KeyCode::Enter));
    app.on_key(k(KeyCode::Backspace));
    type_str(&mut app, "9");
    app.on_key(k(KeyCode::Esc));
    assert_eq!(app.config.tab_width, 4);
    assert!(matches!(app.mode, Mode::Settings(_)), "still in settings");
}

#[test]
fn the_settings_area_can_be_opened_by_command_too() {
    let (_td, mut app) = fixture();
    command(&mut app, "config");
    assert!(matches!(app.mode, Mode::Settings(_)));
}
