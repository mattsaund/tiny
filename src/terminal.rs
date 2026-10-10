//! Another tiny, in a window of its own.
//!
//! `Ctrl+N` starts a second tiny on the same project, in a new window of the
//! terminal this one is running in. There is no standard way to ask a terminal
//! for a window, so [`candidates`] reads the variables terminals set about
//! themselves, and tries what each one answers to — then the system's own
//! terminal, if nothing more specific was found or it would not start.
//!
//! # Not `Ctrl+Enter`
//!
//! It would read well, and it cannot work: in a terminal's legacy encoding
//! `Ctrl+Enter` *is* Enter — macOS Terminal sends the same byte for both — so
//! the key would open the file under the cursor instead. `Ctrl+N` is what
//! every program means by "new window", and every terminal sends it.
//!
//! # Where the window opens
//!
//! On the project's own folder, whichever folder the cursor is in: the point
//! is a second view of the same work, and the folder is what tiny was opened
//! on. Inside tmux the "window" is a tmux one, since the terminal around tmux
//! may be on another machine entirely.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow};

/// The platform, passed in rather than read from `cfg!` so that every
/// platform's commands can be tested on any one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    /// Linux and the BSDs: whatever is running a desktop, if anything is.
    Unix,
}

impl Os {
    pub fn this() -> Self {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Unix
        }
    }
}

/// One way of opening the window: a name to report, and the command.
#[derive(Debug)]
pub struct Candidate {
    /// What to call it on the status line: `Terminal`, `kitty`, `tmux`.
    pub name: &'static str,
    pub program: String,
    pub args: Vec<String>,
}

