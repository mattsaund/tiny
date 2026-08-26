//! tiny — a terminal knowledge manager.
//!
//! Left pane is the project tree, right pane is whatever the cursor is on:
//! markdown renders, code opens in an editor, pictures are described and
//! handed to the desktop's own viewer. `Ctrl+S` saves.
//!
//! # Where things live
//!
//! Reading top to bottom, the program is four layers:
//!
//! 1. **Startup** — this file. Parse the command line, work out what to open
//!    ([`files::project`]), load configuration ([`config`]), then hand control
//!    to a plain blocking event loop.
//! 2. **State** — [`app`]. One `App` struct owns everything mutable: the tree
//!    cursor, the open buffers, the current mode, the status line. Every
//!    keypress goes through `App::on_key`.
//! 3. **Drawing** — [`ui`]. Reads `App` and paints a frame. It is deliberately
//!    the only module that knows about ratatui widgets.
//! 4. **Support** — three folders of self-contained machinery that `app` and
//!    `ui` call into: [`text`] (the buffer, markdown, highlighting, search),
//!    [`files`] (the tree, project resolution, media), and [`map`] (the link
//!    graph and the view of it). [`config`] holds the settings and the key
//!    bindings, and is read once before any of them.
//!
//! # The one-way rule
//!
//! State flows one way: keys mutate `App`, then `ui` reads `App` and draws.
//! `ui` never handles input, and `app` never touches a ratatui widget. The
//! sanctioned exceptions are all one thing: `ui` writes back scroll offsets
//! and the syntax-highlighting cache, because neither can be computed until
//! the pane size is known — and the pane size is only known while drawing.
//! `ui`'s own module docs list them. If you find yourself wanting another,
//! that is usually a sign the state belongs on `App` instead.
//!
//! # The event loop is blocking on purpose
//!
//! There is no tick, no timer, and no background thread. `run` blocks in
//! `event::read()` until the user does something, then redraws once. An idle
//! tiny uses no CPU at all, which is most of why it stays small. The cost is
//! that every operation is synchronous: a slow search or a big graph build
//! freezes the UI while it runs, so anything expensive needs its own budget
//! (see the size caps in `text::search`, `map::graph`, and `text::highlight`).

mod app;
mod config;
mod files;
mod map;
mod text;
mod ui;

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use crossterm::execute;

use app::App;
use config::{CONF_NAME, Config};

/// `--help` output. Kept here as one literal rather than assembled from an
/// argument parser: there are only six options, and a contributor changing
/// the CLI should be able to see the whole surface in one place.
const USAGE: &str = "\
tiny — a terminal knowledge manager

USAGE:
    tiny                open the current directory
    tiny <DIR>          open a folder; one that does not exist is created
    tiny <FILE>         edit one file, with its folder in the tree beside it;
                        a file that does not exist is created too

OPTIONS:
    -h, --help          show this message
    -V, --version       show the version
        --config        print the path of the config file
        --licenses      terms of the bundled syntax definitions
        --uninstall     remove tiny; your notes are not touched

tiny writes nothing into a folder of yours except a starting `README.md`, and
only into one it just created or found empty. Turn even that off with
`starter_readme = false`.

KEYS:
    ?                   every key and command, from inside the app
    /                   the bar: searches, or `*` first for a command
    m                   the project map
";

/// Print to stdout, and do not mind if nobody is listening.
///
/// `print!` panics when the pipe it is writing into closes, which is what
/// happens the moment someone quits the pager they piped `--licenses` into.
/// Every one of these outputs is a whole answer written in one go, so a reader
/// who stops early has read as much as they wanted and there is nothing to
/// report.
fn emit(text: &str) {
    let _ = std::io::Write::write_all(&mut std::io::stdout(), text.as_bytes());
}

/// Thin wrapper so the real work can use `?`.
///
/// Errors are printed with `{e:#}`, which walks anyhow's whole context chain —
/// so "cannot open /nope: No such file or directory" reaches the user instead
/// of just the innermost io error. Exit code 1 on failure, for scripts.
fn main() {
    if let Err(e) = real_main() {
        eprintln!("tiny: {e:#}");
        std::process::exit(1);
    }
}

