//! Reading a command the way it would be said.
//!
//! `*copy temp.txt to notes`, `*goto function main in app.rs`, `*replace hi
//! with hello`: the command line takes sentences, and several ways of saying
//! each one — `go to`, `jump to` and `open` are one verb, as are `delete`,
//! `remove` and `rm`. There is no language model anywhere in this. It is a
//! verb, then what to do it to, then the little words that join the parts —
//! `to`, `with`, `in` — and a list of the ways people tend to say each.
//!
//! This module only reads. It turns a line into a [`Sentence`] without
//! looking at the disk, which is what keeps it testable line by line; finding
//! the files a sentence names, and saying what it would do, is
//! [`super::plan`]'s half.
//!
//! # The rules, briefly
//!
//! - The first word is the verb. Words like `please` before it, and `the` or
//!   `a` anywhere, are passed over.
//! - The joining words split the rest: `copy A to B`, `replace A with B`,
//!   `goto word A in B`. Only the first `to` or `with` splits, and only the
//!   last `in`, so a name may have the other ones in it.
//! - Anything in double quotes is a name, never a joining word:
//!   `*copy "to do.md" to archive`.
//! - Every spelling tiny had before still works — `*line 42`, `*42`,
//!   `*replace old new`, `*cp`, `*mv`, `*rm`. More ways in is the point.

/// One word of a command line, and whether it was written in quotes. A quoted
/// word is always part of a name, even when it is `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    text: String,
    quoted: bool,
}

impl Word {
    /// Whether this is one of `keywords`, as a joining or verb word.
    fn is(&self, keywords: &[&str]) -> bool {
        !self.quoted && keywords.iter().any(|k| self.text.eq_ignore_ascii_case(k))
    }
}

/// Split a line into words, keeping a double-quoted run as one word.
fn words(line: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut started = false;
    for c in line.trim().chars() {
        match c {
            '"' => {
                quoted = !quoted;
                was_quoted = true;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(Word {
                        text: std::mem::take(&mut cur),
                        quoted: was_quoted,
                    });
                    started = false;
                    was_quoted = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(Word {
            text: cur,
            quoted: was_quoted,
        });
    }
    out
}

/// The words as one name again, single-spaced.
fn joined(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where a command is aimed, when it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// `in this file`, `here` — the file being looked at.
    This,
    /// `in notes.md` — a file or folder, by name.
    Named(String),
}

/// What `goto` is asked to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `line 42`, or just `42`. Counted from 1.
    Line(usize),
    /// `function main` — a definition in code.
    Function(String),
    /// `heading install` — a markdown heading.
    Heading(String),
    /// `word test` — the word on its own, not inside a longer one.
    Word(String),
    /// `text "not yet"` — anywhere, inside words too.
    Text(String),
    /// `file notes.md`.
    File(String),
    /// `folder notes`.
    Folder(String),
    /// One of tiny's own windows: `goto map`, `open git`, `show help`.
    Window(Window),
    /// No kind was given — `goto notes` — so whatever answers to the name:
    /// a file, a folder, a function, a heading, then a word.
    Anything(String),
}

/// The windows and overlays a sentence can open by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Browser,
    Map,
    Git,
    Help,
    Settings,
}

/// A command, read but not yet checked against the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sentence {
    /// Nothing typed yet.
    Empty,
    /// The verb was understood but not enough came after it. Carries how to
    /// say it.
    Incomplete(String),
    Goto {
        target: Target,
        within: Option<Place>,
    },
    /// `copy A to B` — B may be a folder to copy into.
    Copy {
        from: Place,
        to: String,
    },
    /// `move A to B` or `rename A to B`. A rename never reads B as a folder
    /// to move into: `rename notes.md to ideas` means a file called `ideas`,
    /// whatever folders the project happens to have.
    Move {
        from: Place,
        to: String,
        rename: bool,
    },
    /// `delete A`, or `delete` alone for whatever the cursor is on.
    Delete(Place),
    /// `new notes/today.md`, `make folder ideas in notes`.
    New {
        name: String,
        folder: bool,
        within: Option<Place>,
    },
    /// `replace A with B`, across the project or `in` one file.
    Replace {
        find: String,
        with: String,
        within: Option<Place>,
        whole_words: bool,
    },
    /// `set tab_width to 2`, with the words after `set` as they were typed.
    Set(Vec<String>),
    Commit(String),
    Reload,
    Update,
    /// A word tiny has no verb for — a plugin's command, perhaps, or a name to
    /// go to. Kept whole so either can be tried.
    Other {
        name: String,
        args: Vec<String>,
    },
}