impl Candidate {
    fn new(name: &'static str, program: &str, args: &[&str]) -> Self {
        Self {
            name,
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }
}

/// Every way this terminal might open a window running `tiny` on `dir`, best
/// first.
///
/// `env` is how the environment is read — `std::env::var` for real, a table
/// in the tests. On a Mac, Terminal and iTerm2 are handed a `.command` script,
/// which both run in a new window without needing permission to be scripted;
/// `script` is where that file would be written.
pub fn candidates(
    os: Os,
    env: &dyn Fn(&str) -> Option<String>,
    tiny: &Path,
    dir: &Path,
    script: &Path,
) -> Vec<Candidate> {
    let tiny = tiny.to_string_lossy().into_owned();
    let dir = dir.to_string_lossy().into_owned();
    let script = script.to_string_lossy().into_owned();
    let has = |var: &str| env(var).is_some_and(|v| !v.is_empty());
    let program = env("TERM_PROGRAM").unwrap_or_default();
    let mut out = Vec::new();

    if has("TMUX") {
        out.push(Candidate::new(
            "tmux",
            "tmux",
            &["new-window", "-c", &dir, &tiny],
        ));
    }
    match os {
        Os::Windows => {
            if has("WT_SESSION") {
                out.push(Candidate::new(
                    "Windows Terminal",
                    "wt",
                    &["-w", "new", "-d", &dir, &tiny],
                ));
            }
            // `start` takes its first quoted argument as the window's title,
            // so it is given an empty one rather than having the path taken
            // for it — the same as the desktop opener in `media`.
            out.push(Candidate::new(
                "a new console",
                "cmd",
                &["/C", "start", "", "/D", &dir, &tiny],
            ));
        }
        Os::Mac => {
            match program.as_str() {
                "WezTerm" => out.push(wezterm(env, &dir, &tiny)),
                "ghostty" => out.push(Candidate::new(
                    "Ghostty",
                    "open",
                    &[
                        "-na",
                        "Ghostty",
                        "--args",
                        &format!("--working-directory={dir}"),
                        "-e",
                        &tiny,
                    ],
                )),
                "iTerm.app" => {
                    out.push(Candidate::new("iTerm2", "open", &["-a", "iTerm", &script]))
                }
                _ if has("KITTY_WINDOW_ID") => out.push(Candidate::new(
                    "kitty",
                    "open",
                    &["-na", "kitty", "--args", "--directory", &dir, &tiny],
                )),
                _ if has("ALACRITTY_WINDOW_ID") => out.push(Candidate::new(
                    "Alacritty",
                    "open",
                    &[
                        "-na",
                        "Alacritty",
                        "--args",
                        "--working-directory",
                        &dir,
                        "-e",
                        &tiny,
                    ],
                )),
                _ => {}
            }
            out.push(Candidate::new(
                "Terminal",
                "open",
                &["-a", "Terminal", &script],
            ));
        }
        Os::Unix => {
            if let Some(chosen) = env("TERMINAL").filter(|t| !t.is_empty()) {
                // The user's own choice, by the convention most terminals
                // follow: `-e` and the command after it.
                out.push(Candidate {
                    name: "$TERMINAL",
                    program: chosen,
                    args: vec!["-e".into(), tiny.clone()],
                });
            }
            if program == "WezTerm" || has("WEZTERM_PANE") {
                out.push(wezterm(env, &dir, &tiny));
            }
            if program == "ghostty" || has("GHOSTTY_RESOURCES_DIR") {
                out.push(Candidate::new(
                    "Ghostty",
                    "ghostty",
                    &[&format!("--working-directory={dir}"), "-e", &tiny],
                ));
            }
            if has("KITTY_WINDOW_ID") {
                out.push(Candidate::new(
                    "kitty",
                    "kitty",
                    &["--directory", &dir, &tiny],
                ));
            }
            if has("ALACRITTY_WINDOW_ID") {
                out.push(Candidate::new(
                    "Alacritty",
                    "alacritty",
                    &["--working-directory", &dir, "-e", &tiny],
                ));
            }
            if has("KONSOLE_VERSION") {
                out.push(Candidate::new(
                    "Konsole",
                    "konsole",
                    &["--workdir", &dir, "-e", &tiny],
                ));
            }
            if has("GNOME_TERMINAL_SCREEN") || has("GNOME_TERMINAL_SERVICE") {
                out.push(Candidate::new(
                    "GNOME Terminal",
                    "gnome-terminal",
                    &[&format!("--working-directory={dir}"), "--", &tiny],
                ));
            }
            if has("TILIX_ID") {
                out.push(Candidate::new("Tilix", "tilix", &["-w", &dir, "-e", &tiny]));
            }
            if env("TERM").is_some_and(|t| t.starts_with("foot")) {
                out.push(Candidate::new("foot", "foot", &["-D", &dir, &tiny]));
            }
            if has("XTERM_VERSION") {
                out.push(Candidate::new("xterm", "xterm", &["-e", &tiny]));
            }
            // The distribution's own idea of a terminal: Debian's alternative,
            // and the freedesktop launcher that is replacing it.
            out.push(Candidate::new(
                "the system terminal",
                "x-terminal-emulator",
                &["-e", &tiny],
            ));
            out.push(Candidate::new(
                "the system terminal",
                "xdg-terminal-exec",
                &[&tiny],
            ));
        }
    }
    out
}

/// WezTerm's own way in, from the binary it says it is running as.
fn wezterm(env: &dyn Fn(&str) -> Option<String>, dir: &str, tiny: &str) -> Candidate {
    let program = env("WEZTERM_EXECUTABLE").unwrap_or_else(|| "wezterm".into());
    Candidate {
        name: "WezTerm",
        program,
        args: ["cli", "spawn", "--new-window", "--cwd", dir, "--", tiny]
            .iter()
            .map(|a| a.to_string())
            .collect(),
    }
}

/// The `.command` script Terminal and iTerm2 run: into the folder, and tiny
/// in place of the shell. It deletes itself first, since nothing else is
/// left to — the terminal reads it after tiny has moved on.
fn command_script(tiny: &Path, dir: &Path) -> String {
    format!(
        "#!/bin/sh\nrm -f \"$0\"\ncd {} && exec {}\n",
        quoted(&dir.to_string_lossy()),
        quoted(&tiny.to_string_lossy()),
    )
}

/// A string as one single-quoted shell word, whatever is in it.
fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Open a new window running tiny on `dir`, and say what opened it.
pub fn open_window(dir: &Path) -> Result<&'static str> {
    let tiny = std::env::current_exe().map_err(|e| anyhow!("cannot find tiny: {e}"))?;
    let os = Os::this();
    // A desktop is something to open a window *on*. Over ssh without one
    // there is nowhere for it to go — except a tmux window, which needs none.
    let env = |var: &str| std::env::var(var).ok();
    if os == Os::Unix
        && env("DISPLAY").is_none()
        && env("WAYLAND_DISPLAY").is_none()
        && env("TMUX").is_none()
    {
        return Err(anyhow!(
            "no desktop to open a window on — run tiny inside tmux for that"
        ));
    }
    let script = std::env::temp_dir().join(format!(
        "tiny-window-{}-{}.command",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    let mut tried = Vec::new();
    for candidate in candidates(os, &env, &tiny, dir, &script) {
        // A test build starts nothing, so it has no script to leave behind.
        if candidate.args.iter().any(|a| Path::new(a) == script) && !cfg!(test) {
            write_script(&script, &tiny, dir)?;
        }
        if spawn(&candidate, dir) {
            return Ok(candidate.name);
        }
        tried.push(candidate.program);
    }
    let _ = std::fs::remove_file(&script);
    Err(anyhow!(
        "could not open a terminal window (tried {})",
        tried.join(", ")
    ))
}

/// Write the `.command` script, runnable.
fn write_script(script: &Path, tiny: &Path, dir: &Path) -> Result<()> {
    std::fs::write(script, command_script(tiny, dir))
        .map_err(|e| anyhow!("cannot write {}: {e}", script.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| anyhow!("cannot make {} runnable: {e}", script.display()))?;
    }
    Ok(())
}

/// Start one candidate, detached from this tiny: its own folder, no shared
/// input or output. Whether it *started* is all that can be known — a terminal
/// opens its window after the program that asked has moved on.
fn spawn(candidate: &Candidate, dir: &Path) -> bool {
    if cfg!(test) {
        spawned::record(candidate);
        return true;
    }
    let child = Command::new(&candidate.program)
        .args(&candidate.args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(child) => {
            crate::files::media::keep_until_done(child);
            true
        }
        Err(_) => false,
    }
}

/// What a test build would have started: nothing, written down — a test must
/// not open windows on the machine running it.
#[cfg(test)]
pub mod spawned {
    use std::cell::RefCell;

    thread_local! {
        static SPAWNED: RefCell<Vec<(String, Vec<String>)>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record(c: &super::Candidate) {
        SPAWNED.with(|s| s.borrow_mut().push((c.program.clone(), c.args.clone())));
    }

    /// Every program this test thread would have started, and forget them.
    pub fn take() -> Vec<(String, Vec<String>)> {
        SPAWNED.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
}

#[cfg(not(test))]
mod spawned {
    pub(super) fn record(_: &super::Candidate) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A project folder with a space in its name, as so many are.
    fn project() -> PathBuf {
        PathBuf::from("/home/me/my notes")
    }

    fn with(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    fn first(os: Os, vars: &'static [(&'static str, &'static str)]) -> Candidate {
        candidates(
            os,
            &with(vars),
            Path::new("/bin/tiny"),
            &project(),
            Path::new("/tmp/t.command"),
        )
        .into_iter()
        .next()
        .expect("at least one way to try")
    }

    #[test]
    fn inside_tmux_a_window_is_a_tmux_window() {
        for os in [Os::Mac, Os::Unix, Os::Windows] {
            let c = first(os, &[("TMUX", "/tmp/tmux-1/default,1,0")]);
            assert_eq!(c.program, "tmux");
            assert_eq!(
                c.args,
                ["new-window", "-c", "/home/me/my notes", "/bin/tiny"],
                "the folder goes whole, spaces and all"
            );
        }
    }

    #[test]
    fn macos_terminal_runs_a_script_rather_than_being_scripted() {
        let c = first(Os::Mac, &[("TERM_PROGRAM", "Apple_Terminal")]);
        assert_eq!((c.name, c.program.as_str()), ("Terminal", "open"));
        assert_eq!(c.args, ["-a", "Terminal", "/tmp/t.command"]);
        let c = first(Os::Mac, &[("TERM_PROGRAM", "iTerm.app")]);
        assert_eq!(c.args, ["-a", "iTerm", "/tmp/t.command"]);
    }

    #[test]
    fn every_platform_ends_on_its_own_terminal() {
        let last = |os| {
            candidates(
                os,
                &with(&[]),
                Path::new("/bin/tiny"),
                &project(),
                Path::new("/tmp/t.command"),
            )
            .pop()
            .unwrap()
        };
        assert_eq!(last(Os::Mac).name, "Terminal");
        assert_eq!(last(Os::Windows).program, "cmd");
        assert_eq!(last(Os::Unix).program, "xdg-terminal-exec");
    }

    #[test]
    fn windows_terminal_opens_a_window_of_its_own() {
        let c = first(Os::Windows, &[("WT_SESSION", "abc")]);
        assert_eq!(c.program, "wt");
        assert_eq!(
            c.args,
            ["-w", "new", "-d", "/home/me/my notes", "/bin/tiny"]
        );
    }

    #[test]
    fn linux_terminals_are_found_by_what_they_set() {
        for (var, value, program) in [
            ("KITTY_WINDOW_ID", "1", "kitty"),
            ("GNOME_TERMINAL_SCREEN", "x", "gnome-terminal"),
            ("KONSOLE_VERSION", "230800", "konsole"),
            ("ALACRITTY_WINDOW_ID", "9", "alacritty"),
            ("TERM", "foot", "foot"),
        ] {
            let vars: &'static [(&'static str, &'static str)] =
                Box::leak(vec![(var, value)].into_boxed_slice());
            assert_eq!(first(Os::Unix, vars).program, program, "{var}");
        }
    }

    #[test]
    fn the_users_own_terminal_comes_first() {
        let c = first(Os::Unix, &[("TERMINAL", "st"), ("KITTY_WINDOW_ID", "1")]);
        assert_eq!(c.program, "st");
        assert_eq!(c.args, ["-e", "/bin/tiny"]);
    }

    #[test]
    fn the_script_goes_into_the_folder_and_becomes_tiny() {
        let script = command_script(Path::new("/opt/it's/tiny"), &project());
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("cd '/home/me/my notes' && exec '/opt/it'\\''s/tiny'"));
        assert!(script.contains("rm -f \"$0\""), "and tidies itself away");
    }
}