/// Startup, in the order things have to happen.
///
/// The ordering here is not arbitrary — each step needs the one before it:
///
/// 1. Handle the options that never touch the disk (`--help`, `--version`,
///    `--config`) and return early.
/// 2. Load the config, which [`files::project::resolve`] needs for
///    `starter_readme` and `default_root` before it can decide what to do
///    with the argument.
/// 3. Resolve the argument into a [`files::project::Target`], creating the
///    project if it does not exist.
/// 4. Write out the defaults on first run.
/// 5. Build the `App`, take over the terminal, loop, and restore.
///
/// There is one config file and it is read once, before anything else: which
/// folder ends up open cannot change how the program behaves. Only one warning
/// survives to the status bar, the more urgent first — a broken config file
/// beats "we just wrote you a config file".
fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            emit(USAGE);
            return Ok(());
        }
        Some("-V") | Some("--version") => {
            println!("tiny {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--config") => {
            match Config::user_path() {
                Some(p) => println!("{}", p.display()),
                None => return Err(anyhow!("no config directory available")),
            }
            return Ok(());
        }
        Some("--licenses") => {
            emit(&text::highlight::acknowledgements());
            return Ok(());
        }
        Some("--uninstall") => return uninstall(),
        Some(a) if a.starts_with('-') => {
            return Err(anyhow!("unknown option `{a}` — try `tiny --help`"));
        }
        _ => {}
    }

    let (cfg, warn_config) = Config::load();
    let target = files::project::resolve(args.first().map(String::as_str), &cfg)?;

    // First run: write the defaults out so there is a file to edit. Done here
    // rather than in `load` so `--help` never touches the disk.
    let warn_first_run = match Config::user_path() {
        Some(p) if !p.exists() => match cfg.save() {
            Ok(p) => Some(format!("wrote {} to {}", CONF_NAME, p.display())),
            Err(e) => Some(format!("could not write {CONF_NAME}: {e}")),
        },
        _ => None,
    };

    let warning = warn_config.or(warn_first_run);
    let mut app = App::new(target, cfg, warning)?;

    // A clean message beats a panic when there is no terminal to draw on —
    // piping the binary or running it from a script hits this.
    let mut terminal = ratatui::try_init().map_err(|e| {
        anyhow!("no interactive terminal available ({e}); tiny needs a real terminal")
    })?;
    // Wheel events only reach a program that asks for them. Without this the
    // terminal translates a notch into three arrow keys of its own, which is
    // why scrolling used to jump. Best-effort: a terminal that will not report
    // the mouse is still perfectly usable from the keyboard.
    let mouse = execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    let result = run(&mut terminal, &mut app);
    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result
}

/// The event loop: draw, block for a key, dispatch, repeat.
///
/// Note the order — the frame is drawn *before* the quit check, so the last
/// action a user takes is visible on screen before the program exits. It also
/// means `App` never has to ask for a redraw: every keypress produces exactly
/// one frame, and nothing else produces any.
///
/// `ratatui::restore` in the caller runs whether or not this returns an error,
/// so a failure in here still gives the terminal back.
fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        match event::read()? {
            // Windows terminals report releases too; only presses, and the
            // repeats from holding a key down, should reach the app.
            Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                app.on_key(k)
            }
            // One notch, one line. Everything else the mouse reports — moves,
            // clicks, drags — is deliberately ignored: tiny is a keyboard
            // program, and the wheel is only here because three-line jumps
            // make a page hard to read.
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollDown => app.on_scroll(true, m.column),
                MouseEventKind::ScrollUp => app.on_scroll(false, m.column),
                _ => {}
            },
            _ => {}
        }
    }
}

// ---- uninstall -------------------------------------------------------------

