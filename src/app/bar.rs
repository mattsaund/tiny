//! The bar: one field that is both a search and a command line.
//!
//! There is no "search mode" and no "command mode" — there is one field, and
//! what it is depends on whether what you typed starts with the sigil. It
//! decides keystroke by keystroke rather than being opened one way or the
//! other, which is why a `*` typed by mistake can simply be deleted.
//!
//! # Searching as you type
//!
//! Every keystroke re-runs the search, and the preview follows the highlighted
//! hit — so the pane shows the file *and* the place in it, marked, before you
//! have committed to opening anything. The budget that makes this affordable
//! is in [`crate::text::search`], not here.
//!
//! # Completion
//!
//! [`completion_for`] is the shared answer: the bar draws the ghost text with
//! it and Tab accepts the same string. Splitting it out is what stops the
//! suggestion on screen from ever differing from the one Tab would take.

use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Config;
use crate::text::search::{self, Hit, HitKind};

use super::App;
use super::mode::{Bar, COMMAND_SIGIL, Focus, Mode};
use super::parts::{char_byte, display_name, safe_join, split_args};
use super::preview::Preview;

/// Entries that could finish the path fragment `typed`, as whole paths
/// relative to the project root.
///
/// Only the one directory being typed into is read — a `read_dir`, never a
/// walk — so completing a path costs the same in a huge project as in a small
/// one. Folders come back with a separator on the end so Tab can carry straight
/// on into them.
///
/// Dotfiles stay out of the way unless they are being shown, or unless the
/// fragment already starts with a dot, which is the only time someone is
/// plainly asking for one.
fn path_candidates(root: &Path, typed: &str, show_hidden: bool) -> Vec<String> {
    // Everything up to the last separator names the folder; the rest is the
    // part being completed inside it.
    let cut = typed.rfind(std::path::is_separator).map_or(0, |i| i + 1);
    let (dir_part, fragment) = typed.split_at(cut);
    let Ok(dir) = safe_join(root, dir_part, root) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !show_hidden && !fragment.starts_with('.') {
                return None;
            }
            let tail = if e.path().is_dir() {
                std::path::MAIN_SEPARATOR_STR
            } else {
                ""
            };
            Some(format!("{dir_part}{name}{tail}"))
        })
        .collect();
    out.sort();
    out
}

/// Fill in the rest of whatever is being typed: a command name, a setting name
/// after `set`, the word `to` in the middle of a copy, or a path.
///
/// Completes to the longest common prefix of the matches rather than to the
/// first one, so Tab on an ambiguous prefix advances as far as it safely can
/// and then stops — the shell behaviour.
///
/// The whole of `copy README.md to notes/today.md` can be typed with Tab, which
/// is the point: a command that reads like a sentence is no use if every word
/// of it has to be spelled out by hand.
fn complete_command(b: &mut Bar, root: &Path, show_hidden: bool) {
    if let Some(rest) = completion_for(b, root, show_hidden) {
        b.input.push_str(&rest);
        b.cursor = b.input.chars().count();
    }
}

