//! Knowing when a newer tiny is out, and installing it.
//!
//! # Asking
//!
//! A release is a tag on GitHub, and `…/releases/latest` redirects to the
//! newest one. [`Check::start`] asks for that redirect on a thread of its own —
//! with `curl`, the program the installers already depend on, so tiny carries
//! no HTTP or TLS code of its own — and the event loop collects the answer on
//! its idle tick, the way it collects a git push. Nothing ever waits on it:
//! offline, behind a firewall, or with no `curl`, the answer is simply that
//! there is nothing newer.
//!
//! It asks once per run, and only from a release build with `update_check` on.
//! A `cargo run` is somebody working on tiny, and should not be nagged about
//! the version they are in the middle of writing.
//!
//! # Installing
//!
//! [`install`] runs the platform's installer — the same one-liner the README
//! gives — once tiny has handed the terminal back, so its progress bar and any
//! question it asks reach the person who said yes. It is pointed at the folder
//! the running binary is in, so the update replaces *this* tiny rather than
//! putting a second one somewhere else on the PATH. Then the new binary is
//! asked its version, which is the only reliable way to know the install
//! worked: a pipe from `curl` into `sh` exits cleanly even when the download
//! did not.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use anyhow::{Context, Result, anyhow};

/// The version of the tiny that is running.
pub fn running() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The GitHub repository releases come from, from `Cargo.toml` — so a fork
/// that changes `repository` checks its own releases, not these.
fn repo() -> &'static str {
    env!("CARGO_PKG_REPOSITORY").trim_end_matches('/')
}

/// Where the installers are served from: the raw files on `main`.
fn raw() -> String {
    format!(
        "{}/main",
        repo().replacen(
            "https://github.com/",
            "https://raw.githubusercontent.com/",
            1
        )
    )
}

/// The release check, running somewhere else with its answer still to come.
pub struct Check {
    answer: Receiver<Option<String>>,
}

impl Check {
    /// Ask GitHub for the newest release, on a thread.
    pub fn start() -> Self {
        let (tx, answer) = channel();
        std::thread::spawn(move || {
            let newer = latest().filter(|v| is_newer(v, running()));
            // The receiver is gone only if tiny has already quit, and then
            // nobody is left to tell.
            let _ = tx.send(newer);
        });
        Self { answer }
    }

    /// The answer, once there is one: `Some(Some(version))` for a newer tiny,
    /// `Some(None)` for nothing newer, and `None` while it is still asking.
    pub fn finished(&self) -> Option<Option<String>> {
        match self.answer.try_recv() {
            Ok(newer) => Some(newer),
            Err(TryRecvError::Empty) => None,
            // A thread that panicked has nothing to report either.
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }
}

/// The newest released version, without the `v`, or `None` if GitHub could
/// not be asked or has no release to name.
///
/// A `HEAD` request: only the redirect is wanted, not the page it points at.
pub fn latest() -> Option<String> {
    let out = Command::new("curl")
        .args(["-fsSI", "--max-time", "8"])
        .arg(format!("{}/releases/latest", repo()))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    tag_from_headers(&String::from_utf8_lossy(&out.stdout))
        .map(|tag| tag.trim_start_matches('v').to_string())
}

/// The release a `…/releases/latest` redirect points at, from the response
/// headers.
///
/// A repository with no release yet redirects to the release *list* instead,
/// which has no tag in it, and that is a `None` rather than an error.
fn tag_from_headers(headers: &str) -> Option<String> {
    let location = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("location")
            .then(|| value.trim())
    })?;
    let (_, tag) = location.rsplit_once("/releases/tag/")?;
    let tag = tag.trim_end_matches('/');
    (!tag.is_empty()).then(|| tag.to_string())
}

/// `0.4.5` as three numbers. A leading `v` is allowed, and anything after a
/// `-` — a pre-release label — is ignored rather than refused.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v');
    let core = core.split('-').next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// Whether `latest` is a later version than `current`. Anything that does not
/// read as a version is never newer: an update offered on a misread number
/// would be an update to who knows what.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// Install `version` over the running binary, with the terminal already given
/// back. Returns where the new binary is.
///
/// The installer runs in the temp directory, not here: run from inside a tiny
/// checkout, it would build that checkout instead of fetching the release.
///
/// Where tiny lives is read once, before the installer runs, and handed back
/// for [`relaunch`] rather than asked again afterwards: on Linux the answer
/// comes from `/proc/self/exe`, which by then names the file the update just
/// replaced — `…/tiny (deleted)`.
pub fn install(version: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot tell where tiny is installed")?;
    let folder = exe
        .parent()
        .ok_or_else(|| anyhow!("cannot tell where tiny is installed"))?;
    let status = installer()
        .env("TINY_PREFIX", folder)
        .current_dir(std::env::temp_dir())
        .status()
        .context("could not start the installer")?;
    let now = Command::new(&exe)
        .arg("--version")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    if now == format!("tiny {version}") {
        return Ok(exe);
    }
    Err(anyhow!(
        "the update did not install{} — {} is still {}",
        if status.success() {
            ""
        } else {
            " (the installer failed; its output is above)"
        },
        exe.display(),
        now.strip_prefix("tiny ").unwrap_or("unchanged")
    ))
}