// ---- the words ------------------------------------------------------------

const GOTO: &[&str] = &[
    "goto", "go", "jump", "open", "show", "find", "view", "visit", "locate", "navigate", "take",
    "head",
];
const COPY: &[&str] = &["copy", "cp", "duplicate", "dup", "clone"];
const MOVE: &[&str] = &["move", "mv", "put", "relocate", "transfer"];
const RENAME: &[&str] = &["rename", "ren"];
const DELETE: &[&str] = &["delete", "del", "remove", "rm", "trash", "erase", "destroy"];
const NEW_FILE: &[&str] = &["new", "create", "make", "add", "touch"];
const NEW_FOLDER: &[&str] = &["mkdir", "md"];
const REPLACE: &[&str] = &["replace", "sub", "substitute", "swap", "change", "switch"];

const LINE: &[&str] = &["line", "ln", "row"];
const FUNCTION: &[&str] = &[
    "function",
    "fn",
    "func",
    "def",
    "method",
    "class",
    "struct",
    "symbol",
    "definition",
];
const HEADING: &[&str] = &["heading", "header", "section", "title", "chapter"];
const WORD: &[&str] = &["word"];
const TEXT: &[&str] = &["text", "phrase", "string"];
const FILE: &[&str] = &["file", "note", "doc", "document", "page"];
const FOLDER: &[&str] = &["folder", "dir", "directory"];

/// Words that say where: `in notes.md`, `inside src`.
const IN: &[&str] = &["in", "inside", "within"];
/// Words joining what is moved to where it goes.
const TO: &[&str] = &["to", "into", "onto"];
/// Words joining what is replaced to what replaces it.
const WITH: &[&str] = &["with", "by", "for", "to", "->", "→", "=>"];

/// Said before a command and meaning nothing to it.
const POLITE: &[&str] = &[
    "please", "pls", "kindly", "just", "now", "then", "ok", "okay", "hey", "so",
];
/// Words with no part in a name when one starts it: `delete the file`,
/// `make a new folder`. Never taken when it is the whole name — a file may be
/// called `a`.
const ARTICLES: &[&str] = &["the", "a", "an"];
/// How a sentence says "the thing I am on".
const THIS: &[&str] = &[
    "this",
    "it",
    "here",
    "current",
    "this file",
    "the current file",
];
/// How a sentence says "the whole project".
const EVERYWHERE: &[&str] = &[
    "everywhere",
    "anywhere",
    "all",
    "all files",
    "every file",
    "everything",
    "project",
    "the project",
    "whole project",
    "the whole project",
];

// ---- reading --------------------------------------------------------------

