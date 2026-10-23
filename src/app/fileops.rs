//! New, rename, delete, copy, paste, save.
//!
//! Everything that changes the filesystem on the user's say-so. What they have
//! in common is that each one is a small transaction with three parts: work
//! out the target, do it, and say what happened, and that the saying is not
//! optional, because a file manager that silently does nothing is
//! indistinguishable from one that is broken.
//!
//! # Nothing destructive happens on one key
//!
//! Delete is armed and then confirmed; quitting with unsaved work asks and
//! names the files. Paste never overwrites: [`free_name`] finds an unused
//! name beside the existing one instead, so the worst a mistaken paste costs
//! you is a copy to delete.
//!
//! # Buffers follow their files
//!
//! A rename moves the buffer's entry to the new key, so unsaved edits follow
//! the file rather than being stranded under a path that no longer exists.
//! Renaming a folder moves every buffer inside it the same way, along with
//! anything else that points into it: the clipboard, and the pane without
//! the keyboard. A delete drops the entry and everything under it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::config::keys::Action;
use crate::text::editor::Editor;

use super::App;
use super::mode::{Confirm, ConfirmKind, Focus, Mode};
use super::parts::{display_name, plural, safe_join};
use super::preview::Preview;

/// Copy a directory and everything under it. Returns how many links were
/// left out.
///
/// Recurses on directories and copies files with `fs::copy`, which carries
/// permissions across on every platform. A symlink is never followed, so a
/// link pointing back up its own tree cannot send this into a loop: on Unix
/// the link itself is made again, pointing where it pointed (a virtualenv's
/// `lib64 -> lib` included), and elsewhere, where making one takes rights a
/// user may not have, it is left out and counted.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<usize> {
    fs::create_dir_all(dst)?;
    let mut left_out = 0;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        // `DirEntry::file_type` does not follow links.
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &target)?;
            #[cfg(not(unix))]
            {
                left_out += 1;
            }
        } else if kind.is_dir() {
            left_out += copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(left_out)
}

/// Whether two paths are one name in two cases: what a rename that changes
/// only the case of a name looks like on a disk that ignores case, where the
/// "new" name already exists because it is the old one.
///
/// Only that. A link to the file, or a second hard link to it, is a name of
/// its own, and a rename onto it would replace it, so the two have to be in
/// the same folder, differ in nothing but case, and be the one entry on disk,
/// asked without following links.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    let lower = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    if a.parent() != b.parent() || lower(a).is_none() || lower(a) != lower(b) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    // Windows ignores case in names, so two that differ only in case are
    // the same entry there.
    #[cfg(not(unix))]
    {
        fs::symlink_metadata(a).is_ok()
    }
}

/// How far a delete question counts the files under a folder before it says
/// "this many and more": a `node_modules` is not worth walking to the end of
/// just to ask a question.
const DELETE_COUNT_LIMIT: usize = 10_000;

/// How many files are under `dir`, however deep, up to `limit`, and whether
/// there were more. Links are counted, not followed, which is how a delete
/// treats them too.
fn count_files(dir: &Path, limit: usize) -> (usize, bool) {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            if n >= limit {
                return (n, true);
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                _ => n += 1,
            }
        }
    }
    (n, false)
}

/// Where `path` is once `from` has moved to `to`: `to` itself, or the same
/// place inside it.
pub(super) fn moved(path: &Path, from: &Path, to: &Path) -> PathBuf {
    match path.strip_prefix(from) {
        // Joining an empty path would add a trailing separator.
        Ok(rest) if rest.as_os_str().is_empty() => to.to_path_buf(),
        Ok(rest) => to.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// A free path in `dir` for something named after `src`: `notes.md`, then
/// `notes copy.md`, then `notes copy 2.md`.
///
/// Only paste uses this. The words are spelled out rather than punctuated
/// (`notes copy.md`, not `notes(1).md`) because everything else the command
/// bar shows reads as English too.
fn free_name(dir: &Path, src: &Path) -> Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| anyhow!("cannot copy {}", display_name(src)))?;
    let first = dir.join(name);
    if !first.exists() {
        return Ok(first);
    }
    let name = Path::new(name);
    let stem = name
        .file_stem()
        .unwrap_or(name.as_os_str())
        .to_string_lossy();
    let ext = name.extension().map(|e| e.to_string_lossy());
    for n in 1..100 {
        let base = if n == 1 {
            format!("{stem} copy")
        } else {
            format!("{stem} copy {n}")
        };
        let candidate = match &ext {
            Some(e) => dir.join(format!("{base}.{e}")),
            None => dir.join(base),
        };
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow!("too many copies of {}", display_name(src)))
}

