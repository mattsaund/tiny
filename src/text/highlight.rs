//! Syntax highlighting, wrapping syntect and converting its output into
//! ratatui spans.
//!
//! Highlighting is stateful (syntect has to walk a file from the top to know
//! whether line 400 sits inside a block comment) so the window API below
//! parses down to the window but only *collects* the lines actually on
//! screen, then stops. Nothing below the window is ever read.
//!
//! # Resuming instead of restarting
//!
//! Parsing from line 0 every frame is correct and, for a note, instant. It is
//! also quadratic in disguise: the cost is proportional to how far down the
//! file you are, and it is paid again on every keystroke. Measured on a
//! 6000-line Python file, one frame took **392ms**: a third of a second
//! between pressing a key and seeing it.
//!
//! So [`Resume`] keeps the parser's state every [`STRIDE`] lines and starts
//! from the nearest one at or above the window. The worst case becomes
//! `STRIDE` lines of parsing rather than however many thousand you have
//! scrolled past, and the checkpoints are filled in by the parse that had to
//! happen anyway.
//!
//! Invalidation is the part that has to be right, and it rests on one fact:
//! every mutation on `Editor` goes through `push_undo`, which records the
//! lowest line it touched. `Editor::take_touched` reports that, everything
//! from it down is dropped, and everything above is still true, because a
//! change on line 900 cannot alter what the parser believed on line 100.
//!
//! # Any length, and nothing waits for it
//!
//! Resuming covers moving about, but not the first jump into a long file: the
//! state at the bottom of a 100,000-line file is only known by reading all
//! 100,000 lines, and that takes nine seconds. A file past 20,000 lines used
//! to be drawn plain rather than read that far.
//!
//! Now a window more than [`REACH`] lines below every exact state is drawn
//! from a guess: a fresh start a little above it, at a line that looks like
//! the start of a function or a type, see [`sync_point`]. That is Vim's answer
//! to the same problem, and the state there is nearly always the parser's
//! first one, so the guess is nearly always right. Then, whenever nothing else
//! is happening, exact states are read on down toward the window, each taking
//! the place of a guessed one, until the window is drawn exactly; a key stops
//! the reading the moment it is pressed, see [`Highlighter::read_on`]. A frame
//! reads a few hundred lines at most, or after a jump into the middle of a
//! long class, up to [`FAR`]; so every file gets colors, however long, and
//! nothing waits long for them.
//!
//! # Each frame reads only what changed
//!
//! Even resumed, a frame read up to fifty lines to reach its window and then
//! the window itself, on every key: 3 ms of a 3.4 ms keystroke, in a long
//! file of Rust. Yet a key changes one line. So [`Resume`] keeps each line as
//! the last frame read it, with the state it was read from and the state it
//! left (see [`Drawn`]), and a line that says the same and is reached in the
//! same state is taken as it was, since syntect colors a line from nothing
//! else. A typed letter reads its own line again, and the lines after it only
//! as far as the state it leaves has changed; an unchanged frame reads
//! nothing. That keystroke now takes 0.36 ms.
//!
//! # Two different highlighters
//!
//! This module and `markdown` both style text, and they do not overlap:
//! `highlight` colors *code* by grammar and is used for the editor pane and
//! for fenced blocks inside notes; `markdown` styles *prose* structure and
//! calls into here for those fenced blocks. Editor styling comes from the
//! syntect theme; everything else comes from the palette.
//!
//! # Where the grammars come from
//!
//! `two_face`, not syntect's own defaults. syntect ships 75 syntaxes and the
//! gaps in them are not exotic: no TypeScript, no TOML, no Kotlin, no Swift,
//! no Dockerfile. `two_face` is the same curated set `bat` uses, and brings it
//! to 213. The cost is about 600 KiB of binary; the alternative was a table
//! mapping `.ts` onto JavaScript and hoping, which is worse than it sounds
//! because a wrong grammar mis-styles a file confidently.
//!
//! Grammars carry their own licenses, listed by [`acknowledgments`] and shown
//! by `tiny --licenses`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use syntect::highlighting::{
    FontStyle, HighlightIterator, HighlightState, Highlighter as SynHighlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// A single highlighted run of text within a line.
///
/// A line is a `Vec<Piece>`; the whole window is a `Vec<Vec<Piece>>`. The text
/// is owned rather than borrowed because it outlives the parse: `ui` clips it
/// horizontally before turning it into spans.
pub type Piece = (Style, String);

/// Owns the loaded syntax definitions and the active theme.
///
/// Construction unpacks 213 syntax definitions, which is the single most
/// expensive thing tiny does at startup, so `App` builds exactly one and keeps
/// it for the life of the program. Changing `syntax_theme` goes through
/// [`Highlighter::set_theme`], which swaps only the theme; rebuilding the
/// whole thing to change a color scheme would cost the same as another
/// startup.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
    /// The name the theme was asked for by, so asking again for the same one
    /// is known to change nothing.
    theme_name: String,
}

/// The longest line, in bytes, that is parsed for color. A longer one is
/// drawn plain and the parser carries on below it as though it were not
/// there: a minified script is one line of a megabyte or more, and parsing
/// it took longer than a frame, every frame. Nobody reads a line that long
/// for its colors; the lines around it still have theirs.
const MAX_HIGHLIGHT_LINE: usize = 16 * 1024;

/// File names whose grammar cannot be worked out from the extension.
///
/// There is exactly one entry and it is tiny's own config file. `tiny.conf` is
/// TOML, but `.conf` belongs to nginx as far as the grammars are concerned, so
/// without this the first file a new user opens to change a setting is styled
/// as a web server config. Resist growing this list: an override is a claim
/// that we know better than the grammar, and it is only true here because we
/// are the ones who chose the extension.
const NAME_OVERRIDES: &[(&str, &str)] = &[(crate::config::CONF_NAME, "toml")];

/// How many lines apart the saved parser states in a [`Resume`] sit.
///
/// This is the whole trade-off in one number: a frame may have to parse up to
/// `STRIDE` lines to reach the top of the window, and a file keeps one saved
/// state per `STRIDE` lines. Fifty puts the worst case at a few milliseconds
/// and a 100,000-line file at 2,000 saved states, which is nothing.
const STRIDE: usize = 50;

/// How far below the nearest exact state a window may be and still be read
/// down to from it: a few dozen milliseconds of Rust at the most. Further,
/// and the parse starts afresh near the window, see the module docs.
const REACH: usize = 300;

/// How far above a window a fresh start is looked for first, see
/// [`sync_point`].
const SYNC: usize = 200;

/// How far above it a block's first line is looked for when nothing nearer
/// will do, see [`sync_point`]: as long as a long class runs. Reading that far
/// costs a jump there a tenth of a second or two, once.
const FAR: usize = 2_000;

