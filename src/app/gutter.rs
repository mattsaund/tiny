//! The marks beside the line numbers: a line new since the last commit in
//! green, a line changed since it in blue, and red where lines were taken
//! out. They are of the buffer, not the file on disk, so they move as you
//! type, and a commit takes them away.
//!
//! # What they are measured against
//!
//! The file as the last commit has it, which git is asked for on a thread
//! the first time the file is drawn: `git cat-file blob HEAD:./name`, in the
//! file's own folder, so the repository asked is whichever one the file is
//! in. Nothing is marked in a file the last commit does not have (a new one,
//! one git ignores, one outside a repository), and outside a repository git
//! is not even asked, see [`might_be_a_repository`].
//!
//! It is asked again once the last commit may be a different one: after
//! anything done in the git window, after `*reload`, and when the
//! repository's index, `HEAD` or the record of where `HEAD` has been moves,
//! which is a commit, an amend, a reset or a switch made by any program,
//! see [`super::marks`]. Until the answer comes the marks are the old ones,
//! so nothing flickers.
//!
//! The marks are worked out from that and the buffer, see
//! [`crate::text::linediff`], once per change to the buffer: a frame that
//! only moved the cursor reuses the last ones.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crate::git::run;
use crate::text::editor::line_ranges;
use crate::text::linediff::{self, LineMark, Lines};

use super::App;
use super::marks::might_be_a_repository;

/// Past this many lines a file is not marked: the last commit's copy is kept
/// beside the buffer, and a diff of each edit is a look at every line.
const MOST_LINES: usize = 100_000;

/// A file as the last commit has it: kept whole, with where each line is,
/// which is half what a string a line would take.
#[derive(Debug, PartialEq, Eq)]
struct Text {
    text: String,
    lines: Vec<(u32, u32)>,
}

impl Text {
    /// `text` split the way a buffer would split it, or nothing if it is too
    /// big to say where its lines are in four bytes.
    fn new(text: String) -> Option<Self> {
        u32::try_from(text.len()).ok()?;
        let lines = line_ranges(&text)
            .into_iter()
            .map(|r| (r.start as u32, r.end as u32))
            .collect();
        Some(Text { text, lines })
    }
}

impl Lines for Text {
    fn count(&self) -> usize {
        self.lines.len()
    }

    fn line(&self, i: usize) -> &str {
        let (start, end) = self.lines[i];
        &self.text[start as usize..end as usize]
    }
}

/// What git is sending back: the file as the last commit has it, or `None`
/// when it has no such file.
type Answer = Option<Text>;

/// What the last commit has of one file, and the marks worked out from it.
#[derive(Debug, Default)]
struct Committed {
    /// The file as the last commit has it, or `None` when it has no such
    /// file, or git has not said yet.
    lines: Answer,
    /// Git being asked, while it is.
    asking: Option<Receiver<Answer>>,
    /// The last commit may be a different one since git was asked: it is
    /// asked again the next time the file is drawn.
    stale: bool,
    /// The marks last worked out, and the version of the buffer they are
    /// for, see [`crate::text::editor::Editor::version`].
    marks: Option<(u64, Arc<[LineMark]>)>,
}

impl Committed {
    /// Take in what git said, if it has. Returns whether the lines changed.
    fn take_answer(&mut self) -> bool {
        let Some(rx) = &self.asking else {
            return false;
        };
        let answer = match rx.try_recv() {
            Err(TryRecvError::Empty) => return false,
            Ok(answer) => answer,
            // The thread is gone without a word: nothing to measure against.
            Err(TryRecvError::Disconnected) => None,
        };
        self.asking = None;
        if answer == self.lines {
            return false;
        }
        self.lines = answer;
        self.marks = None;
        true
    }
}

/// The marks beside the line numbers, for every file drawn.
#[derive(Debug, Default)]
pub struct Gutter {
    /// Off until `main` turns it on, like the browser's colors, so a test
    /// starts no thread it did not ask for.
    pub(super) on: bool,
    files: HashMap<PathBuf, Committed>,
}

/// Ask git, on a thread, for `path` as the last commit has it.
fn ask(path: &Path) -> Receiver<Answer> {
    let (tx, rx) = channel();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        // Nobody to tell if tiny has quit in the meantime.
        let _ = tx.send(committed(&path));
    });
    rx
}

/// `path` as the last commit has it, or `None` when it has no such file.
fn committed(path: &Path) -> Answer {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty())?;
    let name = path.file_name()?.to_str()?;
    let text = run::git(dir, &["cat-file", "blob", &format!("HEAD:./{name}")]).ok()?;
    Text::new(text)
}

impl App {
    /// The mark for each line of the buffer open at `path`, when there are
    /// any to draw: worked out again if the buffer has changed since they
    /// last were. `None` while git has not said what the last commit has,
    /// and for a file it has nothing of.
    pub(crate) fn line_marks(&mut self, path: &Path) -> Option<Arc<[LineMark]>> {
        if !self.gutter.on || !self.config.line_marks {
            return None;
        }
        let ed = self.buffers.get(path)?;
        if ed.line_count() > MOST_LINES {
            return None;
        }
        let file = self
            .gutter
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| Committed {
                stale: true,
                ..Committed::default()
            });
        file.take_answer();
        if file.stale && file.asking.is_none() {
            file.stale = false;
            if path.parent().is_some_and(might_be_a_repository) {
                file.asking = Some(ask(path));
            } else {
                file.lines = None;
                file.marks = None;
            }
        }
        let lines = file.lines.as_ref()?;
        let version = ed.version();
        match &file.marks {
            Some((made_for, marks)) if *made_for == version => Some(marks.clone()),
            _ => {
                let marks: Arc<[LineMark]> = linediff::marks(lines, ed.lines()).into();
                file.marks = Some((version, marks.clone()));
                Some(marks)
            }
        }
    }

    /// Take in whatever git has said since the last frame. Returns whether
    /// any of it changes what is drawn, for the event loop to draw it.
    ///
    /// Files no longer open are forgotten here too.
    pub fn poll_line_marks(&mut self) -> bool {
        let buffers = &self.buffers;
        self.gutter
            .files
            .retain(|path, _| buffers.contains_key(path));
        let mut news = false;
        for file in self.gutter.files.values_mut() {
            news |= file.take_answer();
        }
        news
    }

    /// Whether git is being asked for any file, for the event loop to look
    /// for the answer sooner than it otherwise would.
    pub fn line_marks_coming(&self) -> bool {
        self.gutter.files.values().any(|f| f.asking.is_some())
    }

    /// The last commit may be a different one: ask git again for each file
    /// as it is next drawn, see the module docs.
    pub(super) fn recommitted(&mut self) {
        for file in self.gutter.files.values_mut() {
            file.stale = true;
        }
    }

    /// Wait for git to say what the last commit has of `path`, and give back
    /// the marks: how a test sees them.
    #[cfg(test)]
    pub(super) fn wait_for_line_marks(&mut self, path: &Path) -> Option<Arc<[LineMark]>> {
        self.gutter.on = true;
        for _ in 0..1000 {
            let marks = self.line_marks(path);
            if !self.line_marks_coming() {
                return marks;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("git never answered");
    }
}
