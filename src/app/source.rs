//! The source-control window: what it holds, and what its keys do.
//!
//! [`Window::Source`](super::mode::Window) draws a list of changes on the left
//! and the file under the cursor before and after on the right. This file owns
//! the state behind both, and the handful of actions that change it.
//!
//! # It is a picture of git, taken at a moment
//!
//! Nothing here watches anything. [`App::refresh_git`] asks git what has
//! changed, and the answer is held until something asks again, which is every
//! time the window is switched to, and after every action that could have
//! changed it. That is the same bargain the project map makes, and for the same
//! reason: a repository is cheap to re-read and expensive to keep in step.
//!
//! # One job at a time
//!
//! Pushing and pulling happen on a thread (see [`crate::git::run`]), and only
//! one may be in flight. A second push while the first is still going is not
//! something anyone means, and a queue would only hide which one failed.

use std::path::Path;

use crate::git::{Change, Diff, Job, Repo, Status};
use crate::text::editor::Editor;
use crate::text::highlight::Resume;

use super::App;
use super::mode::Window;

/// The buttons across the top of the pane, in the order they are drawn.
///
/// Ordered by how much they change: reading first, then writing, then the two
/// that rewrite what is already there. Nothing is arranged alphabetically:
/// the shape of the row is a warning label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Fetch,
    Pull,
    Push,
    Sync,
    Rebase,
    Commit,
}

pub const BUTTONS: &[Button] = &[
    Button::Fetch,
    Button::Pull,
    Button::Push,
    Button::Sync,
    Button::Rebase,
    Button::Commit,
];

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Fetch => "fetch",
            Button::Pull => "pull",
            Button::Push => "push",
            Button::Sync => "sync",
            Button::Rebase => "rebase",
            Button::Commit => "commit",
        }
    }

    /// What it says while it runs, and what git is actually asked to do, in
    /// order.
    ///
    /// `sync` is the one that is not a single git command: pull with a rebase
    /// and then push, which is what "get level with the remote" means when the
    /// answer must not be a merge commit nobody wrote. The push only happens
    /// if the pull worked.
    fn command(self) -> Option<(&'static str, &'static [&'static [&'static str]])> {
        match self {
            Button::Fetch => Some(("fetching", &[&["fetch", "--all", "--prune"]])),
            Button::Pull => Some(("pulling", &[&["pull", "--ff-only"]])),
            Button::Push => Some(("pushing", &[&["push"]])),
            Button::Sync => Some(("syncing", &[&["pull", "--rebase"], &["push"]])),
            Button::Rebase => Some(("rebasing", &[&["rebase"]])),
            // A commit needs a message, which is typing, which is the box.
            Button::Commit => None,
        }
    }
}

/// Which half of the window the keyboard is in.
///
/// The same idea as [`Focus`](super::mode::Focus) in the main window, and for
/// the same reason: the change list and the diff both want the arrow keys, and
/// which one gets them has to be a thing you can see rather than a mode you
/// have to remember. Right goes in, left comes back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GitFocus {
    #[default]
    List,
    Diff,
    /// Typing a commit message, in a box over both diff columns.
    Message,
}

/// One line of the left-hand list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// The button row. One row however many buttons it wraps to.
    Buttons,
    /// `STAGED 2` / `UNSTAGED 3`, and Enter takes the whole section.
    Header { staged: bool },
    /// A file, and which side of the index it is on.
    File { staged: bool, index: usize },
}