/// How long a frame drawn only to read exact states on down toward a guess
/// may spend on it, see [`Highlighter::read_on`]. A key pressed meanwhile
/// stops it at once, so this only sets how often the screen is drawn while
/// the reading goes on.
const READ_ON: Duration = Duration::from_millis(25);

/// The furthest down a file exact states are read toward a guess. Past it the
/// guess stands, as it always does in Vim: 200,000 lines of Rust are twenty
/// seconds of reading, and further would keep a core busy for longer than
/// exact colors are worth.
const CATCH_UP_LINES: usize = 200_000;

/// A parser partway through a file: what it had read, and how it was
/// coloring.
type State = (ParseState, HighlightState);

/// A saved [`State`], shared by every checkpoint that saved the same one: a
/// long file leaves the parser in the same few states over and over, and a
/// 100,000-line file of Rust saved 2,000 of them where 330 were different.
type Checkpoint = Rc<State>;

/// How many of the states saved last are looked through for one the same as
/// a new one, see [`Resume::keep`].
const RECENT: usize = 16;

/// One line as a frame read it: the state it was read from, what it said, the
/// colors that came of it (none when it was read only for the state it left,
/// above the window) and the state it left.
struct Read {
    enter: Rc<State>,
    text: String,
    pieces: Option<Vec<Piece>>,
    leave: Rc<State>,
}

/// The lines the last frame read, for the next to take what it can rather
/// than read them again, see the module docs.
#[derive(Default)]
struct Drawn {
    /// The line `lines[0]` is.
    first: usize,
    /// How many lines the file had then, so a line added or taken away above
    /// another does not keep it from being found.
    of: usize,
    lines: Vec<Option<Read>>,
}

impl Drawn {
    /// Line `i` of a file now `now` lines long, as the last frame read it,
    /// when it says the same and was read from the same state, so that what
    /// came of it would come of it again: taken, for this frame to keep.
    /// Looked for where it was, and as far off as the file has grown or
    /// shrunk since.
    fn take(
        &mut self,
        i: usize,
        text: &str,
        now: usize,
        enter: &Rc<State>,
        pieces: bool,
    ) -> Option<Read> {
        let fits = |r: &mut Read| {
            r.text == text
                && (r.pieces.is_some() || !pieces)
                && (Rc::ptr_eq(&r.enter, enter) || *r.enter == **enter)
        };
        let first = self.first;
        let mut at = |j: usize| {
            j.checked_sub(first)
                .and_then(|k| self.lines.get_mut(k))
                .and_then(|slot| slot.take_if(fits))
        };
        let moved = (i + self.of).checked_sub(now).filter(|&j| j != i);
        at(i).or_else(|| moved.and_then(at))
    }
}

/// Saved parser state, so a window deep in a file is reached by resuming
/// rather than by parsing everything above it again.
///
/// Each pane that draws code keeps one (the editor, the pane set aside in a
/// split, each side of the git diff) and it follows whichever file that pane
/// shows. Switching files throws it away and rebuilds: a cache per file would
/// have to be evicted, which is a second problem in exchange for a saving
/// nobody would notice.
///
/// Two kinds of state are kept, see the module docs: the exact ones, reached
/// by reading from the first line, and below them the ones reached by
/// starting afresh near a window that was too far down to read to.
#[derive(Default)]
pub struct Resume {
    /// What these states describe. Anything else and they are meaningless:
    /// the same lines under a different grammar parse differently.
    key: Option<(PathBuf, String)>,
    /// `states[i]` is the parser entering line `i * STRIDE`. Contiguous from
    /// the start of the file, so index `i` is only present once every line
    /// above it has been parsed at least once.
    states: Vec<Checkpoint>,
    /// The same, by index, for lines below the last of `states`, reached by
    /// a fresh start partway down. Nearly always the same as an exact one
    /// would be, and replaced by it when the exact ones reach this far.
    near: BTreeMap<usize, Checkpoint>,
    /// Where the last window drawn from a guess starts, while exact states
    /// are still being read on down toward it, see [`Resume::behind`].
    guessed: Option<usize>,
    /// How far that reading had got when a frame's time ran out between two
    /// saved states: the parser entering that line, for the next frame to
    /// carry on from.
    reading: Option<(usize, State)>,
    /// Whether reading on down met a line syntect would not parse, past which
    /// there are no exact states to be had. Until an edit, guesses stand.
    stuck: bool,
    /// The different states saved last, for a new checkpoint to share.
    recent: Vec<Checkpoint>,
    /// What the last window read, line by line, see [`Drawn`].
    drawn: Option<Drawn>,
    /// How many lines the last window had to read rather than take from the
    /// one before: what a frame cost, for the tests to see.
    parsed: usize,
}

impl std::fmt::Debug for Resume {
    /// What the states are of and how many there are; the parser's own state
    /// is not worth reading.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resume")
            .field("key", &self.key)
            .field("states", &self.states.len())
            .field("near", &self.near.len())
            .field("guessed", &self.guessed)
            .field("parsed", &self.parsed)
            .finish()
    }
}

impl Resume {
    /// Bring the cache into line with what is about to be drawn: throw it out
    /// if it belongs to another file or grammar, and drop the part of it an
    /// edit invalidated.
    ///
    /// Called before every use, and it is the only place either kind of
    /// invalidation happens.
    pub fn sync(&mut self, path: &Path, syntax: &str, touched: Option<usize>) {
        let key = (path.to_path_buf(), syntax.to_string());
        if self.key.as_ref() != Some(&key) {
            self.clear();
            self.key = Some(key);
            return;
        }
        if let Some(line) = touched {
            // A state entering line `i * STRIDE` was decided by the lines
            // above it, so it survives an edit at or after its own line. Keep
            // index `i` while `i * STRIDE <= line`.
            self.states.truncate(line / STRIDE + 1);
            drop(self.near.split_off(&(line / STRIDE + 1)));
            self.reading.take_if(|(at, _)| *at > line);
            self.stuck = false;
        }
    }

    /// Forget everything. Used when the theme changes, since the saved
    /// highlight states carry styles taken from it.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether the last window drawn from here started from a guess that
    /// exact states are still being read toward, see
    /// [`Highlighter::read_on`]. While it did, there is reason to draw again.
    pub fn behind(&self) -> bool {
        self.guessed.is_some()
    }

    /// A checkpoint of `state`: one already saved, when one of the last few
    /// is the same, or else `state` itself.
    fn keep(&mut self, state: &Rc<State>) -> Checkpoint {
        if let Some(same) = self
            .recent
            .iter()
            .find(|c| Rc::ptr_eq(c, state) || ***c == **state)
        {
            return Rc::clone(same);
        }
        if self.recent.len() == RECENT {
            self.recent.remove(0);
        }
        self.recent.push(Rc::clone(state));
        Rc::clone(state)
    }
}