/// Everything an uninstall would delete.
///
/// Built in full before a single file is removed, so the prompt can show the
/// whole list and the user agrees to it as one thing. An uninstaller that
/// discovers what to delete as it goes is one you cannot say no to halfway.
///
/// Note what is *not* in here: anything under a project. tiny writes exactly
/// one file into a folder of yours, a starter `README.md`, and by the time you
/// are uninstalling it is a note like any other. Deleting notes is not what
/// removing a program means.
#[derive(Debug, Default, PartialEq, Eq)]
struct Removal {
    /// The running binary — whatever `tiny` you just typed.
    bin: Option<PathBuf>,
    /// `tiny.conf`, if one was ever written.
    conf: Option<PathBuf>,
    /// The config directory, only when `tiny.conf` is the last thing in it.
    /// A directory someone put their own files in stays.
    conf_dir: Option<PathBuf>,
    /// The `--root` cargo installed into, when its bookkeeping names tiny.
    /// `cargo uninstall` is then the right way to remove the binary, because
    /// it also forgets the entry — unlinking alone leaves cargo believing
    /// tiny is still installed.
    cargo_root: Option<PathBuf>,
}

impl Removal {
    /// Work out what to delete, given where the binary and the config live.
    ///
    /// Both are passed in rather than looked up so this is testable against a
    /// temporary directory instead of against the machine it runs on.
    fn plan(bin: Option<PathBuf>, conf: Option<PathBuf>) -> Self {
        let conf = conf.filter(|p| p.is_file());
        let conf_dir = conf
            .as_ref()
            .and_then(|p| p.parent())
            // Exactly one entry, and it is the file we are about to remove.
            .filter(|d| {
                std::fs::read_dir(d)
                    .map(|mut it| it.next().is_some() && it.next().is_none())
                    .unwrap_or(false)
            })
            .map(PathBuf::from);
        // `cargo install --root R` puts the binary in `R/bin` and its record
        // in `R/.crates.toml`, so the root is two levels up from the binary.
        let cargo_root = bin
            .as_ref()
            .filter(|p| p.is_file())
            .and_then(|p| p.parent()?.parent())
            .filter(|root| {
                std::fs::read_to_string(root.join(".crates.toml"))
                    .map(|s| s.contains("\"tiny "))
                    .unwrap_or(false)
            })
            .map(PathBuf::from);
        Self {
            bin: bin.filter(|p| p.is_file()),
            conf,
            conf_dir,
            cargo_root,
        }
    }

    /// The list shown at the prompt, in the order things are removed.
    fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(p) = &self.conf {
            out.push(format!("  {}", p.display()));
        }
        if let Some(p) = &self.conf_dir {
            out.push(format!("  {}/", p.display()));
        }
        if let Some(p) = &self.bin {
            out.push(format!("  {}", p.display()));
        }
        out
    }

    /// Delete everything, collecting failures rather than stopping at the
    /// first. A config file that will not budge should not leave the binary
    /// behind as well — you would have to run the uninstaller again to finish,
    /// and it would have less to work with the second time.
    ///
    /// Config goes before the binary so that a failure part-way leaves `tiny`
    /// still runnable, and therefore still able to try again.
    fn perform(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some(p) = &self.conf
            && let Err(e) = std::fs::remove_file(p)
        {
            errors.push(format!("{}: {e}", p.display()));
        }
        if let Some(p) = &self.conf_dir
            && let Err(e) = std::fs::remove_dir(p)
        {
            errors.push(format!("{}: {e}", p.display()));
        }
        let Some(bin) = &self.bin else {
            return errors;
        };
        // Hand it to cargo when cargo owns it, so its bookkeeping is updated
        // too. Best effort: an old install whose cargo has since been removed
        // still gets unlinked below.
        if let Some(root) = &self.cargo_root {
            let done = std::process::Command::new("cargo")
                .args(["uninstall", "--quiet", "--root"])
                .arg(root)
                .arg("tiny")
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if done {
                return errors;
            }
        }
        // Unix unlinks a running executable happily; Windows will not, and
        // says so, which is the most useful thing we can do about it.
        if let Err(e) = std::fs::remove_file(bin) {
            errors.push(format!("{}: {e}", bin.display()));
        }
        errors
    }
}

