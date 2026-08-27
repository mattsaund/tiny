//! The plain keys: moving around, opening, folding, refreshing.
//!
//! What is left of the keyboard once the overlays, the commands and the file
//! operations have had their share. These are the ones with no argument, no
//! confirmation and no mode: press the key, something moves.
//!
//! Two of them do two things each, on purpose. Right-arrow expands a folder or
//! steps into an open one or opens a file; left-arrow closes a folder or jumps
//! to the parent. That is what makes them read as "inwards" and "outwards"
//! rather than as four separate keys you have to choose between.

use crate::config::keys::Action;
use crate::files::media;

use super::App;
use super::mode::{Confirm, ConfirmKind, Focus, Mode};
use super::parts::display_name;
use super::preview::Preview;

/// How much of the window one press of the resize keys moves.
///
/// A twentieth: ten presses cross the whole range, which is few enough to be
/// worth holding the key down for and many enough to land where you meant.
const TREE_WIDTH_STEP: f32 = 0.05;

/// The narrowest and widest the browser goes, matching what `Config::sanitized`
/// will accept — a pane the keys could reach but the config would clamp back
/// would be a width that changed by itself on the next restart.
const TREE_WIDTH_MIN: f32 = 0.10;
const TREE_WIDTH_MAX: f32 = 0.60;

impl App {
    // ---- actions ----------------------------------------------------------

    /// Right-arrow on the tree: expand a closed folder, step into an
    /// already-open one, or focus the preview for a file.
    ///
    /// This is the "inwards" key, and the mirror of
    /// [`App::collapse_or_parent`]: holding it down walks you down a branch.
    /// Enter is deliberately different — see [`App::toggle_or_open`].
    pub(super) fn activate(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir {
            if row.expanded {
                self.move_selection(1);
            } else {
                self.tree.expand(&row.path);
                self.rebuild_rows();
            }
        } else {
            self.focus_editor();
        }
    }

    /// Enter on the tree: open a closed folder, close an open one, or focus
    /// the preview for a file.
    ///
    /// Unlike [`App::activate`], the cursor never moves. Pressing Enter twice
    /// on a folder leaves the tree exactly as it was found, which is what most
    /// file browsers do and what the key reads as — one thing, toggled.
    pub(super) fn toggle_or_open(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir {
            self.tree.toggle(&row.path);
            self.rebuild_rows();
        } else {
            self.focus_editor();
        }
    }

    /// Open whatever the cursor is on.
    ///
    /// Every text file opens the same way: into the editor, with the cursor in
    /// it. There is no reading mode to pass through — markdown keeps its
    /// formatting while you edit it (see `ui::live_rows`), so the rendered
    /// view was a step that no longer bought anything.
    ///
    /// A picture or a video is the one thing that opens *out* of tiny, in
    /// whatever program the desktop already uses for it. The keyboard stays on
    /// the tree, because there is nothing in the pane to give it to and
    /// leaving it there means the next arrow key still moves the cursor
    /// instead of needing an Esc first.
    ///
    /// Does nothing for a directory, and explains itself for a binary.
    pub(super) fn focus_editor(&mut self) {
        match &self.preview {
            Preview::Buffer { .. } => {
                self.focus = Focus::Editor;
                self.status = "editing — Ctrl+S save, Esc back".into();
            }
            Preview::Media { path, .. } => {
                let path = path.clone();
                self.status = match media::open_with_desktop(&path) {
                    Ok(()) => format!("opened {} outside tiny", display_name(&path)),
                    Err(e) => format!("{e:#}"),
                };
            }
            Preview::Binary { kind, .. } => {
                self.status = format!("{kind} — not editable");
            }
            _ => {}
        }
    }

    /// Left-arrow: close an open folder, or jump to the parent of anything
    /// else. Two behaviours on one key, which is what makes it feel like
    /// "outwards".
    pub(super) fn collapse_or_parent(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir && row.expanded {
            self.tree.collapse(&row.path);
            self.rebuild_rows();
            return;
        }
        let Some(parent) = row.path.parent() else {
            return;
        };
        if let Some(i) = self.rows.iter().position(|r| r.path == parent) {
            self.select_index(i);
        }
    }

