//! The clipboard for text: the system's, wherever tiny can reach it, and a
//! copy of its own.
//!
//! Text copied in a file goes to the system's clipboard, so it pastes into
//! anything, and tiny keeps it as well, for two reasons. A machine without a
//! way in (a server over SSH, a desktop without the clipboard programs) still
//! copies and pastes inside tiny. And tiny remembers what *kind* of text it
//! was: copying with nothing selected takes whole lines, which paste back in
//! above the line rather than into the middle of it, and the system's
//! clipboard has no way to carry that.
//!
//! # Reaching the system's
//!
//! There is no one way. Each system has its own, tried in turn:
//!
//! - **Windows:** its own clipboard, through the Windows API.
//! - **macOS:** `pbcopy` and `pbpaste`, which every Mac has.
//! - **Linux and the BSDs:** `wl-copy` and `wl-paste` under Wayland, `xclip`
//!   or `xsel` under X. Whichever is installed.
//! - **Anywhere else**, including over SSH: the terminal itself, with the
//!   escape sequence (OSC 52) that asks it to set the clipboard. Most
//!   terminals do it (kitty, WezTerm, foot, Alacritty, iTerm2, Windows
//!   Terminal, tmux with `set-clipboard on`) and the rest ignore it. There is
//!   no reading the clipboard back this way, so a paste there is tiny's own
//!   copy, or the terminal's paste key.
//!
//! The tests never touch any of it: a clipboard made with `system` false is
//! tiny's own alone, so running them does not overwrite what the person
//! running them had copied.

use std::io::Write;

#[derive(Debug, Default)]
pub struct Clipboard {
    /// The last text copied in tiny, and whether it was whole lines.
    last: Option<(String, bool)>,
    /// Whether to use the system's clipboard at all.
    system: bool,
}

impl Clipboard {
    pub fn new(system: bool) -> Self {
        Self { last: None, system }
    }

    /// Copy `text`, which is whole lines when `whole_lines` is set.
    pub fn copy(&mut self, text: String, whole_lines: bool) {
        if self.system && !set_system(&text) {
            osc52(&text);
        }
        self.last = Some((text, whole_lines));
    }

    /// What a paste puts in: the system's clipboard when it can be read and
    /// holds something, otherwise tiny's own copy. The flag says whether it
    /// is whole lines, which it is only when it is exactly what tiny copied
    /// as whole lines.
    pub fn paste(&self) -> Option<(String, bool)> {
        let system = self
            .system
            .then(get_system)
            .flatten()
            .filter(|t| !t.is_empty());
        match system {
            Some(text) => {
                let whole = self.whole_lines(&text);
                Some((text, whole))
            }
            None => self.last.clone(),
        }
    }

    /// Whether `text`, pasted by the terminal or read from the system, is
    /// the whole lines tiny last copied. Line breaks are compared as `\n`,
    /// since a Windows clipboard gives them back as `\r\n`.
    pub fn whole_lines(&self, text: &str) -> bool {
        match &self.last {
            Some((last, true)) => {
                let plain = |s: &str| s.replace("\r\n", "\n");
                plain(last) == plain(text)
            }
            _ => false,
        }
    }
}

/// Set the system's clipboard to `text`. Returns whether something took it.
fn set_system(text: &str) -> bool {
    #[cfg(windows)]
    {
        win::set(text)
    }
    #[cfg(not(windows))]
    {
        programs::all()
            .iter()
            .any(|(copy, _)| programs::run_with_input(copy, text))
    }
}

/// The system's clipboard, when it can be read.
fn get_system() -> Option<String> {
    #[cfg(windows)]
    {
        win::get()
    }
    #[cfg(not(windows))]
    {
        programs::all()
            .iter()
            .find_map(|(_, paste)| programs::run_for_output(paste))
    }
}

/// Everything but Windows: a program for each way in, see the module docs.
#[cfg(not(windows))]
mod programs {
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// How long a clipboard program gets to answer. One that hangs (a
    /// Wayland compositor that has gone, an X server across a dead SSH link)
    /// costs this much once, rather than freezing tiny.
    const PATIENCE: Duration = Duration::from_millis(1500);