/// `tiny --uninstall`: show what will go, ask, then remove it.
///
/// The prompt reads from stdin rather than from the terminal directly, so
/// `echo y | tiny --uninstall` works for anyone scripting it and there is no
/// `--yes` flag to document. End-of-file counts as "no": a program removing
/// itself should need to be told, not merely left alone.
fn uninstall() -> Result<()> {
    let bin = std::env::current_exe().ok();
    let removal = Removal::plan(bin, Config::user_path());
    let lines = removal.lines();
    if lines.is_empty() {
        println!("nothing to remove — no tiny binary or config file found");
        return Ok(());
    }

    println!("This will remove:");
    for line in &lines {
        println!("{line}");
    }
    println!("\nYour notes and projects are not touched.");
    print!("\nRemove tiny? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let mut reply = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut reply).is_err()
        || !matches!(reply.trim(), "y" | "Y" | "yes" | "Yes")
    {
        println!("cancelled — nothing was removed");
        return Ok(());
    }

    let errors = removal.perform();
    if errors.is_empty() {
        println!("tiny is gone. Thanks for trying it.");
        return Ok(());
    }
    for e in &errors {
        eprintln!("tiny: could not remove {e}");
    }
    Err(anyhow!("{} item(s) could not be removed", errors.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// A fake install: `<root>/bin/tiny` plus cargo's record of it, and a
    /// config file in its own directory.
    fn fake_install(td: &Path) -> (PathBuf, PathBuf) {
        let bin_dir = td.join("root").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("tiny");
        fs::write(&bin, "#!/bin/sh\n").unwrap();
        let conf_dir = td.join("config").join("tiny");
        fs::create_dir_all(&conf_dir).unwrap();
        let conf = conf_dir.join(CONF_NAME);
        fs::write(&conf, "borders = true\n").unwrap();
        (bin, conf)
    }

    #[test]
    fn uninstall_removes_the_binary_and_the_config_and_nothing_else() {
        let td = tempfile::tempdir().unwrap();
        let (bin, conf) = fake_install(td.path());
        // A note living beside the install, to prove it survives.
        let note = td.path().join("notes.md");
        fs::write(&note, "mine").unwrap();

        let removal = Removal::plan(Some(bin.clone()), Some(conf.clone()));
        assert!(removal.perform().is_empty(), "clean removal");

        assert!(!bin.exists(), "the binary is gone");
        assert!(!conf.exists(), "the config is gone");
        assert!(
            !conf.parent().unwrap().exists(),
            "its directory went with it"
        );
        assert!(note.exists(), "a note beside it is untouched");
    }

    #[test]
    fn a_config_directory_holding_someone_elses_files_is_left_alone() {
        let td = tempfile::tempdir().unwrap();
        let (_, conf) = fake_install(td.path());
        let theirs = conf.parent().unwrap().join("notes-to-self.txt");
        fs::write(&theirs, "keep me").unwrap();

        let removal = Removal::plan(None, Some(conf.clone()));
        assert_eq!(
            removal.conf_dir, None,
            "the directory is not ours to delete"
        );
        removal.perform();
        assert!(!conf.exists(), "our file still goes");
        assert!(theirs.exists(), "theirs stays");
    }

    #[test]
    fn a_cargo_install_is_recognised_by_its_bookkeeping() {
        let td = tempfile::tempdir().unwrap();
        let (bin, _) = fake_install(td.path());
        let root = td.path().join("root");

        assert_eq!(
            Removal::plan(Some(bin.clone()), None).cargo_root,
            None,
            "no record means no cargo"
        );

        fs::write(
            root.join(".crates.toml"),
            "[v1]\n\"tiny 0.1.2 (path+file:///src)\" = [\"tiny\"]\n",
        )
        .unwrap();
        assert_eq!(
            Removal::plan(Some(bin), None).cargo_root,
            Some(root),
            "a record naming tiny means cargo should do the removing"
        );
    }

    #[test]
    fn planning_against_nothing_finds_nothing_to_remove() {
        let td = tempfile::tempdir().unwrap();
        let removal = Removal::plan(Some(td.path().join("nope")), Some(td.path().join("gone")));
        assert_eq!(removal, Removal::default());
        assert!(removal.lines().is_empty(), "the prompt would be empty");
    }
}
