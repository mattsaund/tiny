//! Every keypress, and where it goes.
//!
//! [`App::on_key`] is the whole of tiny's input handling, and it dispatches on
//! [`Mode`] first and [`Focus`] second — see the folder's docs for why those
//! two answer almost every "why did that key do that".
//!
//! # The mode take-and-replace pattern
//!
//! `on_key` does `std::mem::replace(&mut self.mode, Mode::Normal)` and hands
//! the owned mode to a handler. That is not only a borrow-checker workaround:
//! it makes **closing the default and staying open explicit**. A handler that
//! does nothing leaves `Normal` behind, so every handler that wants its
//! overlay to persist has to put it back. Forget it and the overlay closes,
//! which is the right direction for that mistake to fail in.
//!
//! # The wheel
//!
//! One notch, one line, and nothing else the mouse reports is listened to.
//! [`App::on_scroll`] takes the column so it can tell which pane the pointer
//! was over, which is the only thing tiny ever asks the mouse's position.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::keys::{Action, Context as KeyContext};
use crate::map::view::Intent;
use crate::text::editor::Editor;

use super::App;
use super::mode::{Focus, Mode, Window};
use super::parts::list_move;
use super::preview::Preview;
use super::source::GitFocus;

/// How far `Ctrl+Up` and `Ctrl+Down` jump, in the tree and in the editor.
///
/// A fixed count rather than a fraction of the pane: the point of it is a step
/// you can feel the size of and repeat, which a number tied to the window
/// height would not be. `PageUp`/`PageDown` are the ones that move by a screen.
const JUMP_LINES: usize = 5;

/// Apply one key to a buffer, and say what to complain about if anything.
///
/// The one text-editing keyboard in tiny, in one place. The editor pane uses
/// it, and so does the commit message box in the source window — two panes that
/// type, which must not disagree about what Backspace does.
///
/// `action` is what the keymap made of the key, already resolved by the caller
/// so the borrow of the buffer can come after it. Anything the keymap did not
/// name falls through to the fixed half: the arrows, Enter, Backspace and the
/// characters themselves, which are not bindable because a keyboard that cannot
/// type is not a keyboard.
pub(super) fn edit_with_key(
    ed: &mut Editor,
    action: Option<Action>,
    key: KeyEvent,
    page: usize,
    tab_width: usize,
) -> Option<&'static str> {
    match action {
        Some(Action::EditorUndo) => return (!ed.undo()).then_some("nothing to undo"),
        Some(Action::EditorRedo) => return (!ed.redo()).then_some("nothing to redo"),
        Some(Action::EditorDeleteLine) => {
            ed.delete_line();
            return None;
        }
        Some(Action::EditorWordLeft) => {
            ed.move_word_left();
            return None;
        }
        Some(Action::EditorWordRight) => {
            ed.move_word_right();
            return None;
        }
        Some(Action::EditorJumpUp) => {
            ed.page_up(JUMP_LINES);
            return None;
        }
        Some(Action::EditorJumpDown) => {
            ed.page_down(JUMP_LINES);
            return None;
        }
        Some(Action::EditorLineStart) => {
            ed.move_home();
            return None;
        }
        Some(Action::EditorLineEnd) => {
            ed.move_end();
            return None;
        }
        Some(Action::EditorDocStart) => {
            ed.move_doc_start();
            return None;
        }
        Some(Action::EditorDocEnd) => {
            ed.move_doc_end();
            return None;
        }
        _ => {}
    }
    match key.code {
        KeyCode::Left => ed.move_left(),
        KeyCode::Right => ed.move_right(),
        KeyCode::Up => ed.move_up(),
        KeyCode::Down => ed.move_down(),
        KeyCode::PageUp => ed.page_up(page),
        KeyCode::PageDown => ed.page_down(page),
        KeyCode::Enter => ed.insert_newline(),
        KeyCode::Backspace => ed.backspace(),
        KeyCode::Delete => ed.delete_forward(),
        KeyCode::Tab => ed.insert_tab(tab_width),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => ed.insert_char(c),
        _ => {}
    }
    None
}

impl App {
    // ---- key dispatch -----------------------------------------------------

