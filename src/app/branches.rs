//! Branches, in the git window: the list of them under the changes, and
//! switching, making, renaming, deleting and merging them.
//!
//! The list is the third part of the git window, picked with the arrows like
//! the other two and gone into with Enter. Its first row makes a branch;
//! below it are the branches here, the one checked out marked, then the ones
//! on a remote that no branch here follows yet. Enter on a branch checks it
//! out, and on one from a remote makes a branch here that follows it. `n`
//! makes a new one, `d` deletes, `R` renames and `m` merges the one under
//! the cursor into the one checked out; each that cannot be undone asks
//! first. Every one of them is a command as well: `*branch`, `*switch`,
//! `*delete branch`, `*rename branch` and `*merge`.
//!
//! # Unsaved files
//!
//! Checking out a branch, or merging one, changes the files on disk. A file
//! with edits not yet saved would then be the old branch's text over the new
//! branch's file, and saving it would quietly undo the switch for that file.
//! So those are refused until everything is saved, and the status line says
//! which files are in the way.

use crate::config::keys::Action;
use crate::git::{BranchRef, Repo};

use super::App;
use super::mode::{Confirm, ConfirmKind, Mode};
use super::parts::display_name;

/// What a branch command will do, worked out before it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BranchPlan {
    /// Make `name` from `from`, or from where the checked out branch is,
    /// called `base` for the description, and check it out.
    Create {
        name: String,
        from: Option<String>,
        base: String,
    },
    /// Check out `name`; with `track`, made here first to follow that branch
    /// on a remote.
    Switch {
        name: String,
        track: Option<String>,
    },
    /// Delete `name`, here or on its remote.
    Delete {
        name: String,
        remote: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    /// Merge `name` into `into`, the branch checked out.
    Merge {
        name: String,
        into: String,
    },
}

/// `origin/login` as its remote and its name there.
fn on_remote(name: &str) -> (&str, &str) {
    name.split_once('/').unwrap_or(("origin", name))
}

impl App {
    // ---- the list ---------------------------------------------------------

    /// Move the cursor in the branch list.
    pub(super) fn move_branch_cursor(&mut self, delta: isize) {
        let last = self.git.shown_branches().len() as isize;
        self.git.branch_row = (self.git.branch_row as isize + delta).clamp(0, last) as usize;
        self.status = match self.git.selected_branch() {
            None => format!("{} makes a branch", self.key_name(Action::Open)),
            Some(b) if b.current => format!("{}, checked out", b.name),
            Some(b) if b.remote => format!("{}, on a remote", b.name),
            Some(b) => b.name.clone(),
        };
    }

    /// Enter in the branch list: make a branch, or check out the one the
    /// cursor is on.
    pub(super) fn activate_branch(&mut self) {
        let Some(branch) = self.git.selected_branch().cloned() else {
            return self.begin_command("branch ");
        };
        if branch.current {
            self.status = format!("already on {}", branch.name);
            return;
        }
        let said = crate::app::sentence::Sentence::Switch(branch.name.clone());
        self.status = match self.plan(&said, "") {
            Ok(plan) => match self.carry_out(plan, "") {
                Ok(done) => done,
                Err(e) => format!("{e:#}"),
            },
            Err(why) => why,
        };
    }

    /// The branch keys, `n`, `d`, `R` and `m`. Returns whether `action` was
    /// one of them. A new branch can be made from anywhere in the window;
    /// the other three are about the branch under the cursor, and only mean
    /// something in the branch list.
    pub(super) fn on_branch_action(&mut self, action: Action, in_list: bool) -> bool {
        match action {
            Action::BranchNew => self.begin_command("branch "),
            Action::BranchDelete | Action::BranchRename | Action::BranchMerge if !in_list => {}
            Action::BranchDelete => match self.git.selected_branch().cloned() {
                Some(b) => self.run_command(&format!("delete branch {}", b.name)),
                None => self.status = "the cursor is on no branch".into(),
            },
            Action::BranchRename => match self.git.selected_branch().cloned() {
                Some(b) if b.remote => {
                    self.status = "a branch on a remote is renamed there".into();
                }
                Some(b) => self.begin_command(&format!("rename branch {} to ", b.name)),
                None => self.status = "the cursor is on no branch".into(),
            },
            Action::BranchMerge => match self.git.selected_branch().cloned() {
                Some(b) => self.run_command(&format!("merge {}", b.name)),
                None => self.status = "the cursor is on no branch".into(),
            },
            _ => return false,
        }
        true
    }

    // ---- planning ---------------------------------------------------------

    /// The repository and its branches, or why there are none.
    fn branch_context(&self) -> Result<(Repo, Vec<BranchRef>), String> {
        let repo = self
            .repository()
            .ok_or_else(|| "not a git repository: *init makes one".to_string())?;
        let branches = repo.branches().map_err(|e| format!("{e:#}"))?;
        Ok((repo, branches))
    }

