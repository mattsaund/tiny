//! Noticing that the disk has changed underneath us.
//!
//! Another program writes to a file tiny has open — a formatter, a build step,
//! a `git checkout`, the same project open in another window — and what is on
//! screen is now a picture of a file that no longer exists. This is the part
//! that fixes that: [`App::rescan_disk`] is called by the event loop whenever
//! it has been idle for a moment, and takes on whatever it finds.
//!
//! # Polling, not inotify
//!
//! There is no watcher and no background thread. The event loop waits on the
//! keyboard with a timeout instead of without one, and every time the timeout
//! expires it stats what is on screen: the directories the tree has read, and
//! the files there are buffers for. That is a handful of `stat` calls twice a
//! second — microseconds — against a dependency, a thread, a channel, and a
//! way to wake a blocking read, which is what a real watcher would cost.
//!
//! The trade is honest and worth naming: tiny is no longer a program that uses
//! *no* CPU while idle. It uses a rounding error of one.
//!
//! # Stamps are taken when the thing is read, not when it is scanned
//!
//! Every path is remembered as its modification time and size, and the stamp
//! is taken at the moment tiny reads the thing: [`App::note_file`] when a
//! buffer is opened, [`App::note_dirs`] whenever the tree has been rebuilt.
//!
//! Doing it at the first *scan* instead would leave a hole exactly the width
//! of the poll interval. Expand a folder, have a file land in it 200ms later,
//! and the first scan would record a modification time that already includes
//! the new file without ever having drawn it — so it would stay invisible
//! until something else changed in that folder.
//!
//! A path with no stamp at all is still recorded rather than acted on, which
//! is what keeps the scan after startup from deciding the whole project has
//! just changed.
//!
//! # Our own writes
//!
//! Saving from tiny moves the file's modification time as surely as anyone
//! else's write does, so the next scan sees a difference on a file we just
//! wrote ourselves. Rather than trying to remember which writes were ours,
//! a file whose stamp moved is read and compared: identical text is not a
//! change, and nothing on screen moves. It costs one read of a file small
//! enough to be open in an editor, and only on the scan after a write.
//!
//! # Unsaved work is never overwritten
//!
//! A dirty buffer is never reloaded, whatever the disk says. It gets a line on
//! the status bar — once, because the stamp is updated either way — and keeps
//! the text the user typed. There is nowhere else for that text to live.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::App;
use super::mode::Focus;
use super::parts::display_name;
use super::preview::Preview;

/// What a path looked like when we last stat'd it: modification time and size.
///
/// `None` records that it could not be stat'd at all, which is a state like
/// any other — going from `Some` to `None` is a file being deleted, and going
/// the other way is one appearing.
type Stamp = Option<(SystemTime, u64)>;

/// Both halves of what tiny is looking at, as of the last scan.
///
/// Kept in the same shape for directories and files even though they are
/// checked for different things: a directory's modification time changes when
/// an entry is added or removed, a file's when its contents are written.
#[derive(Debug, Default)]
pub struct Watch {
    dirs: HashMap<PathBuf, Stamp>,
    files: HashMap<PathBuf, Stamp>,
}

