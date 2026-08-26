//! What the cursor is on, and what the right pane becomes because of it.
//!
//! The single seam between the two panes. The tree does not know the preview
//! exists and the preview does not know about the tree; they are coupled only
//! by [`App::sync_preview`], which runs after every cursor move and decides
//! which [`Preview`] the right pane is showing.
//!
//! # The order of the questions
//!
//! [`App::load_file_preview`] asks them in an order that matters. An already
//! open buffer wins over anything on disk, so a file with unsaved edits is
//! never re-read out from under them. Then media, which is described rather
//! than opened. Then the size ceiling. Only then is the file read, and only
//! then is it decided to be text or binary — a file that is not valid UTF-8,
//! or that holds a zero byte, is reported as binary rather than opened as
//! mojibake.
//!
//! # Prose and code are different files
//!
//! [`TextKind`] is what separates "this reads better wrapped and rendered"
//! from "this is source". It decides line numbers, wrapping, and whether an
//! unfocused preview shows a rendered page or the text itself. It never
//! decides whether something can be edited: every text file opens into the
//! editor the same way.

use std::fs;
use std::path::{Path, PathBuf};

use crate::files::media;
use crate::text::editor::Editor;

use super::App;
use super::parts::display_name;

/// How a text file wants to be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// Rendered markdown.
    Markdown,
    /// Wrapped prose — notes, licences, logs.
    Prose,
    /// Source. Straight into the editor, with line numbers.
    Code,
}

impl TextKind {
    /// Whether an *unfocused* preview of this shows it rendered rather than as
    /// source. Prose and markdown read better formatted when there is no
    /// cursor in them; code is already at its most readable as itself.
    ///
    /// Says nothing about editing. Every text file opens into the editor the
    /// same way — see [`App::focus_editor`].
    pub fn reads_first(self) -> bool {
        self != TextKind::Code
    }
}

/// Extensionless files that are prose by convention rather than by suffix.
///
/// Without this, `LICENSE` and `README` would be classified as code and open
/// straight into the editor with line numbers, which is not what anyone wants
/// from a licence file.
pub(super) const PROSE_NAMES: &[&str] = &[
    "LICENCE",
    "LICENSE",
    "COPYING",
    "NOTICE",
    "AUTHORS",
    "CONTRIBUTORS",
    "CHANGELOG",
    "CHANGES",
    "README",
    "TODO",
];

/// Decide how a text file should open.
///
/// Checked in order: markdown first (it is usually also in the prose list but
/// has a renderer of its own), then the configured prose extensions, then
/// anything else with an extension is code. Extensionless files fall back to
/// [`PROSE_NAMES`], and are code otherwise — which is the right default for a
/// `Makefile` or a shell script.
pub(super) fn text_kind(path: &Path, prose_exts: &[String]) -> TextKind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
    {
        // Markdown is checked first: it is also in the prose list by default,
        // but it has a renderer of its own.
        Some(e) if matches!(e.as_str(), "md" | "markdown" | "mdown" | "mkd") => TextKind::Markdown,
        Some(e) if prose_exts.iter().any(|p| p.eq_ignore_ascii_case(&e)) => TextKind::Prose,
        Some(_) => TextKind::Code,
        None => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            if PROSE_NAMES.contains(&name.as_str()) {
                TextKind::Prose
            } else {
                TextKind::Code
            }
        }
    }
}

/// What the preview pane is showing for the current selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// A text file with an open buffer. Notes and prose render when the tree
    /// has focus and show raw source while being edited.
    Buffer {
        path: PathBuf,
        kind: TextKind,
    },
    /// A picture or a video. Described in words, not drawn — Enter hands it
    /// to the desktop's own viewer. See [`crate::files::media`].
    Media {
        path: PathBuf,
        kind: media::Kind,
        size: u64,
        info: media::Info,
    },
    Directory {
        path: PathBuf,
        entries: usize,
    },
    Binary {
        path: PathBuf,
        size: u64,
        kind: String,
    },
    Unreadable(String),
    Empty,
}

/// Files larger than this are not loaded into the editor. Shown as a
/// "large file" preview instead — the undo model snapshots whole buffers, so
/// there is a real ceiling on what can be edited comfortably.
pub(super) const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;

/// A human word for a non-text file, so the preview can say "PDF" or "archive"
/// instead of just "binary".
pub(super) fn binary_kind(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("pdf") => "PDF",
        Some("zip" | "gz" | "tar" | "xz" | "zst" | "7z") => "archive",
        Some("mp3" | "wav" | "flac" | "ogg") => "audio",
        Some("so" | "dll" | "dylib" | "o" | "a") => "object file",
        _ => "binary",
    }
}

impl App {
    // ---- selection & preview ---------------------------------------------