impl Highlighter {
    /// A highlighter with the default theme, discarding the warning that
    /// cannot happen for a name syntect is known to ship. The program always
    /// names its theme, from the config; this is for tests.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_theme("base16-ocean.dark").0
    }

    /// Build with a named syntect theme. Returns a warning alongside when the
    /// name is not one syntect ships, so a typo in `tiny.conf` is visible
    /// rather than silently ignored.
    pub fn with_theme(name: &str) -> (Self, Option<String>) {
        let (theme, warning) = load_theme(name);
        (
            Self {
                syntaxes: two_face::syntax::extra_newlines(),
                theme,
                theme_name: name.to_string(),
            },
            warning,
        )
    }

    /// The name of the theme in use, as it was asked for.
    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// Swap the theme in place, keeping the syntax set.
    ///
    /// This is what `*set syntax_theme` calls. Rebuilding the `Highlighter`
    /// would re-unpack every grammar to change a palette, which is the whole
    /// startup cost paid again for nothing.
    pub fn set_theme(&mut self, name: &str) -> Option<String> {
        let (theme, warning) = load_theme(name);
        self.theme = theme;
        self.theme_name = name.to_string();
        warning
    }

    /// The no-op syntax, used for unknown file types and unlabeled fences.
    fn plain(&self) -> &SyntaxReference {
        self.syntaxes.find_syntax_plain_text()
    }

    /// Pick a syntax from a file path.
    ///
    /// Three passes, narrowest first. The whole file name is tried before the
    /// extension because a great many files that matter have no useful
    /// extension at all (`Makefile`, `Dockerfile`, `.gitignore`, `go.mod`,
    /// `Cargo.lock`) and the grammars register those names in the same field
    /// syntect matches extensions against. Extension is the ordinary case, and
    /// the first line catches `#!/usr/bin/env python` on a script with neither.
    ///
    /// A name in [`NAME_OVERRIDES`] wins over all three; see there for why.
    pub fn syntax_for_path(&self, path: &Path, first_line: &str) -> &SyntaxReference {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(token) = NAME_OVERRIDES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| t)
            && let Some(syntax) = self.syntaxes.find_syntax_by_token(token)
        {
            return syntax;
        }
        self.syntaxes
            .find_syntax_by_extension(name)
            .or_else(|| {
                path.extension()
                    .and_then(|e| e.to_str())
                    .and_then(|e| self.syntaxes.find_syntax_by_extension(e))
            })
            .or_else(|| self.syntaxes.find_syntax_by_first_line(first_line))
            .unwrap_or_else(|| self.plain())
    }

    /// Pick a syntax from a fenced code block's info string (```python).
    ///
    /// Both lookups hand back a reference, and it is worth keeping one: a
    /// clone of a `SyntaxReference` carries its grammar serialized rather than
    /// parsed, and deserializes it all over again the first time it is used.
    /// Cloned per code block per frame, that was most of what drawing a note
    /// cost.
    pub fn syntax_for_token(&self, token: &str) -> &SyntaxReference {
        let token = token.split_whitespace().next().unwrap_or("").trim();
        if token.is_empty() {
            return self.plain();
        }
        self.syntaxes
            .find_syntax_by_token(token)
            .or_else(|| self.syntaxes.find_syntax_by_extension(token))
            .unwrap_or_else(|| self.plain())
    }

    /// Highlight `count` lines starting at `start`, with no saved state: the
    /// parse runs from line 0 every time.
    ///
    /// For snippets and short files this is the whole story. The editor uses
    /// [`Highlighter::highlight_cached`] instead, because for a long file it
    /// is not.
    pub fn highlight_window(
        &self,
        lines: &[String],
        syntax: &SyntaxReference,
        start: usize,
        count: usize,
    ) -> Vec<Vec<Piece>> {
        self.window(lines, syntax, start, count, &mut Resume::default(), false)
    }

    /// The same, resuming from `resume` and extending it as it goes.
    ///
    /// A window far enough down a long file is drawn from a guess, see the
    /// module docs, and [`Resume::behind`] then says so: [`Highlighter::read_on`]
    /// is what puts exact colors in its place.
    ///
    /// The caller must have called [`Resume::sync`] first; without it the
    /// cache could still be describing a different file.
    pub fn highlight_cached(
        &self,
        resume: &mut Resume,
        lines: &[String],
        syntax: &SyntaxReference,
        start: usize,
        count: usize,
    ) -> Vec<Vec<Piece>> {
        self.window(lines, syntax, start, count, resume, true)
    }

    /// Read exact states on down from the last one toward the window last
    /// drawn from a guess, each taking the place of a guessed one, for up to
    /// [`READ_ON`] or until `stop` says something else wants the time. Once
    /// they are within [`REACH`] of the window, the next frame draws it
    /// exactly.
    ///
    /// Only for a frame nobody is waiting on: until it returns, the frame is
    /// not on screen.
    pub fn read_on(
        &self,
        resume: &mut Resume,
        lines: &[String],
        syntax: &SyntaxReference,
        stop: impl Fn() -> bool,
    ) {
        let Some(target) = resume.guessed else {
            return;
        };
        let began = Instant::now();
        let hi = SynHighlighter::new(&self.theme);
        // From where the last frame stopped, when that is past the last
        // saved state, or else from that state.
        let last = resume.states.len().saturating_sub(1) * STRIDE;
        let (from, (mut parse, mut hl)) = match resume.reading.take() {
            Some((at, state)) if at > last => (at, state),
            _ => (
                last,
                resume.states.last().map_or_else(
                    || {
                        (
                            ParseState::new(syntax),
                            HighlightState::new(&hi, ScopeStack::new()),
                        )
                    },
                    |c| State::clone(c),
                ),
            ),
        };
        let mut buf = String::new();
        for (i, line) in lines.iter().enumerate().skip(from) {
            if i % STRIDE == 0 && resume.states.len() == i / STRIDE {
                let saved = resume.keep(&Rc::new((parse.clone(), hl.clone())));
                resume.states.push(saved);
                resume.near.remove(&(i / STRIDE));
                if i + REACH >= target {
                    return;
                }
            }
            // Asking costs a system call, so only every few lines.
            if began.elapsed() >= READ_ON || (i % 8 == 0 && stop()) {
                resume.reading = Some((i, (parse, hl)));
                return;
            }
            if line.len() > MAX_HIGHLIGHT_LINE {
                continue;
            }
            buf.clear();
            buf.push_str(line);
            buf.push('\n');
            let Ok(ops) = parse.parse_line(&buf, &self.syntaxes) else {
                resume.stuck = true;
                resume.guessed = None;
                return;
            };
            HighlightIterator::new(&mut hl, &ops, &buf, &hi).for_each(drop);
        }
    }

    /// The shared body.
    ///
    /// Lines below the window are never touched; lines above it are parsed for
    /// state only, so that a line 400 deep inside a docstring is styled as a
    /// string rather than as code, and `resume` is what keeps "above it" from
    /// meaning "all of them".
    ///
    /// With `guess`, a window too far below every exact state starts afresh
    /// just above itself, see the module docs, so no call reads more than
    /// about [`REACH`] lines before its window, however long the file.
    /// Without it, the parse reads down from the top however far that is.
    ///
    /// Two escape hatches drop to unstyled text rather than failing: a line
    /// past [`MAX_HIGHLIGHT_LINE`], and any line syntect refuses to parse.
    /// Neither should stop you editing.
    fn window(
        &self,
        lines: &[String],
        syntax: &SyntaxReference,
        start: usize,
        count: usize,
        resume: &mut Resume,
        guess: bool,
    ) -> Vec<Vec<Piece>> {
        // Whatever the last window was waiting for, this one is what is on
        // screen now.
        resume.guessed = None;
        let end = (start + count).min(lines.len());
        if start >= end {
            return Vec::new();
        }
        let plain = || -> Vec<Vec<Piece>> {
            lines[start..end]
                .iter()
                .map(|l| vec![(Style::default(), l.clone())])
                .collect()
        };
        let hi = SynHighlighter::new(&self.theme);
        let fresh = || {
            Rc::new((
                ParseState::new(syntax),
                HighlightState::new(&hi, ScopeStack::new()),
            ))
        };
        // Where to read from: the nearest exact state at or above the window,
        // when it is in reach. `states` is contiguous from the start of the
        // file, so when it is too short its last is the furthest down an exact
        // read can start. Out of reach, the nearest state started afresh, and
        // failing that, a fresh start just above the window.
        let at = (start / STRIDE).min(resume.states.len().saturating_sub(1));
        let exact_from = if resume.states.is_empty() {
            0
        } else {
            at * STRIDE
        };
        let near = resume
            .near
            .range(..=start / STRIDE)
            .next_back()
            .filter(|(i, _)| start - *i * STRIDE <= REACH);
        let (mut state, first, exact) = if !guess || start - exact_from <= REACH {
            let state = resume.states.get(at).map_or_else(fresh, Rc::clone);
            (state, exact_from, true)
        } else if let Some((i, state)) = near {
            (Rc::clone(state), i * STRIDE, false)
        } else {
            (fresh(), sync_point(lines, start), false)
        };
        resume.guessed = (!exact && !resume.stuck && start <= CATCH_UP_LINES).then_some(start);

        let mut out = Vec::with_capacity(end - start);
        let mut last = resume.drawn.take().unwrap_or_default();
        let mut drawn = Vec::with_capacity(end - first);
        resume.parsed = 0;
        // One buffer, refilled. syntect's syntaxes are the `_newlines`
        // variants, so each line needs its terminator to parse the way it
        // would in a file; allocating a fresh `String` for that meant one heap
        // allocation per line parsed, every frame.
        let mut buf = String::new();
        for (i, line) in lines.iter().enumerate().take(end).skip(first) {
            // Save the state entering this line if it is a checkpoint we do
            // not already hold. Filled in by the parse that had to happen
            // anyway, so building the cache costs nothing extra. An exact one
            // takes the place of one started afresh.
            if i % STRIDE == 0 {
                let k = i / STRIDE;
                if exact && resume.states.len() == k {
                    let saved = resume.keep(&state);
                    resume.states.push(saved);
                    resume.near.remove(&k);
                } else if !exact && k >= resume.states.len() && !resume.near.contains_key(&k) {
                    let saved = resume.keep(&state);
                    resume.near.insert(k, saved);
                }
            }
            let wanted = i >= start;
            let read = match last.take(i, line, lines.len(), &state, wanted) {
                Some(read) => read,
                None => {
                    resume.parsed += 1;
                    // Bail out to plain text rather than dropping the rest.
                    let Some(read) = self.read(line, &state, wanted, &hi, &mut buf) else {
                        return plain();
                    };
                    read
                }
            };
            if wanted {
                out.push(read.pieces.clone().unwrap_or_default());
            }
            state = Rc::clone(&read.leave);
            drawn.push(Some(read));
        }
        resume.drawn = Some(Drawn {
            first,
            of: lines.len(),
            lines: drawn,
        });
        out
    }

    /// Read one line from the state `enter`: the state it leaves, and with
    /// `pieces`, its colors. `None` for a line syntect will not parse.
    fn read(
        &self,
        line: &str,
        enter: &Rc<State>,
        pieces: bool,
        hi: &SynHighlighter,
        buf: &mut String,
    ) -> Option<Read> {
        let text = line.to_string();
        if line.len() > MAX_HIGHLIGHT_LINE {
            return Some(Read {
                enter: Rc::clone(enter),
                pieces: pieces.then(|| vec![(Style::default(), text.clone())]),
                text,
                leave: Rc::clone(enter),
            });
        }
        let (mut parse, mut hl) = State::clone(enter);
        buf.clear();
        buf.push_str(line);
        buf.push('\n');
        let ops = parse.parse_line(buf, &self.syntaxes).ok()?;
        let ranges = HighlightIterator::new(&mut hl, &ops, buf, hi);
        let pieces = if pieces {
            Some(
                ranges
                    .map(|(sty, text)| {
                        (convert_style(sty), text.trim_end_matches('\n').to_string())
                    })
                    .filter(|(_, t)| !t.is_empty())
                    .collect(),
            )
        } else {
            // Read for its state only. The iterator still has to be run: it
            // is what moves the coloring state on.
            ranges.for_each(drop);
            None
        };
        // Most lines leave the parser as they found it: shared, so the next
        // line's check against the last frame is a pointer's.
        let leave = if enter.0 == parse && enter.1 == hl {
            Rc::clone(enter)
        } else {
            Rc::new((parse, hl))
        };
        Some(Read {
            enter: Rc::clone(enter),
            text,
            pieces,
            leave,
        })
    }

    /// Highlight a short standalone snippet, e.g. a fenced block in markdown.
    ///
    /// A snippet is self-contained, so there is no earlier state to recover
    /// and the whole thing is both parsed and collected.
    pub fn highlight_snippet(&self, text: &str, syntax: &SyntaxReference) -> Vec<Vec<Piece>> {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let n = lines.len();
        self.highlight_window(&lines, syntax, 0, n)
    }
}

