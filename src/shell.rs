//! A shell in a pane: the terminal built into tiny.
//!
//! A pseudo-terminal runs the shell (yours from `$SHELL`, or PowerShell on
//! Windows) in the project's folder, a thread reads what it writes, and a
//! terminal emulator, `vt100`, keeps the screen that writing draws, which
//! `ui` paints into the pane. Keys typed into the pane go back to the shell
//! spelled the way a terminal spells them, see [`bytes_for`].
//!
//! # Answering questions
//!
//! A program can ask its terminal things, and some wait for the answer
//! before going on: where the cursor is (Windows' ConPTY asks that before
//! it draws anything at all), and what kind of terminal this is. The
//! emulator does not answer for itself, so [`Answers`] does.
//!
//! # Saying it is somewhere else
//!
//! tiny's own terminal is kitty, or iTerm2, or Windows Terminal, and says so
//! in variables the shell would inherit. A program that believed them would
//! speak kitty's graphics or iTerm2's escapes to an emulator that knows
//! neither, so those are taken away, and `TERM` says what this is: an xterm,
//! more or less.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How far back the screen is kept, in lines, for the wheel to scroll to.
const SCROLLBACK: usize = 5000;

/// The variables tiny's own terminal sets that are not true of this one, see
/// the module docs.
const OTHER_TERMINALS: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "WEZTERM_PANE",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_UNIX_SOCKET",
    "KONSOLE_VERSION",
    "KONSOLE_DBUS_SESSION",
    "VTE_VERSION",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "GHOSTTY_RESOURCES_DIR",
    "ITERM_SESSION_ID",
    "ITERM_PROFILE",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "TERMINAL_EMULATOR",
    "ConEmuANSI",
];

/// One shell, running.
pub struct Shell {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: vt100::Parser<Answers>,
    /// What the shell wrote, from the thread reading it.
    output: Receiver<Vec<u8>>,
    /// Rows and columns, as the pseudo-terminal was last told.
    size: (u16, u16),
    /// The shell has exited.
    pub done: bool,
}

/// What the emulator does not do itself, see the module docs.
#[derive(Default)]
struct Answers {
    /// Replies waiting to be written back to the shell.
    replies: Vec<u8>,
    /// The title the shell gave its window, which names the pane.
    title: Option<String>,
}

impl vt100::Callbacks for Answers {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        _second: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let n = params.first().and_then(|p| p.first()).copied();
        match (first, c, n) {
            // Where the cursor is.
            (None, 'n', Some(6)) => {
                let (row, col) = screen.cursor_position();
                let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                self.replies.extend_from_slice(reply.as_bytes());
            }
            // Whether all is well.
            (None, 'n', Some(5)) => self.replies.extend_from_slice(b"\x1b[0n"),
            // What kind of terminal: a VT220 with color, as xterm says.
            (None, 'c', None | Some(0)) => self.replies.extend_from_slice(b"\x1b[?62;22c"),
            (Some(b'>'), 'c', _) => self.replies.extend_from_slice(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let title = String::from_utf8_lossy(title).trim().to_string();
        self.title = (!title.is_empty()).then_some(title);
    }
}

fn nonzero(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

impl Shell {
    /// Start a shell in `folder`, `rows` by `cols`.
    pub fn start(folder: &Path, rows: u16, cols: u16) -> Result<Self> {
        let (rows, cols) = (rows.max(2), cols.max(10));
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("could not open a terminal")?;
        let mut cmd = shell_command();
        cmd.cwd(folder);
        for var in OTHER_TERMINALS {
            cmd.env_remove(var);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pty
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("could not start a shell")?;
        // The shell has its own end now. Ours would keep the terminal open
        // after the shell exits, and the reader would never see the end.
        drop(pty.slave);
        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (send, output) = channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if send.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            master: pty.master,
            writer,
            child,
            parser: vt100::Parser::new_with_callbacks(
                nonzero(rows),
                nonzero(cols),
                SCROLLBACK,
                Answers::default(),
            ),
            output,
            size: (rows, cols),
            done: false,
        })
    }

    /// Take in whatever the shell has written since last asked, and notice
    /// it exiting. Returns whether there is anything new to draw.
    pub fn poll(&mut self) -> bool {
        let mut news = false;
        loop {
            match self.output.try_recv() {
                Ok(bytes) => {
                    self.parser.process(&bytes);
                    news = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    news |= !self.done;
                    self.done = true;
                    break;
                }
            }
        }
        let replies = std::mem::take(&mut self.parser.callbacks_mut().replies);
        if !replies.is_empty() {
            self.write(&replies);
        }
        // A ConPTY keeps its end open after the shell has gone, so the reader
        // never sees an end there: the shell itself is asked.
        if !self.done && matches!(self.child.try_wait(), Ok(Some(_))) {
            self.done = true;
            news = true;
        }
        news
    }

    /// Send a key to the shell. Back at the bottom, too, if the wheel had
    /// scrolled up: typing is about now.
    pub fn key(&mut self, key: KeyEvent) {
        let screen = self.parser.screen();
        let bytes = bytes_for(key, screen.application_cursor());
        if !bytes.is_empty() {
            self.parser.screen_mut().set_scrollback(0);
            self.write(&bytes);
        }
    }

    /// Hand the program in the terminal a mouse event at `column` and `row`
    /// of its screen, counted from 0, when it asked for the mouse, spelled
    /// the way it asked. Returns whether it asked: one that did not gets
    /// nothing, and the wheel scrolls back through it instead.
    pub fn mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        mods: KeyModifiers,
    ) -> bool {
        let screen = self.parser.screen();
        let (mode, encoding) = (
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding(),
        );
        if mode == vt100::MouseProtocolMode::None {
            return false;
        }
        if let Some(bytes) = mouse_bytes(mode, encoding, kind, column, row, mods) {
            self.write(&bytes);
        }
        true
    }