    /// The single entry point for input. Dispatches in strict priority order:
    /// map, then mode, then focus.
    ///
    /// See the module docs for the take-and-replace pattern: the mode is moved
    /// out and `Normal` left in its place, so a handler that does nothing
    /// closes its overlay, and one that wants to stay open has to say so.
    pub fn on_key(&mut self, key: KeyEvent) {
        // The window switcher answers before anything else, from every window
        // and through every overlay. A key that changes which window you are
        // looking at cannot be one that a pane might swallow first, or it
        // would work from some places and not others.
        if let Some(window) = self.window_key(&key) {
            return self.show_window(window);
        }
        // An overlay owns the keyboard wherever it was opened from — the bar
        // is drawn over every window, so it has to be typed into from every
        // window. Taken out and left as `Normal`, so a handler that does
        // nothing closes it; see the module docs.
        if !matches!(self.mode, Mode::Normal) {
            return match std::mem::replace(&mut self.mode, Mode::Normal) {
                Mode::Help(scroll) => self.on_help_key(scroll, key),
                Mode::Confirm(c) => self.on_confirm_key(c, key),
                Mode::Bar(b) => self.on_bar_key(b, key),
                Mode::Settings(s) => self.on_settings_key(s, key),
                Mode::Keybinds(kb) => self.on_keybinds_key(kb, key),
                Mode::Normal => {}
            };
        }
        // With nothing over it, the window that is not the main one takes the
        // whole screen and every key with it.
        match self.window {
            Window::Map => self.on_map_key(key),
            Window::Source => self.on_source_key(key),
            Window::Main => match self.focus {
                Focus::Tree => self.on_tree_key(key),
                Focus::Editor => self.on_editor_key(key),
            },
        }
    }

    /// Keys for the source-control window.
    ///
    /// The same shape as the map's: the window owns the whole screen, so there
    /// is no focus to consider and no mode to fall through to — one lookup, one
    /// action, and Esc is the way out.
    fn on_source_key(&mut self, key: KeyEvent) {
        // The message box is a pane that types, so it takes the keyboard whole
        // — a `k` in a commit message is the letter k.
        if self.git.focus == GitFocus::Message {
            return self.on_message_key(key);
        }
        let Some(action) = self.keymap.resolve(KeyContext::Source, &key) else {
            return;
        };
        // The global chords work here too: `Ctrl+P` is how `*commit` is typed,
        // and a window you cannot run a command from is a window with a hole
        // in it.
        if self.on_global_action(action) {
            return;
        }
        match action {
            Action::SourceUp => self.move_git_cursor(-1),
            Action::SourceDown => self.move_git_cursor(1),
            Action::SourceJumpUp => self.move_git_cursor(-(JUMP_LINES as isize)),
            Action::SourceJumpDown => self.move_git_cursor(JUMP_LINES as isize),
            Action::SourceLeft => self.move_git_button(-1),
            Action::SourceRight => self.move_git_button(1),
            Action::SourceEnter => self.activate_git(),
            Action::SourceOpen => self.open_from_git(),
            Action::SourceRefresh => {
                self.refresh_git();
                self.status = "asked git again".into();
            }
            Action::SourcePageUp => self.page_git_diff(-1),
            Action::SourcePageDown => self.page_git_diff(1),
            Action::SourceNarrower => self.resize_tree_pane(-1),
            Action::SourceWider => self.resize_tree_pane(1),
            Action::SourceClose => self.show_window(Window::Main),
            _ => {}
        }
    }

    /// Which window this key asks for, if it asks for one.
    fn window_key(&self, key: &KeyEvent) -> Option<Window> {
        match self.keymap.find(KeyContext::Global, key) {
            Some(Action::WindowMain) => Some(Window::Main),
            Some(Action::WindowSource) => Some(Window::Source),
            Some(Action::WindowMap) => Some(Window::Map),
            _ => None,
        }
    }

    /// Arrows scroll the keymap; anything else puts it away.
    fn on_help_key(&mut self, scroll: usize, key: KeyEvent) {
        let page = self.last_tree_height.max(1);
        // No last row to clamp to here: `ui` knows how long the keymap is and
        // clamps as it draws.
        let Some(next) = list_move(key, scroll, usize::MAX, page) else {
            return;
        };
        self.mode = Mode::Help(next);
    }

