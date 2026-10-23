//! Plugins: Lua files that add commands to tiny, and act when a file is opened
//! or saved.
//!
//! # What a plugin is, and where it goes
//!
//! A plugin is a `.lua` file in the plugins folder — `~/.config/tiny/plugins`,
//! or `%APPDATA%\tiny\plugins` on Windows, which the settings area's `Plugins`
//! button opens — or a folder there with an `init.lua` in it, for a plugin
//! that wants more than one file. Every one is loaded when tiny starts, and
//! again on `*reload`.
//!
//! The same file works on every system. Lua is compiled into tiny, so a plugin
//! needs nothing installed and nothing built: drop it in, and it runs.
//!
//! # What a plugin can do
//!
//! Everything goes through one table, `tiny`:
//!
//! ```lua
//! tiny.command("words", function(args) end) -- `*words` runs it
//! tiny.on("save", function(path) end)       -- and "open"
//! tiny.status("hello")                      -- the status line
//! tiny.text()   tiny.set_text(s)            -- the file being edited
//! tiny.insert(s)                            -- at the cursor
//! tiny.cursor() tiny.go(line, col)          -- counted from 1
//! tiny.path()   tiny.root()                 -- the file, the project
//! tiny.open(path)                           -- a file, in tiny
//! tiny.set("glyphs", "ascii")               -- any setting, as *set
//! tiny.version
//! ```
//!
//! What a plugin asks for while it loads (at the top of its file, outside any
//! function) is carried out once every plugin has loaded. That is where a
//! plugin that sets up tiny's look calls `tiny.set`.
//!
//! A command a plugin adds cannot replace one tiny has: `*delete` always means
//! tiny's own delete, whatever is installed.
//!
//! # Why a plugin never touches `App`
//!
//! A plugin runs while tiny is in the middle of handling a key, with `App`
//! borrowed. So nothing a plugin calls reaches into it. Each call is handed a
//! [`Snapshot`] of what it can read, and gives back the [`Effect`]s it asked
//! for, in order; `App` carries them out once the plugin has returned. A plugin
//! never sees tiny half way through something, and never leaves it that way.
//!
//! # A plugin cannot hang tiny
//!
//! Lua runs on tiny's own thread, so a plugin stuck in a loop would freeze the
//! window. Every call has a deadline — [`PATIENCE`] — checked every few
//! thousand instructions, and a call that runs past it is stopped with an
//! error naming the plugin. An error is all a broken plugin can cause: it is
//! reported on the status line, and tiny carries on.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, HookTriggers, Lua, VmState};

/// How long one call into a plugin may run before it is stopped.
pub const PATIENCE: Duration = Duration::from_secs(2);

/// What a plugin can ask to be told about.
const EVENTS: &[&str] = &["open", "save"];

/// What a plugin can read during a call: the state of tiny as it was when the
/// plugin was called.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// The project folder.
    pub root: String,
    /// What the cursor is on, if anything.
    pub path: Option<String>,
    /// The file being edited, if one is open.
    pub text: Option<String>,
    /// Its cursor, as line and column counted from 1.
    pub cursor: Option<(usize, usize)>,
}

/// Something a plugin asked for, carried out once it has returned.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Status(String),
    /// Replace the whole file being edited.
    SetText(String),
    /// Put text in at the cursor.
    Insert(String),
    /// Move the cursor to a line and column, counted from 1.
    Go(usize, usize),
    /// Open a file — a path from the project folder, or an absolute one.
    Open(String),
    /// Change a setting, the way `*set` does: for this run, and written to
    /// `tiny.conf` only if the settings area is saved.
    Set(String, String),
}

/// What the plugins and tiny share while a plugin runs.
#[derive(Default)]
struct Shared {
    /// The plugin being loaded, so what it registers is filed under its name.
    loading: String,
    /// `*name` → the plugin that added it, and its function.
    commands: BTreeMap<String, (String, Function)>,
    /// Event, plugin, function — in the order they were registered.
    hooks: Vec<(String, String, Function)>,
    snapshot: Snapshot,
    effects: Vec<Effect>,
    /// When the running call has to stop.
    deadline: Option<Instant>,
}