    /// The clipboard programs worth trying here, each as the command that copies
    /// and the one that pastes, most likely first.
    pub fn all() -> Vec<(&'static [&'static str], &'static [&'static str])> {
        if cfg!(target_os = "macos") {
            return vec![(&["pbcopy"], &["pbpaste"])];
        }
        let mut out: Vec<(&'static [&'static str], &'static [&'static str])> = Vec::new();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            out.push((
                &["wl-copy"],
                &["wl-paste", "--no-newline", "--type", "text"],
            ));
        }
        if std::env::var_os("DISPLAY").is_some() {
            out.push((
                &["xclip", "-selection", "clipboard", "-in"],
                &["xclip", "-selection", "clipboard", "-out"],
            ));
            out.push((
                &["xsel", "--clipboard", "--input"],
                &["xsel", "--clipboard", "--output"],
            ));
        }
        out
    }

    /// Run `argv` with `text` on its standard input. Whether it ran and said
    /// yes.
    ///
    /// Its output goes nowhere rather than to a pipe: `xclip` and `wl-copy`
    /// leave a process behind to hand the text to whoever pastes it, and a pipe
    /// it held open would be a pipe never closed.
    pub fn run_with_input(argv: &[&str], text: &str) -> bool {
        let Ok(mut child) = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        matches!(wait(&mut child), Some(true))
    }

    /// Run `argv` and read what it prints, when it says yes in time.
    pub fn run_for_output(argv: &[&str]) -> Option<String> {
        let mut child = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // Read on a thread of its own: a large clipboard fills the pipe, and a
        // program waiting for its pipe to be read never exits.
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        if wait(&mut child) != Some(true) {
            return None;
        }
        String::from_utf8(reader.join().ok()?).ok()
    }

    /// Wait for `child`, at most [`PATIENCE`]. Whether it succeeded; `None` when
    /// it took too long and was stopped.
    fn wait(child: &mut Child) -> Option<bool> {
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status.success()),
                Ok(None) if start.elapsed() < PATIENCE => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }
}

/// Ask the terminal to set the clipboard, see the module docs.
///
/// Written straight to the terminal between frames: it moves no cursor and
/// draws nothing, so the screen is none the wiser.
fn osc52(text: &str) {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

/// Standard base64, with padding: what OSC 52 carries.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The Windows clipboard, through the API: text in UTF-16, the one form
/// every Windows program reads.
#[cfg(windows)]
mod win {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    pub fn set(text: &str) -> bool {
        // Windows' own line break, and the terminating zero it expects.
        let wide: Vec<u16> = text
            .replace("\r\n", "\n")
            .replace('\n', "\r\n")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: the clipboard is opened and closed on every path, and the
        // memory handed to it is ours until `SetClipboardData` takes it.
        unsafe {
            if OpenClipboard(null_mut()) == 0 {
                return false;
            }
            EmptyClipboard();
            let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            let mut done = false;
            if !memory.is_null() {
                let to = GlobalLock(memory).cast::<u16>();
                if !to.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), to, wide.len());
                    GlobalUnlock(memory);
                    done = !SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null();
                }
                if !done {
                    GlobalFree(memory);
                }
            }
            CloseClipboard();
            done
        }
    }

    pub fn get() -> Option<String> {
        // SAFETY: as above; the data is only read while it is locked, and
        // only up to its terminating zero.
        unsafe {
            if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) == 0
                || OpenClipboard(null_mut()) == 0
            {
                return None;
            }
            let memory = GetClipboardData(u32::from(CF_UNICODETEXT));
            let mut text = None;
            if !memory.is_null() {
                let from = GlobalLock(memory).cast::<u16>();
                if !from.is_null() {
                    let mut n = 0;
                    while *from.add(n) != 0 {
                        n += 1;
                    }
                    let wide = std::slice::from_raw_parts(from, n);
                    text = Some(String::from_utf16_lossy(wide).replace("\r\n", "\n"));
                    GlobalUnlock(memory);
                }
            }
            CloseClipboard();
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_the_standard_kind() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("hello, world", "aGVsbG8sIHdvcmxk"),
            ("é", "w6k="),
        ] {
            assert_eq!(base64(plain.as_bytes()), coded, "{plain:?}");
        }
    }

    #[test]
    fn without_the_system_tiny_keeps_its_own_copy() {
        let mut clip = Clipboard::new(false);
        assert_eq!(clip.paste(), None, "nothing copied yet");
        clip.copy("a line\n".into(), true);
        assert_eq!(clip.paste(), Some(("a line\n".to_string(), true)));
        clip.copy("a word".into(), false);
        assert_eq!(clip.paste(), Some(("a word".to_string(), false)));
    }

    #[test]
    fn whole_lines_are_known_again_whatever_their_line_breaks() {
        let mut clip = Clipboard::new(false);
        clip.copy("one\ntwo\n".into(), true);
        assert!(
            clip.whole_lines("one\r\ntwo\r\n"),
            "as Windows hands it back"
        );
        assert!(!clip.whole_lines("one\ntwo"), "not quite the same text");
        clip.copy("one\ntwo\n".into(), false);
        assert!(!clip.whole_lines("one\ntwo\n"), "copied as a selection");
    }
}
