//! What a picture, a video, a PDF or a sound *is*, and how to hand it to the
//! desktop.
//!
//! tiny does not draw media. A terminal is a poor picture frame — half-block
//! rendering costs a decode and a scale on every resize, and what it buys you
//! is a thumbnail you cannot zoom, pan, or seek. A PDF is worse: pages of
//! laid-out type that a grid of characters can only approximate. Your desktop
//! already has a program that does each of these properly, so
//! [`open_with_desktop`] hands the file to it and the pane sticks to what a
//! terminal is actually good at: saying, in words, what the thing is.
//!
//! So the pane shows facts — format, resolution, running time, size — and
//! Enter opens the file in your own viewer.
//!
//! # Only things that are safe to open
//!
//! The list in [`classify`] is deliberately a list of *documents*. "Open with
//! the default program" is also how an executable, a script or an installer
//! gets run, and Enter on a file in a browser must never be that.
//!
//! # Everything here is cheap enough to run on a cursor move
//!
//! [`probe`] is called from `App::sync_preview`, which runs every time the
//! tree cursor lands on a new file, so it must not decode anything. Images go
//! through [`size`], which reads the few bytes of header that carry the
//! dimensions and stops. Video needs `ffprobe`, and that is a process spawn —
//! but it reads the container header and exits, where the old poster frame
//! decoded a whole frame and wrote a PNG to disk. Neither result is cached,
//! because neither is expensive enough to be worth invalidating.
//!
//! # ffprobe is optional
//!
//! It is a runtime dependency, never a build one. Without it a video still
//! previews — name, format and size all come from the filesystem — and the
//! pane says which tool would fill in the rest instead of pretending the
//! fields do not exist.

mod size;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use anyhow::{Context, Result};

/// What kind of file this is, decided from its extension alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    Pdf,
    Audio,
    /// Anything else — text, binaries, files with no extension.
    Other,
}

impl Kind {
    /// The word the pane uses for this kind, as in `PNG image`.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Pdf => "document",
            Kind::Audio => "audio",
            Kind::Other => "file",
        }
    }
}

/// Classify by extension, case-insensitively.
///
/// Extension-only on purpose: this runs on every cursor move in the tree, and
/// sniffing file contents would mean a read per keystroke. [`probe`] does look
/// at the real bytes, so a mislabeled file reports that rather than inventing
/// a resolution for it.
pub fn classify(path: &Path) -> Kind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico") => {
            Kind::Image
        }
        Some("mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg") => {
            Kind::Video
        }
        Some("pdf") => Kind::Pdf,
        Some("mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "aiff" | "aif") => {
            Kind::Audio
        }
        _ => Kind::Other,
    }
}

/// What the pane knows about a media file beyond its name and its size.
///
/// Every field is optional because every field can be unavailable for a
/// reason worth distinguishing: a truncated PNG has no dimensions, a machine
/// without ffprobe has no video anything. `note` carries that reason, and is
/// shown *underneath* the facts that were readable rather than in place of
/// them — a video whose length is unknown still has a format and a size.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    /// Pixel dimensions, when they could be read.
    pub dimensions: Option<(u32, u32)>,
    /// Running time in seconds. Video only.
    pub duration: Option<f64>,
    /// Why something above is missing.
    pub note: Option<String>,
}

impl Info {
    /// `1920 × 1080`, ready to print.
    pub fn resolution(&self) -> Option<String> {
        self.dimensions.map(|(w, h)| format!("{w} × {h}"))
    }

    /// `4:07`, or `1:02:07` once it passes an hour. Sub-second clips round up
    /// to `0:01` rather than reading `0:00`, which looks like a failure.
    pub fn runtime(&self) -> Option<String> {
        let secs = self.duration.filter(|d| d.is_finite() && *d >= 0.0)?;
        let total = (secs.round() as u64).max(1);
        let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
        Some(if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        })
    }
}

/// The extension, upper-cased, for the caption. `"file"` when there is none.
pub fn format_name(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_else(|| "file".into())
}

/// Read what can be read about a media file without decoding it.
///
/// Never fails: an unreadable file comes back as an `Info` whose `note`
/// explains why it is empty, because the pane always has something to draw —
/// at minimum the name and the size, which came from the directory listing.
pub fn probe(path: &Path, kind: Kind) -> Info {
    match kind {
        Kind::Image => match size::read(path) {
            Ok(dimensions) => Info {
                dimensions: Some(dimensions),
                ..Info::default()
            },
            Err(e) => Info {
                note: Some(format!("{e:#}")),
                ..Info::default()
            },
        },
        Kind::Video | Kind::Audio => stream_info(path, kind),
        Kind::Pdf => pdf_info(path),
        Kind::Other => Info::default(),
    }
}