/// What `Tab` would add to the line, if anything.
///
/// Split out from [`complete_command`] so the bar can draw the same answer in
/// grey before the key is pressed — a suggestion you can see is worth more than
/// one you have to guess at, and it costs one `read_dir`.
///
/// Returns only the *remainder*: every candidate is filtered by what has been
/// typed already, so the completion always begins with it.
///
/// Any new command added to [`App::run_command`] needs adding to `COMMANDS`
/// here too, or it will work but never complete — and one that takes a path
/// needs adding to `TAKES_PATHS` as well.
pub fn completion_for(b: &Bar, root: &Path, show_hidden: bool) -> Option<String> {
    const COMMANDS: &[&str] = &[
        "set", "replace", "config", "settings", "map", "write", "save", "quit", "help", "reload",
        "new", "mkdir", "delete", "rm", "copy", "cp", "line",
    ];
    const TAKES_PATHS: &[&str] = &["new", "mkdir", "delete", "rm", "copy", "cp"];

    // The sigil is not part of the command, so completion never sees it. The
    // replacement at the bottom still works on `b.input`, because whatever is
    // being completed is a suffix of both.
    let line = b.command().to_string();
    let args = split_args(&line);
    let trailing_space = line.ends_with(' ');
    // Which argument the cursor is in the middle of. A trailing space means the
    // one after the last complete argument has been started but not typed into.
    let position = if trailing_space {
        args.len()
    } else {
        args.len().saturating_sub(1)
    };
    let typed = if trailing_space {
        String::new()
    } else {
        args.last().cloned().unwrap_or_default()
    };

    let (prefix, candidates): (String, Vec<String>) = match args.first().map(String::as_str) {
        // Still on the command name itself.
        _ if position == 0 => (typed, COMMANDS.iter().map(|s| s.to_string()).collect()),
        Some("set") => (
            typed,
            Config::settings_index()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect(),
        ),
        // `copy README.md ` — what comes next is the word joining the two
        // halves, so offer that and nothing else. Anyone copying a name with
        // spaces in it can type the two letters themselves.
        Some("copy" | "cp") if position == 2 => (typed, vec!["to".to_string()]),
        Some(c) if TAKES_PATHS.contains(&c) => {
            (typed.clone(), path_candidates(root, &typed, show_hidden))
        }
        _ => return None,
    };

    let matches: Vec<&String> = candidates
        .iter()
        .filter(|c| c.starts_with(&prefix))
        .collect();
    let first = matches.first()?;
    // With several options, fill in only what they all agree on.
    let common = matches.iter().skip(1).fold((*first).clone(), |acc, m| {
        acc.chars()
            .zip(m.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    if common.len() <= prefix.len() {
        return None;
    }
    Some(common[prefix.len()..].to_string())
}

impl App {
    // ---- the bar ----------------------------------------------------------

    /// Open the bar, discarding whatever was there before.
    ///
    /// `as_command` only decides what is already typed into it: the sigil, or
    /// nothing. After that the bar decides for itself, keystroke by keystroke,
    /// which of the two things it is.
    pub(super) fn open_bar(&mut self, as_command: bool) {
        let input = if as_command {
            COMMAND_SIGIL.to_string()
        } else {
            String::new()
        };
        self.mode = Mode::Bar(Bar::new(input));
        self.status = if as_command {
            "command — Tab completes | Enter runs | Esc closes".into()
        } else {
            format!("search — or {COMMAND_SIGIL} for a command, like {COMMAND_SIGIL}copy a to b")
        };
    }

    /// Keys for the search and command bar.
    ///
    /// Two structural things to know when editing this:
    ///
    /// - Branches that end with `return` leave `self.mode` as the `Normal` that
    ///   `on_key` swapped in — that is how Esc and Enter close the bar. Every
    ///   other branch has to put the bar back.
    /// - Ctrl chords are handled up front and never fall through, so a chord
    ///   cannot leak its letter into the query.
    ///
    /// Text edits fall out of the `match` to the bottom, where a search bar
    /// re-runs its query and previews the top hit.
    pub(super) fn on_bar_key(&mut self, mut b: Bar, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            // Ctrl+Q means quit everywhere, including here. No other chord is
            // allowed to fall through and type its letter into the query.
            if key.code == KeyCode::Char('q') {
                self.request_quit();
            } else {
                self.mode = Mode::Bar(b);
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.status = "closed".into();
                return;
            }
            KeyCode::Enter => {
                if b.is_command() {
                    self.run_command(b.command());
                } else if let Some(hit) = b.results.get(b.selected).cloned() {
                    self.jump_to(&hit);
                    self.status = display_name(&hit.path);
                } else {
                    self.status = "no matches".into();
                }
                return;
            }
            KeyCode::Up => {
                b.selected = b.selected.saturating_sub(1);
                self.preview_hit(&b);
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Down => {
                if !b.results.is_empty() {
                    b.selected = (b.selected + 1).min(b.results.len() - 1);
                }
                self.preview_hit(&b);
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Tab => {
                if b.is_command() {
                    complete_command(&mut b, self.tree.root_path(), self.config.show_hidden);
                }
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Backspace => {
                if b.cursor > 0 {
                    let s = char_byte(&b.input, b.cursor - 1);
                    let e = char_byte(&b.input, b.cursor);
                    b.input.replace_range(s..e, "");
                    b.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let n = b.input.chars().count();
                if b.cursor < n {
                    let s = char_byte(&b.input, b.cursor);
                    let e = char_byte(&b.input, b.cursor + 1);
                    b.input.replace_range(s..e, "");
                }
            }
            KeyCode::Left => b.cursor = b.cursor.saturating_sub(1),
            KeyCode::Right => {
                // At the end of the line the only thing to the right of the
                // cursor is the grey suggestion, so the arrow takes it up —
                // the same key, doing the same thing, to what is drawn there.
                // Anywhere else it moves along the text as it always did.
                if b.is_command() && b.cursor == b.input.chars().count() {
                    complete_command(&mut b, self.tree.root_path(), self.config.show_hidden);
                } else {
                    b.cursor = (b.cursor + 1).min(b.input.chars().count());
                }
            }
            KeyCode::Home => b.cursor = 0,
            KeyCode::End => b.cursor = b.input.chars().count(),
            KeyCode::Char(c) => {
                let byte = char_byte(&b.input, b.cursor);
                b.input.insert(byte, c);
                b.cursor += 1;
            }
            _ => {
                self.mode = Mode::Bar(b);
                return;
            }
        }

        // The line may have just become a command, or stopped being one.
        if b.is_command() {
            b.results.clear();
            b.searched = false;
            b.selected = 0;
        } else {
            self.run_search(&mut b);
            self.preview_hit(&b);
        }
        self.mode = Mode::Bar(b);
    }

    /// The query the bar is currently searching for, if it is searching.
    ///
    /// `ui` uses this to mark the same word in the preview pane, so that
    /// stepping through results shows you *where* in the file each one is
    /// rather than only which file. `None` for a command, and for an empty
    /// query — neither has a word to point at.
    pub fn live_query(&self) -> Option<&str> {
        match &self.mode {
            Mode::Bar(b) if !b.is_command() => Some(b.input.trim()).filter(|q| !q.is_empty()),
            _ => None,
        }
    }

    /// Re-run the query from scratch and reset the result cursor. Called on
    /// every keystroke — see `search`'s module docs for why that is fine.
    fn run_search(&mut self, b: &mut Bar) {
        let opts = self.search_opts();
        b.results = search::search(self.tree.root_path(), &b.input, &opts);
        b.searched = !b.input.trim().is_empty();
        b.selected = 0;
    }

    /// Show the highlighted result in the preview pane without leaving the bar.
    fn preview_hit(&mut self, b: &Bar) {
        let Some(hit) = b.results.get(b.selected) else {
            return;
        };
        let hit = hit.clone();
        self.reveal(&hit.path);
        self.place_cursor_on(&hit);
    }

    /// Commit to a result: move to it and hand the keyboard to the preview.
    /// The Enter-key counterpart to [`App::preview_hit`].
    fn jump_to(&mut self, hit: &Hit) {
        self.reveal(&hit.path);
        self.place_cursor_on(hit);
        if matches!(self.preview, Preview::Buffer { .. }) {
            self.focus = Focus::Editor;
        }
    }

    /// Put the cursor on a hit.
    ///
    /// A content hit lands on the matching line; a name hit opens the file at
    /// the top. Either way the file opens the same way every file does, so
    /// there is no view to choose between.
    fn place_cursor_on(&mut self, hit: &Hit) {
        if hit.kind == HitKind::Content {
            let (line, col) = (hit.line, hit.col);
            if let Some(ed) = self.active_buffer_mut() {
                ed.goto(line, col);
            }
        }
    }
}