/// Where to start reading afresh for a window at `start`, too far below every
/// exact state to read down to.
///
/// First choice, within [`SYNC`] lines: the nearest line that begins at the
/// margin straight after a blank one. In nearly every language that is where
/// a function, a type or a section begins, outside any string or comment,
/// where the parser's first state is the right one. Inside a long class or
/// block nothing is at the margin, so next, within [`FAR`], the nearest line
/// at the margin that opens a block: the class itself. Failing both, [`SYNC`]
/// lines above, which in a file of code is usually right as well.
///
/// Measured on 136 long files in nine languages, a window drawn from this
/// guess matched one read from the top 95 times in 100, and where it did not,
/// mostly by the shade of a token or two. JavaScript, Rust, HTML and Markdown
/// did best, at 99 in 100 or better.
fn sync_point(lines: &[String], start: usize) -> usize {
    let at_margin = |i: usize| lines[i].starts_with(|c: char| !c.is_whitespace());
    let floor = start.saturating_sub(SYNC);
    (floor + 1..=start)
        .rev()
        .find(|&i| at_margin(i) && lines[i - 1].trim().is_empty())
        .or_else(|| {
            (start.saturating_sub(FAR)..=start)
                .rev()
                .find(|&i| at_margin(i) && opens_block(&lines[i]))
        })
        .unwrap_or(floor)
}