    /// Forward a key to the map and act on what it asks for. The view
    /// handles its own navigation; only opening a file and closing the map
    /// need application state.
    fn on_map_key(&mut self, key: KeyEvent) {
        // Resolved here rather than inside the view: the map does not need to
        // know how a key becomes an action, only which one it was.
        let action = self.keymap.find(KeyContext::Map, &key);
        let intent = match self.project_map.as_mut() {
            Some(view) => view.on_key(key, action),
            None => return,
        };
        match intent {
            Intent::None => {}
            Intent::Close => {
                self.show_window(Window::Main);
                return;
            }
            Intent::Open(path) => {
                self.window = Window::Main;
                self.open_path(&path);
                return;
            }
            Intent::Rebuild => {
                // Same path as opening it, so a rebuilt map is indistinguishable
                // from a freshly opened one.
                self.open_map();
                return;
            }
        }
        // The counts move as filters change, so keep them in front of you.
        if let Some(view) = self.project_map.as_ref() {
            self.status = format!("project map — {}", view.summary());
        }
    }

    /// Every chord in [`KeyContext::Global`], in one place.
    ///
    /// These are the controls that have to work while you are typing into a
    /// file, which is what makes them chords rather than letters: `n` is the
    /// letter n in the editor and cannot be anything else, so "new" is
    /// `Ctrl+N` everywhere instead of `n` in one pane and nothing in the
    /// other.
    ///
    /// Handled here rather than twice, because a global that the tree and the
    /// editor disagreed about would not be global. Returns whether the action
    /// was one of them; [`Action::Save`] and the two "leave this pane" keys
    /// are deliberately not, since what they mean does depend on where you
    /// are.
    fn on_global_action(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => self.request_quit(),
            Action::Bar => self.open_bar(false),
            Action::CommandBar => self.open_bar(true),
            Action::ToggleTreePane => self.toggle_tree_pane(),
            Action::Copy => self.copy_selection(),
            Action::Paste => self.paste_clipboard(),
            Action::Help => self.mode = Mode::Help(0),
            Action::WindowMain => self.show_window(Window::Main),
            Action::WindowSource => self.show_window(Window::Source),
            Action::WindowMap => self.show_window(Window::Map),
            _ => return false,
        }
        true
    }

    /// Keys for the tree pane.
    ///
    /// Single letters are free here — unlike the editor, nothing is being
    /// typed — so `n`, `r`, `m` and friends stay bound as bare keys alongside
    /// the chords that do the same thing from anywhere. Two ways to reach one
    /// action, and the tree is the pane where the short one still works.
    ///
    /// A few keys are *only* here: the browser's own width, on `Ctrl` with an
    /// arrow, which cannot be global because those two are word motions in the
    /// editor.
    fn on_tree_key(&mut self, key: KeyEvent) {
        let page = self.last_tree_height.saturating_sub(1).max(1) as isize;
        let Some(action) = self.keymap.resolve(KeyContext::Tree, &key) else {
            return;
        };
        if self.on_global_action(action) {
            return;
        }
        match action {
            Action::Save => self.save_from_tree(),
            Action::TreeQuit => self.request_quit(),
            Action::TreeBar => self.open_bar(false),
            Action::TreeUp => self.move_selection(-1),
            Action::TreeDown => self.move_selection(1),
            Action::TreeFirst => self.select_index(0),
            Action::TreeLast => self.select_index(usize::MAX),
            Action::TreeJumpUp => self.move_selection(-(JUMP_LINES as isize)),
            Action::TreeJumpDown => self.move_selection(JUMP_LINES as isize),
            Action::TreePageUp => self.move_selection(-page),
            Action::TreePageDown => self.move_selection(page),
            Action::TreeOpen => self.toggle_or_open(),
            Action::TreeInto => self.activate(),
            Action::TreeOut => self.collapse_or_parent(),
            Action::TreePreview => self.focus_editor(),
            Action::TreeHidden => self.toggle_hidden(),
            Action::TreeNarrower => self.resize_tree_pane(-1),
            Action::TreeWider => self.resize_tree_pane(1),
            _ => {}
        }
    }

    /// Keys for the preview pane.
    ///
    /// The escape hatches come first and return early, so nothing that leaves
    /// the pane or acts on the file can be swallowed as text input. That is
    /// also why `Ctrl+P` exists as a second binding for the command bar: a
    /// bare `:` in the editor is a colon being typed.
    ///
    /// Below that the function forks on whether there is a buffer behind the
    /// preview: a text file types, a picture scrolls.
    fn on_editor_key(&mut self, key: KeyEvent) {
        let tab_width = self.config.tab_width;
        let page = self.last_edit_height.saturating_sub(1).max(1);

        // Keys that leave the pane or act on the file come first, so they can
        // never be swallowed as text input. Everything global reaches here,
        // which is the point of the chords: `Ctrl+N` makes a file from inside
        // the editor, where a bare `n` is the letter n and always will be.
        match self.keymap.resolve(KeyContext::Editor, &key) {
            Some(Action::Save) => return self.save_active(),
            // "Back to the tree" means the tree, even when it is folded away,
            // so it brings the pane back rather than handing the keyboard to
            // something that is not on screen.
            Some(Action::EditorBack) => {
                self.focus_tree();
                self.status = "back to tree".into();
                return;
            }
            // The guard does the work: `on_global_action` runs the action if
            // it recognizes it and says so, and an action it does not
            // recognize falls through to the editing keyboard below.
            Some(action) if self.on_global_action(action) => return,
            _ => {}
        }

        // A picture, a directory listing, a binary: nothing to type into, so
        // the arrows move the view instead of a cursor.
        if !matches!(self.preview, Preview::Buffer { .. }) {
            return self.on_view_key(key, page);
        }

        // Read what the key meant before borrowing the buffer, so nothing has
        // to be cloned to satisfy the borrow checker on every keystroke.
        let action = self.keymap.find(KeyContext::Editor, &key);
        let Some(ed) = self.active_buffer_mut() else {
            self.focus = Focus::Tree;
            return;
        };
        if let Some(complaint) = edit_with_key(ed, action, key, page, tab_width) {
            self.status = complaint.into();
        }
    }

    /// Looking at something with no text behind it — a picture, a directory,
    /// a binary. Arrows scroll the view, because there is no cursor to move.
    ///
    /// The one place `preview_scroll` is driven by the keyboard. A text file
    /// never lands here: it has a buffer, and the buffer has a cursor.
    fn on_view_key(&mut self, key: KeyEvent, page: usize) {
        let max = self.preview_len.saturating_sub(1);
        let Some(action) = self.keymap.find(KeyContext::View, &key) else {
            return;
        };
        match action {
            Action::ViewUp => self.preview_scroll = self.preview_scroll.saturating_sub(1),
            Action::ViewDown => self.preview_scroll = (self.preview_scroll + 1).min(max),
            Action::ViewTop => self.preview_scroll = 0,
            Action::ViewBottom => self.preview_scroll = max,
            Action::ViewPageUp => self.preview_scroll = self.preview_scroll.saturating_sub(page),
            Action::ViewPageDown => self.preview_scroll = (self.preview_scroll + page).min(max),
            Action::ViewBar => self.open_bar(false),
            _ => {}
        }
    }

    /// One notch of the mouse wheel, over the pane at `column`.
    ///
    /// Always exactly one line. The wheel is the one input where a terminal
    /// will happily send three of something for one flick of a finger, and a
    /// note that jumps three lines at a time is hard to read against.
    ///
    /// What "one line" means depends on the pane. A rendered note or a picture
    /// has no cursor, so the view itself moves. The editor does have one, and
    /// its view is tied to it, so the cursor moves instead — which scrolls the
    /// view once it reaches an edge, and never leaves you looking at somewhere
    /// you cannot type.
    pub fn on_scroll(&mut self, down: bool, column: u16) {
        let step: isize = if down { 1 } else { -1 };
        if self.over_tree(column) {
            // The preview follows the tree cursor, so moving the view on its
            // own would only be undone on the next frame.
            self.move_selection(step);
            return;
        }
        // A file being edited has a cursor and a view tied to it; anything
        // else is just a picture of something, and the view itself moves.
        if !matches!(self.preview, Preview::Buffer { .. }) || self.focus != Focus::Editor {
            let max = self.preview_len.saturating_sub(1);
            self.preview_scroll = if down {
                (self.preview_scroll + 1).min(max)
            } else {
                self.preview_scroll.saturating_sub(1)
            };
        } else if let Some(ed) = self.active_buffer_mut() {
            if down {
                ed.move_down();
            } else {
                ed.move_up();
            }
        }
    }

    /// Whether `column` fell inside the tree pane as it was last drawn.
    ///
    /// False whenever the tree is not on screen, which covers both `Ctrl+Space`
    /// and the very first frame, before `ui` has said where anything is.
    fn over_tree(&self, column: u16) -> bool {
        matches!(self.last_tree_cols, Some((x0, x1)) if column >= x0 && column < x1)
    }
}
