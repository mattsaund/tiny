//! tiny in a terminal, as near to a real one as a test can get.
//!
//! The rest of the suite draws into ratatui's test backend, which shows what
//! tiny means to put on the screen. These run the binary itself in a
//! pseudo-terminal and read what comes back through a terminal emulator, so
//! they show the rest: that it starts, takes the screen, reads keys spelled
//! the ways terminals spell them, draws at whatever size it is given, follows
//! the window when it changes, and leaves the terminal as it found it.
//!
//! On Windows the pseudo-terminal is a ConPTY, the one Windows Terminal and
//! VS Code are built on, which turns the bytes typed here into the key events
//! a console program reads.
//!
//! # What varies, and why that is enough
//!
//! The terminals people use need a desktop to run, and CI has none. But what
//! tells one terminal from another, to a program, is small: the name it puts
//! in `TERM`, the size of the window, and how it spells each key. Those are
//! what change from test to test below. tmux and GNU screen, which are
//! terminals themselves and need no desktop, are run in the CI workflow.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How long anything may take to appear. Generous on purpose: a debug build
/// on a busy CI machine starts slowly, and a wait that ends early costs
/// nothing.
const PATIENCE: Duration = Duration::from_secs(30);

/// One tiny, running in a terminal of its own, on a small project of its own.
struct Tiny {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Shared with the reader, which answers what the program asks the
    /// terminal, as a terminal does.
    keys: Arc<Mutex<Box<dyn Write + Send>>>,
    screen: Arc<Mutex<vt100::Parser>>,
    reader: Option<JoinHandle<()>>,
    project: tempfile::TempDir,
    /// Where its config lives, so nobody's real one is read or written.
    _config: tempfile::TempDir,
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).expect("at least one")
}

/// Everything a terminal says about itself, which tiny reads to decide
/// what to draw with: none of it is taken from the terminal running the
/// tests, or what tiny draws would depend on where they run.
const TERMINAL_VARIABLES: &[&str] = &[
    "TERM",
    "COLORTERM",
    "TERM_PROGRAM",
    "NO_COLOR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "WT_SESSION",
    "TMUX",
    "STY",
    "KITTY_WINDOW_ID",
    "WEZTERM_PANE",
    "KONSOLE_VERSION",
    "VTE_VERSION",
    "ALACRITTY_WINDOW_ID",
    "GHOSTTY_RESOURCES_DIR",
    "TERMINAL_EMULATOR",
    "ConEmuANSI",
];

impl Tiny {
    /// tiny in a modern terminal called `term`, `cols` wide and `rows` high:
    /// UTF-8, and on Windows, Windows Terminal rather than the bare console.
    fn start(term: &str, cols: u16, rows: u16) -> Self {
        let mut env = vec![("TERM", term), ("LANG", "en_US.UTF-8")];
        if cfg!(windows) {
            env.push(("WT_SESSION", "tests"));
        }
        Self::start_in(cols, rows, &env)
    }

    /// tiny in a terminal that says exactly `env` about itself.
    fn start_in(cols: u16, rows: u16, env: &[(&str, &str)]) -> Self {
        Self::start_through(&[], cols, rows, env)
    }

