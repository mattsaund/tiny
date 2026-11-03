//! The browser's colors from git: a file changed since the last commit in
//! yellow, a new one in green, one git ignores in gray, and one in the middle
//! of a merge conflict in red. A folder takes the color of what matters most
//! inside it, so a change is visible from the top however deep it is.
//!
//! # Asked in the background
//!
//! `git status` takes a few milliseconds in a small project and a good deal
//! more in a big one, so it runs on a thread and the colors arrive when it is
//! done; nothing waits for it. It is asked when there is reason to think the
//! answer moved: at the start, after a save or anything done to a file, when
//! the disk watcher sees a change, after anything done in the git window, and
//! when the repository's own index, `HEAD` or record of where `HEAD` has been
//! moves, which is another program committing, amending, staging, resetting
//! or switching branches. Nothing is asked on a timer, so an idle tiny still
//! uses no CPU.
//!
//! That last is also when the marks beside the line numbers are measured
//! again, see [`super::gutter`]: every one of those can make the last commit
//! a different one.
//!
//! Outside a repository there is nothing to color, and the first answer says
//! so; the browser looks exactly as it always did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::SystemTime;

use crate::git::{Mark, Repo};

use super::App;

/// What git says about the paths in the project, ready to look up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    /// Files git named one by one.
    files: HashMap<PathBuf, Mark>,
    /// Folders new or ignored as a whole, which everything inside takes.
    whole: HashMap<PathBuf, Mark>,
    /// The most that matters inside each folder that has a change in it.
    folders: HashMap<PathBuf, Mark>,
}

impl Marks {
    /// From what [`Repo::marks`] said, its paths being relative to the top
    /// of the repository, for the project at `root`, which is `prefix` down
    /// from the top (see [`Repo::prefix`]). What is outside the project is
    /// left out: the browser does not show it.
    pub fn build(root: &Path, prefix: &str, said: Vec<(String, Mark)>) -> Self {
        let mut marks = Marks::default();
        for (rel, mark) in said {
            let Some(inside) = rel.strip_prefix(prefix) else {
                continue;
            };
            let whole = inside.ends_with('/');
            let inside = inside.trim_end_matches('/');
            let path = if inside.is_empty() {
                root.to_path_buf()
            } else {
                root.join(inside)
            };
            if mark != Mark::Ignored {
                for folder in path.ancestors().skip(1) {
                    if !folder.starts_with(root) {
                        break;
                    }
                    let was = marks.folders.entry(folder.to_path_buf()).or_insert(mark);
                    *was = (*was).max(mark);
                }
            }
            if whole {
                marks.whole.insert(path, mark);
            } else {
                marks.files.insert(path, mark);
            }
        }
        marks
    }

    /// What to color `path` for, if anything.
    ///
    /// Asked of every row on screen on every frame, so what is not there to
    /// find is not looked for: nothing at all outside a repository, and the
    /// handful of whole folders compared with rather than every folder above
    /// the row hashed and looked up.
    pub fn of(&self, path: &Path, is_dir: bool) -> Option<Mark> {
        if self.files.is_empty() && self.whole.is_empty() {
            return None;
        }
        if let Some(mark) = self.files.get(path) {
            return Some(*mark);
        }
        let whole = if self.whole.len() <= 8 {
            self.whole
                .iter()
                .find(|(folder, _)| path.starts_with(folder))
                .map(|(_, mark)| mark)
        } else {
            path.ancestors().find_map(|p| self.whole.get(p))
        };
        if let Some(mark) = whole {
            return Some(*mark);
        }
        if is_dir {
            return self.folders.get(path).copied();
        }
        None
    }
}

/// When the files in a repository's own folder that a commit moves were
/// last changed, see [`stamps`].
pub(super) type Stamps = [Option<SystemTime>; 3];

/// What the thread asking git sends back: the colors, and the repository's
/// own folder with what those files looked like before git was asked; or
/// nothing at all outside a repository.
type Answer = Option<(Marks, Option<(PathBuf, Stamps)>)>;

/// The browser's colors, and the asking that keeps them current.
#[derive(Debug, Default)]
pub struct Coloring {
    pub marks: Marks,
    /// Whether the colors are kept at all. Off until `main` turns them on,
    /// so a test, which builds an `App` directly, starts no thread it did
    /// not ask for.
    on: bool,
    /// The thread asking, while it asks.
    asking: Option<Receiver<Answer>>,
    /// Asked again while it was asking: once it answers, ask once more.
    again: bool,
    /// The repository's own folder, and what the files in it that move
    /// with a commit looked like when git was last asked, see [`stamps`].
    git_dir: Option<PathBuf>,
    stamps: Stamps,
}

/// Whether `folder` could be in a repository: a `.git` in it or a folder
/// above it, which a worktree or a submodule has as a file, or git told
/// where the repository is.
pub(super) fn might_be_a_repository(folder: &Path) -> bool {
    std::env::var_os("GIT_DIR").is_some() || folder.ancestors().any(|d| d.join(".git").exists())
}