/// Every plugin, loaded.
#[derive(Default)]
pub struct Plugins {
    /// `None` until there is a plugin to run: a tiny with no plugins starts no
    /// Lua at all.
    lua: Option<Lua>,
    shared: Rc<RefCell<Shared>>,
    /// The plugins that loaded, by name.
    pub loaded: Vec<String>,
    /// What the plugins asked for while they loaded, in order, waiting for
    /// `App` to carry it out. A plugin that failed to load asks for nothing.
    pub startup: Vec<Effect>,
    /// One line for each that did not, saying why.
    pub errors: Vec<String>,
}

impl Plugins {
    /// Load every plugin in `dir`. A folder that is not there is no plugins,
    /// which is the ordinary case and not an error.
    pub fn load(dir: &Path) -> Self {
        let mut plugins = Plugins::default();
        let found = Self::find(dir);
        if found.is_empty() {
            return plugins;
        }
        let lua = Lua::new();
        if let Err(e) = plugins.install(&lua, dir) {
            plugins.errors.push(format!("plugins: {}", first_line(&e)));
            return plugins;
        }
        for (name, file) in found {
            plugins.shared.borrow_mut().loading = name.clone();
            let ran = std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|source| {
                    with_deadline(&plugins.shared, || {
                        lua.load(source.as_str())
                            .set_name(format!("@{}", display(&file, dir)))
                            .exec()
                    })
                    .map_err(|e| first_line(&e))
                });
            // Taken either way, so one plugin's requests never leak into the
            // next one's, and kept only from a plugin that loaded.
            let asked = std::mem::take(&mut plugins.shared.borrow_mut().effects);
            match ran {
                Ok(()) => {
                    plugins.startup.extend(asked);
                    plugins.loaded.push(name);
                }
                Err(e) => plugins.errors.push(format!("{name}: {e}")),
            }
        }
        plugins.lua = Some(lua);
        plugins
    }

    /// The plugins in `dir`, by name, in the order they load: alphabetical,
    /// so it is the same on every machine.
    fn find(dir: &Path) -> Vec<(String, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_stem()?.to_string_lossy().into_owned();
                if path.is_dir() {
                    let init = path.join("init.lua");
                    init.is_file().then_some((name, init))
                } else if path.extension().is_some_and(|e| e == "lua") {
                    Some((name, path))
                } else {
                    None
                }
            })
            .collect();
        found.sort();
        found
    }

    /// Give Lua the `tiny` table, the plugins folder to `require` from, and
    /// the deadline.
    fn install(&self, lua: &Lua, dir: &Path) -> mlua::Result<()> {
        let tiny = lua.create_table()?;
        tiny.set("version", crate::update::running())?;

        let shared = self.shared.clone();
        tiny.set(
            "command",
            lua.create_function(move |_, (name, f): (String, Function)| {
                let mut s = shared.borrow_mut();
                let owner = s.loading.clone();
                s.commands.insert(name, (owner, f));
                Ok(())
            })?,
        )?;

        let shared = self.shared.clone();
        tiny.set(
            "on",
            lua.create_function(move |_, (event, f): (String, Function)| {
                if !EVENTS.contains(&event.as_str()) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "there is no event called `{event}`; there is {}",
                        EVENTS.join(" and ")
                    )));
                }
                let mut s = shared.borrow_mut();
                let owner = s.loading.clone();
                s.hooks.push((event, owner, f));
                Ok(())
            })?,
        )?;

        let shared = self.shared.clone();
        tiny.set(
            "root",
            lua.create_function(move |_, ()| Ok(shared.borrow().snapshot.root.clone()))?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "path",
            lua.create_function(move |_, ()| Ok(shared.borrow().snapshot.path.clone()))?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "text",
            lua.create_function(move |_, ()| Ok(shared.borrow().snapshot.text.clone()))?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "cursor",
            lua.create_function(move |_, ()| {
                let cursor = shared.borrow().snapshot.cursor;
                Ok((cursor.map(|c| c.0), cursor.map(|c| c.1)))
            })?,
        )?;

        let shared = self.shared.clone();
        tiny.set(
            "status",
            lua.create_function(move |_, message: String| {
                shared.borrow_mut().effects.push(Effect::Status(message));
                Ok(())
            })?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "set_text",
            lua.create_function(move |_, text: String| {
                let mut s = shared.borrow_mut();
                if s.snapshot.text.is_none() {
                    return Err(no_file());
                }
                // What `tiny.text()` says from here on, in this same call.
                s.snapshot.text = Some(text.clone());
                s.effects.push(Effect::SetText(text));
                Ok(())
            })?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "insert",
            lua.create_function(move |_, text: String| {
                let mut s = shared.borrow_mut();
                if s.snapshot.text.is_none() {
                    return Err(no_file());
                }
                s.effects.push(Effect::Insert(text));
                Ok(())
            })?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "go",
            lua.create_function(move |_, (line, col): (usize, Option<usize>)| {
                let mut s = shared.borrow_mut();
                if s.snapshot.text.is_none() {
                    return Err(no_file());
                }
                s.effects
                    .push(Effect::Go(line.max(1), col.unwrap_or(1).max(1)));
                Ok(())
            })?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "open",
            lua.create_function(move |_, path: String| {
                shared.borrow_mut().effects.push(Effect::Open(path));
                Ok(())
            })?,
        )?;
        let shared = self.shared.clone();
        tiny.set(
            "set",
            lua.create_function(move |_, (name, value): (String, mlua::Value)| {
                let value = match value {
                    mlua::Value::String(s) => s.to_str()?.to_string(),
                    mlua::Value::Integer(n) => n.to_string(),
                    mlua::Value::Number(n) => n.to_string(),
                    mlua::Value::Boolean(b) => b.to_string(),
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "a setting is text, a number or true or false, not a {}",
                            other.type_name()
                        )));
                    }
                };
                // Tried against a spare config first, so a misspelled name or
                // a value that will not parse is an error at the line of Lua
                // that wrote it, rather than a status message later.
                crate::config::Config::default()
                    .set(&name, &value)
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                shared.borrow_mut().effects.push(Effect::Set(name, value));
                Ok(())
            })?,
        )?;
        lua.globals().set("tiny", tiny)?;

        // `require("mine.helpers")` finds `plugins/mine/helpers.lua`, so a
        // plugin in a folder can be more than one file.
        let package: mlua::Table = lua.globals().get("package")?;
        let path: String = package.get("path")?;
        let ours = format!(
            "{};{}",
            dir.join("?.lua").display(),
            dir.join("?").join("init.lua").display()
        );
        package.set("path", format!("{ours};{path}"))?;

        // Global, so a coroutine a plugin starts is held to it too.
        let shared = self.shared.clone();
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(10_000),
            move |_, _| match shared.borrow().deadline {
                Some(deadline) if Instant::now() > deadline => {
                    Err(mlua::Error::RuntimeError(format!(
                        "took longer than {} seconds, and was stopped",
                        PATIENCE.as_secs()
                    )))
                }
                _ => Ok(VmState::Continue),
            },
        )
    }

    /// Every command a plugin added, by name, for completion and the help.
    pub fn commands(&self) -> Vec<String> {
        self.shared.borrow().commands.keys().cloned().collect()
    }

    /// The plugin that added `*name`, if one did.
    pub fn command_owner(&self, name: &str) -> Option<String> {
        self.shared
            .borrow()
            .commands
            .get(name)
            .map(|(owner, _)| owner.clone())
    }

    /// Run `*name`, if a plugin added it. `None` when none did; otherwise
    /// what the plugin asked for, or what went wrong.
    pub fn run(
        &self,
        name: &str,
        args: &[String],
        snapshot: Snapshot,
    ) -> Option<Result<Vec<Effect>, String>> {
        let lua = self.lua.as_ref()?;
        // Cloned out, so nothing is borrowed while the plugin runs and the
        // `tiny` functions it calls borrow in their turn.
        let (owner, f) = self.shared.borrow().commands.get(name).cloned()?;
        Some(self.call(&owner, snapshot, || {
            let args = lua.create_sequence_from(args.iter().cloned())?;
            f.call::<()>(args)
        }))
    }

    /// Tell every plugin listening for `event` about `path`. Returns what they
    /// asked for, and one line for each that failed.
    pub fn fire(&self, event: &str, path: &Path, snapshot: Snapshot) -> (Vec<Effect>, Vec<String>) {
        let listening: Vec<(String, Function)> = self
            .shared
            .borrow()
            .hooks
            .iter()
            .filter(|(e, ..)| e == event)
            .map(|(_, owner, f)| (owner.clone(), f.clone()))
            .collect();
        let path = path.to_string_lossy().into_owned();
        let (mut effects, mut errors) = (Vec::new(), Vec::new());
        for (owner, f) in listening {
            match self.call(&owner, snapshot.clone(), || f.call::<()>(path.clone())) {
                Ok(asked) => effects.extend(asked),
                Err(e) => errors.push(e),
            }
        }
        (effects, errors)
    }

    /// One call into a plugin: hand it `snapshot`, run it against the
    /// deadline, and take back what it asked for. A call that fails asks for
    /// nothing — half of what a broken plugin meant to do is not worth doing.
    fn call(
        &self,
        owner: &str,
        snapshot: Snapshot,
        run: impl FnOnce() -> mlua::Result<()>,
    ) -> Result<Vec<Effect>, String> {
        {
            let mut s = self.shared.borrow_mut();
            s.snapshot = snapshot;
            s.effects.clear();
        }
        let result = with_deadline(&self.shared, run);
        let effects = std::mem::take(&mut self.shared.borrow_mut().effects);
        match result {
            Ok(()) => Ok(effects),
            Err(e) => Err(format!("{owner}: {}", first_line(&e))),
        }
    }
}