    /// tiny started by `through` (a program and its arguments, to which
    /// tiny and its project are added) rather than by the terminal itself:
    /// a multiplexer, say, which is a terminal of its own in between.
    fn start_through(through: &[String], cols: u16, rows: u16, env: &[(&str, &str)]) -> Self {
        let project = tempfile::tempdir().unwrap();
        let p = project.path();
        std::fs::create_dir(p.join("src")).unwrap();
        std::fs::write(
            p.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        std::fs::write(p.join("notes.md"), "# Notes\n\nfirst line\n").unwrap();
        std::fs::write(p.join("README.md"), "# Hello\n\nThe readme.\n").unwrap();

        // `XDG_CONFIG_HOME` comes first wherever tiny looks for its config,
        // Windows included. No update check: nothing here should touch the
        // network.
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir(config.path().join("tiny")).unwrap();
        std::fs::write(
            config.path().join("tiny/tiny.conf"),
            "update_check = false\n",
        )
        .unwrap();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty = native_pty_system().openpty(size).unwrap();
        let tiny = env!("CARGO_BIN_EXE_tiny");
        let mut cmd = match through.split_first() {
            Some((program, args)) => {
                let mut cmd = CommandBuilder::new(program);
                cmd.args(args);
                cmd.arg(tiny);
                cmd
            }
            None => CommandBuilder::new(tiny),
        };
        cmd.arg(p);
        cmd.cwd(p);
        cmd.env("XDG_CONFIG_HOME", config.path());
        for var in TERMINAL_VARIABLES {
            cmd.env_remove(var);
        }
        for (var, value) in env {
            cmd.env(var, value);
        }
        let child = pty.slave.spawn_command(cmd).unwrap();
        // The child has its own end now. Ours would keep the terminal open
        // after tiny quits, so the reader below would never see the end.
        drop(pty.slave);

        let screen = Arc::new(Mutex::new(vt100::Parser::new(nz(rows), nz(cols), 0)));
        let keys = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
        let mut from_tiny = pty.master.try_clone_reader().unwrap();
        let into = Arc::clone(&screen);
        let answer = Arc::clone(&keys);
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                let n = match from_tiny.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut parser = into.lock().unwrap();
                parser.process(&buf[..n]);
                // Where the cursor is, when asked. Windows' ConPTY asks
                // before it shows anything at all, and waits for the answer.
                if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                    let (row, col) = parser.screen().cursor_position();
                    let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                    let mut keys = answer.lock().unwrap();
                    let _ = keys.write_all(reply.as_bytes());
                    let _ = keys.flush();
                }
            }
        });
        Self {
            master: pty.master,
            child,
            keys,
            screen,
            reader: Some(reader),
            project,
            _config: config,
        }
    }

    /// Type `bytes` the way a terminal would send them.
    fn send(&mut self, bytes: &str) {
        let mut keys = self.keys.lock().unwrap();
        keys.write_all(bytes.as_bytes()).unwrap();
        keys.flush().unwrap();
    }

    /// The screen as text, a line per row.
    fn text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    /// One row of the screen, every column of it.
    fn row(&self, n: u16) -> String {
        let parser = self.screen.lock().unwrap();
        let (_, cols) = parser.screen().size();
        parser
            .screen()
            .rows(0, cols.get())
            .nth(n as usize)
            .unwrap_or_default()
    }

    /// Wait for the screen to satisfy `ok`, and say what it showed if it
    /// never does.
    fn wait_until(&self, what: &str, ok: impl Fn(&Self) -> bool) {
        let start = Instant::now();
        while !ok(self) {
            if start.elapsed() > PATIENCE {
                panic!("never saw {what} on:\n{}", self.text());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for(&self, text: &str) {
        self.wait_until(&format!("{text:?}"), |t| t.text().contains(text));
    }

    /// Whether the screen comes to show `text` within `limit`, for what is
    /// worth knowing but not worth failing over.
    fn shows_within(&self, text: &str, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if self.text().contains(text) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// What the pane beside the browser is titled: whatever the browser's
    /// cursor is on, since the pane follows it.
    fn on(&self) -> String {
        let top = self.row(0);
        // Corners in either set of characters tiny draws with.
        let Some((_, after)) = top
            .split_once("\u{2510}\u{250c} ")
            .or_else(|| top.split_once("++ "))
        else {
            return String::new();
        };
        after
            .split([' ', '\u{2500}'])
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// Press a key and wait for the browser's cursor to land somewhere else.
    fn move_off(&mut self, key: &str) -> String {
        let from = self.on();
        self.send(key);
        self.wait_until(&format!("the cursor to leave {from}"), |t| {
            let now = t.on();
            !now.is_empty() && now != from
        });
        self.on()
    }

    /// The window changes size, as when someone drags its corner.
    fn resize(&mut self, cols: u16, rows: u16) {
        self.screen
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(nz(rows), nz(cols));
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
    }

    /// `Ctrl+Q`, then the terminal as tiny left it, once it has gone.
    fn quit(mut self) -> vt100::Screen {
        self.send("\x11");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() > PATIENCE {
                let _ = self.child.kill();
                panic!("tiny did not quit:\n{}", self.text());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success(), "tiny exited {status:?}:\n{}", self.text());
        // Everything it wrote on the way out, before reading the result. A
        // ConPTY keeps its end open until it is closed from this side, so on
        // Windows the reader is not waited for.
        if cfg!(unix)
            && let Some(reader) = self.reader.take()
        {
            let start = Instant::now();
            while !reader.is_finished() && start.elapsed() < PATIENCE {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.screen.lock().unwrap().screen().clone()
    }
}

/// The names terminals give themselves in `TERM`, from the oldest to the
/// newest. tiny reads none of them, so it has to start the same under all.
const TERMS: &[&str] = &[
    "xterm-256color",
    "xterm",
    "vt100",
    "linux",
    "screen-256color",
    "tmux-256color",
    "xterm-kitty",
    "alacritty",
    "wezterm",
    "foot",
    "dumb",
];

#[test]
fn it_starts_and_hands_the_terminal_back_under_every_name() {
    // A ConPTY is one terminal whatever `TERM` says, so there it runs once.
    let terms = if cfg!(windows) { &TERMS[..1] } else { TERMS };
    for term in terms {
        let tiny = Tiny::start(term, 100, 30);
        tiny.wait_for("BROWSER");
        tiny.wait_for("README.md");
        let screen = tiny.quit();
        if cfg!(unix) {
            assert!(
                !screen.alternate_screen(),
                "{term}: the shell's screen is back"
            );
            assert!(!screen.hide_cursor(), "{term}: the cursor is shown again");
            assert_eq!(
                screen.mouse_protocol_mode(),
                vt100::MouseProtocolMode::None,
                "{term}: the mouse is the terminal's again"
            );
        }
    }
}

#[test]
fn it_fills_the_window_at_any_size_and_follows_it_when_it_changes() {
    for (cols, rows) in [(40, 12), (80, 24), (200, 60)] {
        let mut tiny = Tiny::start("xterm-256color", cols, rows);
        // Not a file name: at forty columns the browser shortens them.
        tiny.wait_for("BROWSER");
        let filled = |t: &Tiny, cols: u16| {
            let top = t.row(0);
            top.starts_with('\u{250c}')
                && top.ends_with('\u{2510}')
                && top.chars().count() == cols as usize
        };
        tiny.wait_until(&format!("a frame {cols} wide"), |t| filled(t, cols));
        // Wider and taller, then narrower than it started.
        for (wider, taller) in [(cols + 30, rows + 6), (cols - 10, rows - 2)] {
            tiny.resize(wider, taller);
            tiny.wait_until(&format!("a frame {wider} wide"), |t| filled(t, wider));
            tiny.wait_until("the status line on the last row", |t| {
                t.row(taller - 1).contains("KiB") || t.row(taller - 1).contains(" B")
            });
        }
        tiny.quit();
    }
}

#[test]
fn a_window_too_small_to_draw_in_is_survived() {
    let mut tiny = Tiny::start("xterm-256color", 8, 3);
    std::thread::sleep(Duration::from_millis(500));
    // Still answering: grown back, it draws.
    tiny.resize(80, 24);
    tiny.wait_for("README.md");
    tiny.quit();
}

#[test]
fn the_arrows_work_however_the_terminal_spells_them() {
    let mut tiny = Tiny::start("xterm-256color", 100, 30);
    tiny.wait_for("README.md");
    let root = tiny.on();
    // CSI is what a terminal sends normally, SS3 what it sends once a
    // program has asked for the cursor keys' "application" mode.
    let first = tiny.move_off("\x1b[B");
    let second = tiny.move_off("\x1bOB");
    assert_ne!(first, second);
    assert_eq!(tiny.move_off("\x1bOA"), first);
    assert_eq!(tiny.move_off("\x1b[A"), root);
    tiny.quit();
}

#[test]
fn home_and_end_work_however_the_terminal_spells_them() {
    let mut tiny = Tiny::start("xterm-256color", 100, 30);
    tiny.wait_for("README.md");
    let root = tiny.on();
    // xterm's two spellings, the VT220's, and rxvt's. ConPTY reads the
    // common ones; the others only ever reach a program on Unix.
    let mut pairs = vec![
        ("\x1b[F", "\x1b[H"),
        ("\x1bOF", "\x1bOH"),
        ("\x1b[4~", "\x1b[1~"),
    ];
    if cfg!(unix) {
        pairs.push(("\x1b[8~", "\x1b[7~"));
    }
    let last = tiny.move_off(pairs[0].0);
    assert_eq!(tiny.move_off(pairs[0].1), root);
    for (end, home) in &pairs[1..] {
        assert_eq!(tiny.move_off(end), last, "end as {end:?}");
        assert_eq!(tiny.move_off(home), root, "home as {home:?}");
    }
    tiny.quit();
}

#[test]
fn typing_editing_and_saving_reach_the_file() {
    let mut tiny = Tiny::start("xterm-256color", 100, 30);
    tiny.wait_for("README.md");
    while tiny.on() != "notes.md" {
        tiny.move_off("\x1b[B");
    }
    tiny.send("\r");
    tiny.wait_for("EDIT");
    tiny.send("abcd");
    tiny.wait_for("abcd# Notes");
    // Backspace as nearly every terminal sends it, and as the ones that send
    // Ctrl+H do.
    tiny.send("\x7f");
    tiny.wait_for("abc# Notes");
    if cfg!(unix) {
        tiny.send("\x08");
        tiny.wait_for("ab# Notes");
        tiny.send("c");
    }
    tiny.send("\x13");
    tiny.wait_for("saved");
    let saved = std::fs::read_to_string(tiny.project.path().join("notes.md")).unwrap();
    assert!(saved.starts_with("abc# Notes"), "{saved:?}");
    // Esc on its own, back to the browser: the pane says VIEW again.
    tiny.send("\x1b");
    tiny.wait_for("VIEW");
    tiny.quit();
}

#[test]
fn the_function_and_control_keys_open_what_they_should() {
    let mut tiny = Tiny::start("xterm-256color", 100, 30);
    tiny.wait_for("README.md");
    // The help, and Esc to close it.
    let help = "any other key closes";
    if cfg!(windows) {
        // A ConPTY turns what it is sent into key events, and which of the
        // spellings of F1 it knows is its own business. `?` always arrives.
        tiny.send("?");
        tiny.wait_for(help);
        tiny.send("\x1b");
        tiny.wait_until("the help to close", |t| !t.text().contains(help));
        for (spelling, f1) in [("xterm's", "\x1bOP"), ("the VT220's", "\x1b[11~")] {
            tiny.send(f1);
            let opened = tiny.shows_within(help, Duration::from_secs(3));
            eprintln!("F1 spelled {spelling} way through a ConPTY: opened help {opened}");
            if opened {
                tiny.send("\x1b");
                tiny.wait_until("the help to close", |t| !t.text().contains(help));
            }
        }
    } else {
        // Both ways F1 is spelled: SS3 by xterm and most, CSI by the VT220
        // family.
        for f1 in ["\x1bOP", "\x1b[11~"] {
            tiny.send(f1);
            tiny.wait_for(help);
            tiny.send("\x1b");
            tiny.wait_until("the help to close", |t| !t.text().contains(help));
        }
    }
    // Ctrl+L, the project map, which also takes Shift+Tab as a terminal
    // sends it.
    tiny.send("\x0c");
    tiny.wait_for("PROJECT MAP");
    tiny.send("\x1b[Z");
    tiny.send("\x1b");
    tiny.wait_until("the map to close", |t| !t.text().contains("PROJECT MAP"));
    // Ctrl+W, two panes: two titles beside the browser's.
    tiny.send("\x17");
    tiny.wait_until("two panes", |t| t.row(0).matches('\u{250c}').count() == 3);
    tiny.quit();

    if cfg!(windows) {
        // F1 as Windows Terminal sends it to a ConPTY: a key event written
        // out in full, down and then up. In a ConPTY of its own, since once
        // one has read keys this way a lone Esc waits to be the start of
        // another, so Esc goes the same way.
        let mut tiny = Tiny::start("xterm-256color", 100, 30);
        tiny.wait_for("README.md");
        tiny.send("\x1b[112;59;0;1;0;1_\x1b[112;59;0;0;0;1_");
        tiny.wait_for(help);
        tiny.send("\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_");
        tiny.wait_until("the help to close", |t| !t.text().contains(help));
        tiny.quit();
    }
}

#[test]
fn wide_characters_and_emoji_keep_the_frame_square() {
    let mut tiny = Tiny::start("xterm-256color", 100, 30);
    std::fs::write(
        tiny.project.path().join("\u{65e5}\u{672c}\u{8a9e}.md"),
        "# \u{1f44d} wide \u{65e5}\u{672c}\n",
    )
    .unwrap();
    tiny.wait_for("\u{65e5}\u{672c}\u{8a9e}.md");
    while tiny.on() != "\u{65e5}\u{672c}\u{8a9e}.md" {
        tiny.move_off("\x1b[B");
    }
    tiny.wait_for("wide");
    // Every row ends where the frame does, however wide what is on it.
    let parser = tiny.screen.lock().unwrap();
    let screen = parser.screen();
    // The rows between the frame's top and bottom edges.
    for row in 1..28 {
        let last = screen
            .cell(row, 99)
            .map(|c| c.contents())
            .unwrap_or_default();
        assert_eq!(
            last,
            "\u{2502}",
            "row {row} ends in the frame:\n{}",
            screen.contents()
        );
    }
    drop(parser);
    tiny.quit();
}

// ---- what it adapts to ----------------------------------------------------

/// Whether the frame is drawn with line-drawing characters, once tiny has
/// drawn it: its top-left corner says.
fn draws_lines(tiny: &Tiny) -> bool {
    tiny.wait_until("a frame", |t| {
        t.row(0).starts_with('\u{250c}') || t.row(0).starts_with('+')
    });
    tiny.row(0).starts_with('\u{250c}')
}

#[test]
fn the_frame_is_drawn_in_characters_the_terminal_has() {
    let utf8 = ("LANG", "en_US.UTF-8");
    let cases: Vec<(Vec<(&str, &str)>, bool)> = if cfg!(windows) {
        vec![
            // The console Windows opens by itself, and Windows Terminal.
            (vec![], false),
            (vec![("WT_SESSION", "tests")], true),
        ]
    } else {
        vec![
            (vec![("TERM", "xterm-256color"), utf8], true),
            (
                vec![("TERM", "xterm-kitty"), ("KITTY_WINDOW_ID", "1"), utf8],
                true,
            ),
            // The Linux console's font, and a vt100's, have no such lines.
            (vec![("TERM", "linux"), utf8], false),
            (vec![("TERM", "vt100"), utf8], false),
            // Nor does a terminal that is not reading UTF-8.
            (vec![("TERM", "xterm-256color"), ("LANG", "C")], false),
        ]
    };
    for (env, lines) in cases {
        let tiny = Tiny::start_in(100, 30, &env);
        assert_eq!(draws_lines(&tiny), lines, "{env:?}:\n{}", tiny.text());
        tiny.quit();
    }
}

/// Every color on screen, foreground and background, once code is shown in
/// its syntax colors.
#[cfg(unix)]
fn colors_of_code(env: &[(&str, &str)]) -> Vec<vt100::Color> {
    let mut tiny = Tiny::start_in(100, 30, env);
    tiny.wait_for("README.md");
    while tiny.on() != "src" {
        tiny.move_off("\x1b[B");
    }
    tiny.send("\x1b[C");
    while tiny.on() != "main.rs" {
        tiny.move_off("\x1b[B");
    }
    tiny.wait_for("println");
    let colors = {
        let parser = tiny.screen.lock().unwrap();
        let screen = parser.screen();
        let mut all = Vec::new();
        for row in 0..30 {
            for col in 0..100 {
                if let Some(cell) = screen.cell(row, col) {
                    all.push(cell.fgcolor());
                    all.push(cell.bgcolor());
                }
            }
        }
        all
    };
    tiny.quit();
    colors
}

#[cfg(unix)]
#[test]
fn colors_come_down_to_what_the_terminal_shows() {
    use vt100::Color;
    let truecolor = colors_of_code(&[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]);
    assert!(
        truecolor.iter().any(|c| matches!(c, Color::Rgb(..))),
        "code in 24-bit color where the terminal has it"
    );
    let xterm = colors_of_code(&[("TERM", "xterm-256color")]);
    assert!(
        xterm.iter().all(|c| !matches!(c, Color::Rgb(..))),
        "the 256 of xterm's palette where it does not"
    );
    assert!(xterm.iter().any(|c| matches!(c, Color::Idx(n) if *n >= 16)));
    let console = colors_of_code(&[("TERM", "linux")]);
    assert!(
        console
            .iter()
            .all(|c| matches!(c, Color::Default) || matches!(c, Color::Idx(n) if *n < 16)),
        "the sixteen on a console"
    );
    let asked = colors_of_code(&[("TERM", "xterm-256color"), ("NO_COLOR", "1")]);
    assert!(
        asked.iter().all(|c| matches!(c, Color::Default)),
        "none at all when NO_COLOR asks"
    );
}

// ---- inside a multiplexer ---------------------------------------------------

/// Whether `program` can be run from here.
#[cfg(unix)]
fn installed(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

#[cfg(unix)]
#[test]
fn it_works_inside_tmux_and_screen() {
    // Each is a terminal of its own, between tiny and the one on screen: it
    // reads what tiny draws and draws it again, translates keys on their way
    // in, and gives tiny a name of its own in `TERM`. Skipped where one is
    // not installed; the CI workflow installs both.
    let socket = format!("tiny-tests-{}", std::process::id());
    // Without one, screen opens on a page of its own that waits for a key.
    // And Ctrl+S and Ctrl+Q go to the program, not to flow control, the way
    // anyone who uses them inside screen has it set up.
    let rc = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(rc.path(), "startup_message off\ndefflow off\n").unwrap();
    let rc = rc.path().to_string_lossy().into_owned();
    let multiplexers: [(&str, Vec<&str>); 2] = [
        // UTF-8 whatever the locale, no config, a server of its own.
        (
            "tmux",
            vec![
                "tmux",
                "-u",
                "-f",
                "/dev/null",
                "-L",
                &socket,
                "new-session",
            ],
        ),
        // UTF-8, a session of its own, and a terminal that leaves Ctrl+S and
        // Ctrl+Q alone too (`stty -ixon`), or it stops at the first Ctrl+S.
        (
            "screen",
            vec![
                "sh",
                "-c",
                "stty -ixon; exec \"$0\" \"$@\"",
                "screen",
                "-U",
                "-fn",
                "-c",
                &rc,
                "-S",
                &socket,
            ],
        ),
    ];
    for (name, through) in multiplexers {
        if !installed(name) {
            eprintln!("{name} is not installed here, so it is not tested");
            continue;
        }
        let through: Vec<String> = through.iter().map(|s| s.to_string()).collect();
        let mut tiny = Tiny::start_through(
            &through,
            100,
            30,
            &[("TERM", "xterm-256color"), ("LANG", "en_US.UTF-8")],
        );
        tiny.wait_for("BROWSER");
        tiny.wait_for("README.md");
        while tiny.on() != "notes.md" {
            tiny.move_off("\x1b[B");
        }
        tiny.send("\r");
        tiny.wait_for("EDIT");
        tiny.send("zz");
        tiny.wait_for("zz# Notes");
        tiny.send("\x13");
        tiny.wait_for("saved");
        let saved = std::fs::read_to_string(tiny.project.path().join("notes.md")).unwrap();
        assert!(saved.starts_with("zz# Notes"), "{name}: {saved:?}");
        tiny.quit();
        // A server or session left behind by a failure is not left running.
        let _ = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .stderr(std::process::Stdio::null())
            .status();
    }
}