/// Everything the source window draws.
#[derive(Debug, Default)]
pub struct GitPane {
    /// The repository, or `None` when the project is not in one.
    pub repo: Option<Repo>,
    /// Why there is nothing to show, when there is nothing to show.
    pub error: Option<String>,
    pub status: Status,
    /// `git log --graph`, as git drew it.
    pub graph: Vec<String>,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Which button the cursor is on, while it is on the button row.
    pub button: usize,
    /// The before and after of the file under the cursor.
    pub diff: Diff,
    /// Each side's text, as the highlighter takes it, made once per diff
    /// rather than once per frame.
    pub diff_text: [Vec<String>; 2],
    /// Each side's saved highlighting, so scrolling down a long diff resumes
    /// from where the last frame got to rather than parsing from the top.
    pub diff_highlight: [Resume; 2],
    pub diff_scroll: usize,
    /// Which half has the arrow keys.
    pub focus: GitFocus,
    /// The commit message being written, if one is.
    ///
    /// A real [`Editor`], so the message gets the same keyboard as everything
    /// else that types in tiny: undo, word motions, the lot. It is never
    /// saved: git is handed its text and the buffer is dropped.
    pub message: Option<Editor>,
    /// Rows the two diff columns had on the last draw, so scrolling knows what
    /// a page is. Written by `ui`.
    pub diff_height: usize,
    /// A push or a pull, still running.
    pub job: Option<Job>,
    /// The first line of the list on screen. Nudged by `ui` as little as
    /// keeps the cursor's row in view, the way the browser's is.
    pub list_scroll: usize,
    /// The columns the list covered in the last frame, as `[x0, x1)`, so the
    /// wheel can tell the list from the diff. Written by `ui`.
    pub list_cols: Option<(u16, u16)>,
}

impl GitPane {
    /// The change the cursor is on, and whether it is staged.
    pub fn current(&self) -> Option<(&Change, bool)> {
        match self.rows.get(self.selected) {
            Some(Row::File { staged, index }) => {
                let list = if *staged {
                    &self.status.staged
                } else {
                    &self.status.unstaged
                };
                list.get(*index).map(|c| (c, *staged))
            }
            _ => None,
        }
    }

    /// Rebuild the list from the status, putting the cursor back on what it
    /// was on: the same file if it is still in the list, the buttons or a
    /// section heading if it was on one.
    ///
    /// `keep` is read *before* the new status arrives and passed in, because
    /// after it arrives the old row indices point into the new lists and name
    /// whichever file has taken that place.
    fn rebuild(&mut self, keep: Option<String>) {
        let was_on = self.rows.get(self.selected).cloned();
        self.rows = vec![Row::Buttons];
        for staged in [true, false] {
            let list = if staged {
                &self.status.staged
            } else {
                &self.status.unstaged
            };
            if list.is_empty() {
                continue;
            }
            self.rows.push(Row::Header { staged });
            self.rows
                .extend((0..list.len()).map(|index| Row::File { staged, index }));
        }
        // Staging moves a file from one section to the other, so the cursor
        // follows it by name and does not care which side it lands on; that
        // is the whole point of Enter being a toggle.
        self.selected = keep
            .and_then(|path| {
                self.rows.iter().position(|r| match r {
                    Row::File { staged, index } => {
                        let list = if *staged {
                            &self.status.staged
                        } else {
                            &self.status.unstaged
                        };
                        list.get(*index).is_some_and(|c| c.path == path)
                    }
                    _ => false,
                })
            })
            .or_else(|| match &was_on {
                // The buttons, or a heading, are rows that survive a refresh.
                Some(row @ (Row::Buttons | Row::Header { .. })) => {
                    self.rows.iter().position(|r| r == row)
                }
                _ => None,
            })
            .or_else(|| {
                // Otherwise, the first file if there is one.
                self.rows.iter().position(|r| matches!(r, Row::File { .. }))
            })
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
    }
}

impl App {
    /// Ask git everything the window needs, from scratch.
    ///
    /// Called on every switch into the window and after everything that could
    /// have changed the answer. A failure (no git, no repository) is kept and
    /// drawn rather than thrown, because "this is not a repository" is a
    /// perfectly good thing for the window to say.
    pub(super) fn refresh_git(&mut self) {
        let root = self.root().to_path_buf();
        self.git.error = None;
        if self.git.repo.is_none() {
            match Repo::find(&root) {
                Ok(repo) => self.git.repo = Some(repo),
                Err(e) => {
                    self.git.error = Some(format!("{e:#}"));
                    self.git.status = Status::default();
                    self.git.graph.clear();
                    self.git.rows.clear();
                    return;
                }
            }
        }
        let Some(repo) = self.git.repo.clone() else {
            return;
        };
        // Read before the new status lands: see `rebuild`.
        let before = self
            .git
            .current()
            .map(|(c, staged)| (c.path.clone(), staged));
        match repo.status() {
            Ok(status) => self.git.status = status,
            Err(e) => self.git.error = Some(format!("{e:#}")),
        }
        self.git.graph = repo.graph(GRAPH_ROWS).unwrap_or_default();
        self.git
            .rebuild(before.as_ref().map(|(path, _)| path.clone()));
        let after = self
            .git
            .current()
            .map(|(c, staged)| (c.path.clone(), staged));
        if after == before {
            // Still on the same change: a refresh, after a push finished say,
            // leaves the reader where they were in it, and a message being
            // written keeps the keyboard.
            let (focus, scroll) = (self.git.focus, self.git.diff_scroll);
            self.load_diff();
            self.git.diff_scroll = scroll.min(self.git.diff.len().saturating_sub(1));
            self.git.focus = match focus {
                GitFocus::Diff if self.git.diff.is_empty() => GitFocus::List,
                focus => focus,
            };
        } else {
            self.sync_diff();
        }
    }