    pub(super) fn plan_create_branch(
        &self,
        name: &str,
        from: Option<&str>,
    ) -> Result<BranchPlan, String> {
        let (repo, branches) = self.branch_context()?;
        if !repo.branch_name_ok(name) {
            return Err(format!("{name} is not a name git takes for a branch"));
        }
        if branches.iter().any(|b| !b.remote && b.name == name) {
            return Err(format!(
                "{name} is a branch already: switch {name} goes to it"
            ));
        }
        if let Some(from) = from
            && !branches.iter().any(|b| b.name == from)
        {
            return Err(format!("there is no branch {from}"));
        }
        let base = match from {
            Some(from) => from.to_string(),
            None => branches
                .iter()
                .find(|b| b.current)
                .map_or_else(|| "here".to_string(), |b| b.name.clone()),
        };
        Ok(BranchPlan::Create {
            name: name.to_string(),
            from: from.map(str::to_string),
            base,
        })
    }

    pub(super) fn plan_switch(&self, name: &str) -> Result<BranchPlan, String> {
        let (_, branches) = self.branch_context()?;
        let local = |n: &str| branches.iter().find(|b| !b.remote && b.name == n);
        if let Some(b) = local(name) {
            if b.current {
                return Err(format!("already on {name}"));
            }
            return Ok(BranchPlan::Switch {
                name: name.to_string(),
                track: None,
            });
        }
        // One on a remote, by its whole name or by its name there.
        let remote = branches
            .iter()
            .filter(|b| b.remote)
            .find(|b| b.name == name || on_remote(&b.name).1 == name);
        let Some(remote) = remote else {
            return Err(format!("there is no branch {name}"));
        };
        let here = on_remote(&remote.name).1;
        if local(here).is_some() {
            return self.plan_switch(here);
        }
        Ok(BranchPlan::Switch {
            name: here.to_string(),
            track: Some(remote.name.clone()),
        })
    }

    pub(super) fn plan_delete_branch(&self, name: &str) -> Result<BranchPlan, String> {
        let (_, branches) = self.branch_context()?;
        let Some(b) = branches.iter().find(|b| b.name == name) else {
            return Err(format!("there is no branch {name}"));
        };
        if b.current {
            return Err(format!("{name} is checked out: switch to another first"));
        }
        Ok(BranchPlan::Delete {
            name: name.to_string(),
            remote: b.remote,
        })
    }

    pub(super) fn plan_rename_branch(
        &self,
        from: Option<&str>,
        to: &str,
    ) -> Result<BranchPlan, String> {
        let (repo, branches) = self.branch_context()?;
        let from = match from {
            Some(from) => from.to_string(),
            None => branches
                .iter()
                .find(|b| b.current)
                .map(|b| b.name.clone())
                .ok_or("no branch is checked out to rename")?,
        };
        if !branches.iter().any(|b| !b.remote && b.name == from) {
            return Err(format!("there is no branch {from} here"));
        }
        if !repo.branch_name_ok(to) {
            return Err(format!("{to} is not a name git takes for a branch"));
        }
        if branches.iter().any(|b| !b.remote && b.name == to) {
            return Err(format!("{to} is a branch already"));
        }
        Ok(BranchPlan::Rename {
            from,
            to: to.to_string(),
        })
    }

    pub(super) fn plan_merge(&self, name: &str) -> Result<BranchPlan, String> {
        let (_, branches) = self.branch_context()?;
        let Some(into) = branches.iter().find(|b| b.current) else {
            return Err("no branch is checked out to merge into".into());
        };
        if into.name == name {
            return Err(format!("{name} is the branch checked out"));
        }
        if !branches.iter().any(|b| b.name == name) {
            return Err(format!("there is no branch {name}"));
        }
        Ok(BranchPlan::Merge {
            name: name.to_string(),
            into: into.name.clone(),
        })
    }

    /// What a branch command will do, for the line under the bar.
    pub(super) fn describe_branch(&self, plan: &BranchPlan) -> String {
        match plan {
            BranchPlan::Create { name, base, .. } => {
                format!("makes the branch {name} from {base} and switches to it")
            }
            BranchPlan::Switch { name, track: None } => format!("switches to {name}"),
            BranchPlan::Switch {
                name,
                track: Some(remote),
            } => format!("makes {name} here, following {remote}, and switches to it"),
            BranchPlan::Delete {
                name,
                remote: false,
            } => format!("deletes the branch {name}, asks first"),
            BranchPlan::Delete { name, remote: true } => {
                let (remote, there) = on_remote(name);
                format!("deletes {there} from {remote} for everyone, asks first")
            }
            BranchPlan::Rename { from, to } => format!("renames the branch {from} to {to}"),
            BranchPlan::Merge { name, into } => {
                format!("merges {name} into {into}, asks first")
            }
        }
    }