    /// Send pasted text, bracketed when the program reading it asked for
    /// that, so it knows a paste from typing.
    pub fn paste(&mut self, text: &str) {
        let mut bytes = Vec::new();
        let bracketed = self.parser.screen().bracketed_paste();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        // A terminal sends a line break as Enter, which is a carriage return.
        bytes.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.parser.screen_mut().set_scrollback(0);
        self.write(&bytes);
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Make the terminal `rows` by `cols`, when that is not what it is.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(2), cols.max(10));
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        self.parser
            .screen_mut()
            .set_size(nonzero(rows), nonzero(cols));
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Scroll back through what has gone off the top, or forward again.
    pub fn scroll(&mut self, up: bool, lines: usize) {
        let screen = self.parser.screen_mut();
        let now = screen.scrollback();
        screen.set_scrollback(if up {
            now + lines
        } else {
            now.saturating_sub(lines)
        });
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// The title the shell set, if it set one.
    pub fn title(&self) -> Option<&str> {
        self.parser.callbacks().title.as_deref()
    }
}

impl Drop for Shell {
    /// The shell goes with its pane, and with tiny.
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// The shell to run: yours on Unix, PowerShell on Windows when there is one.
fn shell_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        for name in ["pwsh.exe", "powershell.exe"] {
            if on_path(name) {
                return CommandBuilder::new(name);
            }
        }
    }
    CommandBuilder::new_default_prog()
}

/// Whether `name` is a program on the PATH.
#[cfg(windows)]
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// The bytes a terminal sends for `key`: what the shell, and whatever runs
/// in it, expects to read. `app_cursor` is the arrows' other spelling, which
/// a full-screen program asks for.
pub fn bytes_for(key: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);
    // The number xterm adds to a key's spelling for its modifiers.
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let mut out = Vec::new();
    let csi = |end: &str| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{end}").into_bytes()
        } else if app_cursor && "ABCDHF".contains(end) {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    match key.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                match control(c) {
                    Some(b) => out.push(b),
                    None => out.extend_from_slice(c.to_string().as_bytes()),
                }
            } else {
                out.extend_from_slice(c.to_string().as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out = csi("A"),
        KeyCode::Down => out = csi("B"),
        KeyCode::Right => out = csi("C"),
        KeyCode::Left => out = csi("D"),
        KeyCode::Home => out = csi("H"),
        KeyCode::End => out = csi("F"),
        KeyCode::Insert => out = tilde(2),
        KeyCode::Delete => out = tilde(3),
        KeyCode::PageUp => out = tilde(5),
        KeyCode::PageDown => out = tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let end = ["P", "Q", "R", "S"][usize::from(n - 1)];
            out = if m > 1 {
                format!("\x1b[1;{m}{end}").into_bytes()
            } else {
                format!("\x1bO{end}").into_bytes()
            };
        }
        KeyCode::F(n @ 5..=12) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)];
            out = tilde(code);
        }
        _ => {}
    }
    out
}

