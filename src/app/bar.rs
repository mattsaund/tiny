//! The bar: one field that is both a search and a command line.
//!
//! There is no "search mode" and no "command mode"; there is one field, and
//! what it is depends on whether what you typed starts with the sigil. It
//! decides keystroke by keystroke rather than being opened one way or the
//! other, which is why a `*` typed by mistake can simply be deleted.
//!
//! # Searching as you type
//!
//! Every change to the query re-runs the search (once typing pauses, not once
//! a letter) and the preview follows the highlighted hit, so the pane shows
//! the file *and* the place in it, marked, before you have committed to
//! opening anything. The budget that makes this affordable is in
//! [`crate::text::search`], not here.
//!
//! A command gets the same treatment: once typing pauses, the line under the
//! bar says what Enter would do with it, see [`super::plan`].
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
use super::mode::{Bar, COMMAND_SIGIL, Cycle, Focus, Mode, Window};
use super::parts::{display_name, edit_field, safe_join};
use super::preview::Preview;

/// Entries that could finish the path fragment `typed`, as whole paths
/// relative to the project root.
///
/// Only the one directory being typed into is read (a `read_dir`, never a
/// walk) so completing a path costs the same in a huge project as in a small
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

/// Fill in the rest of whatever is being typed: a verb, a setting name after
/// `set` and then its value, the kind of thing after `goto`, a joining word,
/// or a name.
///
/// Completes to the longest common prefix of the matches rather than to the
/// first one, so Tab on an ambiguous prefix advances as far as it safely can,
/// the shell behavior. Where they share nothing more, and something has been
/// typed of the word, Tab puts in the first of them, and again the next,
/// round all of them and back to what was typed: the menu completion of zsh.
/// Returns what to say about that, when anything.
///
/// The whole of `copy README.md to notes/today.md` can be typed with Tab, which
/// is the point: a command that reads like a sentence is no use if every word
/// of it has to be spelled out by hand.
fn complete_command(
    b: &mut Bar,
    root: &Path,
    show_hidden: bool,
    added: &[String],
    known: &[String],
) -> Option<String> {
    // Tab again, with nothing typed since: the next of them.
    if let Some(c) = &mut b.cycle
        && b.input == c.shown
    {
        c.at = (c.at + 1) % (c.rests.len() + 1);
        let rest = c.rests.get(c.at).map_or("", String::as_str);
        b.input = format!("{}{rest}", c.before);
        b.cursor = b.input.chars().count();
        c.shown = b.input.clone();
        return Some(match c.rests.get(c.at) {
            Some(_) => format!("{} of {}, Tab for the next", c.at + 1, c.rests.len()),
            None => "back to what was typed".into(),
        });
    }
    b.cycle = None;
    let (typed, matches) = completion_options(b, root, show_hidden, added, known)?;
    if let Some(rest) = common_rest(&typed, &matches) {
        b.input.push_str(&rest);
        b.cursor = b.input.chars().count();
        return None;
    }
    // Not from nothing at all: after a space, every file in the project
    // might be one, and the first of them is no answer. A letter or two
    // narrows it to something worth going round.
    if typed.is_empty() {
        return None;
    }
    // What was typed may be one of them already, and the round comes back
    // to that anyway.
    let rests: Vec<String> = matches
        .iter()
        .map(|m| m[typed.len()..].to_string())
        .filter(|r| !r.is_empty())
        .collect();
    if rests.is_empty() {
        return None;
    }
    let before = b.input.clone();
    b.input = format!("{before}{}", rests[0]);
    b.cursor = b.input.chars().count();
    let n = rests.len();
    b.cycle = Some(Cycle {
        before,
        rests,
        at: 0,
        shown: b.input.clone(),
    });
    Some(format!("1 of {n}, Tab for the next"))
}

