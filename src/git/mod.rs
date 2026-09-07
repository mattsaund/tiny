//! Git, as the source-control window sees it.
//!
//! Everything here goes through the `git` binary rather than through a library.
//! That is a deliberate choice and worth defending: linking libgit2 or `gix`
//! would add several megabytes and a minute of build time to a program that
//! measures both, and it would still be reimplementing decisions — what counts
//! as a rename, how a diff is bounded — that the user's own git already makes
//! the way the user expects. Shelling out means tiny agrees with the `git` on
//! the same machine by construction.
//!
//! # The shape of it
//!
//! | file | what it does |
//! |------|--------------|
//! | [`run`] | starting `git`, in the foreground and in the background |
//! | [`parse`] | turning its output into the types below, with no process in sight |
//! | this file | the types, and the calls that put the two together |
//!
//! The split matters for testing: [`parse`] is pure text in, values out, so the
//! awkward cases — a rename with a quoted path, a conflicted file, a diff with
//! no trailing newline — are tested as strings rather than by trying to talk a
//! real repository into that state.
//!
//! # Nothing here writes without being asked
//!
//! Reading git's opinion is safe and happens whenever the window is open.
//! Everything that changes the repository — staging, committing, pushing — is a
//! separate call made only from a keypress, and each one is named in the window
//! before it runs. There is no autosave, no auto-stage, and no command that
//! rewrites history without the word `rebase` on screen.

pub mod parse;
pub mod run;

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

pub use self::run::Job;

/// What happened to one file, in git's own letters.
///
/// The letters are git's, not ours: they are what `git status` prints, what
/// every tutorial names, and what the window shows, so there is one vocabulary
/// rather than a translation layer nobody asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// `A` — new to the index.
    Added,
    /// `M` — changed.
    Modified,
    /// `D` — gone.
    Deleted,
    /// `R` — moved, with git having matched the two halves up.
    Renamed,
    /// `C` — copied from another file.
    Copied,
    /// `U` — untracked: git has never been told about it.
    ///
    /// Git itself writes this as `??` and reserves `U` for unmerged, but `U`
    /// for untracked is what people say out loud, and an unmerged file is
    /// [`Code::Conflict`] here where it cannot be mistaken for anything.
    Untracked,
    /// A merge left both sides in the file.
    Conflict,
}

impl Code {
    /// The single letter the window draws in the left column.
    pub fn letter(self) -> char {
        match self {
            Code::Added => 'A',
            Code::Modified => 'M',
            Code::Deleted => 'D',
            Code::Renamed => 'R',
            Code::Copied => 'C',
            Code::Untracked => 'U',
            Code::Conflict => '!',
        }
    }

    /// The word, for the status line — a letter is a reminder, not an
    /// explanation.
    pub fn word(self) -> &'static str {
        match self {
            Code::Added => "added",
            Code::Modified => "modified",
            Code::Deleted => "deleted",
            Code::Renamed => "renamed",
            Code::Copied => "copied",
            Code::Untracked => "untracked",
            Code::Conflict => "conflicted",
        }
    }
}

/// One file, and what has happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub code: Code,
    /// Relative to the repository root, the way git prints it.
    pub path: String,
    /// Where a rename or a copy came from.
    pub from: Option<String>,
}

impl Change {
    /// `old.md → new.md` for a rename, and just the path for everything else.
    pub fn label(&self) -> String {
        match &self.from {
            Some(from) => format!("{from} → {}", self.path),
            None => self.path.clone(),
        }
    }
}

/// Which branch, and how it stands against the one it tracks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

impl Branch {
    /// `main → origin/main  ahead 2, behind 1`, as short as it can honestly be.
    pub fn summary(&self) -> String {
        let mut out = self.name.clone();
        if let Some(up) = &self.upstream {
            out.push_str(" → ");
            out.push_str(up);
        }
        match (self.ahead, self.behind) {
            (0, 0) => {}
            (a, 0) => out.push_str(&format!("  ahead {a}")),
            (0, b) => out.push_str(&format!("  behind {b}")),
            (a, b) => out.push_str(&format!("  ahead {a}, behind {b}")),
        }
        out
    }
}

/// Everything the window draws, as of the last time git was asked.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub branch: Branch,
    /// Changes git would commit.
    pub staged: Vec<Change>,
    /// Changes it would not.
    pub unstaged: Vec<Change>,
}

impl Status {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty()
    }
}