    // ---- doing ------------------------------------------------------------

    /// Why the files cannot change under tiny now, if they cannot: unsaved
    /// edits, see the module docs.
    fn unsaved_in_the_way(&self, doing: &str) -> Option<String> {
        let mut dirty: Vec<String> = self
            .dirty_buffers()
            .iter()
            .map(|p| display_name(p))
            .collect();
        if dirty.is_empty() {
            return None;
        }
        dirty.sort();
        Some(format!(
            "save {} first: {doing} changes the files under {}",
            dirty.join(", "),
            if dirty.len() == 1 { "it" } else { "them" }
        ))
    }

    pub(super) fn carry_out_branch(&mut self, plan: BranchPlan) -> anyhow::Result<String> {
        let Some(repo) = self.repository() else {
            anyhow::bail!("not a git repository");
        };
        match plan {
            BranchPlan::Create { name, from, .. } => {
                // From where the checked out branch is, nothing on disk moves.
                if from.is_some()
                    && let Some(why) = self.unsaved_in_the_way("switching")
                {
                    anyhow::bail!(why);
                }
                repo.create_branch(&name, from.as_deref())?;
                self.after_branch_change();
                Ok(format!("on the new branch {name}"))
            }
            BranchPlan::Switch { name, track } => {
                if let Some(why) = self.unsaved_in_the_way("switching") {
                    anyhow::bail!(why);
                }
                match &track {
                    Some(remote) => repo.switch_tracking(remote)?,
                    None => repo.switch(&name)?,
                }
                self.after_branch_change();
                Ok(format!("on {name}"))
            }
            BranchPlan::Delete { name, remote } => {
                let (kind, message) = if remote {
                    let (there, branch) = on_remote(&name);
                    (
                        ConfirmKind::DeleteRemoteBranch(name.clone()),
                        format!("Delete {branch} from {there}, for everyone who uses it?  (y/n)"),
                    )
                } else {
                    (
                        ConfirmKind::DeleteBranch {
                            name: name.clone(),
                            force: false,
                        },
                        format!("Delete the branch {name}?  (y/n)"),
                    )
                };
                self.mode = Mode::Confirm(Confirm { kind, message });
                Ok(String::new())
            }
            BranchPlan::Rename { from, to } => {
                repo.rename_branch(&from, &to)?;
                self.refresh_git();
                Ok(format!("renamed {from} to {to}"))
            }
            BranchPlan::Merge { name, into } => {
                if let Some(why) = self.unsaved_in_the_way("merging") {
                    anyhow::bail!(why);
                }
                self.mode = Mode::Confirm(Confirm {
                    kind: ConfirmKind::Merge(name.clone()),
                    message: format!("Merge {name} into {into}?  (y/n)"),
                });
                Ok(String::new())
            }
        }
    }

    /// The files may all have changed: the git window, the browser, and the
    /// files open, read again.
    fn after_branch_change(&mut self) {
        self.refresh_git();
        self.refresh();
    }

    /// `y` to deleting a branch here. One with commits on it that are in no
    /// other branch is asked about again, by name.
    pub(super) fn do_delete_branch(&mut self, name: &str, force: bool) {
        let Some(repo) = self.repository() else {
            return;
        };
        match repo.delete_branch(name, force) {
            Ok(()) => {
                self.refresh_git();
                self.status = format!("deleted the branch {name}");
            }
            Err(e) if !force && format!("{e:#}").contains("not fully merged") => {
                self.mode = Mode::Confirm(Confirm {
                    kind: ConfirmKind::DeleteBranch {
                        name: name.to_string(),
                        force: true,
                    },
                    message: format!(
                        "{name} has commits on no other branch. Delete them too?  (y/n)"
                    ),
                });
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// `y` to deleting a branch from its remote: a push, so on a thread.
    pub(super) fn do_delete_remote_branch(&mut self, name: &str) {
        let Some(repo) = self.repository() else {
            return;
        };
        let (remote, branch) = on_remote(name);
        let steps = vec![
            ["git", "push", remote, "--delete", branch]
                .map(str::to_string)
                .to_vec(),
        ];
        if let Err(e) = self.start_job(&repo.root, "deleting", steps, None) {
            self.status = format!("{e:#}");
        }
    }

    /// `y` to a merge. One that stops on a conflict says so, and the files
    /// in conflict are in the list, in red, to be put right and committed.
    pub(super) fn do_merge(&mut self, name: &str) {
        let Some(repo) = self.repository() else {
            return;
        };
        let result = repo.merge(name);
        self.after_branch_change();
        self.status = match result {
            Ok(out) => match out.lines().rfind(|l| !l.trim().is_empty()) {
                Some(line) => format!("merged {name}: {}", line.trim()),
                None => format!("merged {name}"),
            },
            Err(e) => format!("{e:#}"),
        };
    }
}