    /// Read the diff for whatever the cursor is on now, from the top, with
    /// the keyboard in the list: a new file is a new thing to read. A message
    /// being written keeps the keyboard, though.
    pub(super) fn sync_diff(&mut self) {
        self.load_diff();
        self.git.diff_scroll = 0;
        if self.git.focus == GitFocus::Diff {
            self.git.focus = GitFocus::List;
        }
    }

    /// Read the diff of the change under the cursor, and nothing else.
    fn load_diff(&mut self) {
        self.git.diff = match (&self.git.repo, self.git.current()) {
            (Some(repo), Some((change, staged))) => repo.diff(change, staged).unwrap_or_default(),
            _ => Diff::default(),
        };
        // What the diff is drawn from, and its saved highlighting, belong to
        // the old diff.
        self.git.diff_text = [
            self.git
                .diff
                .before
                .iter()
                .map(|l| l.text.clone())
                .collect(),
            self.git.diff.after.iter().map(|l| l.text.clone()).collect(),
        ];
        self.git.diff_highlight = Default::default();
    }

    /// Move the cursor in the list, or scroll the diff, whichever half has
    /// the keyboard.
    pub(super) fn move_git_cursor(&mut self, delta: isize) {
        if self.git.focus == GitFocus::Diff {
            return self.scroll_git_diff(delta);
        }
        self.move_git_selection(delta);
    }

    /// Home and End: the first row of the list or the last, or the top or the
    /// bottom of the diff, whichever half has the keyboard.
    pub(super) fn git_to_end(&mut self, last: bool) {
        // Further than either can go, and both clamp.
        let far = self.git.rows.len().max(self.git.diff.len()) as isize;
        self.move_git_cursor(if last { far } else { -far });
    }

    /// One notch of the wheel in the git window: over the list it moves the
    /// list's cursor, the way the wheel over the browser does, and anywhere
    /// else it scrolls what is drawn there.
    pub(super) fn scroll_git(&mut self, down: bool, column: u16) {
        let step = if down { 1 } else { -1 };
        let over_list = matches!(self.git.list_cols, Some((x0, x1)) if column >= x0 && column < x1);
        match (over_list, &mut self.git.message) {
            // A message being written keeps the keyboard: moving the list's
            // cursor would read another diff and take the keyboard with it.
            (true, _) if self.git.focus == GitFocus::Message => {}
            (true, _) => self.move_git_selection(step),
            (false, Some(editor)) => {
                if down {
                    editor.move_down();
                } else {
                    editor.move_up();
                }
            }
            (false, None) => self.scroll_git_diff(step),
        }
    }

    /// Move the list's cursor, and read the diff of whatever it lands on.
    fn move_git_selection(&mut self, delta: isize) {
        let last = self.git.rows.len().saturating_sub(1);
        let next = (self.git.selected as isize + delta).clamp(0, last as isize) as usize;
        if next == self.git.selected {
            return;
        }
        self.git.selected = next;
        self.sync_diff();
        // The letter is a reminder; the word is the explanation, and there is
        // room for it on the status line.
        self.status = match self.git.current() {
            Some((change, staged)) => format!(
                "{}: {}{}",
                change.code.word(),
                change.label(self.palette.glyphs.arrow),
                if staged { "" } else { ", not staged" }
            ),
            None => self.git.status.branch.summary(self.palette.glyphs.arrow),
        };
    }