/// One line of a file, and whether the change touched it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub text: String,
    pub kind: LineKind,
}

/// What a line is doing in a side-by-side diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// In both sides, unchanged.
    Same,
    /// On this side only.
    Changed,
    /// Nothing here — the other side has a line this one does not, and the two
    /// stay level so the eye can read across.
    Filler,
}

/// A file's before and after, line for line.
///
/// The two are the same length: where one side has a line the other does not,
/// the other gets a [`LineKind::Filler`]. That is what lets the window draw
/// them as two columns you can read across rather than two lists that drift
/// apart after the first change.
#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub before: Vec<DiffLine>,
    pub after: Vec<DiffLine>,
}

impl Diff {
    pub fn len(&self) -> usize {
        self.before.len().max(self.after.len())
    }

    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }
}

/// The repository a project sits in.
#[derive(Debug, Clone)]
pub struct Repo {
    /// The top level, which is not always the project root — tiny can be
    /// opened on a subdirectory of a repository.
    pub root: PathBuf,
}

impl Repo {
    /// Find the repository `dir` is in, or say why there is not one.
    pub fn find(dir: &Path) -> Result<Self> {
        let top = run::git(dir, &["rev-parse", "--show-toplevel"])?;
        let top = top.trim();
        if top.is_empty() {
            return Err(anyhow!("not a git repository"));
        }
        Ok(Self {
            root: PathBuf::from(top),
        })
    }

    /// What has changed, and where the branch stands.
    pub fn status(&self) -> Result<Status> {
        let out = run::git(&self.root, &["status", "--porcelain=v1", "--branch", "-z"])?;
        Ok(parse::status(&out))
    }

    /// The commit graph, as git draws it.
    ///
    /// `--all` so the picture includes the branches you are not on, which is
    /// most of what makes it a map rather than a list.
    pub fn graph(&self, rows: usize) -> Result<Vec<String>> {
        let n = rows.to_string();
        let out = run::git(
            &self.root,
            &[
                "log",
                "--graph",
                "--all",
                "--decorate",
                "--color=never",
                "--date=short",
                "--pretty=format:%h %d %s",
                "-n",
                &n,
            ],
        )?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// One file's before and after.
    ///
    /// `staged` picks which of the two diffs git can produce: the index against
    /// `HEAD`, or the working tree against the index. They are different
    /// questions and the window asks whichever matches the row the cursor is
    /// on.
    ///
    /// The huge context number is the trick that makes this a whole-file view:
    /// git emits one hunk covering everything, so the unchanged lines are there
    /// to read instead of being elided into `@@` markers.
    pub fn diff(&self, change: &Change, staged: bool) -> Result<Diff> {
        if change.code == Code::Untracked {
            // Git has nothing to say about a file it does not know. The whole
            // file is new, which is exactly what the right-hand side should
            // show.
            let text = std::fs::read_to_string(self.root.join(&change.path)).unwrap_or_default();
            return Ok(parse::whole_file_added(&text));
        }
        let mut args = vec!["diff", "--no-color", "-U1000000", "--find-renames"];
        if staged {
            args.push("--cached");
        }
        args.push("--");
        args.push(&change.path);
        let out = run::git(&self.root, &args)?;
        Ok(parse::diff(&out))
    }

    /// Stage a path, or everything.
    pub fn stage(&self, path: Option<&str>) -> Result<String> {
        match path {
            Some(p) => run::git(&self.root, &["add", "--", p])?,
            None => run::git(&self.root, &["add", "--all"])?,
        };
        Ok(match path {
            Some(p) => format!("staged {p}"),
            None => "staged everything".into(),
        })
    }

    /// Take a path back out of the index, or all of them.
    pub fn unstage(&self, path: Option<&str>) -> Result<String> {
        match path {
            Some(p) => run::git(&self.root, &["restore", "--staged", "--", p])?,
            None => run::git(&self.root, &["restore", "--staged", "."])?,
        };
        Ok(match path {
            Some(p) => format!("unstaged {p}"),
            None => "unstaged everything".into(),
        })
    }

    /// Commit what is staged.
    pub fn commit(&self, message: &str) -> Result<String> {
        if message.trim().is_empty() {
            return Err(anyhow!("a commit needs a message"));
        }
        let out = run::git(&self.root, &["commit", "-m", message])?;
        // Git's own first line is the useful one: `[main abc1234] the message`.
        Ok(out.lines().next().unwrap_or("committed").to_string())
    }
}