    /// Fold the tree away, or bring it back.
    ///
    /// The keyboard cannot sit in a pane that is not on screen, so folding
    /// moves it to the preview and unfolding puts it back where it was. Press
    /// the key twice and nothing has changed, which is what a toggle should
    /// mean.
    ///
    /// The message names the key from the keymap rather than spelling it out,
    /// because this one is rebindable and a hint that lies is worse than none.
    pub(super) fn toggle_tree_pane(&mut self) {
        let key = self.keymap.spec(Action::ToggleTreePane);
        if self.tree_hidden {
            self.tree_hidden = false;
            // Back to whichever pane had it, so the key is a true toggle.
            self.focus = self.focus_before_hide;
            self.status = format!("tree back — {key} to hide");
        } else {
            self.tree_hidden = true;
            self.focus_before_hide = self.focus;
            self.focus = Focus::Editor;
            self.status = format!("tree hidden — {key} to bring it back");
        }
    }

    /// One step of `Ctrl+Shift+Left` / `Ctrl+Shift+Right`: a narrower or a
    /// wider browser, and the file pane takes whatever it gives up.
    ///
    /// The two ends of the range meet the fold key rather than stopping dead.
    /// Narrowing past the minimum folds the browser away, and widening from
    /// there brings it back — so one pair of keys covers everything from "gone"
    /// to "most of the window", and you never have to remember a second chord
    /// to get out of the corner you just walked into.
    ///
    /// Live only. The width is a view state like the fold, and writing it to
    /// `tiny.conf` on every keypress would make a gesture you use while
    /// reading into a change to your configuration. `*set tree_width 0.4` is
    /// still there for a width you want to keep.
    pub(super) fn resize_tree_pane(&mut self, step: i32) {
        if self.tree_hidden {
            // Nothing to resize while it is folded away; widening is the
            // gesture that asks for it back, and narrowing has nowhere to go.
            if step > 0 {
                self.toggle_tree_pane();
            }
            return;
        }
        let want = self.config.tree_width + step as f32 * TREE_WIDTH_STEP;
        if want < TREE_WIDTH_MIN {
            // Already as narrow as it goes: the next press is asking for it
            // gone, which is what the fold key does.
            self.toggle_tree_pane();
            return;
        }
        self.config.tree_width = want.min(TREE_WIDTH_MAX);
        self.status = format!(
            "browser {}% of the window",
            (self.config.tree_width * 100.0).round()
        );
    }

    /// Put the tree on screen and give it the keyboard. What Esc means from
    /// the preview, whether or not the tree was folded away — "back to the
    /// tree" has to end up at the tree either way.
    pub(super) fn focus_tree(&mut self) {
        self.tree_hidden = false;
        self.focus = Focus::Tree;
    }

    /// `.`: show or hide dotfiles. Writes through to the config so the setting
    /// is consistent with `:set show_hidden`, but does not persist it — that
    /// still needs an explicit save.
    pub(super) fn toggle_hidden(&mut self) {
        let next = !self.tree.show_hidden();
        self.tree.set_show_hidden(next);
        self.config.show_hidden = next;
        self.rebuild_rows();
        self.status = if next {
            "showing hidden files".into()
        } else {
            "hiding hidden files".into()
        };
    }

    /// Re-read the project from disk. The manual replacement for a file
    /// watcher (see `tree`'s module docs). Clean buffers are dropped so they
    /// pick up external changes; dirty ones are kept.
    pub(super) fn refresh(&mut self) {
        self.tree.refresh_all();
        // Drop clean buffers so they re-read from disk; keep unsaved work.
        self.buffers.retain(|_, e| e.dirty);
        self.rebuild_rows();
        self.status = "refreshed".into();
    }

    /// Quit, or ask first if anything is unsaved. The prompt names every dirty
    /// file, since "discard changes?" is unanswerable without knowing which.
    pub(super) fn request_quit(&mut self) {
        let dirty = self.dirty_buffers();
        if dirty.is_empty() {
            self.should_quit = true;
            return;
        }
        let names: Vec<String> = dirty.iter().map(|p| display_name(p)).collect();
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::QuitUnsaved,
            message: format!("Discard unsaved changes to {}?  (y/n)", names.join(", ")),
        });
    }
}