/// Run `f` with the deadline set, and cleared again afterwards however it
/// went.
fn with_deadline<T>(shared: &Rc<RefCell<Shared>>, f: impl FnOnce() -> T) -> T {
    shared.borrow_mut().deadline = Some(Instant::now() + PATIENCE);
    let result = f();
    shared.borrow_mut().deadline = None;
    result
}

/// The error for an editing call made with no file open.
fn no_file() -> mlua::Error {
    mlua::Error::RuntimeError("no file is open to edit".into())
}

/// A file's path from the plugins folder, for error messages: `words.lua`,
/// or `mine/init.lua`.
fn display(file: &Path, dir: &Path) -> String {
    file.strip_prefix(dir)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The part of a Lua error worth a status line: what went wrong and where,
/// without the traceback.
fn first_line(e: &mlua::Error) -> String {
    let text = match e {
        mlua::Error::CallbackError { cause, .. } => cause.to_string(),
        other => other.to_string(),
    };
    let line = text.lines().next().unwrap_or("").trim();
    line.strip_prefix("runtime error: ")
        .unwrap_or(line)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        for (name, source) in files {
            let path = td.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        td
    }

    fn editing(text: &str) -> Snapshot {
        Snapshot {
            root: "/project".into(),
            path: Some("/project/notes.md".into()),
            text: Some(text.into()),
            cursor: Some((1, 1)),
        }
    }

    #[test]
    fn a_folder_with_no_plugins_starts_no_lua() {
        let td = folder(&[("notes.txt", "not a plugin")]);
        let plugins = Plugins::load(td.path());
        assert!(plugins.lua.is_none());
        assert!(plugins.loaded.is_empty() && plugins.errors.is_empty());
        let missing = Plugins::load(&td.path().join("nope"));
        assert!(missing.loaded.is_empty() && missing.errors.is_empty());
    }

    #[test]
    fn a_plugin_adds_a_command_that_reads_the_file_and_says_so() {
        let td = folder(&[(
            "words.lua",
            r#"
            tiny.command("words", function(args)
              local n = 0
              for _ in tiny.text():gmatch("%S+") do n = n + 1 end
              tiny.status(n .. " words, " .. #args .. " args")
            end)
            "#,
        )]);
        let plugins = Plugins::load(td.path());
        assert_eq!(plugins.loaded, ["words"]);
        assert_eq!(plugins.commands(), ["words"]);
        let effects = plugins
            .run("words", &["a".into()], editing("one two three"))
            .expect("the plugin answers")
            .expect("and does not fail");
        assert_eq!(effects, [Effect::Status("3 words, 1 args".into())]);
    }

    #[test]
    fn edits_come_back_in_the_order_they_were_asked_for() {
        let td = folder(&[(
            "shout.lua",
            r#"
            tiny.command("shout", function()
              tiny.set_text(tiny.text():upper())
              tiny.insert("!")
              tiny.go(2, 3)
              tiny.open("other.md")
            end)
            "#,
        )]);
        let plugins = Plugins::load(td.path());
        let effects = plugins.run("shout", &[], editing("hey")).unwrap().unwrap();
        assert_eq!(
            effects,
            [
                Effect::SetText("HEY".into()),
                Effect::Insert("!".into()),
                Effect::Go(2, 3),
                Effect::Open("other.md".into()),
            ]
        );
    }

    #[test]
    fn a_plugin_hears_about_saves() {
        let td = folder(&[(
            "log.lua",
            r#"tiny.on("save", function(path) tiny.status("saved " .. path) end)"#,
        )]);
        let plugins = Plugins::load(td.path());
        let (effects, errors) = plugins.fire("save", Path::new("/p/a.md"), editing(""));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(effects, [Effect::Status("saved /p/a.md".into())]);
        let (effects, _) = plugins.fire("open", Path::new("/p/a.md"), editing(""));
        assert!(effects.is_empty(), "nobody asked to hear about opening");
    }

    #[test]
    fn a_broken_plugin_is_reported_and_the_rest_still_load() {
        let td = folder(&[
            ("a_broken.lua", "this is not lua"),
            ("b_fine.lua", r#"tiny.command("fine", function() end)"#),
        ]);
        let plugins = Plugins::load(td.path());
        assert_eq!(plugins.loaded, ["b_fine"]);
        assert_eq!(plugins.errors.len(), 1);
        assert!(
            plugins.errors[0].starts_with("a_broken: "),
            "{:?}",
            plugins.errors
        );
        assert!(plugins.errors[0].contains("a_broken.lua"), "it says where");
    }

    #[test]
    fn an_error_while_running_names_the_plugin_and_asks_for_nothing() {
        let td = folder(&[(
            "oops.lua",
            r#"tiny.command("oops", function() tiny.status("half") error("broke") end)"#,
        )]);
        let plugins = Plugins::load(td.path());
        let err = plugins.run("oops", &[], editing("")).unwrap().unwrap_err();
        assert!(err.starts_with("oops: "), "{err}");
        assert!(err.contains("broke"), "{err}");
    }

    #[test]
    fn a_plugin_that_never_finishes_is_stopped() {
        let td = folder(&[(
            "spin.lua",
            r#"tiny.command("spin", function() while true do end end)"#,
        )]);
        let plugins = Plugins::load(td.path());
        let started = Instant::now();
        let err = plugins.run("spin", &[], editing("")).unwrap().unwrap_err();
        assert!(err.contains("stopped"), "{err}");
        assert!(started.elapsed() < PATIENCE * 3, "and promptly");
    }

    #[test]
    fn editing_with_no_file_open_says_so() {
        let td = folder(&[(
            "edit.lua",
            r#"tiny.command("edit", function() tiny.insert("x") end)"#,
        )]);
        let plugins = Plugins::load(td.path());
        let browsing = Snapshot {
            root: "/project".into(),
            ..Snapshot::default()
        };
        let err = plugins.run("edit", &[], browsing).unwrap().unwrap_err();
        assert!(err.contains("no file is open"), "{err}");
    }

    #[test]
    fn a_plugin_in_a_folder_can_require_its_own_files() {
        let td = folder(&[
            (
                "mine/init.lua",
                r#"local h = require("mine.helpers")
                   tiny.command("mine", function() tiny.status(h.greet()) end)"#,
            ),
            (
                "mine/helpers.lua",
                r#"return { greet = function() return "hi" end }"#,
            ),
        ]);
        let plugins = Plugins::load(td.path());
        assert_eq!(plugins.loaded, ["mine"], "{:?}", plugins.errors);
        let effects = plugins.run("mine", &[], editing("")).unwrap().unwrap();
        assert_eq!(effects, [Effect::Status("hi".into())]);
    }

    #[test]
    fn an_unknown_event_is_refused_by_name() {
        let td = folder(&[("bad.lua", r#"tiny.on("sav", function() end)"#)]);
        let plugins = Plugins::load(td.path());
        assert!(plugins.errors[0].contains("`sav`"), "{:?}", plugins.errors);
    }

    #[test]
    fn a_command_nobody_added_is_none() {
        let td = folder(&[("x.lua", "")]);
        let plugins = Plugins::load(td.path());
        assert!(plugins.run("nothing", &[], editing("")).is_none());
        assert_eq!(plugins.command_owner("nothing"), None);
    }
}