    /// Re-flatten the tree and keep the cursor on the same *file*, not the same
    /// index.
    ///
    /// This is the difference between a refresh feeling stable and feeling like
    /// the list jumped: rows shift whenever anything above them is added,
    /// removed, expanded or collapsed, so the path is remembered and looked up
    /// again afterwards. Only when the file is genuinely gone does the index
    /// get clamped instead.
    pub(super) fn rebuild_rows(&mut self) {
        let want = self.selected_path().map(Path::to_path_buf);
        self.rows = self.tree.flatten();
        if let Some(want) = want {
            if let Some(i) = self.rows.iter().position(|r| r.path == want) {
                self.selected = i;
            } else {
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            }
        }
        self.sync_preview();
    }

    /// Open every folder on the way to `path` and put the cursor on it.
    ///
    /// Necessary because the tree loads lazily: a file three directories deep
    /// does not exist as a row until its parents have been expanded. Returns
    /// false when the path is outside the project or could not be found, and
    /// still syncs the preview either way so the pane never shows something
    /// stale.
    pub(super) fn reveal(&mut self, path: &Path) -> bool {
        let root = self.tree.root_path().to_path_buf();
        let Ok(rel) = path.strip_prefix(&root) else {
            return false;
        };
        let mut cur = root;
        let parts: Vec<_> = rel.components().collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            cur.push(part);
            self.tree.expand(&cur);
        }
        self.rows = self.tree.flatten();
        match self.rows.iter().position(|r| r.path == path) {
            Some(i) => {
                self.selected = i;
                self.sync_preview();
                true
            }
            None => {
                self.sync_preview();
                false
            }
        }
    }

    /// Point the preview at whatever the cursor is now on, and reset its
    /// scroll.
    ///
    /// Called after every cursor move. This is the single seam between the two
    /// panes — the tree does not know about the preview and vice versa; they
    /// are coupled only here.
    pub(super) fn sync_preview(&mut self) {
        self.preview_scroll = 0;
        let Some(row) = self.selected_row().cloned() else {
            self.preview = Preview::Empty;
            return;
        };
        if row.is_dir {
            if row.unreadable {
                self.preview = Preview::Unreadable(format!("cannot read {}", row.name));
                return;
            }
            let entries = fs::read_dir(&row.path).map(|d| d.count()).unwrap_or(0);
            self.preview = Preview::Directory {
                path: row.path,
                entries,
            };
            return;
        }
        self.preview = self.load_file_preview(&row.path);
    }

    /// Classify a file and, if it is text, open a buffer for it.
    ///
    /// Order matters. An already-open buffer wins over anything on disk, so a
    /// file with unsaved edits is never re-read out from under them. Then
    /// media, then the size ceiling, then a decode attempt — a file that is not
    /// valid UTF-8, or that contains a zero byte, is reported as binary rather
    /// than opened as mojibake.
    ///
    /// The `\0` check catches UTF-16 and similar, which decode as valid UTF-8
    /// often enough to slip past `from_utf8` alone.
    fn load_file_preview(&mut self, path: &Path) -> Preview {
        // A buffer already open, possibly with unsaved edits, wins over disk.
        if self.buffers.contains_key(path) {
            return Preview::Buffer {
                path: path.to_path_buf(),
                kind: text_kind(path, &self.config.prose_extensions),
            };
        }
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => return Preview::Unreadable(format!("{}: {e}", display_name(path))),
        };
        let size = meta.len();

        let kind = media::classify(path);
        if kind != media::Kind::Other {
            return Preview::Media {
                path: path.to_path_buf(),
                kind,
                size,
                info: media::probe(path, kind),
            };
        }
        if size > MAX_EDIT_BYTES {
            return Preview::Binary {
                path: path.to_path_buf(),
                size,
                kind: "large file".into(),
            };
        }
        match fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => {
                    // A fresh buffer reports no edits, so the highlight cache
                    // would go on believing whatever it last knew about this
                    // path — which, after a rename or a delete, is a different
                    // file's contents.
                    self.highlight_cache.clear();
                    self.buffers.insert(
                        path.to_path_buf(),
                        Editor::from_str(path.to_path_buf(), &text),
                    );
                    Preview::Buffer {
                        path: path.to_path_buf(),
                        kind: text_kind(path, &self.config.prose_extensions),
                    }
                }
                _ => Preview::Binary {
                    path: path.to_path_buf(),
                    size,
                    kind: binary_kind(path).into(),
                },
            },
            Err(e) => Preview::Unreadable(format!("{}: {e}", display_name(path))),
        }
    }

    /// Move the tree cursor by `delta` rows, clamped to the list. Only syncs
    /// the preview when the cursor actually moved, so holding a key at the end
    /// of the list does not re-read the same file repeatedly.
    pub(super) fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        if next != self.selected {
            self.selected = next;
            self.sync_preview();
        }
    }

    pub(super) fn select_index(&mut self, i: usize) {
        let i = i.min(self.rows.len().saturating_sub(1));
        if i != self.selected {
            self.selected = i;
            self.sync_preview();
        }
    }
}