/// Modification time and size, or `None` for anything that cannot be stat'd.
///
/// A filesystem with no usable modification time still gets its size watched,
/// which catches most edits — better than a stamp that can never differ.
fn stamp(path: &Path) -> Stamp {
    let m = fs::metadata(path).ok()?;
    Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

impl App {
    /// Remember what the directories on screen look like right now.
    ///
    /// Called from `rebuild_rows`, which runs after every structural change to
    /// the tree, so the stamps are always as fresh as the rows drawn from them.
    pub(super) fn note_dirs(&mut self) {
        self.watch.dirs = self
            .tree
            .loaded_dirs()
            .into_iter()
            .map(|d| {
                let now = stamp(&d);
                (d, now)
            })
            .collect();
    }

    /// Remember what a file looked like as it was read into a buffer.
    pub(super) fn note_file(&mut self, path: &Path) {
        let now = stamp(path);
        self.watch.files.insert(path.to_path_buf(), now);
    }

    /// Take on everything that changed on disk since the last look. Returns
    /// whether anything did, which is the event loop's cue to draw a frame.
    ///
    /// Directories first, so that a file which has just been deleted is
    /// already out of the tree by the time its buffer is dealt with.
    pub fn rescan_disk(&mut self) -> bool {
        if !self.config.auto_reload {
            return false;
        }
        let dirs = self.rescan_dirs();
        let files = self.rescan_files();
        dirs || files
    }

    /// Entries appearing in or disappearing from the directories on screen.
    ///
    /// One stat per directory the tree has read; collapsed ones are not read,
    /// so a project with a large unopened subtree costs nothing to watch. Any
    /// difference re-reads the lot, because [`Tree::refresh_all`] is cheap
    /// relative to the drawing that follows it and the alternative is a
    /// per-directory merge that would have to reimplement it.
    ///
    /// [`Tree::refresh_all`]: crate::files::tree::Tree::refresh_all
    fn rescan_dirs(&mut self) -> bool {
        let mut changed = false;
        let mut fresh: HashMap<PathBuf, Stamp> = HashMap::new();
        for dir in self.tree.loaded_dirs() {
            let now = stamp(&dir);
            changed |= self.watch.dirs.get(&dir).is_some_and(|was| *was != now);
            fresh.insert(dir, now);
        }
        self.watch.dirs = fresh;
        if !changed {
            return false;
        }
        // `rebuild_rows` re-derives the preview, which resets its scroll. That
        // is right when the cursor has moved to another file and wrong when it
        // has not — a file being written elsewhere in the project should not
        // send you back to the top of the one you are reading.
        let was_on = self.selected_path().map(Path::to_path_buf);
        let scroll = self.preview_scroll;
        self.tree.refresh_all();
        self.rebuild_rows();
        if was_on.as_deref() == self.selected_path() {
            self.preview_scroll = scroll;
        } else if self.focus == Focus::Editor {
            // The row the keyboard was in has gone, and the cursor has fallen
            // onto a neighbor. Take the keyboard out of the file: otherwise
            // the next thing typed is typed into whichever file that turned
            // out to be. What happened to the file itself is `take_on_disk`'s
            // to say.
            self.focus_tree();
        }
        true
    }

    /// Open files whose contents have been written by someone else.
    fn rescan_files(&mut self) -> bool {
        let mut changed = false;
        for path in self.buffers.keys().cloned().collect::<Vec<_>>() {
            let now = stamp(&path);
            match self.watch.files.get(&path) {
                Some(was) if *was == now => continue,
                Some(_) => {
                    self.watch.files.insert(path.clone(), now);
                    changed |= self.take_on_disk(&path);
                }
                // Never seen before: this is the scan that learns it, not the
                // one that acts on it.
                None => {
                    self.watch.files.insert(path, now);
                }
            }
        }
        // A buffer can go away — a delete, or a rename moving it to another
        // key — and its stamp should not outlive it.
        let buffers = &self.buffers;
        self.watch.files.retain(|p, _| buffers.contains_key(p));
        changed
    }

    /// One file that is no longer what it was. Returns whether the screen
    /// needs redrawing.
    fn take_on_disk(&mut self, path: &Path) -> bool {
        let name = display_name(path);
        let fresh = fs::read(path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            // The same guard `load_file_preview` uses: a file that has become
            // binary is not text that happens to decode.
            .filter(|text| !text.contains('\0'));
        let dirty = self.is_dirty(path);

        let Some(text) = fresh else {
            // Deleted, unreadable, or no longer text.
            if dirty {
                // The buffer stays. There is no row to put the cursor back on,
                // but the text is not lost: it is still listed on the way out,
                // and saving it there writes the file back.
                self.status = format!("{name} is gone from disk — your unsaved copy is kept");
                return true;
            }
            let showing = matches!(&self.preview, Preview::Buffer { path: p, .. } if p == path);
            self.buffers.remove(path);
            self.highlight_cache.clear();
            // Dropping the buffer is what lets the file be classified afresh
            // if it comes back, since an open buffer always wins over disk.
            if showing {
                self.sync_preview();
            }
            self.status = format!("{name} is gone from disk");
            return true;
        };

        if dirty {
            self.status = format!("{name} changed on disk — your unsaved copy is what you see");
            return true;
        }
        let Some(editor) = self.buffers.get_mut(path) else {
            return false;
        };
        // Our own save, or someone else's write that changed nothing.
        if editor.to_text() == text {
            return false;
        }
        editor.reload_from(&text);
        // The cache holds parser state keyed by line number, and every line
        // number in this file has just been reconsidered.
        self.highlight_cache.clear();
        self.status = format!("{name} changed on disk — reloaded");
        true
    }
}