    /// Scroll the two halves of the diff together, by `lines`.
    ///
    /// One offset for both columns, which is the whole reason the two sides are
    /// padded to the same length: scrolling them separately would let them
    /// drift, and then reading across would mean nothing.
    pub(super) fn scroll_git_diff(&mut self, lines: isize) {
        let last = self.git.diff.len().saturating_sub(1) as isize;
        let next = (self.git.diff_scroll as isize + lines).clamp(0, last.max(0));
        self.git.diff_scroll = next as usize;
    }

    /// A screenful of the diff, in either direction.
    pub(super) fn page_git_diff(&mut self, pages: isize) {
        let page = self.git.diff_height.saturating_sub(1).max(1) as isize;
        self.scroll_git_diff(pages * page);
    }

    /// Left and right: along the button row, or in and out of the diff.
    ///
    /// One key with two jobs, decided by what the cursor is on. On the button
    /// row there is a row to walk; on a file, right is the way into the thing
    /// that file is about, which is what right-arrow means everywhere else in
    /// tiny too.
    pub(super) fn move_git_button(&mut self, delta: isize) {
        if self.git.focus == GitFocus::Diff {
            if delta < 0 {
                self.git.focus = GitFocus::List;
                self.status = "back to the changes".into();
            }
            return;
        }
        if self.git.rows.get(self.git.selected) != Some(&Row::Buttons) {
            if delta <= 0 {
                return;
            }
            // Into what is drawn on the right: a message put down with Esc,
            // to go on writing it, or else the diff, where up and down scroll
            // both columns at once.
            if self.git.message.is_some() {
                self.git.focus = GitFocus::Message;
                self.status = "back in the message: Ctrl+S commits, Esc puts it down".into();
            } else if !self.git.diff.is_empty() {
                self.git.focus = GitFocus::Diff;
                self.status = "reading the change, left to come back".into();
            }
            return;
        }
        let last = BUTTONS.len() - 1;
        self.git.button = (self.git.button as isize + delta).clamp(0, last as isize) as usize;
        self.status = BUTTONS[self.git.button].label().to_string();
    }

    /// Enter: stage a file, stage a section, or press a button.
    pub(super) fn activate_git(&mut self) {
        let Some(repo) = self.git.repo.clone() else {
            return;
        };
        let row = self.git.rows.get(self.git.selected).cloned();
        let done = match row {
            Some(Row::Buttons) => return self.press_git_button(BUTTONS[self.git.button]),
            // The header takes the whole section, which is the "all at once"
            // half of the promise the window makes.
            Some(Row::Header { staged: true }) => repo.unstage(None),
            Some(Row::Header { staged: false }) => repo.stage(None),
            Some(Row::File { .. }) => {
                let Some((change, staged)) = self.git.current() else {
                    return;
                };
                let path = change.path.clone();
                if staged {
                    repo.unstage(Some(&path))
                } else {
                    repo.stage(Some(&path))
                }
            }
            None => return,
        };
        match done {
            Ok(message) => self.status = message,
            Err(e) => self.status = format!("{e:#}"),
        }
        self.refresh_git();
    }

    /// Run what a button says, or open the message box where it needs words.
    ///
    /// Commit is the one button that is two presses: the first opens a box to
    /// write in, the second sends what is in it. That is deliberate: a commit
    /// message is the one piece of writing in this window, and a one-line
    /// prompt is not where anyone wants to write the second paragraph of it.
    fn press_git_button(&mut self, button: Button) {
        let Some((what, steps)) = button.command() else {
            return self.commit_button();
        };
        self.start_git_job(what, steps);
    }

    /// The commit button: open the box, or send what is in it.
    fn commit_button(&mut self) {
        match self.git.message.is_some() {
            true => self.commit_message(),
            false => self.open_message(),
        }
    }

    /// Open the message box over both diff columns, with the keyboard in it.
    pub(super) fn open_message(&mut self) {
        if self.git.status.staged.is_empty() {
            self.status = "nothing staged, Enter on a file stages it".into();
            return;
        }
        let path = self
            .git
            .repo
            .as_ref()
            .map(|r| r.root.join(".git").join("COMMIT_EDITMSG"))
            .unwrap_or_default();
        // Kept across a close and reopen, so an Esc does not cost a paragraph.
        if self.git.message.is_none() {
            self.git.message = Some(Editor::from_str(path, ""));
        }
        self.git.focus = GitFocus::Message;
        self.status = "write the message: Ctrl+S commits, Esc puts it down".into();
    }

