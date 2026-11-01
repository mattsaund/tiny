//! Getting a project into git, and out to where others can reach it:
//! `*init`, `*publish` and `*clone`, and the buttons that start them.
//!
//! A project with no repository yet is offered all three in the git window.
//! `*init` makes it one. `*publish` does whatever is still missing in one
//! go: makes it a repository, commits everything as "first commit" when
//! nothing has been committed yet, and sends it somewhere. Where is the
//! remote it already has; failing that, an address typed after the word; or
//! failing that, a new repository on GitHub, made with GitHub's own `gh`
//! program when it is installed. Private unless `public` is said, and named
//! after the folder unless another name is.
//!
//! `*clone <address>` brings a repository the other way: into the project's
//! folder when that is empty, and otherwise into a new folder inside it,
//! named after the repository. `owner/name` is GitHub's, the way `gh` reads
//! it.
//!
//! Everything that talks to another machine runs on a thread like a push
//! does, see `git::run`, and the line under the bar says what Enter will do
//! before anything is done.

use std::path::{Path, PathBuf};

use crate::git::{Repo, run};

use super::App;
use super::parts::{display_name, safe_join};
use super::sentence::remote_address;

/// Where `*publish` sends the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Destination {
    /// A remote the repository already has.
    Remote(String),
    /// A new repository on GitHub, made with `gh`.
    GitHub { name: String, public: bool },
    /// The address of a repository somewhere else, made to receive it.
    Url(String),
}

/// What `*publish` will do, every step of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Publish {
    /// There is no repository yet: one is made first.
    init: bool,
    /// Nothing is committed yet, so the first commit is made: of everything
    /// when `true`, of what is staged when `false`.
    first_commit: Option<bool>,
    to: Destination,
}

/// What `*clone` will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cloning {
    /// The address, as git is given it.
    url: String,
    /// The folder it goes into, or `None` for the project's own.
    into: Option<PathBuf>,
}

/// What to do once a job is done, beyond saying what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Show the project as a repository that has gone somewhere.
    Published,
    /// Show what came, in the folder it came into.
    Cloned { into: Option<PathBuf> },
}

/// A repository's name, from its address: the last part, without `.git`.
fn name_in(url: &str) -> String {
    url.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or(url)
        .trim_end_matches(".git")
        .to_string()
}

/// A folder's name as a repository name: what GitHub accepts, which is
/// letters, digits, `.`, `-` and `_`.
fn repository_name(folder: &Path) -> String {
    let name: String = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "project".into()
    } else {
        name
    }
}

