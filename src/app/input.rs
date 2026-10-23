//! Every keypress, and where it goes.
//!
//! [`App::handle_key`] is the whole of tiny's input handling, and it
//! dispatches on [`Mode`] first and [`Focus`] second, see the folder's docs
//! for why those two answer almost every "why did that key do that".
//!
//! # The mode take-and-replace pattern
//!
//! `handle_key` does `std::mem::replace(&mut self.mode, Mode::Normal)` and
//! hands the owned mode to a handler. That is not only a borrow-checker
//! workaround: it makes **closing the default and staying open explicit**. A
//! handler that does nothing leaves `Normal` behind, so every handler that
//! wants its overlay to persist has to put it back. Forget it and the overlay
//! closes, which is the right direction for that mistake to fail in.
//!
//! # The wheel
//!
//! One notch, one line, and nothing else the mouse reports is listened to.
//! [`App::on_scroll`] takes the column so it can tell which pane the pointer
//! was over, which is the only thing tiny ever asks the mouse's position.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::keys::{Action, Context as KeyContext, Keymap, from_terminal};
use crate::map::view::Intent;
use crate::text::editor::Editor;

use super::App;
use super::mode::{Focus, Mode, Window};
use super::parts::list_move;
use super::preview::Preview;
use super::source::GitFocus;

/// What a key means in a file: the action the keymap names for it, and
/// whether it selects as it moves.
///
/// Shift with a key that moves the cursor selects as it moves, see
/// `config::keys`. When the key with Shift is not bound to anything itself,
/// the same key without it is looked up, and a movement found that way
/// selects. The arrows and Page Up and Down are not looked up at all, being
/// the fixed half of the keyboard, so for them the Shift alone says so.
pub(super) fn editing_action(keymap: &Keymap, key: &KeyEvent) -> (Option<Action>, bool) {
    if let Some(action) = keymap.find(KeyContext::Editor, key) {
        return (Some(action), false);
    }
    let shifted = key.modifiers.contains(KeyModifiers::SHIFT)
        && !matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab);
    if !shifted {
        return (None, false);
    }
    let plain = KeyEvent {
        modifiers: key.modifiers - KeyModifiers::SHIFT,
        ..*key
    };
    match keymap.find(KeyContext::Editor, &plain) {
        Some(action) if moves(action) => (Some(action), true),
        _ => (None, true),
    }
}

/// The cursor movements a key can be bound to, each with the version that
/// selects as it goes.
fn movement(action: Action) -> Option<[fn(&mut Editor); 2]> {
    Some(match action {
        Action::EditorWordLeft => [Editor::move_word_left, Editor::select_word_left],
        Action::EditorWordRight => [Editor::move_word_right, Editor::select_word_right],
        Action::EditorLineStart => [Editor::move_home, Editor::select_home],
        Action::EditorLineEnd => [Editor::move_end, Editor::select_end],
        Action::EditorDocStart => [Editor::move_doc_start, Editor::select_doc_start],
        Action::EditorDocEnd => [Editor::move_doc_end, Editor::select_doc_end],
        _ => return None,
    })
}

/// Whether `action` moves the cursor, so Shift with it selects.
fn moves(action: Action) -> bool {
    movement(action).is_some() || matches!(action, Action::PageUp | Action::PageDown)
}