/// Whether a line opens a block: ends in `{`, `(`, `[` or `:`. Not a C
/// preprocessor line or a label (`out:`), which sit at the margin in the
/// middle of a function.
fn opens_block(line: &str) -> bool {
    let line = line.trim_end();
    let label = line
        .strip_suffix(':')
        .is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_alphanumeric() || c == '_'));
    line.ends_with(['{', '(', '[', ':']) && !line.starts_with('#') && !label
}

/// Look up a named theme, falling back with a warning when it is not one we
/// ship. Shared by [`Highlighter::with_theme`] and [`Highlighter::set_theme`]
/// so a typo reports the same way whether it came from the config file or from
/// `*set`.
fn load_theme(name: &str) -> (Theme, Option<String>) {
    let themes = ThemeSet::load_defaults();
    match themes.themes.get(name) {
        Some(t) => (t.clone(), None),
        None => {
            let fallback = themes
                .themes
                .get("base16-ocean.dark")
                .or_else(|| themes.themes.values().next())
                .cloned()
                .expect("syntect ships with default themes");
            (fallback, Some(unknown_theme(name, &themes)))
        }
    }
}

/// Whether a theme by this name ships, and if not, the warning a typo in the
/// config file gets: for a setting typed in tiny to be refused with, rather
/// than taken and then complained about.
pub fn check_theme(name: &str) -> Result<(), String> {
    let themes = ThemeSet::load_defaults();
    if themes.themes.contains_key(name) {
        Ok(())
    } else {
        Err(unknown_theme(name, &themes))
    }
}

/// What is said about a theme name that does not ship.
fn unknown_theme(name: &str, themes: &ThemeSet) -> String {
    let available: Vec<&str> = themes.themes.keys().map(String::as_str).collect();
    format!(
        "unknown syntax_theme `{name}`, try one of: {}",
        available.join(", ")
    )
}

/// Licenses for the bundled grammars, for `tiny --licenses`.
///
/// The grammars are third-party files redistributed inside the binary, and a
/// few of them ask to be acknowledged. Printing the listing is the cheapest
/// honest way to do that; it costs about 11 KiB and only when called.
pub fn acknowledgments() -> String {
    format!(
        "tiny bundles syntax definitions curated by the bat project, by way of \
         the two-face crate. They are third-party files with their own terms; \
         those that ask to be acknowledged are reproduced below. The full \
         list, including the ones that do not, is at\n{}\n\n{}\n",
        // Someone else's API, and spelled their way.
        two_face::acknowledgement::url(),
        two_face::acknowledgement::listing().to_md().trim_end(),
    )
}