/// `git` and its arguments, as a step of a job.
fn git(args: &[&str]) -> Vec<String> {
    std::iter::once("git")
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

impl App {
    /// The repository the project is in, if it is in one.
    fn repository(&self) -> Option<Repo> {
        self.git
            .repo
            .clone()
            .or_else(|| Repo::find(self.root()).ok())
    }

    /// `*init`, before it is done.
    pub(super) fn plan_init(&self) -> Result<(), String> {
        match self.repository() {
            Some(repo) => Err(format!(
                "already a git repository, at {}",
                repo.root.display()
            )),
            None => Ok(()),
        }
    }

    /// Sort out what `*publish` and the words after it ask for.
    pub(super) fn plan_publish(&self, words: &[String]) -> Result<Publish, String> {
        let repo = self.repository();
        let (mut public, mut url, mut name) = (false, None, None);
        for word in words {
            match word.to_lowercase().as_str() {
                "public" => public = true,
                "private" => public = false,
                "to" | "on" | "at" | "github" | "as" | "named" | "called" | "it" | "this" => {}
                _ if remote_address(word) => url = Some(word.clone()),
                _ => name = Some(word.clone()),
            }
        }
        let remotes = repo.as_ref().map(Repo::remotes).unwrap_or_default();
        let has = remotes
            .iter()
            .find(|r| *r == "origin")
            .or(remotes.first())
            .cloned();
        let to = match (url, has) {
            (Some(_), Some(remote)) => {
                return Err(format!(
                    "this repository already sends to {remote}: publish alone pushes there"
                ));
            }
            (Some(url), None) => Destination::Url(url),
            (None, Some(remote)) => Destination::Remote(remote),
            (None, None) if run::on_path("gh") => Destination::GitHub {
                name: name.unwrap_or_else(|| repository_name(self.root())),
                public,
            },
            (None, None) => {
                return Err(
                    "publish to where? Paste the address of an empty repository, or install gh to make one on GitHub"
                        .into(),
                );
            }
        };
        let first_commit = match &repo {
            None => Some(true),
            Some(repo) if !repo.has_commits() => Some(
                repo.status()
                    .map(|status| status.staged.is_empty())
                    .unwrap_or(true),
            ),
            Some(_) => None,
        };
        Ok(Publish {
            init: repo.is_none(),
            first_commit,
            to,
        })
    }

    /// What publishing will do, for the line under the bar.
    pub(super) fn describe_publish(&self, p: &Publish) -> String {
        let mut steps = Vec::new();
        if p.init {
            steps.push("makes this a git repository".to_string());
        }
        match p.first_commit {
            Some(true) => steps.push("commits every file as \"first commit\"".into()),
            Some(false) => steps.push("commits what is staged as \"first commit\"".into()),
            None => {}
        }
        steps.push(match &p.to {
            Destination::Remote(remote) => format!("pushes this branch to {remote}"),
            Destination::GitHub { name, public } => format!(
                "makes a {} GitHub repository named {name} with gh and pushes to it{}",
                if *public { "public" } else { "private" },
                if *public {
                    ""
                } else {
                    " (say public for a public one)"
                }
            ),
            Destination::Url(url) => format!("pushes to {url}"),
        });
        match steps.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{}, then {last}", rest.join(", ")),
            None => String::new(),
        }
    }

    /// Publish, as planned: every step on a thread, one after another.
    pub(super) fn publish(&mut self, p: Publish) -> anyhow::Result<String> {
        let dir = self
            .repository()
            .map_or_else(|| self.root().to_path_buf(), |repo| repo.root);
        let mut steps = Vec::new();
        if p.init {
            steps.push(git(&["init", "--quiet"]));
            if run::git(&dir, &["config", "--get", "init.defaultBranch"]).is_err() {
                steps.push(git(&["symbolic-ref", "HEAD", "refs/heads/main"]));
            }
        }
        if let Some(everything) = p.first_commit {
            if everything {
                steps.push(git(&["add", "--all"]));
            }
            steps.push(git(&["commit", "--quiet", "-m", "first commit"]));
        }
        match &p.to {
            Destination::Remote(remote) => {
                steps.push(git(&["push", "--set-upstream", remote, "HEAD"]));
            }
            Destination::GitHub { name, public } => {
                let seen = if *public { "--public" } else { "--private" };
                steps.push(
                    [
                        "gh", "repo", "create", name, seen, "--source", ".", "--remote", "origin",
                        "--push",
                    ]
                    .map(str::to_string)
                    .to_vec(),
                );
            }
            Destination::Url(url) => {
                steps.push(git(&["remote", "add", "origin", url]));
                steps.push(git(&["push", "--set-upstream", "origin", "HEAD"]));
            }
        }
        self.start_job(&dir, "publishing", steps, Some(Then::Published))?;
        Ok(String::new())
    }

    /// Sort out what `*clone` asks for: what to fetch, and where to.
    pub(super) fn plan_clone(&self, from: &str, into: Option<&str>) -> Result<Cloning, String> {
        let root = self.root().to_path_buf();
        // GitHub's `owner/name`, unless it names something here.
        let shorthand = !remote_address(from)
            && from.matches('/').count() == 1
            && !from.starts_with(['.', '/', '~'])
            && !root.join(from).exists();
        let url = if shorthand {
            format!("https://github.com/{from}.git")
        } else {
            from.to_string()
        };
        let empty = std::fs::read_dir(&root).is_ok_and(|mut d| d.next().is_none());
        let into = match into {
            Some(folder) => Some(safe_join(&root, folder, &root).map_err(|e| format!("{e:#}"))?),
            None if empty => None,
            None => Some(root.join(name_in(&url))),
        };
        if let Some(dir) = into.as_ref().filter(|d| d.exists()) {
            return Err(format!("{} is already here", display_name(dir)));
        }
        Ok(Cloning { url, into })
    }

    /// What cloning will do, for the line under the bar.
    pub(super) fn describe_clone(&self, c: &Cloning) -> String {
        match &c.into {
            None => format!("clones {} into this folder", c.url),
            Some(dir) => format!("clones {} into {}/", c.url, self.rel(dir)),
        }
    }

    /// Clone, as planned, on a thread.
    pub(super) fn clone_repository(&mut self, c: Cloning) -> anyhow::Result<String> {
        let root = self.root().to_path_buf();
        let target = match &c.into {
            None => ".".to_string(),
            Some(dir) => dir.to_string_lossy().into_owned(),
        };
        let steps = vec![git(&["clone", &c.url, &target])];
        self.start_job(&root, "cloning", steps, Some(Then::Cloned { into: c.into }))?;
        Ok(String::new())
    }

    /// The end of a job that publishes or clones: the repository it made,
    /// and the files that came.
    pub(super) fn after_job(&mut self, then: Then) {
        // A repository may be here now that was not, or have a remote.
        self.git.repo = None;
        self.refresh_git();
        self.refresh();
        if let Then::Cloned { into: Some(dir) } = then {
            self.reveal(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_is_named_for_the_last_part_of_its_address() {
        assert_eq!(name_in("https://github.com/me/notes.git"), "notes");
        assert_eq!(name_in("git@github.com:me/notes.git"), "notes");
        assert_eq!(name_in("https://example.com/me/notes/"), "notes");
        assert_eq!(name_in("../other"), "other");
        assert_eq!(name_in("C:\\Users\\me\\shared.git"), "shared", "on Windows");
    }

    #[test]
    fn a_folder_name_becomes_one_github_takes() {
        assert_eq!(repository_name(Path::new("/x/my notes")), "my-notes");
        assert_eq!(repository_name(Path::new("/x/tiny")), "tiny");
        assert_eq!(repository_name(Path::new("/x/café")), "caf");
    }
}