/// Apply one key to a buffer, and say what to complain about if anything.
///
/// The one text-editing keyboard in tiny, in one place. The editor pane uses
/// it, and so does the commit message box in the source window: two panes that
/// type, which must not disagree about what Backspace does.
///
/// `action` is what the keymap made of the key, and `select` whether it moves
/// selecting, both from [`editing_action`] and worked out by the caller so
/// the borrow of the buffer can come after. Anything the keymap did not name
/// falls through to the fixed half: the arrows, Enter, Backspace and the
/// characters themselves, which are not bindable because a keyboard that
/// cannot type is not a keyboard.
pub(super) fn edit_with_key(
    ed: &mut Editor,
    action: Option<Action>,
    select: bool,
    key: KeyEvent,
    page: usize,
    tab_width: usize,
) -> Option<&'static str> {
    if let Some(action) = action {
        if let Some([plain, selecting]) = movement(action) {
            if select {
                selecting(ed);
            } else {
                plain(ed);
            }
            return None;
        }
        match action {
            Action::EditorUndo => return (!ed.undo()).then_some("nothing to undo"),
            Action::EditorRedo => return (!ed.redo()).then_some("nothing to redo"),
            Action::EditorDeleteLine => ed.delete_line(),
            Action::EditorSelectAll => ed.select_all(),
            Action::EditorNextMatch => ed.add_cursor_at_next_match(),
            Action::EditorCursorUp => ed.add_cursor_vertically(false),
            Action::EditorCursorDown => ed.add_cursor_vertically(true),
            Action::PageUp if select => ed.select_page(page, false),
            Action::PageDown if select => ed.select_page(page, true),
            Action::PageUp => ed.page_up(page),
            Action::PageDown => ed.page_down(page),
            _ => return None,
        }
        return None;
    }
    match key.code {
        KeyCode::Left if select => ed.select_left(),
        KeyCode::Right if select => ed.select_right(),
        KeyCode::Up if select => ed.select_up(),
        KeyCode::Down if select => ed.select_down(),
        KeyCode::PageUp if select => ed.select_page(page, false),
        KeyCode::PageDown if select => ed.select_page(page, true),
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
        KeyCode::BackTab => ed.outdent(tab_width),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => ed.insert_char(c),
        _ => {}
    }
    None
}

impl App {
    // ---- key dispatch -----------------------------------------------------

    /// One key, and everything it leads to, done before this returns: how
    /// the tests type.
    ///
    /// The event loop does not use this: with several keys already waiting it
    /// hands each to [`App::handle_key`] and calls [`App::settle`] once, so a
    /// burst of typing into the bar is searched for once rather than once a
    /// letter.
    #[cfg(test)]
    pub fn on_key(&mut self, key: KeyEvent) {
        self.handle_key(key);
        self.settle();
    }