/// Read a command line, without the `*`.
pub fn parse(line: &str) -> Sentence {
    let mut w = words(line);
    drop_politeness(&mut w);
    let Some(first) = w.first().cloned() else {
        return Sentence::Empty;
    };
    let rest = &w[1..];

    if first.quoted {
        return goto(rest_from(&w, 0));
    }
    if first.text.parse::<usize>().is_ok() {
        // `*42`, the way `:42` is everywhere else.
        return goto(&w);
    }
    let verb = first.text.to_lowercase();
    let v = verb.as_str();

    // `move to line 4` is going somewhere, not moving a file.
    if (MOVE.contains(&v) || v == "skip") && rest.first().is_some_and(|w| w.is(&["to"])) {
        return goto(rest);
    }
    match v {
        _ if GOTO.contains(&v) => goto(rest),
        _ if LINE.contains(&v) => goto(&w),
        _ if COPY.contains(&v) => copy(rest),
        _ if MOVE.contains(&v) => moving(rest, false),
        _ if RENAME.contains(&v) => moving(rest, true),
        _ if DELETE.contains(&v) => delete(rest),
        _ if NEW_FOLDER.contains(&v) => new(rest, Some(true)),
        _ if NEW_FILE.contains(&v) => new(rest, None),
        _ if REPLACE.contains(&v) => replace(rest),
        "set" => Sentence::Set(rest.iter().map(|w| w.text.clone()).collect()),
        "commit" => Sentence::Commit(joined(rest)),
        "reload" | "refresh" | "rescan" => Sentence::Reload,
        "update" | "upgrade" => Sentence::Update,
        "config" | "settings" | "preferences" | "prefs" | "options" => Sentence::Goto {
            target: Target::Window(Window::Settings),
            within: None,
        },
        "help" | "keys" => Sentence::Goto {
            target: Target::Window(Window::Help),
            within: None,
        },
        _ => Sentence::Other {
            name: first.text.clone(),
            args: rest.iter().map(|w| w.text.clone()).collect(),
        },
    }
}

/// The words from `i` on.
fn rest_from(w: &[Word], i: usize) -> &[Word] {
    w.get(i..).unwrap_or(&[])
}

/// Take `please`, `can you` and their like off the front, and `please` and
/// `thanks` off the end.
fn drop_politeness(w: &mut Vec<Word>) {
    loop {
        if w.first().is_some_and(|x| x.is(POLITE)) {
            w.remove(0);
        } else if w.len() >= 2 && w[0].is(&["can", "could", "would", "will"]) && w[1].is(&["you"]) {
            w.drain(..2);
        } else if w.len() >= 3 && w[0].is(&["i"]) && w[1].is(&["want", "need"]) && w[2].is(&["to"])
        {
            w.drain(..3);
        } else if w.first().is_some_and(|x| x.is(&["let's", "lets"])) {
            w.remove(0);
        } else {
            break;
        }
    }
    while w
        .last()
        .is_some_and(|x| x.is(&["please", "thanks", "thx", "now"]))
    {
        w.pop();
    }
    if w.len() >= 2 && w[w.len() - 2].is(&["thank"]) && w[w.len() - 1].is(&["you"]) {
        w.truncate(w.len() - 2);
    }
}

/// `w` without a `the`, `a` or `an` in front, as long as something is left.
/// Only in front: inside a name they are part of it, as in `notes on a
/// train.md`. Quoted words are always kept.
fn without_articles(w: &[Word]) -> Vec<Word> {
    let skip = w
        .iter()
        .take(w.len().saturating_sub(1))
        .take_while(|x| x.is(ARTICLES))
        .count();
    w[skip..].to_vec()
}

/// Split at the first word in `seps`: what is before it and what is after.
fn split_first<'a>(w: &'a [Word], seps: &[&str]) -> Option<(&'a [Word], &'a [Word])> {
    let i = w.iter().position(|x| x.is(seps))?;
    Some((&w[..i], &w[i + 1..]))
}

/// Split at the last word in `seps`.
fn split_last<'a>(w: &'a [Word], seps: &[&str]) -> Option<(&'a [Word], &'a [Word])> {
    let i = w.iter().rposition(|x| x.is(seps))?;
    Some((&w[..i], &w[i + 1..]))
}

/// What `in ...` named: this file, the whole project, or a name.
///
/// `None` for the whole project, which is also what no `in` at all means.
fn place(w: &[Word]) -> Option<Place> {
    let said = joined(&without_articles(w));
    let lower = said.to_lowercase();
    if w.iter().all(|x| !x.quoted) {
        if THIS.contains(&lower.as_str()) || lower == "file" {
            return Some(Place::This);
        }
        if EVERYWHERE.contains(&lower.as_str()) || lower.is_empty() {
            return None;
        }
    }
    Some(Place::Named(said))
}