/// Whether a PDF really is one: the format puts `%PDF-` in its first kilobyte,
/// and a file called `.pdf` without it is going to disappoint whatever it is
/// handed to. Said in a note rather than refused, the way a mislabeled picture
/// is — the viewer may still make something of it.
fn pdf_info(path: &Path) -> Info {
    use std::io::Read;
    let mut head = Vec::with_capacity(1024);
    let read = std::fs::File::open(path).and_then(|f| f.take(1024).read_to_end(&mut head));
    let note = match read {
        Ok(_) if head.windows(5).any(|w| w == b"%PDF-") => None,
        Ok(_) => Some(format!("{} does not start like a PDF", name(path))),
        Err(e) => Some(format!("cannot read {}: {e}", name(path))),
    };
    Info {
        note,
        ..Info::default()
    }
}

/// Resolution and running time, via ffprobe — of a video, or the running time
/// alone of a sound, which has no picture to measure.
///
/// One invocation asks for both, in ffprobe's flat `key=value` output — the
/// simplest of its formats to parse and the only one that needs no JSON. A
/// stream with no duration reports `N/A`, which parses as `None` and reads as
/// "unknown" rather than as zero.
fn stream_info(path: &Path, kind: Kind) -> Info {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output();
    let Ok(output) = output else {
        let what = if kind == Kind::Audio {
            "its length"
        } else {
            "size and length"
        };
        return Info {
            note: Some(format!("install ffprobe (part of ffmpeg) to see {what}")),
            ..Info::default()
        };
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let dimensions = match (
        field("width").and_then(|v| v.parse().ok()),
        field("height").and_then(|v| v.parse().ok()),
    ) {
        (Some(w), Some(h)) => Some((w, h)),
        _ => None,
    };
    let duration = field("duration").and_then(|v| v.parse::<f64>().ok());
    let note = (dimensions.is_none() && duration.is_none())
        .then(|| "ffprobe could not read this file".to_string());
    Info {
        dimensions,
        duration,
        note,
    }
}

/// Viewers we have launched and not yet reaped.
///
/// `Child::drop` does not wait, so every launched viewer would sit in the
/// process table as a zombie until tiny itself exited. Waiting at launch is
/// not an option either: `xdg-open` sometimes runs the viewer in the
/// foreground, and that would freeze the editor until the picture was closed.
/// So each launch first clears out whichever earlier ones have since finished.
/// The list holds only the viewers still open, which is a number the user is
/// looking at.
static LAUNCHED: Mutex<Vec<Child>> = Mutex::new(Vec::new());

/// Hand `path` to whatever the desktop opens that kind of file with.
///
/// Returns as soon as the launcher is running, not when the viewer closes —
/// see [`LAUNCHED`]. A failure here means the launcher itself could not be
/// run, which on a bare server usually means there is no desktop to hand it
/// to; that is worth reporting, and it is the only thing this can detect,
/// since the launcher exits long before the viewer decides whether it liked
/// the file.
pub fn open_with_desktop(path: &Path) -> Result<()> {
    // The tests press the key that opens a file. They must not open one on the
    // machine running them, so in a test build the request is written down for
    // the test to read instead.
    if cfg!(test) {
        opened::record(path);
        return Ok(());
    }
    let child = launcher(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not open {}", name(path)))?;
    keep_until_done(child);
    Ok(())
}

/// Hold on to a program tiny started and does not wait for, so that it is
/// reaped once it finishes rather than lingering — see [`LAUNCHED`]. The
/// desktop's viewers come through here, and so do the windows `Ctrl+N` opens.
pub fn keep_until_done(child: Child) {
    if let Ok(mut open) = LAUNCHED.lock() {
        open.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
        open.push(child);
    }
}

/// What a test build hands to the desktop: nothing, written down.
#[cfg(test)]
pub mod opened {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static OPENED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record(path: &Path) {
        OPENED.with(|o| o.borrow_mut().push(path.to_path_buf()));
    }

    /// Every file this test thread asked the desktop to open, oldest first,
    /// and forget them.
    pub fn take() -> Vec<PathBuf> {
        OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
    }
}

/// In a real build there is nobody to tell.
#[cfg(not(test))]
mod opened {
    pub(super) fn record(_: &std::path::Path) {}
}

/// The platform's "open this with the right program" command.
///
/// Split out from [`open_with_desktop`] so the argument list can be tested
/// without launching anything.
fn launcher(path: &Path) -> Command {
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        // `start` is a shell builtin, not a program, so it needs cmd. The
        // empty string is `start`'s title argument: without it the first
        // quoted argument is taken as the window title and the file is never
        // opened.
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", ""]);
        cmd
    } else {
        Command::new("xdg-open")
    };
    cmd.arg(path);
    cmd
}

/// The file name, for messages. Falls back to the whole path.
fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A PNG header and nothing else. Only the header is ever read — see
    /// [`size`] — so there are no pixels to invent.
    fn write_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        let path = dir.join(name);
        std::fs::write(&path, v).unwrap();
        path
    }

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify(Path::new("a.png")), Kind::Image);
        assert_eq!(classify(Path::new("a.JPG")), Kind::Image);
        assert_eq!(classify(Path::new("a.mp4")), Kind::Video);
        assert_eq!(classify(Path::new("a.mkv")), Kind::Video);
        assert_eq!(classify(Path::new("a.rs")), Kind::Other);
        assert_eq!(classify(Path::new("noext")), Kind::Other);
        assert_eq!(classify(Path::new("paper.PDF")), Kind::Pdf);
        assert_eq!(classify(Path::new("song.mp3")), Kind::Audio);
    }

    #[test]
    fn nothing_runnable_is_ever_handed_to_the_desktop() {
        // "Open with the default program" runs these, and Enter in a file
        // browser must not.
        for name in [
            "setup.exe",
            "run.sh",
            "tool.bat",
            "Installer.app",
            "a.command",
            "x.msi",
            "p.ps1",
            "lib.dll",
            "noext",
        ] {
            assert_eq!(classify(Path::new(name)), Kind::Other, "{name}");
        }
    }

    #[test]
    fn a_pdf_is_recognized_by_its_header() {
        let td = tempfile::tempdir().unwrap();
        let real = td.path().join("paper.pdf");
        std::fs::write(&real, b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj").unwrap();
        assert_eq!(probe(&real, Kind::Pdf), Info::default(), "nothing to say");

        let fake = td.path().join("fake.pdf");
        std::fs::write(&fake, b"just some words").unwrap();
        let note = probe(&fake, Kind::Pdf).note.expect("a reason is given");
        assert!(note.contains("fake.pdf"), "{note}");
    }

    #[test]
    fn a_picture_reports_its_real_size() {
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "grad.png", 640, 480);
        let info = probe(&p, Kind::Image);
        assert_eq!(info.dimensions, Some((640, 480)));
        assert_eq!(info.resolution().as_deref(), Some("640 × 480"));
        assert_eq!(info.note, None, "nothing went wrong");
    }

    #[test]
    fn the_size_is_read_from_the_header_not_the_extension() {
        // A PNG wearing a .jpg suffix still reports its true dimensions,
        // because the format is read from the bytes.
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "liar.jpg", 33, 17);
        assert_eq!(probe(&p, Kind::Image).dimensions, Some((33, 17)));
    }

    #[test]
    fn a_file_that_is_not_an_image_explains_itself_and_still_previews() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("fake.png");
        std::fs::write(&p, b"this is not a png").unwrap();
        let info = probe(&p, Kind::Image);
        assert_eq!(info.dimensions, None);
        let note = info.note.expect("a reason is given");
        assert!(note.contains("fake.png"), "the note names the file: {note}");
    }

    #[test]
    fn a_video_tiny_cannot_read_still_says_something_useful() {
        // With no ffprobe the note names the tool; with ffprobe it fails on
        // the bogus contents instead. Either way the pane is not left blank.
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("clip.mp4");
        std::fs::write(&p, b"not really a video").unwrap();
        let info = probe(&p, Kind::Video);
        assert_eq!(info.dimensions, None);
        assert!(info.note.is_some(), "an empty probe explains itself");
    }

    #[test]
    fn runtimes_read_as_clock_times() {
        let at = |secs| {
            Info {
                duration: Some(secs),
                ..Info::default()
            }
            .runtime()
        };
        assert_eq!(at(0.4).as_deref(), Some("0:01"), "a flicker is not 0:00");
        assert_eq!(at(9.0).as_deref(), Some("0:09"));
        assert_eq!(at(247.0).as_deref(), Some("4:07"));
        assert_eq!(at(3727.0).as_deref(), Some("1:02:07"));
        assert_eq!(Info::default().runtime(), None, "unknown stays unknown");
        assert_eq!(at(f64::NAN), None, "and so does nonsense");
    }

    #[test]
    fn a_probe_with_nothing_to_say_says_nothing() {
        assert_eq!(probe(Path::new("notes.md"), Kind::Other), Info::default());
    }

    #[test]
    fn the_launcher_hands_the_whole_path_to_the_platforms_opener() {
        let path = Path::new("/tmp/a picture.png");
        let cmd = launcher(path);
        let program = cmd.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            matches!(program.as_str(), "open" | "xdg-open" | "cmd"),
            "unexpected launcher {program}"
        );
        // The path goes last and goes whole — a space in a file name must not
        // become two arguments.
        assert_eq!(args.last().map(String::as_str), Some("/tmp/a picture.png"));
    }

    #[test]
    fn format_names_come_from_the_extension() {
        assert_eq!(format_name(Path::new("a.png")), "PNG");
        assert_eq!(format_name(Path::new("a.MP4")), "MP4");
        assert_eq!(format_name(Path::new("plain")), "file");
    }
}
