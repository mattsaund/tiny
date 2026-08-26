//! Syntax highlighting, wrapping syntect and converting its output into
//! ratatui spans.
//!
//! Highlighting is stateful — syntect has to walk a file from the top to know
//! whether line 400 sits inside a block comment — so the window API below runs
//! the parser from line 0 but only *collects* the lines actually on screen,
//! then stops. Cost scales with scroll depth rather than file size, and no
//! cache has to be invalidated on every keystroke.
//!
//! # Resuming instead of restarting
//!
//! Parsing from line 0 every frame is correct and, for a note, instant. It is
//! also quadratic in disguise: the cost is proportional to how far down the
//! file you are, and it is paid again on every keystroke. Measured on a
//! 6000-line Python file, one frame took **392ms** — a third of a second
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
//! from it down is dropped, and everything above is still true — because a
//! change on line 900 cannot alter what the parser believed on line 100.
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
//! gaps in them are not exotic — no TypeScript, no TOML, no Kotlin, no Swift,
//! no Dockerfile. `two_face` is the same curated set `bat` uses, and brings it
//! to 213. The cost is about 600 KiB of binary; the alternative was a table
//! mapping `.ts` onto JavaScript and hoping, which is worse than it sounds
//! because a wrong grammar mis-styles a file confidently.
//!
//! Grammars carry their own licences, listed by [`acknowledgements`] and shown
//! by `tiny --licenses`.

use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use syntect::highlighting::{
    FontStyle, HighlightIterator, HighlightState, Highlighter as SynHighlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// A single highlighted run of text within a line.
///
/// A line is a `Vec<Piece>`; the whole window is a `Vec<Vec<Piece>>`. The text
/// is owned rather than borrowed because it outlives the parse — `ui` clips it
/// horizontally before turning it into spans.
pub type Piece = (Style, String);

/// Owns the loaded syntax definitions and the active theme.
///
/// Construction unpacks 213 syntax definitions, which is the single most
/// expensive thing tiny does at startup, so `App` builds exactly one and keeps
/// it for the life of the program. Changing `syntax_theme` goes through
/// [`Highlighter::set_theme`], which swaps only the theme — rebuilding the
/// whole thing to change a colour scheme would cost the same as another
/// startup.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

/// Files past this many lines are shown unhighlighted — the parse cost stops
/// being worth it and plain text still edits fine.
const MAX_HIGHLIGHT_LINES: usize = 20_000;

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
/// and a 20,000-line file — the most [`MAX_HIGHLIGHT_LINES`] allows — at 400
/// saved states, which is nothing.
const STRIDE: usize = 50;

/// Saved parser state, so a window deep in a file is reached by resuming
/// rather than by parsing everything above it again.
///
/// One of these lives on `App` and follows whichever buffer is being edited.
/// Switching files throws it away and rebuilds: you edit one file at a time,
/// and a cache per buffer would have to be evicted, which is a second problem
/// in exchange for a saving nobody would notice.
#[derive(Default)]
pub struct Resume {
    /// What these states describe. Anything else and they are meaningless —
    /// the same lines under a different grammar parse differently.
    key: Option<(PathBuf, String)>,
    /// `states[i]` is the parser entering line `i * STRIDE`. Contiguous from
    /// the start of the file, so index `i` is only present once every line
    /// above it has been parsed at least once.
    states: Vec<(ParseState, HighlightState)>,
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
            self.key = Some(key);
            self.states.clear();
            return;
        }
        if let Some(line) = touched {
            // A state entering line `i * STRIDE` was decided by the lines
            // above it, so it survives an edit at or after its own line. Keep
            // index `i` while `i * STRIDE <= line`.
            self.states.truncate(line / STRIDE + 1);
        }
    }

    /// Forget everything. Used when the theme changes, since the saved
    /// highlight states carry styles taken from it.
    pub fn clear(&mut self) {
        self.key = None;
        self.states.clear();
    }
}