/// syntect style to ratatui style.
///
/// Only the foreground crosses over. syntect themes also carry a background
/// per token, but painting it would fight the terminal's own background and
/// leave the editor looking like a patchwork: the pane keeps one ground and
/// varies only the ink.
fn convert_style(s: syntect::highlighting::Style) -> Style {
    let mut out = Style::default().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b));
    if s.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if s.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(pieces: &[Vec<Piece>]) -> Vec<String> {
        pieces
            .iter()
            .map(|line| line.iter().map(|(_, t)| t.as_str()).collect())
            .collect()
    }

    #[test]
    fn a_line_too_long_to_parse_is_drawn_plain_and_the_next_still_colors() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.js"), "");
        let long = format!("var a = \"{}\";", "x".repeat(MAX_HIGHLIGHT_LINE));
        let lines = vec![long.clone(), "var b = 1;".to_string()];
        let out = h.highlight_window(&lines, syntax, 0, 2);
        assert_eq!(out[0], vec![(Style::default(), long)]);
        assert!(out[1].len() > 1, "still colored: {:?}", out[1]);
    }

    #[test]
    fn a_known_theme_loads_without_complaint() {
        let (_, warning) = Highlighter::with_theme("base16-ocean.dark");
        assert!(warning.is_none());
    }

    #[test]
    fn an_unknown_theme_falls_back_and_lists_the_real_ones() {
        let (_, warning) = Highlighter::with_theme("not-a-theme");
        let w = warning.expect("should warn");
        assert!(w.contains("not-a-theme"));
        assert!(
            w.contains("base16-ocean.dark"),
            "names a working option: {w}"
        );
    }

    #[test]
    fn detects_syntax_from_extension() {
        let h = Highlighter::new();
        let s = h.syntax_for_path(Path::new("main.py"), "");
        assert_eq!(s.name, "Python");
        let s = h.syntax_for_path(Path::new("lib.rs"), "");
        assert_eq!(s.name, "Rust");
    }

    #[test]
    fn the_languages_people_actually_write_in_are_all_covered() {
        let h = Highlighter::new();
        // One per language family that syntect's own defaults do not ship.
        // If a swap of syntax set ever loses these, this is what says so.
        let want = [
            ("app.ts", "TypeScript"),
            ("app.tsx", "TypeScriptReact"),
            ("Cargo.toml", "TOML"),
            ("main.kt", "Kotlin"),
            ("View.swift", "Swift"),
            ("build.zig", "Zig"),
            ("main.dart", "Dart"),
            ("mix.exs", "Elixir"),
            ("default.nix", "Nix"),
            ("schema.graphql", "GraphQL"),
            ("api.proto", "Protocol Buffer"),
            ("style.scss", "SCSS"),
            ("plot.jl", "Julia"),
            ("token.sol", "Solidity"),
            ("App.vue", "Vue Component"),
            ("Card.svelte", "Svelte"),
            ("main.tf", "Terraform"),
            ("setup.ini", "INI"),
        ];
        for (name, syntax) in want {
            assert_eq!(
                h.syntax_for_path(Path::new(name), "").name,
                syntax,
                "{name} should highlight as {syntax}"
            );
        }
    }

    #[test]
    fn files_with_no_extension_are_recognized_by_name() {
        let h = Highlighter::new();
        // These are the ones an editor looks silly not knowing, and every one
        // of them has an empty `Path::extension`.
        for (name, syntax) in [
            ("Makefile", "Makefile"),
            ("Dockerfile", "Dockerfile"),
            (".gitignore", "Git Ignore"),
            ("go.mod", "Gomod"),
            ("Cargo.lock", "TOML"),
        ] {
            assert_eq!(
                h.syntax_for_path(Path::new(name), "").name,
                syntax,
                "{name} should highlight as {syntax}"
            );
        }
    }

    #[test]
    fn a_named_override_beats_the_extension() {
        let h = Highlighter::new();
        // `.conf` belongs to nginx, but tiny's own config file is TOML.
        assert_eq!(
            h.syntax_for_path(Path::new("tiny.conf"), "").name,
            "TOML",
            "our own config should not be styled as a web server"
        );
        assert_eq!(
            h.syntax_for_path(Path::new("nginx.conf"), "").name,
            "nginx",
            "someone else's .conf is left alone"
        );
    }

    #[test]
    fn a_named_extension_does_not_shadow_a_real_one() {
        let h = Highlighter::new();
        // The file-name pass runs first, so it must not swallow ordinary
        // files: nothing registers "main.rs" as a name, and `.rs` must win.
        assert_eq!(h.syntax_for_path(Path::new("main.rs"), "").name, "Rust");
        assert_eq!(h.syntax_for_path(Path::new("mod.py"), "").name, "Python");
    }

    #[test]
    fn falls_back_to_shebang_then_plain_text() {
        let h = Highlighter::new();
        let s = h.syntax_for_path(Path::new("script"), "#!/usr/bin/env python");
        assert_eq!(s.name, "Python");
        let s = h.syntax_for_path(Path::new("mystery"), "just words");
        assert_eq!(s.name, "Plain Text");
    }

    #[test]
    fn code_fence_tokens_resolve() {
        let h = Highlighter::new();
        assert_eq!(h.syntax_for_token("python").name, "Python");
        assert_eq!(h.syntax_for_token("rs").name, "Rust");
        assert_eq!(h.syntax_for_token("").name, "Plain Text");
        assert_eq!(h.syntax_for_token("nonsense-lang").name, "Plain Text");
    }

    #[test]
    fn window_returns_only_requested_lines_with_original_text() {
        let h = Highlighter::new();
        let lines: Vec<String> = (0..50).map(|i| format!("x = {i}")).collect();
        let syn = h.syntax_for_token("python").clone();
        let got = h.highlight_window(&lines, &syn, 10, 5);
        assert_eq!(got.len(), 5);
        assert_eq!(
            text(&got),
            ["x = 10", "x = 11", "x = 12", "x = 13", "x = 14"]
        );
    }

    #[test]
    fn window_past_the_end_is_clamped_not_panicking() {
        let h = Highlighter::new();
        let lines: Vec<String> = vec!["a".into(), "b".into()];
        let syn = h.syntax_for_token("python").clone();
        assert_eq!(h.highlight_window(&lines, &syn, 1, 100).len(), 1);
        assert!(h.highlight_window(&lines, &syn, 5, 10).is_empty());
    }

    /// A file whose middle is one long Python docstring: the state that says
    /// "inside a string" has to be carried across several checkpoints, which
    /// is exactly what a resumed parse could get wrong.
    fn straddling_file() -> Vec<String> {
        let mut lines: Vec<String> = (0..30).map(|i| format!("a = {i}")).collect();
        lines.push("\"\"\"".into());
        lines.extend((0..200).map(|i| format!("still inside the docstring {i}")));
        lines.push("\"\"\"".into());
        lines.extend((0..100).map(|i| format!("b = {i}")));
        lines
    }

    #[test]
    fn resuming_gives_the_same_answer_as_parsing_from_the_top() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.py"), "").clone();
        let lines = straddling_file();

        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &syntax.name, None);
        // Forwards, backwards, and a jump: the cache is warm for some of these
        // and cold ahead of others.
        for start in [0, 40, 120, 300, 60, 250, 0] {
            r.sync(Path::new("x.py"), &syntax.name, None);
            assert_eq!(
                h.highlight_cached(&mut r, &lines, &syntax, start, 20),
                h.highlight_window(&lines, &syntax, start, 20),
                "window at {start}"
            );
        }
    }

    #[test]
    fn a_cold_cache_may_start_deep_in_the_file() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.py"), "").clone();
        let lines = straddling_file();
        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &syntax.name, None);
        assert_eq!(
            h.highlight_cached(&mut r, &lines, &syntax, 150, 20),
            h.highlight_window(&lines, &syntax, 150, 20),
            "nothing saved yet, so it parses from the top and saves as it goes"
        );
    }

    #[test]
    fn an_edit_above_the_window_invalidates_what_it_should() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.py"), "").clone();
        let mut lines = straddling_file();
        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &syntax.name, None);
        h.highlight_cached(&mut r, &lines, &syntax, 300, 20);

        // Close the docstring early. Everything from here down changes meaning,
        // and the saved states from here down are now lies.
        lines[100] = "\"\"\"".into();
        r.sync(Path::new("x.py"), &syntax.name, Some(100));
        assert_eq!(
            h.highlight_cached(&mut r, &lines, &syntax, 300, 20),
            h.highlight_window(&lines, &syntax, 300, 20),
            "the window past the edit is re-parsed"
        );
    }

    #[test]
    fn a_different_file_or_grammar_throws_the_cache_away() {
        let h = Highlighter::new();
        let py = h.syntax_for_path(Path::new("x.py"), "").clone();
        let rs = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let lines = straddling_file();

        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &py.name, None);
        h.highlight_cached(&mut r, &lines, &py, 300, 20);
        assert!(!r.states.is_empty(), "warm");

        r.sync(Path::new("y.py"), &py.name, None);
        assert!(r.states.is_empty(), "another file means nothing is known");

        h.highlight_cached(&mut r, &lines, &py, 300, 20);
        r.sync(Path::new("y.py"), &rs.name, None);
        assert!(r.states.is_empty(), "so does another grammar");
    }

    #[test]
    fn an_edit_keeps_every_state_decided_before_it() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.py"), "").clone();
        let lines = straddling_file();
        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &syntax.name, None);
        h.highlight_cached(&mut r, &lines, &syntax, 300, 20);
        let warm = r.states.len();
        assert!(warm > 4, "several checkpoints, got {warm}");

        // `states[i]` enters line `i * STRIDE`, so it was decided by the lines
        // above that one and survives an edit at or below its own line.
        r.sync(Path::new("x.py"), &syntax.name, Some(2 * STRIDE));
        assert_eq!(r.states.len(), 3, "0, STRIDE and 2*STRIDE are still true");

        r.sync(Path::new("x.py"), &syntax.name, Some(2 * STRIDE - 1));
        assert_eq!(r.states.len(), 2, "one line earlier takes 2*STRIDE with it");
    }

    #[test]
    fn multiline_state_resolves_correctly_deep_in_a_file() {
        let h = Highlighter::new();
        let syn = h.syntax_for_token("python").clone();
        // A docstring opened on line 0 and closed on line 40: line 20 is inside
        // it, and can only be known to be a string by parsing from the top.
        let mut lines = vec!["'''".to_string()];
        lines.extend((1..40).map(|i| format!("still in the docstring {i}")));
        lines.push("'''".into());
        lines.push("code = 1".into());

        let inside = &h.highlight_window(&lines, &syn, 20, 1)[0];
        let outside = &h.highlight_window(&lines, &syn, 41, 1)[0];
        assert_ne!(
            inside[0].0.fg, outside[0].0.fg,
            "line inside the docstring is styled as a string, unlike real code"
        );
    }

    #[test]
    fn snippet_highlighting_preserves_content() {
        let h = Highlighter::new();
        let syn = h.syntax_for_token("python").clone();
        let got = h.highlight_snippet("def f():\n    return 1", &syn);
        assert_eq!(text(&got), ["def f():", "    return 1"]);
    }

    /// One function, over and over, a blank line after each.
    const RUST_BLOCK: [&str; 6] = [
        "/// Adds one.",
        "pub fn next(x: u32) -> u32 {",
        "    let s = \"a string\"; // and a comment",
        "    x + 1",
        "}",
        "",
    ];

    /// A Rust file of `n` lines of [`RUST_BLOCK`]. Every window that starts
    /// where a block does is colored exactly as the top of the file is, so
    /// its right colors are known without reading down to it.
    fn long_rust(n: usize) -> Vec<String> {
        RUST_BLOCK
            .iter()
            .cycle()
            .take(n)
            .map(|l| l.to_string())
            .collect()
    }

    /// The start of the block at or above `line`.
    fn block_at(line: usize) -> usize {
        line / RUST_BLOCK.len() * RUST_BLOCK.len()
    }

    #[test]
    fn a_fresh_start_is_made_where_a_definition_begins() {
        let mut lines: Vec<String> = vec!["    body();".to_string(); 3000];
        lines[699] = String::new();
        lines[700] = "fn here() {".into();
        lines[749] = String::new();
        lines[750] = "    indented() {".into();
        assert_eq!(sync_point(&lines, 800), 700, "at the margin after a blank");
        assert_eq!(sync_point(&lines, 700), 700, "the window's own first line");
        // Further down, nothing near is at the margin after a blank: the
        // block it is all in, skipping what only looks like one.
        lines[1500] = "#if FEATURE".into();
        lines[1600] = "out:".into();
        lines[1700] = "}".into();
        assert_eq!(sync_point(&lines, 2000), 700, "the block it is in");
        assert_eq!(sync_point(&lines, 2800), 2800 - SYNC, "none near enough");
    }

    #[test]
    fn what_opens_a_block() {
        for line in [
            "class Cache {",
            "impl Thing {",
            "def f(x):",
            "x = [",
            "f(  ",
        ] {
            assert!(opens_block(line), "{line:?}");
        }
        for line in ["#ifdef X {", "out:", "}", "return x;", ""] {
            assert!(!opens_block(line), "{line:?}");
        }
    }

    #[test]
    fn far_down_a_long_file_colors_come_at_once_from_a_guess() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let lines = long_rust(100_000);
        let far = block_at(99_000);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        assert_eq!(
            h.highlight_cached(&mut r, &lines, &syntax, far, 30),
            h.highlight_window(&lines, &syntax, 0, 30),
            "the same lines as the top, so the same colors"
        );
        assert!(r.behind(), "exact ones are still to come");
        assert!(r.states.is_empty(), "nothing read from the top: {r:?}");
        // A window with nothing in it waits for nothing, or the event loop
        // would go on drawing it.
        h.highlight_cached(&mut r, &lines, &syntax, lines.len(), 30);
        assert!(!r.behind());
    }

    #[test]
    fn exact_colors_are_read_in_behind_a_wrong_guess() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.py"), "").clone();
        // What fools a guess: a blank line, then one at the margin, inside a
        // string, which looks just like code starting.
        let mut lines: Vec<String> = (0..400).map(|i| format!("a = {i}")).collect();
        lines.push("HELP = \"\"\"".into());
        lines.extend((0..20).map(|i| format!("help, line {i}")));
        lines.push(String::new());
        lines.push("Usage: tiny [FILE]".into());
        lines.extend((0..20).map(|i| format!("more help {i}")));
        lines.push("\"\"\"".into());
        lines.extend((0..40).map(|i| format!("b = {i}")));
        let at = 430;
        let exact = h.highlight_window(&lines, &syntax, at, 20);

        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &syntax.name, None);
        let guessed = h.highlight_cached(&mut r, &lines, &syntax, at, 20);
        assert_ne!(guessed, exact, "the guess took the string for code");
        // Each frame reads at least a line, however slow the machine.
        let mut frames = 0;
        while r.behind() {
            h.read_on(&mut r, &lines, &syntax, || false);
            h.highlight_cached(&mut r, &lines, &syntax, at, 20);
            frames += 1;
            assert!(frames <= at, "never caught up: {r:?}");
        }
        assert_eq!(h.highlight_cached(&mut r, &lines, &syntax, at, 20), exact);
    }

    #[test]
    fn the_same_state_saved_twice_is_kept_once() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let lines = long_rust(3_000);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        for start in (0..3_000).step_by(250) {
            h.highlight_cached(&mut r, &lines, &syntax, start, 250);
        }
        assert_eq!(r.states.len(), 60);
        // The file repeats every six lines, so the parser is in one of three
        // states at a checkpoint fifty lines on: as many as are kept.
        let mut kept: Vec<*const State> = r.states.iter().map(Rc::as_ptr).collect();
        kept.sort();
        kept.dedup();
        assert!(kept.len() <= 3, "{} kept", kept.len());
        // And sharing changed no color.
        assert_eq!(
            h.highlight_cached(&mut r, &lines, &syntax, 2_500, 40),
            h.highlight_window(&lines, &syntax, 2_500, 40)
        );
    }

    #[test]
    fn a_frame_with_nothing_new_reads_nothing_again() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.py"), "").clone();
        let lines = straddling_file();
        let mut r = Resume::default();
        r.sync(Path::new("x.py"), &syntax.name, None);
        let first = h.highlight_cached(&mut r, &lines, &syntax, 120, 40);
        assert!(r.parsed > 0);
        let again = h.highlight_cached(&mut r, &lines, &syntax, 120, 40);
        assert_eq!(r.parsed, 0, "every line taken from the frame before");
        assert_eq!(again, first);
        // A line further down comes into view: that line and no other.
        h.highlight_cached(&mut r, &lines, &syntax, 121, 40);
        assert_eq!(r.parsed, 1);
    }

    #[test]
    fn a_key_reads_its_own_line_again_and_no_other() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let mut lines = long_rust(600);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        h.highlight_cached(&mut r, &lines, &syntax, 200, 40);

        assert_eq!(lines[213], "    x + 1");
        lines[213] = "    x + 12".into();
        r.sync(Path::new("x.rs"), &syntax.name, Some(213));
        let got = h.highlight_cached(&mut r, &lines, &syntax, 200, 40);
        assert_eq!(r.parsed, 1, "the line typed into");
        assert_eq!(got, h.highlight_window(&lines, &syntax, 200, 40));

        // A line added in the window: the ones below it are found where
        // they went.
        lines.insert(213, String::new());
        r.sync(Path::new("x.rs"), &syntax.name, Some(213));
        let got = h.highlight_cached(&mut r, &lines, &syntax, 200, 40);
        assert_eq!(r.parsed, 1, "the new line");
        assert_eq!(got, h.highlight_window(&lines, &syntax, 200, 40));
    }

    #[test]
    fn an_edit_that_changes_what_follows_reads_on_as_far_as_it_reaches() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let mut lines = long_rust(600);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        h.highlight_cached(&mut r, &lines, &syntax, 200, 40);

        // A comment opened and left open: every line after it means
        // something else now.
        lines[210] = "/*".into();
        r.sync(Path::new("x.rs"), &syntax.name, Some(210));
        let got = h.highlight_cached(&mut r, &lines, &syntax, 200, 40);
        assert_eq!(r.parsed, 30, "from the edit to the foot of the window");
        assert_eq!(got, h.highlight_window(&lines, &syntax, 200, 40));

        // Closed again a few lines on: back to what was there before.
        lines[215] = "*/".into();
        r.sync(Path::new("x.rs"), &syntax.name, Some(215));
        let got = h.highlight_cached(&mut r, &lines, &syntax, 200, 40);
        assert_eq!(got, h.highlight_window(&lines, &syntax, 200, 40));
    }

    #[test]
    fn whatever_is_edited_the_colors_are_those_read_from_the_top() {
        // Lines that open and close strings and comments, typed, added and
        // taken away at random: after every edit the window must be colored
        // exactly as a fresh read from the first line colors it.
        let pools: [(&str, &[&str]); 2] = [
            (
                "x.py",
                &[
                    "def f(x):",
                    "    return x + 1",
                    "\"\"\"",
                    "inside, maybe",
                    "",
                    "x = 'str'",
                    "# comment",
                    "class A:",
                    "    s = \"\"\"one line\"\"\"",
                ],
            ),
            (
                "x.rs",
                &[
                    "fn f() {",
                    "    let x = 1;",
                    "}",
                    "/*",
                    " * in a comment, maybe",
                    "*/",
                    "",
                    "let s = \"open",
                    "closed\";",
                    "// a line comment",
                    "struct A { x: u32 }",
                ],
            ),
        ];
        let h = Highlighter::new();
        let mut seed: u64 = 7;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % n.max(1)
        };
        for (name, pool) in pools {
            let syntax = h.syntax_for_path(Path::new(name), "").clone();
            let mut lines: Vec<String> =
                (0..80).map(|i| pool[i % pool.len()].to_string()).collect();
            let mut r = Resume::default();
            r.sync(Path::new(name), &syntax.name, None);
            for round in 0..80 {
                let at = next(lines.len());
                let line = pool[next(pool.len())].to_string();
                match next(3) {
                    0 => lines[at] = line,
                    1 => lines.insert(at, line),
                    _ if lines.len() > 20 => drop(lines.remove(at)),
                    _ => {}
                }
                r.sync(Path::new(name), &syntax.name, Some(at));
                let start = next(lines.len() - 20);
                assert_eq!(
                    h.highlight_cached(&mut r, &lines, &syntax, start, 30),
                    h.highlight_window(&lines, &syntax, start, 30),
                    "{name}, round {round}, window at {start}"
                );
            }
        }
    }

    #[test]
    fn reading_on_stops_when_asked_and_carries_on_after() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let lines = long_rust(2_000);
        let far = block_at(1_900);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        h.highlight_cached(&mut r, &lines, &syntax, far, 30);
        h.read_on(&mut r, &lines, &syntax, || true);
        assert!(r.states.len() <= 1, "a key was waiting: {r:?}");
        assert!(r.behind());

        // Asked again and again, it gets there, and stops of itself.
        let mut frames = 0;
        while r.behind() {
            h.read_on(&mut r, &lines, &syntax, || false);
            h.highlight_cached(&mut r, &lines, &syntax, far, 30);
            frames += 1;
            assert!(frames <= far, "never caught up: {r:?}");
        }
        assert!(r.states.len() * STRIDE + REACH >= far, "{r:?}");
    }

    #[test]
    fn an_edit_drops_the_guesses_below_it_too() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let lines = long_rust(6_000);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        h.highlight_cached(&mut r, &lines, &syntax, 5_000, 30);
        let guesses = r.near.len();
        assert!(guesses > 0, "{r:?}");

        r.sync(Path::new("x.rs"), &syntax.name, Some(5_020));
        assert_eq!(r.near.len(), guesses, "one below them leaves them be");
        r.sync(Path::new("x.rs"), &syntax.name, Some(4_900));
        assert!(r.near.is_empty(), "one above takes them with it");

        // And so does where a frame stopped reading in behind them.
        let hi = SynHighlighter::new(&h.theme);
        let state = (
            ParseState::new(&syntax),
            HighlightState::new(&hi, ScopeStack::new()),
        );
        r.reading = Some((120, state));
        r.sync(Path::new("x.rs"), &syntax.name, Some(120));
        assert!(r.reading.is_some(), "decided by the lines above it");
        r.sync(Path::new("x.rs"), &syntax.name, Some(119));
        assert!(r.reading.is_none());
    }

    #[test]
    fn far_enough_down_the_guess_stands() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path(Path::new("x.rs"), "").clone();
        let lines = long_rust(CATCH_UP_LINES + 10_000);
        let far = block_at(CATCH_UP_LINES + 5_000);
        let mut r = Resume::default();
        r.sync(Path::new("x.rs"), &syntax.name, None);
        assert_eq!(
            h.highlight_cached(&mut r, &lines, &syntax, far, 30),
            h.highlight_window(&lines, &syntax, 0, 30),
        );
        assert!(!r.behind(), "nothing more is read");
        assert!(r.states.is_empty(), "{r:?}");
    }
}
