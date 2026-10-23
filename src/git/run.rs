//! Starting `git`, and waiting for it, or not.
//!
//! # Two ways, split by what they wait on
//!
//! Reading the repository is fast and local: `git status` on a large project
//! is milliseconds, and [`git`] runs it in the foreground where the answer is
//! immediately usable.
//!
//! Talking to a remote is neither. A push over a slow link is seconds at best,
//! and a synchronous one would freeze the window with no way to say what it was
//! doing: the worst possible moment to look broken. [`Job::start`] runs those
//! on a thread and hands back a receiver; the event loop already wakes twice a
//! second for the disk watcher, and it collects finished jobs on the same tick.
//!
//! # Environment
//!
//! Every call sets `GIT_OPTIONAL_LOCKS=0`, which stops a mere status check from
//! taking the index lock. Without it, a window left open on a repository would
//! fight `git` in a terminal next door over a file neither of them is writing.
//!
//! # Git never asks the terminal anything
//!
//! tiny is drawing on that terminal, in raw mode. A password prompt would land
//! in the middle of the picture, and the Enter typed to answer it arrives as a
//! `\r` that git never takes for the end of a line, so the command would wait
//! forever and every button after it would say it is still running. So git
//! gets no input, `GIT_TERMINAL_PROMPT=0` stops it asking for a password, and
//! ssh runs in batch mode (unless you have set your own ssh command). Anything
//! that needed asking fails instead, with git's own message: set up a
//! credential helper or an ssh agent, and the buttons work.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use anyhow::{Context, Result, anyhow};

/// Run git in `dir` and give back its standard output.
///
/// A non-zero exit is an error carrying git's own message, because git's
/// complaints are already written for a person to read and anything we put in
/// front of them would be in the way.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run(dir, args, &[])
}

/// [`git`], with `env` set as well.
fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(env.iter().copied())
        .args(args)
        .output()
        .with_context(|| match args.first() {
            Some(a) => format!("cannot run git {a}"),
            None => "cannot run git".to_string(),
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let message = stderr.trim();
        return Err(anyhow!(
            "{}",
            if message.is_empty() {
                format!("git {} failed", args.first().unwrap_or(&""))
            } else {
                message.to_string()
            }
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A git command running somewhere else, with its answer still to come.
///
/// Held on `App` while it runs. Nothing else can start while one is in flight
/// (see `App::start_job`) because two pushes at once is not a thing anyone
/// means to do, and a queue would only hide the first one's failure.
pub struct Job {
    /// What to call it on screen: `pushing`, `pulling`.
    pub what: String,
    done: Receiver<Result<String, String>>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job").field("what", &self.what).finish()
    }
}

impl Job {
    /// Start each of `steps` in turn in `dir`, on a thread, and return at
    /// once. A step that fails stops the ones after it, and what the last one
    /// to run said is the answer.
    ///
    /// The thread is detached deliberately: if tiny exits while a push is in
    /// flight, the push is already git's business and killing it half way
    /// through would be worse than letting it finish.
    pub fn start(dir: &Path, what: &str, steps: &[&[&str]]) -> Self {
        let (tx, done) = channel();
        let dir: PathBuf = dir.to_path_buf();
        let steps: Vec<Vec<String>> = steps
            .iter()
            .map(|args| args.iter().map(|a| a.to_string()).collect())
            .collect();
        std::thread::spawn(move || {
            // Batch mode, so ssh fails rather than asking for a passphrase or
            // about an unknown host on the terminal tiny is drawing on; but
            // never over an ssh command someone has chosen for themselves.
            let quiet = std::env::var_os("GIT_SSH_COMMAND").is_none()
                && git(&dir, &["config", "--get", "core.sshCommand"]).is_err();
            let env: &[(&str, &str)] = if quiet {
                &[("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")]
            } else {
                &[]
            };
            let mut result = Ok(String::new());
            for args in &steps {
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                result = run(&dir, &borrowed, env).map_err(|e| format!("{e:#}"));
                if result.is_err() {
                    break;
                }
            }
            // The receiver is gone if tiny has closed the window or quit, and
            // there is nothing to report to.
            let _ = tx.send(result);
        });
        Self {
            what: what.to_string(),
            done,
        }
    }

    /// Has it finished? `None` while it is still running.
    pub fn finished(&self) -> Option<Result<String, String>> {
        match self.done.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            // The thread died without sending, which a panic in `git` would do.
            Err(TryRecvError::Disconnected) => {
                Some(Err(format!("{} stopped without saying why", self.what)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_git_command_carries_gits_own_complaint() {
        let td = tempfile::tempdir().unwrap();
        let err = git(td.path(), &["rev-parse", "--show-toplevel"])
            .expect_err("a temp dir is not a repository");
        let text = format!("{err:#}").to_lowercase();
        assert!(
            text.contains("not a git repository"),
            "git's own words, not ours: {text}"
        );
    }

    #[test]
    fn a_job_that_finishes_reports_what_it_did() {
        let td = tempfile::tempdir().unwrap();
        let job = Job::start(td.path(), "asking", &[&["--version"]]);
        // Give the thread a moment; this is the one place a test waits.
        let mut answer = None;
        for _ in 0..200 {
            if let Some(result) = job.finished() {
                answer = Some(result);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let answer = answer.expect("git --version finishes").expect("and works");
        assert!(answer.starts_with("git version"), "{answer}");
    }
}