    /// Send what is in the box.
    pub(super) fn commit_message(&mut self) {
        let Some(editor) = &self.git.message else {
            return;
        };
        let text = editor.to_text();
        match self.commit(&text) {
            Ok(done) => {
                self.git.message = None;
                self.git.focus = GitFocus::List;
                self.status = done;
            }
            // The message stays in the box: a commit that git refused is one
            // you are about to try again, and retyping it is not part of that.
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// A key while the message box has the keyboard.
    ///
    /// Two keys are the box's own and everything else is typing, which is why
    /// they are checked first: `Ctrl+S` is "done" here as it is everywhere, and
    /// Esc puts the box down without throwing away what is in it.
    pub(super) fn on_message_key(&mut self, key: crossterm::event::KeyEvent) {
        use crate::config::keys::{Action, Context as KeyContext};
        let action = self.keymap.resolve_while_typing(KeyContext::Editor, &key);
        match action {
            Some(Action::Save) => return self.commit_message(),
            Some(Action::Back) => {
                self.git.focus = GitFocus::List;
                self.status = "message kept, commit to send it".into();
                return;
            }
            _ => {}
        }
        let page = self.git.diff_height.saturating_sub(1).max(1);
        let tab_width = self.config.tab_width;
        let Some(editor) = &mut self.git.message else {
            return;
        };
        if let Some(complaint) = super::input::edit_with_key(editor, action, key, page, tab_width) {
            self.status = complaint.into();
        }
    }

    /// Start git commands on a thread, one after another, unless a job is
    /// already running.
    pub(super) fn start_git_job(&mut self, what: &str, steps: &[&[&str]]) {
        let Some(repo) = self.git.repo.clone() else {
            self.status = "not a git repository".into();
            return;
        };
        if let Some(job) = &self.git.job {
            self.status = format!("still {}", job.what);
            return;
        }
        self.status = format!("{what}{}", self.palette.glyphs.ellipsis);
        self.git.job = Some(Job::start(&repo.root, what, steps));
    }

    /// Collect a finished push or pull. Returns whether anything changed, for
    /// the same reason the disk watcher does: the event loop draws only when
    /// something has.
    pub fn poll_git_job(&mut self) -> bool {
        let Some(job) = &self.git.job else {
            return false;
        };
        let Some(result) = job.finished() else {
            return false;
        };
        let what = self.git.job.take().map(|j| j.what).unwrap_or_default();
        match result {
            Ok(out) => {
                // Git says nothing at all when a push had nothing to do.
                let last = out.lines().rfind(|l| !l.trim().is_empty());
                self.status = match last {
                    Some(line) => format!("{what}: {}", line.trim()),
                    None => format!("{what}: nothing to do"),
                };
            }
            Err(e) => self.status = format!("{what} failed: {e}"),
        }
        // Whatever it did, the repository may look different now.
        if self.window == Window::Source {
            self.refresh_git();
        }
        true
    }

    /// `*commit [message]`.
    pub(super) fn commit(&mut self, message: &str) -> anyhow::Result<String> {
        let repo = match &self.git.repo {
            Some(repo) => repo.clone(),
            None => Repo::find(self.root())?,
        };
        let done = repo.commit(message)?;
        if self.window == Window::Source {
            self.refresh_git();
        }
        Ok(done)
    }

    /// Tab: leave the window with the file under the cursor open in the editor.
    ///
    /// The one bridge between the two windows, and it goes this way only: a
    /// change you are looking at is very often one you want to keep editing.
    pub(super) fn open_from_git(&mut self) {
        let Some(repo) = self.git.repo.as_ref() else {
            return;
        };
        let Some((change, _)) = self.git.current() else {
            return;
        };
        let path = repo.root.join(Path::new(&change.path));
        self.window = Window::Main;
        self.open_path(&path);
    }
}

/// How far back the branch map goes. Enough to see where the branches parted,
/// short enough that reading it is not a job.
const GRAPH_ROWS: usize = 40;
