//! `*cd`: another folder as the project, without leaving tiny.
//!
//! tiny opens on one folder, and that folder is the project: what the
//! browser shows, what search and the map look through, and what the git
//! window is about. `*cd ~/notes` makes another folder the project, the way
//! `cd` moves a shell: a name is a folder in the project, `..` is the folder
//! above it, `~` is home and `*cd` alone goes there, a path from the top of
//! the disk goes there, and `*cd -` goes back to the project before. Tab
//! finishes the folder's name. `*project ~/notes` and `*open project
//! ~/notes` say the same.
//!
//! Open files stay open, unsaved changes and all, the way they do when the
//! browser moves away from them: changing projects drops nothing anyone
//! typed. What belonged to the folder left behind is let go of and found
//! again for the new one: the browser's rows and colors, the names a command
//! looks up, the git window and the map. The browser comes back if it was
//! hidden, with the keyboard in it, since a folder just opened is somewhere
//! to look around in first. A push or a clone that is still running has to
//! finish first, since what it does when it is done is about the project it
//! started in.

use std::path::{Path, PathBuf};

use crate::config::home_dir;
use crate::files::project::{canonical, expand};
use crate::files::tree::Tree;

use super::App;
use super::mode::{Focus, Window};
use super::source::GitPane;
use super::watch::Watch;

/// `path` the way a person says it: the home folder written as `~`.
pub(super) fn shown(path: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
        };
    }
    path.display().to_string()
}

impl App {
    /// The folder `*cd` is asked for, found: `typed` from the project, from
    /// home after `~`, or from the top of the disk; home for nothing, and
    /// the project before for `-`.
    pub(super) fn plan_project(&self, typed: &str) -> Result<PathBuf, String> {
        if let Some(job) = &self.git.job {
            return Err(format!(
                "{} is still running here; wait for it to finish",
                job.what
            ));
        }
        let typed = typed.trim();
        let path = match typed {
            "" => home_dir().ok_or("there is no home folder to go to")?,
            "-" => self
                .previous_root
                .clone()
                .ok_or("there was no project before this one")?,
            _ => {
                let path = expand(typed);
                if path.is_absolute() {
                    path
                } else {
                    self.root().join(path)
                }
            }
        };
        let dir = canonical(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("there is no folder {typed}"),
            _ => format!("cannot open {typed}: {e}"),
        })?;
        if !dir.is_dir() {
            return Err(format!("{typed} is a file; cd opens a folder"));
        }
        // Compared as the disk resolves it: the same folder can be named by
        // more than one path, through a link (a Mac's /var is /private/var).
        let here = canonical(self.root()).unwrap_or_else(|_| self.root().to_path_buf());
        if dir == here {
            return Err(format!("{} is the project already", shown(&dir)));
        }
        Ok(dir)
    }

    /// Make `dir` the project, see the module docs. Returns the status line.
    pub(super) fn open_project(&mut self, dir: &Path) -> anyhow::Result<String> {
        let before = self.root().to_path_buf();
        // Everything that starts something somewhere (the terminal, a second
        // window, git) starts it in the project's folder by name, so the
        // folder tiny itself is running in can stay where it was.
        let show_hidden = self.tree.show_hidden();
        self.tree = Tree::new(dir.to_path_buf(), show_hidden);
        self.rows = self.tree.flatten();
        self.selected = 0;
        self.tree_scroll = 0;
        self.watch = Watch::default();
        // The rows, and with them what the disk is watched for and what is
        // previewed.
        self.rebuild_rows();
        self.forget_names();
        self.last_jump = None;
        self.git = GitPane::default();
        self.map = None;
        self.recolor_elsewhere();
        self.recommitted();
        // The new folder, in the main window, with the browser on screen and
        // the keyboard in it.
        self.window = Window::Main;
        self.tree_hidden = false;
        self.focus = Focus::Tree;
        self.entered = true;
        self.picked_pane = None;
        self.focus_before_hide = (Focus::Tree, true);
        self.sync_preview();
        self.previous_root = Some(before);
        Ok(format!("now in {}", shown(dir)))
    }
}