/// What a bare `this` or nothing at all means for the thing being acted on.
fn subject(w: &[Word]) -> Place {
    let mut w = without_articles(w);
    // `delete the file old.md`: the kind word in front says nothing a name
    // does not, so it goes too — when a name follows it.
    if w.len() > 1 && (w[0].is(FILE) || w[0].is(FOLDER)) {
        w.remove(0);
    }
    let said = joined(&w);
    if said.is_empty()
        || (w.iter().all(|x| !x.quoted) && THIS.contains(&said.to_lowercase().as_str()))
    {
        Place::This
    } else {
        Place::Named(said)
    }
}

/// `goto ...`: what kind of thing, its name, and where to look.
fn goto(w: &[Word]) -> Sentence {
    // `go to`, `take me to`, `jump to the`.
    let mut w: Vec<Word> = w
        .iter()
        .skip_while(|x| x.is(&["to", "me", "for"]))
        .cloned()
        .collect();
    w = without_articles(&w);
    let Some(first) = w.first().cloned() else {
        return Sentence::Incomplete("say where: goto line 42, goto file notes.md".into());
    };
    let kind_given = !first.quoted
        && [LINE, FUNCTION, HEADING, WORD, TEXT, FILE, FOLDER]
            .iter()
            .any(|k| first.is(k));
    let after_kind: &[Word] = if kind_given { &w[1..] } else { &w };

    // Lines also take `of`: `line 12 of main.rs`.
    let seps: &[&str] = if first.is(LINE) || first.text.parse::<usize>().is_ok() {
        &["in", "inside", "within", "of"]
    } else {
        IN
    };
    let (name, within) = match split_last(after_kind, seps) {
        Some((name, place_words)) if !name.is_empty() => (joined(name), place(place_words)),
        _ => (joined(after_kind), None),
    };

    let target = if first.is(LINE) || (!kind_given && first.text.parse::<usize>().is_ok()) {
        match name.split_whitespace().next() {
            Some(n) => match n.parse::<usize>() {
                Ok(n) => Target::Line(n),
                Err(_) => {
                    return Sentence::Incomplete(format!(
                        "`{n}` is not a line number — say it like: line 42"
                    ));
                }
            },
            None => return Sentence::Incomplete("say which line: goto line 42".into()),
        }
    } else if name.is_empty() {
        return Sentence::Incomplete("say what to go to: goto function main".into());
    } else if first.is(FUNCTION) {
        Target::Function(name)
    } else if first.is(HEADING) {
        Target::Heading(name)
    } else if first.is(WORD) {
        Target::Word(name)
    } else if first.is(TEXT) {
        Target::Text(name)
    } else if first.is(FILE) {
        Target::File(name)
    } else if first.is(FOLDER) {
        Target::Folder(name)
    } else if let Some(window) = window_named(&w) {
        Target::Window(window)
    } else {
        Target::Anything(name)
    };
    Sentence::Goto { target, within }
}

/// `goto map`, `open git`: one of tiny's own windows, when that is all that
/// was said. A quoted word is a file's name instead.
fn window_named(w: &[Word]) -> Option<Window> {
    let [only] = w else { return None };
    if only.quoted {
        return None;
    }
    Some(match only.text.to_lowercase().as_str() {
        "map" | "graph" => Window::Map,
        "git" | "source" | "changes" => Window::Git,
        "browser" | "tree" | "files" => Window::Browser,
        "help" | "keys" => Window::Help,
        "settings" | "config" | "preferences" | "options" => Window::Settings,
        _ => return None,
    })
}

/// `copy A to B`, or the older `copy A B`.
fn copy(w: &[Word]) -> Sentence {
    match two_ends(w) {
        Some((from, to)) => Sentence::Copy { from, to },
        None => Sentence::Incomplete("say it like: copy notes.md to archive".into()),
    }
}

/// `move A to B`, `put A in B`, `rename A to B`, or `rename B` for whatever
/// the cursor is on.
fn moving(w: &[Word], rename: bool) -> Sentence {
    if let Some((from, to)) = two_ends(w) {
        return Sentence::Move { from, to, rename };
    }
    let w = without_articles(w);
    match w.as_slice() {
        [] => Sentence::Incomplete(
            if rename {
                "say it like: rename notes.md to ideas.md"
            } else {
                "say it like: move notes.md to archive"
            }
            .into(),
        ),
        // One name: the new one, for what the cursor is on.
        _ => Sentence::Move {
            from: Place::This,
            to: joined(&w),
            rename,
        },
    }
}

