//! Plugins, from tiny's side: what they are shown, and doing what they ask.
//!
//! The plugins themselves (loading them, calling into Lua, keeping a broken
//! one from hanging the window) are [`crate::plugins`]. This is the seam: a
//! [`Snapshot`] goes out before each call, and the [`Effect`]s that come back
//! are carried out here, once the plugin has returned. See that module's docs
//! for why a plugin never reaches into `App` directly.

use std::path::{Path, PathBuf};

use anyhow::anyhow;

use crate::files::media;
use crate::plugins::{Effect, Plugins, Snapshot};

use super::App;
use super::mode::Focus;
use super::preview::Preview;

impl App {
    /// Load every plugin in the plugins folder, replacing any loaded before.
    /// Called once at startup, and again by `*reload`.
    ///
    /// The first plugin that failed to load is named on the status line;
    /// tiny carries on without it either way.
    pub fn load_plugins(&mut self) {
        let Some(dir) = self.plugins_dir.clone() else {
            return;
        };
        self.plugins = Plugins::load(&dir, self.root());
        let asked = std::mem::take(&mut self.plugins.startup);
        self.apply_plugin_effects(asked, None);
        if let Some(first) = self.plugins.errors.first() {
            self.status = format!("plugin {first}");
        }
    }

    /// At startup, once the plugins are loaded: a file named on the command
    /// line was opened before there was anyone to tell.
    pub fn tell_plugins_what_is_open(&mut self) {
        if let (Focus::Editor, Preview::Buffer { path, .. }) = (self.focus, &self.preview) {
            let path = path.clone();
            self.plugin_event("open", &path);
        }
    }

    /// What a plugin can read: the project, and a file.
    ///
    /// For a command, the file is what the cursor is on and the one open in
    /// the right pane, whether or not it has the keyboard, so a command run
    /// from the browser can work on the note under the cursor. For an event,
    /// it is the file the event is about, which is not always that one:
    /// saving a folder saves every file in it.
    fn plugin_snapshot(&self, about: Option<&Path>) -> Snapshot {
        let (path, editing) = match about {
            Some(path) => (Some(path), self.buffers.get(path)),
            None => (self.selected_path(), self.active_buffer()),
        };
        Snapshot {
            root: self.root().to_string_lossy().into_owned(),
            path: path.map(|p| p.to_string_lossy().into_owned()),
            text: editing.map(|ed| ed.to_text()),
            cursor: editing.map(|ed| (ed.cursor_line + 1, ed.cursor_col + 1)),
        }
    }

    /// `*name`, for a command a plugin added. `None` when no plugin did, so
    /// the caller can say it is not a command at all.
    pub(super) fn run_plugin_command(
        &mut self,
        name: &str,
        args: &[String],
    ) -> Option<anyhow::Result<String>> {
        let snapshot = self.plugin_snapshot(None);
        let outcome = self.plugins.run(name, args, snapshot)?;
        Some(match outcome {
            Ok(effects) => {
                let said = effects.iter().any(|e| matches!(e, Effect::Status(_)));
                self.apply_plugin_effects(effects, None);
                // A plugin that said nothing still did something.
                Ok(if said {
                    String::new()
                } else {
                    format!("ran *{name}")
                })
            }
            Err(e) => Err(anyhow!("plugin {e}")),
        })
    }

    /// Tell the plugins that `path` was opened or saved, and do what they ask
    /// to that file.
    ///
    /// Not while doing what an earlier event asked: a hook on `open` that
    /// opens a file would otherwise hear about its own open, and open it
    /// again, until the stack ran out.
    pub(super) fn plugin_event(&mut self, event: &str, path: &Path) {
        if self.in_plugin_event || !self.plugins.listens(event) {
            return;
        }
        let snapshot = self.plugin_snapshot(Some(path));
        let (effects, errors) = self.plugins.fire(event, path, snapshot);
        self.in_plugin_event = true;
        self.apply_plugin_effects(effects, Some(path));
        self.in_plugin_event = false;
        if let Some(first) = errors.first() {
            self.status = format!("plugin {first}");
        }
    }

    /// Carry out what a plugin asked for, in the order it asked, to the file
    /// it was shown: `about`, or the one in the right pane.
    ///
    /// The edits go through the editor's own methods, so each is one step of
    /// undo and marks the file unsaved like any other change: a plugin can
    /// change a file, but never write one behind your back.
    fn apply_plugin_effects(&mut self, effects: Vec<Effect>, about: Option<&Path>) {
        for effect in effects {
            let buffer = match about {
                Some(path) => self.buffers.get_mut(path),
                None => self.active_buffer_mut(),
            };
            match effect {
                Effect::Status(message) => self.status = message,
                Effect::SetText(text) => {
                    // A formatter with nothing to change hands the text back
                    // as it was, and that is not an edit to save.
                    if let Some(ed) = buffer.filter(|ed| ed.to_text() != text) {
                        ed.replace_all(&text);
                    }
                }
                Effect::Insert(text) => {
                    if let Some(ed) = buffer {
                        ed.insert_text(&text);
                    }
                }
                Effect::Go(line, col) => {
                    if let Some(ed) = buffer {
                        ed.goto(line - 1, col - 1);
                    }
                }
                Effect::Open(path) => {
                    let path = PathBuf::from(path);
                    let path = if path.is_absolute() {
                        path
                    } else {
                        self.root().join(path)
                    };
                    self.open_path(&path);
                }
                // Already checked against a spare config when the plugin
                // asked, so this does not fail in practice; if it ever did,
                // the reason goes on the status line rather than nowhere.
                Effect::Set(name, value) => match self.config.set(&name, &value) {
                    Ok(()) => self.apply_config(),
                    Err(e) => self.status = format!("plugin: {e}"),
                },
            }
        }
    }

    /// The settings area's `Plugins` button: the folder, in the desktop's own
    /// file manager, made first if this is the first time, so there is
    /// somewhere to drop a plugin.
    pub(super) fn open_plugins_folder(&mut self) {
        let Some(dir) = self.plugins_dir.clone() else {
            self.status = "there is no config folder to keep plugins in".into();
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status = format!("cannot make {}: {e}", dir.display());
            return;
        }
        self.status = match media::open_with_desktop(&dir) {
            Ok(()) => format!(
                "{}: drop a .lua file in, then *reload ({} loaded)",
                dir.display(),
                self.plugins.loaded.len()
            ),
            Err(e) => format!("{e:#}; the folder is {}", dir.display()),
        };
    }
}