/// When `path` was last changed, if it can be told.
fn changed(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// When the files in the repository's folder `dir` that something done
/// with git moves were last changed: the index, which staging and most
/// commits rewrite; `HEAD`, which a switch does; and the record of where
/// `HEAD` has been, which every commit, amend and reset adds to, even the
/// ones that leave the other two alone.
fn stamps(dir: &Path) -> Stamps {
    [
        changed(&dir.join("index")),
        changed(&dir.join("HEAD")),
        changed(&dir.join("logs").join("HEAD")),
    ]
}

impl App {
    /// Start keeping the browser's colors, see the module docs.
    ///
    /// Not at all in a folder with no `.git` in it or above it, which is no
    /// repository: no thread, and no git, is cheaper than one asked to say
    /// so. A `.git` that turns up later, from `*init` or a clone, starts
    /// them then.
    ///
    /// The marks beside the line numbers start here too: they are git's
    /// colors as well, and asked for as each file is drawn.
    pub fn color_from_git(&mut self) {
        self.coloring.on = true;
        self.gutter.on = true;
        if might_be_a_repository(self.root()) {
            self.recolor();
        }
    }

    /// Ask git again, or once it has answered, if it is asking now.
    pub(super) fn recolor(&mut self) {
        if !self.coloring.on {
            return;
        }
        // No repository any more, if there ever was: nothing to color.
        if !might_be_a_repository(self.root()) {
            self.coloring.marks = Marks::default();
            self.coloring.git_dir = None;
            return;
        }
        if self.coloring.asking.is_some() {
            self.coloring.again = true;
            return;
        }
        let root = self.root().to_path_buf();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let answer: Answer = Repo::find(&root).ok().and_then(|repo| {
                // Looked at before git is asked, not once it has answered:
                // a commit made while it answers is then still a change to
                // be noticed, rather than taken for one the answer has seen.
                let git = repo.git_dir().ok().map(|dir| {
                    let then = stamps(&dir);
                    (dir, then)
                });
                let said = repo.marks().ok()?;
                let prefix = Repo::prefix(&root).ok()?;
                Some((Marks::build(&root, &prefix, said), git))
            });
            // Nobody to tell if tiny has quit in the meantime.
            let _ = tx.send(answer);
        });
        self.coloring.asking = Some(rx);
    }

    /// Take in an answer, and notice the repository moving under us.
    /// Returns whether the colors changed, for the event loop to draw them.
    pub fn poll_colors(&mut self) -> bool {
        let mut news = false;
        if let Some(rx) = &self.coloring.asking {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                done => {
                    self.coloring.asking = None;
                    let (marks, git) = done.ok().flatten().unwrap_or_default();
                    self.coloring.git_dir = git.map(|(dir, then)| {
                        self.coloring.stamps = then;
                        dir
                    });
                    news = marks != self.coloring.marks;
                    self.coloring.marks = marks;
                    if std::mem::take(&mut self.coloring.again) {
                        self.recolor();
                    }
                }
            }
        }
        // Another program committing, staging or switching branches moves
        // these, and nothing in the browser would show it otherwise. Nor in
        // the marks beside the line numbers, which are measured against the
        // last commit: they keep their own record of these, since an answer
        // about the colors moves this one.
        if let Some(dir) = &self.coloring.git_dir {
            let now = stamps(dir);
            if now != self.coloring.stamps {
                self.coloring.stamps = now;
                self.recolor();
            }
            if now != self.gutter.seen {
                self.gutter.seen = now;
                self.recommitted();
            }
        }
        news
    }

    /// Ask git, and wait for the answer: how a test sees the colors.
    #[cfg(test)]
    pub(super) fn wait_for_colors(&mut self) {
        self.coloring.on = true;
        self.recolor();
        for _ in 0..1000 {
            self.poll_colors();
            if self.coloring.asking.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("git never answered");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_shows_what_matters_most_inside_it() {
        let top = Path::new("/p");
        let marks = Marks::build(
            top,
            "",
            vec![
                ("src/app/new.rs".into(), Mark::Added),
                ("src/main.rs".into(), Mark::Modified),
                ("docs/guide.md".into(), Mark::Added),
                ("target/".into(), Mark::Ignored),
                ("fresh/".into(), Mark::Added),
            ],
        );
        let of = |p: &str, dir| marks.of(&top.join(p), dir);
        assert_eq!(of("src/main.rs", false), Some(Mark::Modified));
        assert_eq!(of("src/lib.rs", false), None, "an unchanged file");
        assert_eq!(of("src", true), Some(Mark::Modified), "the change wins");
        assert_eq!(of("src/app", true), Some(Mark::Added));
        assert_eq!(of("docs", true), Some(Mark::Added));
        assert_eq!(of("target", true), Some(Mark::Ignored));
        assert_eq!(
            of("target/debug/tiny", false),
            Some(Mark::Ignored),
            "inside it too"
        );
        assert_eq!(of("fresh/a/b.md", false), Some(Mark::Added));
        assert_eq!(
            of("", true),
            Some(Mark::Modified),
            "the top shows it as well"
        );
    }

    #[test]
    fn a_project_in_a_folder_of_a_repository_sees_only_its_own() {
        let root = Path::new("/elsewhere/notes");
        let marks = Marks::build(
            root,
            "notes/",
            vec![
                ("notes/today.md".into(), Mark::Modified),
                ("src/main.rs".into(), Mark::Modified),
                ("notes/drafts/".into(), Mark::Added),
            ],
        );
        assert_eq!(
            marks.of(&root.join("today.md"), false),
            Some(Mark::Modified)
        );
        assert_eq!(
            marks.of(&root.join("drafts/a.md"), false),
            Some(Mark::Added)
        );
        assert_eq!(marks.of(root, true), Some(Mark::Modified));
        assert_eq!(marks.files.len(), 1, "src/main.rs is not in the project");
    }
}