impl Highlighter {
    /// A highlighter with the default theme, discarding the warning that
    /// cannot happen for a name syntect is known to ship.
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
            },
            warning,
        )
    }

    /// Swap the theme in place, keeping the syntax set.
    ///
    /// This is what `:set syntax_theme` calls. Rebuilding the `Highlighter`
    /// would re-unpack every grammar to change a palette, which is the whole
    /// startup cost paid again for nothing.
    pub fn set_theme(&mut self, name: &str) -> Option<String> {
        let (theme, warning) = load_theme(name);
        self.theme = theme;
        warning
    }

    /// The no-op syntax, used for unknown file types and unlabelled fences.
    fn plain(&self) -> &SyntaxReference {
        self.syntaxes.find_syntax_plain_text()
    }

    /// Pick a syntax from a file path.
    ///
    /// Three passes, narrowest first. The whole file name is tried before the
    /// extension because a great many files that matter have no useful
    /// extension at all — `Makefile`, `Dockerfile`, `.gitignore`, `go.mod`,
    /// `Cargo.lock` — and the grammars register those names in the same field
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

    /// Highlight `count` lines starting at `start`, with no saved state — the
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
        self.window(lines, syntax, start, count, &mut Resume::default())
    }

    /// The same, resuming from `resume` and extending it as it goes.
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
        self.window(lines, syntax, start, count, resume)
    }

    /// The shared body.
    ///
    /// Lines below the window are never touched; lines above it are parsed for
    /// state only, so that a line 400 deep inside a docstring is styled as a
    /// string rather than as code — and `resume` is what keeps "above it" from
    /// meaning "all of them".
    ///
    /// Two escape hatches drop to unstyled text rather than failing: files past
    /// [`MAX_HIGHLIGHT_LINES`], and any line syntect refuses to parse. Neither
    /// should stop you editing.
    fn window(
        &self,
        lines: &[String],
        syntax: &SyntaxReference,
        start: usize,
        count: usize,
        resume: &mut Resume,
    ) -> Vec<Vec<Piece>> {
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
        if lines.len() > MAX_HIGHLIGHT_LINES {
            return plain();
        }

        let hi = SynHighlighter::new(&self.theme);
        // The nearest saved state at or above the window. `states` is
        // contiguous from the start of the file, so when it is too short the
        // last one is still the furthest down we can legitimately start.
        let at = (start / STRIDE).min(resume.states.len().saturating_sub(1));
        let (mut parse, mut hl, first) = match resume.states.get(at) {
            Some((p, h)) => (p.clone(), h.clone(), at * STRIDE),
            None => (
                ParseState::new(syntax),
                HighlightState::new(&hi, ScopeStack::new()),
                0,
            ),
        };

        let mut out = Vec::with_capacity(end - start);
        // One buffer, refilled. syntect's syntaxes are the `_newlines`
        // variants, so each line needs its terminator to parse the way it
        // would in a file; allocating a fresh `String` for that meant one heap
        // allocation per line parsed, every frame.
        let mut buf = String::new();
        for (i, line) in lines.iter().enumerate().take(end).skip(first) {
            // Save the state entering this line if it is a checkpoint we do
            // not already hold. Filled in by the parse that had to happen
            // anyway, so building the cache costs nothing extra.
            if i % STRIDE == 0 && resume.states.len() == i / STRIDE {
                resume.states.push((parse.clone(), hl.clone()));
            }
            buf.clear();
            buf.push_str(line);
            buf.push('\n');
            let Ok(ops) = parse.parse_line(&buf, &self.syntaxes) else {
                // Bail out to plain text rather than dropping the rest.
                return plain();
            };
            let ranges = HighlightIterator::new(&mut hl, &ops, &buf, &hi);
            if i < start {
                // Parsed for state only. The iterator still has to be run —
                // it is what advances the highlight state.
                ranges.for_each(drop);
                continue;
            }
            out.push(
                ranges
                    .map(|(sty, text)| {
                        (convert_style(sty), text.trim_end_matches('\n').to_string())
                    })
                    .filter(|(_, t)| !t.is_empty())
                    .collect(),
            );
        }
        out
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

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Look up a named theme, falling back with a warning when it is not one we
/// ship. Shared by [`Highlighter::with_theme`] and [`Highlighter::set_theme`]
/// so a typo reports the same way whether it came from the config file or from
/// `:set`.
fn load_theme(name: &str) -> (Theme, Option<String>) {
    let themes = ThemeSet::load_defaults();
    match themes.themes.get(name) {
        Some(t) => (t.clone(), None),
        None => {
            let available: Vec<&str> = themes.themes.keys().map(String::as_str).collect();
            let fallback = themes
                .themes
                .get("base16-ocean.dark")
                .or_else(|| themes.themes.values().next())
                .cloned()
                .expect("syntect ships with default themes");
            (
                fallback,
                Some(format!(
                    "unknown syntax_theme `{name}` — try one of: {}",
                    available.join(", ")
                )),
            )
        }
    }
}

/// Licences for the bundled grammars, for `tiny --licenses`.
///
/// The grammars are third-party files redistributed inside the binary, and a
/// few of them ask to be acknowledged. Printing the listing is the cheapest
/// honest way to do that; it costs about 11 KiB and only when called.
pub fn acknowledgements() -> String {
    format!(
        "tiny bundles syntax definitions curated by the bat project, by way of \
         the two-face crate. They are third-party files with their own terms; \
         those that ask to be acknowledged are reproduced below. The full \
         list, including the ones that do not, is at\n{}\n\n{}\n",
        two_face::acknowledgement::url(),
        two_face::acknowledgement::listing().to_md().trim_end(),
    )
}

/// syntect style to ratatui style.
///
/// Only the foreground crosses over. syntect themes also carry a background
/// per token, but painting it would fight the terminal's own background and
/// leave the editor looking like a patchwork — the pane keeps one ground and
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
    fn files_with_no_extension_are_recognised_by_name() {
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
}
