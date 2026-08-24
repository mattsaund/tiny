//! tiny — a terminal knowledge manager.
//!
//! Left pane is the project tree, right pane is whatever the cursor is on:
//! markdown renders, code opens in an editor, pictures draw. `Ctrl+S` saves.

mod app;
mod config;
mod editor;
mod graph;
mod highlight;
mod markdown;
mod media;
mod project;
mod search;
mod tree;
mod ui;
mod web;

use std::time::Duration;

use anyhow::{Result, anyhow};
use crossterm::event::{self, Event, KeyEventKind};

use app::App;
use config::{CONF_NAME, Config};

const USAGE: &str = "\
tiny — a terminal knowledge manager

USAGE:
    tiny                open the current directory, or the project it is inside
    tiny <PATH>         open a folder; a path that does not exist is created
    tiny <FILE>         open the file's folder, with the file already in the editor

OPTIONS:
    -h, --help          show this message
    -V, --version       show the version
        --config        print the path of the config file

Any folder opened without a project gets one: a `.tiny/` directory, and
nothing else. Turn that off with `auto_init = false`.

KEYS:
    ?                   the full keymap, from inside the app
    /                   search the project
    :                   commands and settings
";

fn main() {
    if let Err(e) = real_main() {
        eprintln!("tiny: {e:#}");
        std::process::exit(1);
    }
}

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

    // The project has to be located before its own config can be read, so the
    // user config is loaded first and the project's file layered on after.
    let (user_cfg, warn_user) = Config::load(None);
    let target = project::resolve(args.first().map(String::as_str), &user_cfg)?;
    let (cfg, warn_project) = Config::load(Some(&target.root));

    // First run: write the defaults out so there is a file to edit. Done here
    // rather than in `load` so `--help` never touches the disk.
    let warn_first_run = match Config::user_path() {
        Some(p) if !p.exists() => match cfg.save() {
            Ok(p) => Some(format!("wrote {} to {}", CONF_NAME, p.display())),
            Err(e) => Some(format!("could not write {CONF_NAME}: {e}")),
        },
        _ => None,
    };

    let warning = warn_project.or(warn_user).or(warn_first_run);
    let mut app = App::new(target, cfg, warning)?;

    // A clean message beats a panic when there is no terminal to draw on —
    // piping the binary or running it from a script hits this.
    let mut terminal = ratatui::try_init().map_err(|e| {
        anyhow!("no interactive terminal available ({e}); tiny needs a real terminal")
    })?;
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    // Draw only when something changed. The loop still has to wake up while
    // the web view is open, so a click in the browser can move the cursor
    // here; with no web view it just blocks on the keyboard.
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            dirty = false;
        }
        if app.should_quit {
            return Ok(());
        }
        let wait = if app.web.is_some() {
            Duration::from_millis(120)
        } else {
            Duration::from_secs(3600)
        };
        if event::poll(wait)? {
            match event::read()? {
                // Windows terminals report releases too; only presses, and the
                // repeats from holding a key down, should reach the app.
                Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                    app.on_key(k);
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
        if let Some(path) = app.take_web_open() {
            app.open_path(&path);
            dirty = true;
        }
    }
}
