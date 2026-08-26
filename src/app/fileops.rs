//! New, rename, delete, copy, paste, save.
//!
//! Everything that changes the filesystem on the user's say-so. What they have
//! in common is that each one is a small transaction with three parts: work
//! out the target, do it, and say what happened — and that the saying is not
//! optional, because a file manager that silently does nothing is
//! indistinguishable from one that is broken.
//!
//! # Nothing destructive happens on one key
//!
//! Delete is armed and then confirmed; quitting with unsaved work asks and
//! names the files. Paste never overwrites — [`free_name`] finds an unused
//! name beside the existing one instead, so the worst a mistaken paste costs
//! you is a copy to delete.
//!
//! # Buffers follow their files
//!
//! A rename moves the buffer's entry to the new key, so unsaved edits follow
//! the file rather than being stranded under a path that no longer exists. A
//! delete drops the entry and everything under it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::files::project;
use crate::text::editor::Editor;

use super::App;
use super::mode::{Confirm, ConfirmKind, Focus, Mode, Prompt, PromptKind};
use super::parts::{display_name, safe_join};
use super::preview::Preview;

/// Copy a directory and everything under it.
///
/// Recurses on directories and copies everything else with `fs::copy`, which
/// carries permissions across on every platform. A symlink counts as
/// "everything else": its target is copied as a plain file rather than
/// followed as a directory, so a link pointing back up its own tree cannot
/// send this into a loop.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        // `DirEntry::file_type` does not follow links, which is what keeps the
        // symlink case out of the recursive branch.
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// A free path in `dir` for something named after `src`: `notes.md`, then
/// `notes copy.md`, then `notes copy 2.md`.
///
/// Only paste uses this. The words are spelled out rather than punctuated —
/// `notes copy.md`, not `notes(1).md` — because everything else the command
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
        self.status = format!("copied {} — ^V to paste", row.name);
        self.clipboard = Some(row.path);
    }

    /// `Ctrl+V`: drop the clipboard into the folder the cursor is in.
    ///
    /// Where `:copy` refuses to write over something — you named that
    /// destination, so a collision means you were wrong about it — a paste
    /// picks the next free name instead. The destination here is implied
    /// rather than stated, and pasting into the folder you copied from is the
    /// ordinary way to duplicate a file; erroring there would make the gesture
    /// useless.
    pub(super) fn paste_clipboard(&mut self) {
        let Some(src) = self.clipboard.clone() else {
            self.status = "nothing copied — ^C first".into();
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

    /// Copy a file, or a whole folder, to a new path inside the project.
    ///
    /// Naming an existing folder as the destination copies *into* it, which is
    /// what `copy README.md to notes` reads like. Anything else is the new name
    /// itself, and an existing one is never written over.
    pub(super) fn copy_entry(&mut self, src: &Path, dst: &Path) -> Result<String> {
        let meta = fs::symlink_metadata(src)
            .with_context(|| format!("cannot copy {}", display_name(src)))?;
        // `copy a to notes` means "put a in notes", not "rename a to notes".
        let dst = if dst.is_dir() {
            match src.file_name() {
                Some(name) => dst.join(name),
                None => return Err(anyhow!("cannot copy {}", display_name(src))),
            }
        } else {
            dst.to_path_buf()
        };
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

        if meta.is_dir() {
            copy_tree(src, &dst)
        } else {
            fs::copy(src, &dst).map(|_| ())
        }
        .with_context(|| {
            format!(
                "cannot copy {} to {}",
                display_name(src),
                display_name(&dst)
            )
        })?;

        self.tree.refresh_all();
        self.reveal(&dst);
        Ok(format!(
            "copied {} to {}",
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
        let name = display_name(&ed.path.clone());
        match ed.save() {
            Ok(()) => self.status = format!("saved {name}"),
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// `Ctrl+S` from the tree: save what the cursor is on.
    ///
    /// On a file that is the file. On a folder it is everything unsaved
    /// underneath it, however deep — so the root row saves the whole project
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
        let (mut saved, mut failed) = (0usize, Vec::new());
        for path in &under {
            match self.buffers.get_mut(path).map(Editor::save) {
                Some(Ok(())) => saved += 1,
                Some(Err(e)) => failed.push(format!("{}: {e}", display_name(path))),
                None => {}
            }
        }
        self.status = match failed.first() {
            Some(first) => format!("saved {saved}, {} failed — {first}", failed.len()),
            None if saved == 1 => format!("saved {}", display_name(&under[0])),
            None => format!("saved {saved} files in {}", row.name),
        };
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

    /// Open a naming prompt. Rename pre-fills the current name and refuses to
    /// touch the project root, which has no parent inside the tree to rename
    /// it within.
    pub(super) fn begin_prompt(&mut self, kind: PromptKind) {
        let (label, input, base) = match kind {
            PromptKind::New => ("New".to_string(), String::new(), self.creation_base()),
            PromptKind::Rename => {
                let Some(row) = self.selected_row() else {
                    return;
                };
                if row.path == self.tree.root_path() {
                    self.status = "cannot rename the project root".into();
                    return;
                }
                let parent = row
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.tree.root_path().to_path_buf());
                ("Rename".to_string(), row.name.clone(), parent)
            }
        };
        let cursor = input.chars().count();
        self.mode = Mode::Prompt(Prompt {
            kind,
            label,
            input,
            cursor,
            base,
        });
    }

    /// Act on a confirmed prompt. An empty name cancels rather than erroring —
    /// pressing Enter on a blank field clearly means "never mind".
    pub(super) fn commit_prompt(&mut self, p: Prompt) {
        let name = p.input.trim().to_string();
        if name.is_empty() {
            self.status = "cancelled".into();
            return;
        }
        let result = match p.kind {
            // The same rule the command line uses: a name with an extension is
            // a file, one without is a folder.
            PromptKind::New => {
                let is_dir = !project::names_a_file(&name, Path::new(&name));
                self.create_entry(&p.base, &name, is_dir)
            }
            PromptKind::Rename => self.rename_selected(&p.base, &name),
        };
        match result {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("{e:#}"),
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
        if is_dir {
            self.tree.expand(&target);
        }
        self.reveal(&target);
        Ok(format!("created {}", display_name(&target)))
    }

    /// Rename the selected entry, moving any open buffer with it so unsaved
    /// edits follow the file to its new name. Refuses to overwrite an existing
    /// path.
    fn rename_selected(&mut self, base: &Path, name: &str) -> Result<String> {
        let Some(row) = self.selected_row().cloned() else {
            return Err(anyhow!("nothing selected"));
        };
        let target = safe_join(base, name, self.tree.root_path())?;
        if target == row.path {
            return Ok("unchanged".into());
        }
        if target.exists() {
            return Err(anyhow!("{} already exists", display_name(&target)));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&row.path, &target)
            .with_context(|| format!("cannot rename {}", display_name(&row.path)))?;

        // Carry any open buffer, and its unsaved edits, to the new path.
        if let Some(mut ed) = self.buffers.remove(&row.path) {
            ed.path = target.clone();
            self.buffers.insert(target.clone(), ed);
            self.highlight_cache.clear();
        }
        self.tree.refresh_all();
        self.reveal(&target);
        Ok(format!("renamed to {}", display_name(&target)))
    }

    /// Ask before deleting. The message counts a directory's entries first,
    /// because `remove_dir_all` takes everything underneath and the user should
    /// see the number before pressing `y`.
    pub(super) fn begin_delete(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if let Err(e) = self.arm_delete(&row.path) {
            self.status = format!("{e:#}");
        }
    }

    /// Put up the yes/no question for removing `path`.
    ///
    /// The single gate on every delete: `d` and `:delete` both arrive here, so
    /// there is one place deciding what may be removed and one question to
    /// answer. Nothing touches the disk until [`App::do_delete`].
    pub(super) fn arm_delete(&mut self, path: &Path) -> Result<()> {
        if path == self.tree.root_path() {
            return Err(anyhow!("cannot delete the project root"));
        }
        // `symlink_metadata` rather than `exists`, which follows links and so
        // reports a broken symlink as missing — that is a thing you very much
        // want to be able to delete.
        let meta = fs::symlink_metadata(path)
            .with_context(|| format!("cannot delete {}", display_name(path)))?;
        let name = display_name(path);
        // Say how much is at stake before asking — deleting a folder takes
        // everything under it.
        let message = if meta.is_dir() {
            let n = fs::read_dir(path).map(|d| d.count()).unwrap_or(0);
            format!(
                "Delete folder {} and its {} entr{}?  (y/n)",
                name,
                n,
                if n == 1 { "y" } else { "ies" }
            )
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
    /// Buffers under the deleted path are dropped — including unsaved ones,
    /// since there is no longer a file to save them to. The cursor moves to the
    /// parent directory where it can, rather than staying on an index that now
    /// points at a different file.
    pub(super) fn do_delete(&mut self, path: &Path) {
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
                self.rows = self.tree.flatten();
                if let Some(i) = parent.and_then(|p| self.rows.iter().position(|r| r.path == p)) {
                    self.selected = i;
                } else {
                    self.selected = self.selected.min(self.rows.len().saturating_sub(1));
                }
                self.sync_preview();
                // `:delete` can be run from the editor. If the pane holding the
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