/// What a terminal sends for a mouse event, in the `mode` and `encoding`
/// the program reading it asked for, or nothing when the mode does not
/// report that kind of event.
fn mouse_bytes(
    mode: vt100::MouseProtocolMode,
    encoding: vt100::MouseProtocolEncoding,
    kind: MouseEventKind,
    column: u16,
    row: u16,
    mods: KeyModifiers,
) -> Option<Vec<u8>> {
    use vt100::{MouseProtocolEncoding, MouseProtocolMode};
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0u8,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    // Which button (a motion is 32 more), and whether it went down.
    let (code, down) = match kind {
        MouseEventKind::Down(b) if mode != MouseProtocolMode::None => (button(b), true),
        MouseEventKind::Up(b) if mode != MouseProtocolMode::Press => (button(b), false),
        MouseEventKind::Drag(b)
            if matches!(
                mode,
                MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
            ) =>
        {
            (button(b) + 32, true)
        }
        MouseEventKind::Moved if mode == MouseProtocolMode::AnyMotion => (35, true),
        MouseEventKind::ScrollUp => (64, true),
        MouseEventKind::ScrollDown => (65, true),
        _ => return None,
    };
    let code =
        code + if mods.contains(KeyModifiers::SHIFT) {
            4
        } else {
            0
        } + if mods.contains(KeyModifiers::ALT) {
            8
        } else {
            0
        } + if mods.contains(KeyModifiers::CONTROL) {
            16
        } else {
            0
        };
    let (x, y) = (u32::from(column) + 1, u32::from(row) + 1);
    if encoding == MouseProtocolEncoding::Sgr {
        let end = if down { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // The older spellings have no release of a button by name: a release
    // is button 3.
    let code = if down { code } else { (code & !3) | 3 };
    let mut out = b"\x1b[M".to_vec();
    for n in [u32::from(code) + 32, x + 32, y + 32] {
        if encoding == MouseProtocolEncoding::Utf8 {
            let c = char::from_u32(n).unwrap_or(' ');
            out.extend_from_slice(c.to_string().as_bytes());
        } else {
            out.push(n.min(255) as u8);
        }
    }
    Some(out)
}

/// The control character Ctrl and `c` make, when it makes one.
fn control(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn keys_are_spelled_as_a_terminal_spells_them() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        for (k, want) in [
            (key(KeyCode::Char('a'), none), &b"a"[..]),
            (key(KeyCode::Char('é'), none), "é".as_bytes()),
            (key(KeyCode::Char('c'), ctrl), b"\x03"),
            (key(KeyCode::Char('b'), alt), b"\x1bb"),
            (key(KeyCode::Enter, none), b"\r"),
            (key(KeyCode::Backspace, none), b"\x7f"),
            (key(KeyCode::Up, none), b"\x1b[A"),
            (key(KeyCode::Left, ctrl), b"\x1b[1;5D"),
            (key(KeyCode::Delete, none), b"\x1b[3~"),
            (key(KeyCode::PageUp, ctrl), b"\x1b[5;5~"),
            (key(KeyCode::F(1), none), b"\x1bOP"),
            (key(KeyCode::F(5), none), b"\x1b[15~"),
            (key(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z"),
        ] {
            assert_eq!(bytes_for(k, false), want, "{k:?}");
        }
        assert_eq!(
            bytes_for(key(KeyCode::Up, none), true),
            b"\x1bOA",
            "what a full-screen program asks for"
        );
    }

    #[test]
    fn the_mouse_is_spelled_the_way_the_program_asked() {
        use vt100::{MouseProtocolEncoding as E, MouseProtocolMode as M};
        let none = KeyModifiers::NONE;
        let left = MouseButton::Left;
        let spell = |m, e, kind| mouse_bytes(m, e, kind, 4, 2, none);
        assert_eq!(
            spell(M::PressRelease, E::Sgr, MouseEventKind::Down(left)).unwrap(),
            b"\x1b[<0;5;3M"
        );
        assert_eq!(
            spell(M::PressRelease, E::Sgr, MouseEventKind::Up(left)).unwrap(),
            b"\x1b[<0;5;3m"
        );
        assert_eq!(
            spell(M::Press, E::Default, MouseEventKind::Down(left)).unwrap(),
            b"\x1b[M\x20\x25\x23"
        );
        assert_eq!(
            spell(M::PressRelease, E::Default, MouseEventKind::Up(left)).unwrap(),
            b"\x1b[M\x23\x25\x23",
            "a release is button 3"
        );
        assert_eq!(
            spell(M::Press, E::Sgr, MouseEventKind::ScrollDown).unwrap(),
            b"\x1b[<65;5;3M"
        );
        assert_eq!(spell(M::Press, E::Sgr, MouseEventKind::Up(left)), None);
        assert_eq!(
            spell(M::PressRelease, E::Sgr, MouseEventKind::Drag(left)),
            None
        );
        assert_eq!(
            spell(M::ButtonMotion, E::Sgr, MouseEventKind::Drag(left)).unwrap(),
            b"\x1b[<32;5;3M"
        );
    }

    #[test]
    fn questions_a_program_waits_on_are_answered() {
        let mut parser =
            vt100::Parser::new_with_callbacks(nonzero(10), nonzero(40), 0, Answers::default());
        parser.process(b"abc\x1b[6n\x1b[c\x1b]2;~/notes\x07");
        let answers = parser.callbacks();
        assert_eq!(answers.replies, b"\x1b[1;4R\x1b[?62;22c");
        assert_eq!(answers.title.as_deref(), Some("~/notes"));
    }
}