    /// The single entry point for input. Dispatches in strict priority order:
    /// the window switching keys, then any overlay, then the window on
    /// screen, then the pane with the keyboard.
    ///
    /// See the module docs for the take-and-replace pattern: the mode is moved
    /// out and `Normal` left in its place, so a handler that does nothing
    /// closes its overlay, and one that wants to stay open has to say so.
    ///
    /// Work that only matters to what is drawn next (a search of the
    /// project) is left for [`App::settle`].
    pub fn handle_key(&mut self, key: KeyEvent) {
        // One spelling per key, before anything reads it: macOS Terminal's
        // `⌥←` arrives as `Alt+B`, and becomes the arrow it was.
        // The terminal takes every key as it came, before anything reads it
        // as one of tiny's: `Ctrl+L` clears its screen, Alt+B goes back a
        // word in it. Only the key that leaves is tiny's, see `term`.
        if self.focus == Focus::Terminal
            && matches!(self.mode, Mode::Normal)
            && self.window == Window::Main
        {
            self.record_key(&key);
            return self.on_terminal_key(key);
        }
        let key = from_terminal(key);
        self.record_key(&key);
        // The window switcher answers before anything else, from every window
        // and through every overlay. A key that changes which window you are
        // looking at cannot be one that a pane might swallow first, or it
        // would work from some places and not others.
        if let Some(window) = self.window_key(&key) {
            return self.toggle_window(window);
        }
        // An overlay owns the keyboard wherever it was opened from: the bar
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
                Focus::Terminal => self.on_terminal_key(key),
            },
        }
    }

    /// Keys for the source-control window.
    ///
    /// The same shape as the map's: the window owns the whole screen, so there
    /// is no focus to consider and no mode to fall through to; one lookup, one
    /// action, and Esc is the way out.
    fn on_source_key(&mut self, key: KeyEvent) {
        // The message box is a pane that types, so it takes the keyboard whole:
        // a `k` in a commit message is the letter k.
        if self.git.focus == GitFocus::Message {
            return self.on_message_key(key);
        }
        let Some(action) = self.keymap.resolve(KeyContext::Source, &key) else {
            return;
        };
        // The global chords work here too: the bar opens over this window, so
        // `*commit` can be typed from it, and a window you cannot run a
        // command from is a window with a hole in it.
        if self.on_global_action(action) {
            return;
        }
        match action {
            Action::Up => self.move_git_cursor(-1),
            Action::Down => self.move_git_cursor(1),
            Action::First => self.git_to_end(false),
            Action::Last => self.git_to_end(true),
            Action::Left => self.move_git_button(-1),
            Action::Right => self.move_git_button(1),
            Action::Open => self.activate_git(),
            Action::SourceOpen => self.open_from_git(),
            Action::Refresh => {
                self.refresh_git();
                self.status = "asked git again".into();
            }
            Action::PageUp => self.page_git_diff(-1),
            Action::PageDown => self.page_git_diff(1),
            Action::Narrower => self.resize_tree_pane(-1),
            Action::Wider => self.resize_tree_pane(1),
            Action::Back => self.show_window(Window::Main),
            _ => {}
        }
    }

    /// Which window this key asks for, if it asks for one.
    ///
    /// Read with [`resolve_while_typing`](crate::config::keys::Keymap::resolve_while_typing)
    /// so that, if someone binds a window to a bare letter, that letter is
    /// still a letter in a file or a commit message.
    fn window_key(&self, key: &KeyEvent) -> Option<Window> {
        // While the keybinds window is waiting for a key, every key is data,
        // these included, or nothing could ever be bound to them.
        if matches!(&self.mode, Mode::Keybinds(kb) if kb.capturing) {
            return None;
        }
        let typing = matches!(self.mode, Mode::Bar(_) | Mode::Settings(_))
            || (self.window == Window::Main
                && self.focus == Focus::Editor
                && matches!(self.preview, Preview::Buffer { .. }))
            || (self.window == Window::Source && self.git.focus == GitFocus::Message);
        let action = if typing {
            self.keymap.resolve_while_typing(KeyContext::Global, key)
        } else {
            self.keymap.find(KeyContext::Global, key)
        };
        match action {
            Some(Action::WindowSource) => Some(Window::Source),
            Some(Action::WindowMap) => Some(Window::Map),
            _ => None,
        }
    }

    /// The window keys are toggles: the key for the window you are in takes
    /// you back to the browser, so one key opens git and the same key puts it
    /// away, the way the browser's own key folds it and brings it back.
    fn toggle_window(&mut self, window: Window) {
        if self.window == window {
            self.show_window(Window::Main);
        } else {
            self.show_window(window);
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
        // The global chords work here too, and so does the bare `/` that
        // opens the bar: the map's filter is the bar, drawn where it always
        // is. Checked before the map's own keys, like everywhere else.
        if let Some(action) = self.keymap.find(KeyContext::Global, &key)
            && self.on_global_action(action)
        {
            return;
        }
        // Resolved here rather than inside the view: the map does not need to
        // know how a key becomes an action, only which one it was.
        let action = self.keymap.find(KeyContext::Map, &key);
        let intent = match self.project_map.as_mut() {
            Some(view) => view.on_key(action),
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
                // Read the project again, and keep looking at it the way it
                // was being looked at.
                let old = self.project_map.take();
                self.open_map();
                if let (Some(old), Some(view)) = (old, &mut self.project_map) {
                    view.carry_over(&old);
                    self.status = format!("project map: {}", view.summary());
                }
                return;
            }
        }
        // The counts move as filters change, so keep them in front of you.
        if let Some(view) = self.project_map.as_ref() {
            self.status = format!("project map: {}", view.summary());
        }
    }

    /// Every action in [`KeyContext::Global`] that means the same thing
    /// wherever you are, in one place.
    ///
    /// These are the controls that have to work while you are typing into a
    /// file, which is what makes them chords: `f` is the letter f in the
    /// editor and cannot be anything else, so search is `Ctrl+F` there, with
    /// the bare `/` beside it for the panes where nothing is being typed.
    ///
    /// Handled here rather than twice, because a global that the tree and the
    /// editor disagreed about would not be global. Returns whether the action
    /// was one of them; [`Action::Save`] is deliberately not, since what it
    /// saves depends on where you are. The two window keys are not either:
    /// [`App::handle_key`] answers them before any pane is asked.
    fn on_global_action(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => self.request_quit(),
            Action::Bar => self.open_bar(),
            Action::ToggleTreePane => self.toggle_tree_pane(),
            Action::NewWindow => self.open_new_window(),
            Action::NextPane => self.next_pane(),
            Action::Terminal => self.toggle_terminal(),
            Action::Help => self.mode = Mode::Help(0),
            _ => return false,
        }
        true
    }

    /// Keys for the tree pane.
    ///
    /// Single letters are free here (unlike the editor, nothing is being
    /// typed) which is why the browser's own keys are bare: `.` for dotfiles,
    /// `<` and `>` for its width, `i` `j` `k` `l` for the arrows.
    ///
    /// Esc is `back` here as it is everywhere, and back from the browser is
    /// out of tiny, asking first if anything is unsaved.
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
            Action::Back => self.request_quit(),
            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::First => self.select_index(0),
            Action::Last => self.select_index(usize::MAX),
            Action::PageUp => self.move_selection(-page),
            Action::PageDown => self.move_selection(page),
            Action::Open => self.toggle_or_open(),
            Action::Right => self.activate(),
            Action::Left => self.collapse_or_parent(),
            Action::TreeHidden => self.toggle_hidden(),
            Action::Copy => self.copy_selection(),
            Action::Paste => self.paste_clipboard(),
            Action::Narrower => self.resize_tree_pane(-1),
            Action::Wider => self.resize_tree_pane(1),
            _ => {}
        }
    }

    /// Keys for the preview pane.
    ///
    /// The escape hatches come first and return early, so nothing that leaves
    /// the pane or acts on the file can be swallowed as text input. That is
    /// why every one of them is a chord, or Esc: a bare `/` in the editor is a
    /// slash being typed.
    ///
    /// Below that the function forks on whether there is a buffer behind the
    /// preview: a text file types, a picture scrolls.
    fn on_editor_key(&mut self, key: KeyEvent) {
        let tab_width = self.config.tab_width;
        let page = self.last_edit_height.saturating_sub(1).max(1);

        // Keys that leave the pane or act on the file come first, so they can
        // never be swallowed as text input. Everything global reaches here,
        // which is the point of the chords: `Ctrl+F` searches from inside the
        // editor, where a bare `/` is a slash and always will be. A buffer is
        // being typed into; a picture or a folder listing is not, and in front
        // of one a bare `/` should still open the bar.
        let resolved = if matches!(self.preview, Preview::Buffer { .. }) {
            self.keymap.resolve_while_typing(KeyContext::Editor, &key)
        } else {
            self.keymap.resolve(KeyContext::Editor, &key)
        };
        match resolved {
            Some(Action::Save) => return self.save_active(),
            // "Back to the tree" means the tree, even when it is folded away,
            // so it brings the pane back rather than handing the keyboard to
            // something that is not on screen. A selection, or more cursors
            // than one, is let go of first, as Esc does everywhere else.
            Some(Action::Back) => {
                if self.active_buffer_mut().is_some_and(|ed| ed.collapse()) {
                    return;
                }
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

        // A picture, a directory listing, a binary: a few lines of facts
        // with nothing to type into and nothing to scroll, the same as the
        // wheel finds over them. Copying one copies the file, there being no
        // text to copy.
        if !matches!(self.preview, Preview::Buffer { .. }) {
            if resolved == Some(Action::EditorCopy) {
                self.copy_selection();
            }
            return;
        }

        // Read what the key meant before borrowing the buffer, so nothing has
        // to be cloned to satisfy the borrow checker on every keystroke.
        let (action, select) = editing_action(&self.keymap, &key);
        match action {
            Some(Action::EditorCopy) => return self.copy_text(false),
            Some(Action::EditorCut) => return self.copy_text(true),
            Some(Action::EditorPaste) => return self.paste_text(),
            Some(Action::MacroRecord) => return self.toggle_recording(),
            Some(Action::MacroPlay) => return self.play_macro(1),
            _ => {}
        }
        let Some(ed) = self.active_buffer_mut() else {
            self.focus = Focus::Tree;
            return;
        };
        if let Some(complaint) = edit_with_key(ed, action, select, key, page, tab_width) {
            self.status = complaint.into();
        }
    }

    /// One notch of the mouse wheel, over the pane at `column` and `row`.
    ///
    /// Always exactly one line. The wheel is the one input where a terminal
    /// will happily send three of something for one flick of a finger, and a
    /// note that jumps three lines at a time is hard to read against.
    ///
    /// What "one line" means depends on the pane. A rendered note or a picture
    /// has no cursor, so the view itself moves. The editor does have one, and
    /// its view is tied to it, so the cursor moves instead, which scrolls the
    /// view once it reaches an edge, and never leaves you looking at somewhere
    /// you cannot type.
    pub fn on_scroll(&mut self, down: bool, column: u16, row: u16) {
        // The wheel goes to what is drawn under it: an overlay first, then
        // the window that is up, never a window that is not on screen.
        match &mut self.mode {
            Mode::Help(scroll) => {
                *scroll = if down {
                    scroll.saturating_add(1)
                } else {
                    scroll.saturating_sub(1)
                };
                return;
            }
            Mode::Settings(_) | Mode::Keybinds(_) => return,
            _ => {}
        }
        match self.window {
            Window::Source => return self.scroll_git(down, column),
            // One screen, walked with the arrows: nothing to scroll.
            Window::Map => return,
            Window::Main => {}
        }
        let step: isize = if down { 1 } else { -1 };
        let at = ratatui::layout::Position::new(column, row);
        if self.last_term_area.is_some_and(|r| r.contains(at)) {
            if let Some(shell) = &mut self.term {
                shell.scroll(!down, 1);
            }
            return;
        }
        if self.over_tree(column) {
            // The preview follows the tree cursor, so moving the view on its
            // own would only be undone on the next frame.
            self.move_selection(step);
            return;
        }
        let under = self.last_pinned.iter().find(|(_, r)| r.contains(at));
        if let Some(&(id, _)) = under {
            self.scroll_pinned(id, down);
            return;
        }
        match &self.preview {
            // A note the browser is looking at is drawn as a page, which
            // scrolls by its own count.
            Preview::Buffer { kind, .. } if kind.reads_first() && self.focus != Focus::Editor => {
                let max = self.preview_len.saturating_sub(1);
                self.preview_scroll = if down {
                    (self.preview_scroll + 1).min(max)
                } else {
                    self.preview_scroll.saturating_sub(1)
                };
            }
            // Anything else with text is drawn by the editor, whose view
            // follows its cursor, so the cursor is what moves, the same rule
            // the other pane scrolls by.
            Preview::Buffer { .. } => {
                if let Some(ed) = self.active_buffer_mut() {
                    if down {
                        ed.move_down();
                    } else {
                        ed.move_up();
                    }
                }
            }
            // A picture, a folder, a binary: one screen, nothing to scroll.
            _ => {}
        }
    }

    /// Whether `column` fell inside the tree pane as it was last drawn.
    ///
    /// False whenever the tree is not on screen, which covers both a tree
    /// folded away with `Ctrl+T` and the very first frame, before `ui` has
    /// said where anything is.
    fn over_tree(&self, column: u16) -> bool {
        matches!(self.last_tree_cols, Some((x0, x1)) if column >= x0 && column < x1)
    }
}
