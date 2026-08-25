//! tiny — a terminal knowledge manager.
//!
//! Left pane is the project tree, right pane is whatever the cursor is on:
//! markdown renders, code opens in an editor, pictures draw. `Ctrl+S` saves.
//!
//! # Where things live
//!
//! Reading top to bottom, the program is four layers:
//!
//! 1. **Startup** — this file. Parse the command line, work out what to open
//!    ([`project`]), load configuration ([`config`]), then hand control to a
//!    plain blocking event loop.
//! 2. **State** — [`app`]. One `App` struct owns everything mutable: the tree
//!    cursor, the open buffers, the current mode, the status line. Every
//!    keypress goes through `App::on_key`.
//! 3. **Drawing** — [`ui`]. Reads `App` and paints a frame. It is deliberately
//!    the only module that knows about ratatui widgets.
//! 4. **Support** — [`tree`], [`editor`], [`search`], [`markdown`],
//!    [`highlight`], [`media`], [`graph`], [`projectmap`]. Each is a
//!    self-contained piece of machinery `app` and `ui` call into.
//!
//! # The one-way rule
//!
//! State flows one way: keys mutate `App`, then `ui` reads `App` and draws.
//! `ui` never handles input, and `app` never touches a ratatui widget. The one
//! sanctioned exception is that `ui` writes back scroll offsets and the media
//! cache, because those genuinely cannot be computed until the pane size is
//! known — and the pane size is only known while drawing. If you find yourself
//! wanting a second exception, that is usually a sign the state belongs on
//! `App` instead.
//!
//! # The event loop is blocking on purpose
//!
//! There is no tick, no timer, and no background thread. `run` blocks in
//! `event::read()` until the user does something, then redraws once. An idle
//! tiny uses no CPU at all, which is most of why it stays small. The cost is
//! that every operation is synchronous: a slow search or a big graph build
//! freezes the UI while it runs, so anything expensive needs its own budget
//! (see the size caps in `search`, `graph`, and `highlight`).

mod app;
mod config;
mod editor;
mod graph;
mod highlight;
mod markdown;
mod media;
mod project;
mod projectmap;
mod search;
mod tree;
mod ui;

use anyhow::{Result, anyhow};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use crossterm::execute;

use app::App;
use config::{CONF_NAME, Config};

/// `--help` output. Kept here as one literal rather than assembled from an
/// argument parser: there are only four options, and a contributor changing
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

tiny writes nothing into a folder of yours except a starting `README.md`, and
only into one it just created or found empty. Turn even that off with
`starter_readme = false`.

KEYS:
    ?                   every key and command, from inside the app
    /                   the bar: searches, or `*` first for a command
    w                   the project map
";

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
/// 2. Load the config, which [`project::resolve`] needs for `starter_readme`
///    and `default_root` before it can decide what to do with the argument.
/// 3. Resolve the argument into a [`project::Target`], creating the project
///    if it does not exist.
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
            print!("{USAGE}");
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
        Some(a) if a.starts_with('-') => {
            return Err(anyhow!("unknown option `{a}` — try `tiny --help`"));
        }
        _ => {}
    }

    let (cfg, warn_config) = Config::load();
    let target = project::resolve(args.first().map(String::as_str), &cfg)?;

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