/// The two sides of `A to B`: what is acted on, and where it goes. Two words
/// with nothing between them are read the same way, since `copy a b` cannot
/// mean anything else.
fn two_ends(w: &[Word]) -> Option<(Place, String)> {
    let (from, to) = match split_first(w, TO).or_else(|| split_last(w, IN)) {
        Some(split) => split,
        // Two words and nothing between them are two names, whatever they
        // are: `cp a b` copies a file called `a`.
        None => {
            return match w {
                [a, b] => Some((subject(std::slice::from_ref(a)), b.text.clone())),
                _ => None,
            };
        }
    };
    let to = joined(&without_articles(to));
    if to.is_empty() {
        return None;
    }
    Some((subject(from), to))
}

/// `delete A`, or `delete` alone.
fn delete(w: &[Word]) -> Sentence {
    Sentence::Delete(subject(w))
}

/// `new A`, `new folder A`, `make a file called A in B`. `folder` is already
/// decided for `mkdir`.
fn new(w: &[Word], folder: Option<bool>) -> Sentence {
    let w: Vec<Word> = w
        .iter()
        .filter(|x| !x.is(ARTICLES) && !x.is(&["new", "called", "named"]))
        .cloned()
        .collect();
    let (is_folder, w) = match (folder, w.first()) {
        (Some(f), _) => (f, &w[..]),
        (None, Some(k)) if k.is(FOLDER) => (true, &w[1..]),
        (None, Some(k)) if k.is(FILE) => (false, &w[1..]),
        (None, _) => (false, &w[..]),
    };
    let (name, within) = match split_last(w, &["in", "inside", "within", "under"]) {
        Some((name, place_words)) if !name.is_empty() => (joined(name), place(place_words)),
        _ => (joined(w), None),
    };
    if name.is_empty() {
        return Sentence::Incomplete("give it a name: new notes/today.md".into());
    }
    // A trailing slash says folder however it was asked for.
    let is_folder = is_folder || name.ends_with('/') || name.ends_with('\\');
    Sentence::New {
        name,
        folder: is_folder,
        within,
    }
}

/// `replace A with B [in C]`, `change A to B`, or the older `replace A B`.
fn replace(w: &[Word]) -> Sentence {
    let mut w: Vec<Word> = w
        .iter()
        .skip_while(|x| x.is(&["all", "every", "each", "the"]))
        .cloned()
        .collect();
    // `replace word hi` and `replace text hi`: whole words, or anywhere.
    let mut whole_words = true;
    if w.first().is_some_and(|x| x.is(&["word", "words"])) {
        w.remove(0);
    } else if w
        .first()
        .is_some_and(|x| x.is(&["text", "phrase", "string", "part"]))
    {
        w.remove(0);
        whole_words = false;
    }

    let Some((find, rest)) = split_first(&w, WITH) else {
        // The older spelling: two arguments, matched anywhere, everywhere.
        return match w.as_slice() {
            [find, with] => Sentence::Replace {
                find: find.text.clone(),
                with: with.text.clone(),
                within: None,
                whole_words: false,
            },
            _ => Sentence::Incomplete("say it like: replace hi with hello".into()),
        };
    };
    let (with, within) = match split_last(rest, IN) {
        Some((with, place_words)) if !with.is_empty() => (joined(with), place(place_words)),
        _ => {
            // `replace hi with hello everywhere`
            // `replace hi with hello everywhere` says what no `in` means.
            match rest.split_last() {
                Some((last, before)) if last.is(EVERYWHERE) && !before.is_empty() => {
                    (joined(before), None)
                }
                _ => (joined(rest), None),
            }
        }
    };
    let find = joined(find);
    if find.is_empty() {
        return Sentence::Incomplete("say what to replace: replace hi with hello".into());
    }
    Sentence::Replace {
        find,
        with,
        within,
        whole_words,
    }
}