/// What the matches all have after `typed`, when it is anything: what Tab
/// fills in, and the bar shows in gray before it is pressed.
fn common_rest(typed: &str, matches: &[String]) -> Option<String> {
    let first = matches.first()?;
    let common = matches.iter().skip(1).fold(first.clone(), |acc, m| {
        acc.chars()
            .zip(m.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    (common.len() > typed.len()).then(|| common[typed.len()..].to_string())
}

/// What `Tab` would add to the line, if anything.
///
/// Split out from [`complete_command`] so the bar can draw the same answer in
/// gray before the key is pressed: a suggestion you can see is worth more than
/// one you have to guess at, and it costs one `read_dir`.
///
/// Returns only the *remainder*: every candidate is filtered by what has been
/// typed already, so the completion always begins with it.
pub fn completion_for(
    b: &Bar,
    root: &Path,
    show_hidden: bool,
    added: &[String],
    known: &[String],
) -> Option<String> {
    let (typed, matches) = completion_options(b, root, show_hidden, added, known)?;
    common_rest(&typed, &matches)
}

/// The word being typed, and every way of finishing it, each once.
///
/// What is offered follows the sentence: a verb first, then (depending on
/// the verb) the kind of thing (`line`, `function`, `file`...), a name, or
/// the joining word (`to`, `with`, `in`) that comes next. `added` is the
/// commands plugins have added, which complete like tiny's own; `known` is
/// every file and folder name in the project, when it has been listed.
fn completion_options(
    b: &Bar,
    root: &Path,
    show_hidden: bool,
    added: &[String],
    known: &[String],
) -> Option<(String, Vec<String>)> {
    use super::sentence::{
        COPY, DELETE, GOTO, KINDS, MOVE, NEW_FILE, NEW_FOLDER, RENAME, REPLACE, VERBS,
        drop_leading_politeness, words,
    };

    // The sigil is not part of the command, so completion never sees it. The
    // replacement at the bottom still works on `b.input`, because whatever is
    // being completed is a suffix of both.
    let line = b.command();
    let mut found = words(line);
    // A space inside a quote that is still open is part of the name being
    // typed, not the end of a word.
    let open_quote = line.matches('"').count() % 2 == 1;
    let trailing_space = line.ends_with(' ') && !open_quote;
    // Which word the cursor is in the middle of. A trailing space means the
    // one after the last complete word has been started but not typed into.
    let typing = if trailing_space {
        found.len()
    } else {
        found.len().saturating_sub(1)
    };
    // A polite start (`please`, `can you`) is passed over, the way the
    // reading passes over it, once it has been typed in full.
    let mut done = found[..typing].to_vec();
    let polite = drop_leading_politeness(&mut done);
    found.drain(..polite);
    let mut args: Vec<String> = found.into_iter().map(|w| w.text).collect();
    let mut position = typing - polite;
    let typed = if open_quote {
        // Exactly as typed since the quote, spaces and all: the word read
        // out of the line has had its end trimmed.
        line.rsplit('"').next().unwrap_or_default().to_string()
    } else if trailing_space {
        String::new()
    } else {
        args.last().cloned().unwrap_or_default()
    };
    let verb = args.first().map(|v| v.to_lowercase()).unwrap_or_default();
    let v = verb.as_str();
    // `go to line 5`, `take me to line 5` and `move to line 5` are all read
    // as `goto line 5`, so they complete like it too: the words leading in
    // are passed over, the same ones the reading passes over.
    let to_after = position > 1 && args[1].eq_ignore_ascii_case("to");
    let goto = GOTO.contains(&v) || (to_after && (MOVE.contains(&v) || v == "skip"));
    if goto && position > 1 {
        let lead_in = args[1..position]
            .iter()
            .take_while(|a| {
                ["to", "me", "for"]
                    .iter()
                    .any(|w| a.eq_ignore_ascii_case(w))
            })
            .count();
        args.drain(1..1 + lead_in);
        position -= lead_in;
    }
    let two_ends = !goto && (COPY.contains(&v) || MOVE.contains(&v) || RENAME.contains(&v));
    let making = NEW_FILE.contains(&v) || NEW_FOLDER.contains(&v);
    let said = |w: &str| args.iter().any(|a| a.eq_ignore_ascii_case(w));
    // A name: a path from the project root while one is being typed with a
    // slash in it, and otherwise any file or folder in the project by name.
    let names = || -> Vec<String> {
        if typed.contains(['/', '\\']) || known.is_empty() {
            path_candidates(root, &typed, show_hidden)
        } else {
            let mut out: Vec<String> = known.to_vec();
            out.extend(path_candidates(root, &typed, show_hidden));
            out
        }
    };
    let offer = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let candidates: Vec<String> = match v {
        // Still on the verb itself.
        _ if position == 0 => offer(VERBS)
            .into_iter()
            .chain(added.iter().cloned())
            .collect(),
        "set" if position == 1 => Config::settings_index()
            .iter()
            .map(|(k, _)| k.to_string())
            .collect(),
        // `set glyphs ascii`, or `set glyphs to ascii`.
        "set" if position == 2 || (position == 3 && args[2].eq_ignore_ascii_case("to")) => {
            offer(Config::values_of(&args[1]))
        }
        "split" if position == 1 => offer(&["down"]),
        "close" if position == 1 => offer(&["others", "terminal"]),
        _ if goto && position == 1 => offer(KINDS).into_iter().chain(names()).collect(),
        _ if goto
            && position == 2
            && args
                .get(1)
                .is_some_and(|k| matches!(k.to_lowercase().as_str(), "file" | "folder")) =>
        {
            names()
        }
        // `copy README.md `: what comes next is the word joining the two
        // halves, so offer that and nothing else. Anyone copying a name with
        // spaces in it can type the two letters themselves.
        _ if two_ends && position == 2 => offer(&["to"]),
        _ if two_ends && (position == 1 || said("to")) => names(),
        _ if DELETE.contains(&v) => names(),
        // `mkdir` has already said it makes a folder.
        _ if NEW_FILE.contains(&v) && position == 1 => offer(&["file", "folder"])
            .into_iter()
            .chain(path_candidates(root, &typed, show_hidden))
            .collect(),
        _ if making => path_candidates(root, &typed, show_hidden),
        // `replace hi ` wants its `with`; `replace hi with hello ` its `in`.
        _ if REPLACE.contains(&v) && position == 2 && !said("with") => offer(&["with"]),
        _ if REPLACE.contains(&v) && said("with") && !said("in") && position == args.len() => {
            offer(&["in"])
        }
        _ if REPLACE.contains(&v) && said("in") => names(),
        _ => return None,
    };
    let mut matches: Vec<String> = Vec::new();
    for c in candidates {
        if c.starts_with(&typed) && !matches.contains(&c) {
            matches.push(c);
        }
    }
    (!matches.is_empty()).then_some((typed, matches))
}

impl App {
    // ---- the bar ----------------------------------------------------------

    /// Open the bar, empty, discarding whatever was there before.
    ///
    /// It opens as a search, and decides for itself, keystroke by keystroke,
    /// whether it is still one: a `*` typed first makes it the command line.
    /// There used to be a second key that opened it with the star already
    /// typed: one key to save one keystroke, and one more row to read past.
    ///
    /// The file being edited is remembered as the bar opens, so a search
    /// started from inside a file can put that file's hits at the top. Only
    /// when the editor actually has the keyboard: hovering a file in the tree
    /// is looking around the project, and a search from there has no reason to
    /// favor whichever row the cursor happened to be resting on.
    pub(super) fn open_bar(&mut self) {
        let home = match (&self.preview, self.focus) {
            (Preview::Buffer { path, .. }, Focus::Editor) => Some(path.clone()),
            _ => None,
        };
        // Over the map the bar is the map's filter, and opening it starts a
        // new one: the last filter is not something to carry into this search
        // without being asked for again.
        let filtering = self.window == Window::Map;
        if filtering {
            self.filter_map("");
        }
        self.mode = Mode::Bar(Bar::new(String::new(), home));
        // The names a command looks up are listed once per bar, and the disk
        // may have moved on since the last one.
        self.forget_names();
        self.status = if filtering {
            "filter the map: Enter keeps it | Esc clears it".into()
        } else {
            format!("search, or {COMMAND_SIGIL} for a command, like {COMMAND_SIGIL}copy a to b")
        };
    }

    /// Narrow the map to what has been typed, and say what is left of it.
    ///
    /// The map's half of the bar. Filtering the map and searching the project
    /// are the same question asked about two pictures of the same files, so
    /// they are one field in one place; which one the typing reaches is
    /// decided by the window the bar is drawn over, not by a second key.
    pub(super) fn filter_map(&mut self, query: &str) {
        let Some(view) = self.project_map.as_mut() else {
            return;
        };
        view.set_filter(query);
        let summary = view.summary();
        self.status = format!("project map: {summary}");
    }

    /// Keys for the search and command bar.
    ///
    /// Two structural things to know when editing this:
    ///
    /// - Branches that end with `return` leave `self.mode` as the `Normal` that
    ///   `handle_key` swapped in; that is how Esc and Enter close the bar.
    ///   Every other branch has to put the bar back.
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
        // Moving through the results, or taking one, needs them to be the
        // results for what is typed. A command plans afresh on Enter anyway.
        if b.stale
            && !b.is_command()
            && matches!(key.code, KeyCode::Enter | KeyCode::Up | KeyCode::Down)
        {
            self.refresh_search(&mut b);
        }
        match key.code {
            KeyCode::Esc => {
                // Putting the bar away over the map puts its filter away too,
                // so what you come back to is the whole map.
                if self.filtering_map(&b) {
                    self.filter_map("");
                } else {
                    self.status = "closed".into();
                }
                return;
            }
            KeyCode::Enter => {
                if b.is_command() {
                    self.run_command(b.command());
                } else if self.filtering_map(&b) {
                    // The filter stays and the keyboard goes back to the map,
                    // which is what Enter meant when the map had its own box.
                    self.filter_map(&b.input);
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
                    if let Some(said) = complete_command(
                        &mut b,
                        self.tree.root_path(),
                        self.config.show_hidden,
                        &self.plugins.commands(),
                        self.known_names(),
                    ) {
                        self.status = said;
                    }
                    b.stale = true;
                }
                self.mode = Mode::Bar(b);
                return;
            }
            _ => {}
        }

        // Editing the line. What follows only matters if the text changed:
        // moving the cursor along it is not a new search, and redoing one
        // would put the selection back on the first hit.
        let before = b.input.clone();
        // A command's cursor stops after its `*`: the sigil is drawn apart
        // from the text, and a letter typed in front of it would turn the
        // command back into a search. Backspace can still take it away.
        let floor = usize::from(b.is_command());
        // At the end of the line the only thing to the right of the cursor
        // is the gray suggestion, so the arrow takes it up: the same key,
        // doing the same thing, to what is drawn there. Anywhere else it
        // moves along the text as it always did.
        if key.code == KeyCode::Right && b.is_command() && b.cursor == b.input.chars().count() {
            if let Some(said) = complete_command(
                &mut b,
                self.tree.root_path(),
                self.config.show_hidden,
                &self.plugins.commands(),
                self.known_names(),
            ) {
                self.status = said;
            }
            b.stale = true;
        } else {
            edit_field(&mut b.input, &mut b.cursor, floor, key);
        }
        if b.input == before {
            self.mode = Mode::Bar(b);
            return;
        }

        // The line may have just become a command, or stopped being one.
        if b.is_command() {
            b.results.clear();
            b.searched = false;
            b.selected = 0;
            b.stale = true;
        } else {
            b.plan = None;
            if self.window == Window::Map {
                self.filter_map(&b.input);
            } else {
                b.stale = true;
            }
        }
        self.mode = Mode::Bar(b);
    }

    /// Every file and folder name in the project, for Tab to offer, once
    /// the bar has listed them, and nothing until then. Each name once.
    pub fn known_names(&self) -> &[String] {
        self.names.as_ref().map_or(&[], |n| n.all_names())
    }

    /// Search for what the bar says now, if it has changed since the last
    /// search. The event loop calls this before every frame, and `on_key`
    /// after every key a test sends, so nothing ever draws or reads stale
    /// results.
    pub fn settle(&mut self) {
        if !matches!(&self.mode, Mode::Bar(b) if b.stale) {
            return;
        }
        // Taken out while it runs, the same as while a key is handled.
        if let Mode::Bar(mut b) = std::mem::replace(&mut self.mode, Mode::Normal) {
            if b.is_command() {
                b.stale = false;
                let line = b.command().to_string();
                b.plan = (!line.trim().is_empty()).then(|| self.preview_command(&line));
            } else {
                self.refresh_search(&mut b);
            }
            self.mode = Mode::Bar(b);
        }
    }

    /// The search and its first hit in the preview, for the query as typed.
    fn refresh_search(&mut self, b: &mut Bar) {
        b.stale = false;
        self.run_search(b);
        self.preview_hit(b);
    }

    /// Whether what is being typed is narrowing the map rather than searching
    /// the project. A command is neither, whatever window it was typed in.
    fn filtering_map(&self, b: &Bar) -> bool {
        self.window == Window::Map && !b.is_command()
    }

    /// The query the bar is currently searching for, if it is searching.
    ///
    /// `ui` uses this to mark the same word in the preview pane, so that
    /// stepping through results shows you *where* in the file each one is
    /// rather than only which file. `None` for a command, and for an empty
    /// query: neither has a word to point at.
    pub fn live_query(&self) -> Option<&str> {
        match &self.mode {
            Mode::Bar(b) if !b.is_command() => Some(b.input.trim()).filter(|q| !q.is_empty()),
            _ => None,
        }
    }

    /// Re-run the query from scratch and reset the result cursor. Called once
    /// the query has stopped changing for the moment, see [`App::settle`],
    /// and `search`'s module docs for why from scratch is fine.
    ///
    /// A search started from inside a file lists that file's hits first. When
    /// you search while writing, the word you are looking for is usually one
    /// you just wrote: the other occurrence of the name you are renaming, the
    /// heading you are about to link to. Those hits were already in the list
    /// (somewhere down it, in whatever order the directory walk happened to
    /// reach them) and this is only about which end they come out of.
    ///
    /// A stable sort on one boolean, so everything else keeps the order
    /// [`search::search`] gave it: names before contents, directory order
    /// within each.
    fn run_search(&mut self, b: &mut Bar) {
        let opts = self.search_opts();
        b.results = search::search(self.tree.root_path(), &b.input, &opts);
        if let Some(home) = &b.home {
            b.results.sort_by_key(|hit| &hit.path != home);
        }
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
    ///
    /// Into the main window, from wherever the bar was opened over: a hit is
    /// a place in a file, and the file is there.
    pub(super) fn jump_to(&mut self, hit: &Hit) {
        self.window = super::mode::Window::Main;
        if !self.reveal(&hit.path) {
            return;
        }
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