/// The README's install line for this platform, as a command to run.
fn installer() -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("powershell");
        // `exit` hands the installer's verdict to the exit code: it sets
        // `LASTEXITCODE` on failure rather than exiting, because piped
        // through `iex` an exit would close the user's own window.
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
            .arg(format!(
                "irm {}/install.ps1 | iex; exit $global:LASTEXITCODE",
                raw()
            ));
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("curl -fsSL {}/install.sh | sh", raw()));
        cmd
    }
}

/// Start the freshly installed tiny at `exe` with the arguments this one was
/// given, in place of this one.
///
/// Unix replaces the process outright, so the shell that started tiny is
/// waiting on the new one with nothing in between. Windows has no `exec`, so
/// the old process waits for the new one and leaves with its exit code.
pub fn relaunch(exe: &Path) -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = Command::new(exe);
    cmd.args(&args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `exec` only comes back if it failed.
        Err(cmd.exec()).context("could not start the new tiny")
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("could not start the new tiny")?;
        std::process::exit(status.code().unwrap_or(0));
    }
}

/// `tiny --update`: the same check and the same install, without a window.
pub fn run_cli() -> Result<()> {
    println!("tiny {} — asking GitHub for a newer one", running());
    let Some(latest) = latest() else {
        println!("no release found (or GitHub could not be reached)");
        return Ok(());
    };
    if !is_newer(&latest, running()) {
        println!("tiny {} is the newest", running());
        return Ok(());
    }
    println!("updating to {latest}\n");
    install(&latest)?;
    println!("\ntiny {latest} installed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers_not_as_text() {
        assert!(is_newer("0.4.10", "0.4.9"), "10 is more than 9");
        assert!(is_newer("v0.5.0", "0.4.5"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.4.5", "0.4.5"), "the same is not newer");
        assert!(
            !is_newer("0.4.4", "0.4.5"),
            "an older release is not offered"
        );
    }

    #[test]
    fn something_that_is_not_a_version_is_never_newer() {
        assert!(!is_newer("latest", "0.4.5"));
        assert!(!is_newer("0.5", "0.4.5"), "two numbers is not a version");
        assert!(!is_newer("0.5.0.1", "0.4.5"), "nor is four");
        assert!(!is_newer("", "0.4.5"));
        assert!(
            is_newer("0.5.0-rc1", "0.4.5"),
            "a pre-release label is read past, not refused"
        );
    }

    #[test]
    fn the_running_version_reads_as_a_version() {
        assert!(parse(running()).is_some(), "{}", running());
    }

    #[test]
    fn the_newest_release_is_read_off_the_redirect() {
        let headers = "HTTP/2 302 \r\n\
                       date: Sat, 10 Oct 2026 22:00:00 GMT\r\n\
                       location: https://github.com/mattsaund/tiny/releases/tag/v0.4.6\r\n\
                       \r\n";
        assert_eq!(tag_from_headers(headers).as_deref(), Some("v0.4.6"));
        // Header names are not case-sensitive, and HTTP/1.1 capitalizes them.
        let headers = "HTTP/1.1 302 Found\nLocation: https://github.com/a/b/releases/tag/v1.2.3\n";
        assert_eq!(tag_from_headers(headers).as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn a_repository_with_no_release_names_none() {
        // GitHub sends the release list instead, which has no tag in it.
        let headers = "HTTP/2 302\r\nlocation: https://github.com/mattsaund/tiny/releases\r\n";
        assert_eq!(tag_from_headers(headers), None);
        assert_eq!(
            tag_from_headers("HTTP/2 200\r\n"),
            None,
            "no redirect at all"
        );
    }

    #[test]
    fn the_installers_come_from_the_same_repository_as_the_releases() {
        assert!(repo().starts_with("https://github.com/"), "{}", repo());
        assert!(
            raw().starts_with("https://raw.githubusercontent.com/"),
            "{}",
            raw()
        );
        assert!(raw().ends_with("/main"));
        let program = installer().get_program().to_string_lossy().into_owned();
        assert!(matches!(program.as_str(), "sh" | "powershell"), "{program}");
    }
}