// ---- what a sentence starts with ----------------------------------------

/// Every verb tiny reads, for completing the first word. One spelling of
/// each idea leads, so Tab offers the plainest.
pub const VERBS: &[&str] = &[
    "goto",
    "copy",
    "move",
    "rename",
    "delete",
    "new",
    "mkdir",
    "replace",
    "set",
    "commit",
    "reload",
    "settings",
    "update",
    "help",
    "line",
    "open",
    "find",
    "jump",
    "make",
    "create",
    "remove",
    "put",
    "change",
    "swap",
    "duplicate",
    "trash",
    "config",
    "cp",
    "mv",
    "rm",
    "sub",
];

/// What may follow `goto`, for completing the second word.
pub const KINDS: &[&str] = &[
    "line", "function", "word", "text", "heading", "file", "folder", "map", "git", "help",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn named(s: &str) -> Place {
        Place::Named(s.to_string())
    }

    fn goes(target: Target, within: Option<Place>) -> Sentence {
        Sentence::Goto { target, within }
    }

    // ---- goto -------------------------------------------------------------

    #[test]
    fn every_goto_in_the_notes_reads() {
        assert_eq!(parse("goto line 432"), goes(Target::Line(432), None));
        assert_eq!(
            parse("goto function test"),
            goes(Target::Function("test".into()), None)
        );
        assert_eq!(
            parse("goto word test"),
            goes(Target::Word("test".into()), None)
        );
        assert_eq!(
            parse("goto word test in testing.txt"),
            goes(Target::Word("test".into()), Some(named("testing.txt")))
        );
        assert_eq!(
            parse("goto file testing.txt"),
            goes(Target::File("testing.txt".into()), None)
        );
        assert_eq!(
            parse("goto folder test"),
            goes(Target::Folder("test".into()), None)
        );
    }

    #[test]
    fn going_somewhere_can_be_said_many_ways() {
        let want = goes(Target::Line(12), None);
        for line in [
            "goto line 12",
            "go to line 12",
            "go to 12",
            "jump to line 12",
            "line 12",
            "12",
            "please go to line 12",
            "can you go to line 12 please",
            "move to line 12",
            "take me to line 12",
        ] {
            assert_eq!(parse(line), want, "{line}");
        }
        assert_eq!(
            parse("open the file notes.md"),
            goes(Target::File("notes.md".into()), None)
        );
        assert_eq!(
            parse("find function main in app.rs"),
            goes(Target::Function("main".into()), Some(named("app.rs")))
        );
    }

    #[test]
    fn a_line_can_be_of_a_file() {
        assert_eq!(
            parse("goto line 12 of main.rs"),
            goes(Target::Line(12), Some(named("main.rs")))
        );
        assert_eq!(
            parse("line 3 in notes.md"),
            goes(Target::Line(3), Some(named("notes.md")))
        );
    }

    #[test]
    fn of_only_splits_a_line_so_a_heading_can_say_it() {
        assert_eq!(
            parse("goto heading history of tiny"),
            goes(Target::Heading("history of tiny".into()), None)
        );
    }

    #[test]
    fn with_no_kind_the_name_is_anything() {
        assert_eq!(
            parse("goto notes"),
            goes(Target::Anything("notes".into()), None)
        );
        assert_eq!(
            parse("open the notes.md"),
            goes(Target::Anything("notes.md".into()), None),
            "`the` in front is not part of the name"
        );
        assert_eq!(
            parse("open my notes.md"),
            goes(Target::Anything("my notes.md".into()), None),
            "but `my` may well be"
        );
        assert_eq!(
            parse("open a"),
            goes(Target::Anything("a".into()), None),
            "and a name can be just `a`"
        );
    }

    #[test]
    fn windows_open_by_name_unless_quoted() {
        assert_eq!(parse("goto map"), goes(Target::Window(Window::Map), None));
        assert_eq!(parse("open git"), goes(Target::Window(Window::Git), None));
        assert_eq!(parse("help"), goes(Target::Window(Window::Help), None));
        assert_eq!(
            parse("settings"),
            goes(Target::Window(Window::Settings), None)
        );
        assert_eq!(
            parse("open \"map\""),
            goes(Target::Anything("map".into()), None),
            "a file called map"
        );
    }

    #[test]
    fn in_this_file_and_everywhere_are_places_too() {
        assert_eq!(
            parse("goto word fixme in this file"),
            goes(Target::Word("fixme".into()), Some(Place::This))
        );
        assert_eq!(
            parse("goto word fixme in the whole project"),
            goes(Target::Word("fixme".into()), None)
        );
    }

    #[test]
    fn only_the_last_in_splits_so_a_name_can_have_one() {
        assert_eq!(
            parse("goto text sign in in login.md"),
            goes(Target::Text("sign in".into()), Some(named("login.md")))
        );
    }

    #[test]
    fn a_goto_with_nothing_to_go_to_says_how() {
        assert!(matches!(parse("goto"), Sentence::Incomplete(_)));
        assert!(matches!(parse("goto line"), Sentence::Incomplete(_)));
        assert!(
            matches!(parse("goto line twelve"), Sentence::Incomplete(why) if why.contains("not a line number"))
        );
        assert!(matches!(parse("goto function"), Sentence::Incomplete(_)));
    }

    // ---- files ------------------------------------------------------------

    #[test]
    fn copy_reads_like_the_sentence() {
        let want = Sentence::Copy {
            from: named("temp.txt"),
            to: "notes".into(),
        };
        for line in [
            "copy temp.txt to notes",
            "copy temp.txt into notes",
            "cp temp.txt notes",
            "duplicate the file temp.txt into the notes",
            "please copy temp.txt to notes",
        ] {
            assert_eq!(parse(line), want, "{line}");
        }
    }

    #[test]
    fn quotes_keep_a_joining_word_in_a_name() {
        assert_eq!(
            parse("copy \"to do.md\" to archive"),
            Sentence::Copy {
                from: named("to do.md"),
                to: "archive".into()
            }
        );
    }

    #[test]
    fn copy_keeps_every_word_of_a_name() {
        assert_eq!(
            parse("copy my notes.md to old work.md"),
            Sentence::Copy {
                from: named("my notes.md"),
                to: "old work.md".into()
            }
        );
        assert_eq!(
            parse("cp a b"),
            Sentence::Copy {
                from: named("a"),
                to: "b".into()
            },
            "short names are names"
        );
    }

    #[test]
    fn copying_this_means_the_cursor() {
        assert_eq!(
            parse("copy this to notes"),
            Sentence::Copy {
                from: Place::This,
                to: "notes".into()
            }
        );
    }

    #[test]
    fn move_and_rename_differ_only_in_what_the_destination_may_be() {
        assert_eq!(
            parse("move temp.txt to notes"),
            Sentence::Move {
                from: named("temp.txt"),
                to: "notes".into(),
                rename: false
            }
        );
        assert_eq!(
            parse("put temp.txt in notes"),
            Sentence::Move {
                from: named("temp.txt"),
                to: "notes".into(),
                rename: false
            }
        );
        assert_eq!(
            parse("rename old.md to new.md"),
            Sentence::Move {
                from: named("old.md"),
                to: "new.md".into(),
                rename: true
            }
        );
        assert_eq!(
            parse("rename ideas.md"),
            Sentence::Move {
                from: Place::This,
                to: "ideas.md".into(),
                rename: true
            },
            "one name renames what the cursor is on"
        );
        assert_eq!(
            parse("mv a.md b.md"),
            Sentence::Move {
                from: named("a.md"),
                to: "b.md".into(),
                rename: false
            }
        );
    }

    #[test]
    fn delete_takes_a_name_or_the_cursor() {
        assert_eq!(parse("delete old.md"), Sentence::Delete(named("old.md")));
        assert_eq!(
            parse("remove the file old.md"),
            Sentence::Delete(named("old.md"))
        );
        assert_eq!(parse("rm"), Sentence::Delete(Place::This));
        assert_eq!(parse("delete this"), Sentence::Delete(Place::This));
        assert_eq!(parse("trash it"), Sentence::Delete(Place::This));
    }

    #[test]
    fn new_things_can_be_files_or_folders_and_somewhere() {
        assert_eq!(
            parse("new notes/today.md"),
            Sentence::New {
                name: "notes/today.md".into(),
                folder: false,
                within: None
            }
        );
        assert_eq!(
            parse("make a new folder called ideas in notes"),
            Sentence::New {
                name: "ideas".into(),
                folder: true,
                within: Some(named("notes"))
            }
        );
        assert_eq!(
            parse("mkdir drafts"),
            Sentence::New {
                name: "drafts".into(),
                folder: true,
                within: None
            }
        );
        assert_eq!(
            parse("create drafts/"),
            Sentence::New {
                name: "drafts/".into(),
                folder: true,
                within: None
            }
        );
    }

    // ---- replace ----------------------------------------------------------

    #[test]
    fn replace_with_no_in_is_the_whole_project() {
        assert_eq!(
            parse("replace hi with hello"),
            Sentence::Replace {
                find: "hi".into(),
                with: "hello".into(),
                within: None,
                whole_words: true
            }
        );
        assert_eq!(
            parse("replace word hi with hello in greetings.txt"),
            Sentence::Replace {
                find: "hi".into(),
                with: "hello".into(),
                within: Some(named("greetings.txt")),
                whole_words: true
            }
        );
    }

    #[test]
    fn replace_can_be_said_many_ways() {
        let want = Sentence::Replace {
            find: "colour".into(),
            with: "color".into(),
            within: None,
            whole_words: true,
        };
        for line in [
            "replace colour with color",
            "change colour to color",
            "swap colour for color",
            "replace all colour with color",
            "replace colour with color everywhere",
            "replace colour with color in all files",
        ] {
            assert_eq!(parse(line), want, "{line}");
        }
    }

    #[test]
    fn the_older_replace_still_matches_anywhere() {
        assert_eq!(
            parse("replace \"old thing\" \"new thing\""),
            Sentence::Replace {
                find: "old thing".into(),
                with: "new thing".into(),
                within: None,
                whole_words: false
            }
        );
        assert_eq!(
            parse("sub colour color"),
            Sentence::Replace {
                find: "colour".into(),
                with: "color".into(),
                within: None,
                whole_words: false
            }
        );
    }

    #[test]
    fn replace_text_matches_inside_words() {
        assert_eq!(
            parse("replace text ing with ed in this file"),
            Sentence::Replace {
                find: "ing".into(),
                with: "ed".into(),
                within: Some(Place::This),
                whole_words: false
            }
        );
    }

    #[test]
    fn replacing_with_nothing_is_allowed() {
        assert_eq!(
            parse("replace \"very \" with \"\""),
            Sentence::Replace {
                find: "very ".into(),
                with: String::new(),
                within: None,
                whole_words: true
            }
        );
    }

    // ---- the rest ---------------------------------------------------------

    #[test]
    fn the_older_commands_still_read() {
        assert_eq!(
            parse("set tab_width to 2"),
            Sentence::Set(vec!["tab_width".into(), "to".into(), "2".into()])
        );
        assert_eq!(
            parse("commit fix the map"),
            Sentence::Commit("fix the map".into())
        );
        assert_eq!(parse("reload"), Sentence::Reload);
        assert_eq!(parse("refresh"), Sentence::Reload);
        assert_eq!(parse("update"), Sentence::Update);
    }

    #[test]
    fn an_unknown_word_is_kept_for_a_plugin_or_a_name() {
        assert_eq!(
            parse("words now"),
            Sentence::Other {
                name: "words".into(),
                args: vec![],
            },
            "a trailing `now` is politeness"
        );
        assert_eq!(
            parse("wordcount notes.md"),
            Sentence::Other {
                name: "wordcount".into(),
                args: vec!["notes.md".into()],
            }
        );
    }

    #[test]
    fn nothing_is_nothing() {
        assert_eq!(parse(""), Sentence::Empty);
        assert_eq!(parse("   please  "), Sentence::Empty);
    }
}