impl App {
    /// `Ctrl+C`: remember what the cursor is on. Nothing is read or written
    /// until the paste.
    pub(super) fn copy_selection(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.path == self.tree.root_path() {
            self.status = "cannot copy the project root".into();
            return;
        }
        self.status = format!(
            "copied {}, {} to paste",
            row.name,
            self.key_name(Action::Paste)
        );
        self.clipboard = Some(row.path);
    }

    /// `Ctrl+V`: drop the clipboard into the folder the cursor is in.
    ///
    /// Where `*copy` refuses to write over something (you named that
    /// destination, so a collision means you were wrong about it) a paste
    /// picks the next free name instead. The destination here is implied
    /// rather than stated, and pasting into the folder you copied from is the
    /// ordinary way to duplicate a file; erroring there would make the gesture
    /// useless.
    pub(super) fn paste_clipboard(&mut self) {
        let Some(src) = self.clipboard.clone() else {
            self.status = format!("nothing copied, {} first", self.key_name(Action::Copy));
            return;
        };
        let base = self.creation_base();
        let dst = match free_name(&base, &src) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("{e:#}");
                return;
            }
        };
        match self.copy_entry(&src, &dst) {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Copy a file, or a whole folder, to `dst` inside the project.
    ///
    /// `dst` is the copy's own path, already worked out: "into a folder" is
    /// the caller's question (see `App::destination`), and an existing path
    /// is never written over.
    pub(super) fn copy_entry(&mut self, src: &Path, dst: &Path) -> Result<String> {
        let meta = fs::symlink_metadata(src)
            .with_context(|| format!("cannot copy {}", display_name(src)))?;
        let dst = dst.to_path_buf();
        if dst == src {
            return Err(anyhow!("{} is already there", display_name(src)));
        }
        if dst.exists() {
            return Err(anyhow!("{} already exists", display_name(&dst)));
        }
        // Copying a folder into itself would walk into what it is writing.
        if meta.is_dir() && dst.starts_with(src) {
            return Err(anyhow!("cannot copy {} into itself", display_name(src)));
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }

        let left_out = if meta.is_dir() {
            copy_tree(src, &dst)
        } else {
            fs::copy(src, &dst).map(|_| 0)
        }
        .with_context(|| {
            format!(
                "cannot copy {} to {}",
                display_name(src),
                display_name(&dst)
            )
        })?;

        self.tree.refresh_all();
        self.size_cache.clear();
        // Shown where it landed, unless a file is being typed into: moving
        // the browser onto the copy would hand it the next keystrokes, so the
        // keyboard stays in the file it was in and the line below says where
        // the copy went.
        if self.focus == Focus::Editor && matches!(self.preview, Preview::Buffer { .. }) {
            self.rebuild_rows();
        } else {
            self.reveal(&dst);
        }
        let links = match left_out {
            0 => String::new(),
            1 => ", one link left out".into(),
            n => format!(", {n} links left out"),
        };
        Ok(format!(
            "copied {} to {}{links}",
            display_name(src),
            display_name(&dst)
        ))
    }

    /// Write the current buffer. Reports "nothing to save" for a non-text
    /// preview and "no changes" for a clean one, rather than silently doing
    /// nothing in either case.
    pub fn save_active(&mut self) {
        let Some(ed) = self.active_buffer_mut() else {
            self.status = "nothing to save".into();
            return;
        };
        if !ed.dirty {
            self.status = "no changes".into();
            return;
        }
        let path = ed.path.clone();
        let name = display_name(&path);
        match ed.save() {
            Ok(()) => {
                self.status = format!("saved {name}");
                // Our own write, stamped as ours, so the disk watcher does not
                // take it for someone else's.
                self.note_file(&path);
                self.plugin_event("save", &path);
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
        // The file just changed length under a cursor that did not move, and
        // the readout at the end of the status line is showing what it was.
        self.note_size();
    }

    /// `Ctrl+S` from the tree: save what the cursor is on.
    ///
    /// On a file that is the file. On a folder it is everything unsaved
    /// underneath it, however deep, so the root row saves the whole project
    /// and a subfolder saves just its own. That makes the one key mean "save
    /// this", whatever "this" happens to be, which is the same thing the tree
    /// cursor means everywhere else in the program.
    pub(super) fn save_from_tree(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return self.save_active();
        };
        if !row.is_dir {
            return self.save_active();
        }
        let under: Vec<PathBuf> = self
            .buffers
            .values()
            .filter(|e| e.dirty && e.path.starts_with(&row.path))
            .map(|e| e.path.clone())
            .collect();
        if under.is_empty() {
            self.status = format!("nothing to save in {}", row.name);
            return;
        }
        // Sorted so the report is stable, and so the first failure named is
        // the same one on every run.
        let mut under = under;
        under.sort();
        let (saved, failed) = self.save_each(&under);
        self.status = match failed.first() {
            Some(first) => format!("saved {saved}, {} failed: {first}", failed.len()),
            None if saved == 1 => format!("saved {}", display_name(&under[0])),
            None => format!("saved {saved} files in {}", row.name),
        };
    }

    /// Write each of `paths` that is open, returning how many were written
    /// and one message per failure.
    ///
    /// Every save in tiny that touches more than one file goes through here:
    /// saving a folder from the tree, and answering yes to the question on the
    /// way out. Both need the same thing: keep going after a failure, and be
    /// able to say afterwards which ones did not make it. A loop that stopped
    /// at the first error would leave the rest unsaved without saying so.
    ///
    /// Paths with no buffer open are skipped rather than reported: nothing was
    /// asked of them.
    pub(super) fn save_each(&mut self, paths: &[PathBuf]) -> (usize, Vec<String>) {
        let (mut saved, mut failed) = (0usize, Vec::new());
        for path in paths {
            match self.buffers.get_mut(path).map(Editor::save) {
                Some(Ok(())) => {
                    saved += 1;
                    self.note_file(path);
                    self.plugin_event("save", path);
                }
                Some(Err(e)) => failed.push(format!("{}: {e}", display_name(path))),
                None => {}
            }
        }
        (saved, failed)
    }

    /// Where a new file or folder should go: the selected directory, or the
    /// parent of the selected file. Matches what most file managers do, and
    /// means you rarely have to type a path.
    pub(super) fn creation_base(&self) -> PathBuf {
        match self.selected_row() {
            Some(r) if r.is_dir => r.path.clone(),
            Some(r) => r
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.tree.root_path().to_path_buf()),
            None => self.tree.root_path().to_path_buf(),
        }
    }

    /// Create a file or directory. A name containing separators creates the
    /// intermediate directories too, so `notes/2026/today.md` works in one go.
    pub(super) fn create_entry(&mut self, base: &Path, name: &str, is_dir: bool) -> Result<String> {
        let target = safe_join(base, name, self.tree.root_path())?;
        if target.exists() {
            return Err(anyhow!("{} already exists", display_name(&target)));
        }
        if is_dir {
            fs::create_dir_all(&target)
                .with_context(|| format!("cannot create {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            fs::write(&target, "")
                .with_context(|| format!("cannot create {}", target.display()))?;
        }
        self.tree.refresh_all();
        self.size_cache.clear();
        if is_dir {
            self.tree.expand(&target);
        }
        self.reveal(&target);
        Ok(format!("created {}", display_name(&target)))
    }

    /// Move `from` to `target`, carrying any open buffer with it so unsaved
    /// edits follow the file to its new name. Refuses to overwrite.
    ///
    /// One path in and one path out, rather than "the selection, renamed
    /// within its folder", because `*rename a to b` can move a file between
    /// folders and the old naming prompt could not. Both are still a rename to
    /// the filesystem, which is the only reason this is not called move.
    pub(super) fn rename_path(&mut self, from: &Path, target: &Path) -> Result<String> {
        if from == self.tree.root_path() {
            return Err(anyhow!("cannot rename the project root"));
        }
        if !from.exists() {
            return Err(anyhow!("{} is not here", display_name(from)));
        }
        if target == from {
            return Ok("unchanged".into());
        }
        // Before anything is made on the way there: a folder cannot go
        // inside itself, and the folders made for it would be left behind.
        if target.starts_with(from) {
            return Err(anyhow!("cannot move {} into itself", display_name(from)));
        }
        if target.exists() && !same_file(target, from) {
            return Err(anyhow!("{} already exists", display_name(target)));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from, target)
            .with_context(|| format!("cannot rename {}", display_name(from)))?;

        // Carry every open buffer, and its unsaved edits, to the new path: the
        // file's own, or every one inside a folder.
        let moving: Vec<PathBuf> = self
            .buffers
            .keys()
            .filter(|p| p.starts_with(from))
            .cloned()
            .collect();
        for old in moving {
            if let Some(mut ed) = self.buffers.remove(&old) {
                let new = moved(&old, from, target);
                ed.path = new.clone();
                self.buffers.insert(new, ed);
            }
        }
        if let Some(copied) = &mut self.clipboard
            && copied.starts_with(from)
        {
            *copied = moved(copied, from, target);
        }
        let pinned_moved = self
            .split
            .as_ref()
            .is_some_and(|s| s.pinned.preview.path().is_some_and(|p| p.starts_with(from)));
        if let Some(split) = &mut self.split {
            split.pinned.preview.moved(from, target);
        }
        // The pane without the keyboard reads its file again under the new
        // name, which can make it another kind of file: notes.txt renamed
        // notes.rs is code now.
        if pinned_moved
            && let Some(path) = self
                .split
                .as_ref()
                .and_then(|s| s.pinned.preview.path())
                .filter(|p| p.is_file())
                .map(Path::to_path_buf)
        {
            let fresh = self.load_file_preview(&path);
            if let Some(split) = &mut self.split {
                split.pinned.preview = fresh;
                split.pinned.highlight = Default::default();
            }
        }
        self.tree.refresh_all();
        self.size_cache.clear();
        self.reveal(target);
        Ok(format!(
            "renamed {} to {}",
            display_name(from),
            display_name(target)
        ))
    }

    /// Put up the yes/no question for removing `path`.
    ///
    /// The single gate on every delete, and there is now only one way in:
    /// `*delete`, with a path or with none for whatever the cursor is on.
    /// Deleting used to have a key as well, which is exactly the overlap that
    /// went; it is a deliberate act, and a deliberate act can afford to be
    /// typed. Nothing touches the disk until [`App::do_delete`].
    ///
    /// The message counts a directory's entries first, because `remove_dir_all`
    /// takes everything underneath and the number should be on screen before
    /// anyone presses `y`.
    pub(super) fn arm_delete(&mut self, path: &Path) -> Result<()> {
        if path == self.tree.root_path() {
            return Err(anyhow!("cannot delete the project root"));
        }
        // `symlink_metadata` rather than `exists`, which follows links and so
        // reports a broken symlink as missing; that is a thing you very much
        // want to be able to delete.
        let meta = fs::symlink_metadata(path)
            .with_context(|| format!("cannot delete {}", display_name(path)))?;
        let name = display_name(path);
        // Say how much is at stake before asking: deleting a folder takes
        // everything under it, however deep, and unsaved edits go with their
        // files, since there will be nothing to save them to.
        let unsaved = self
            .buffers
            .values()
            .filter(|ed| ed.dirty && ed.path.starts_with(path))
            .count();
        let message = if meta.is_dir() {
            let (n, more) = count_files(path, DELETE_COUNT_LIMIT);
            let files = format!("{n}{} file{}", if more { "+" } else { "" }, plural(n));
            let edits = match unsaved {
                0 => String::new(),
                1 => ", one with unsaved edits".into(),
                n => format!(", {n} with unsaved edits"),
            };
            format!("Delete folder {name} and the {files} in it{edits}?  (y/n)")
        } else if unsaved > 0 {
            format!("Delete {name} and its unsaved edits?  (y/n)")
        } else {
            format!("Delete {name}?  (y/n)")
        };
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::Delete(path.to_path_buf()),
            message,
        });
        Ok(())
    }

    /// Carry out a confirmed delete, then put the cursor somewhere sensible.
    ///
    /// Buffers under the deleted path are dropped, including unsaved ones,
    /// since there is no longer a file to save them to. A cursor that was on
    /// what went moves to its parent folder, rather than onto whichever row
    /// slid into its place; one that was anywhere else stays where it was.
    pub(super) fn do_delete(&mut self, path: &Path) {
        let was_on_it = self.selected_path().is_some_and(|p| p.starts_with(path));
        // `symlink_metadata` again, so a link is unlinked rather than followed
        // into: deleting a shortcut must never delete what it points at.
        let is_dir = fs::symlink_metadata(path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let result = if is_dir {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match result {
            Ok(()) => {
                self.buffers.retain(|p, _| !p.starts_with(path));
                let parent = path.parent().map(Path::to_path_buf);
                self.tree.refresh_all();
                // Through `rebuild_rows`, so a folder's remembered size and the
                // watcher's picture of the tree are both brought up to date.
                self.rebuild_rows();
                if was_on_it
                    && let Some(i) = parent.and_then(|p| self.rows.iter().position(|r| r.path == p))
                {
                    self.selected = i;
                    self.sync_preview();
                }
                // `*delete` can be run from the editor. If the pane holding the
                // keyboard just lost its file, the tree is the only place left
                // with something to point at.
                if self.focus == Focus::Editor && !matches!(self.preview, Preview::Buffer { .. }) {
                    self.focus_tree();
                }
                self.status = format!("deleted {}", display_name(path));
            }
            Err(e) => self.status = format!("delete failed: {e}"),
        }
    }
}
