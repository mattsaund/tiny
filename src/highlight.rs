//! Syntax highlighting, wrapping syntect and converting its output into
//! ratatui spans.
//!
//! Highlighting is stateful — syntect has to walk a file from the top to know
//! whether line 400 sits inside a block comment — so the window API below runs
//! the parser from line 0 but only *collects* the lines actually on screen,
//! then stops. Cost scales with scroll depth rather than file size, and no
//! cache has to be invalidated on every keystroke.
//!
//! # Why re-parse from the top every frame
//!
//! It looks wasteful, and the alternative looks obvious: cache the parser
//! state at each line and resume from the nearest one. That cache would then
//! have to be invalidated on every edit, keyed per buffer, and kept correct
//! when a line is inserted or removed — a whole subsystem, with its own bugs,
//! to speed up something that is already fast enough. Parsing a few hundred
//! lines of text takes well under a frame, and the parse only ever runs to the
//! *bottom of the visible window*, not to the end of the file. A file long
//! enough for this to hurt is past [`MAX_HIGHLIGHT_LINES`] anyway.
//!
//! # Two different highlighters
//!
//! This module and `markdown` both style text, and they do not overlap:
//! `highlight` colours *code* by grammar and is used for the editor pane and
//! for fenced blocks inside notes; `markdown` styles *prose* structure and
//! calls into here for those fenced blocks. Editor styling comes from the
//! syntect theme; everything else comes from the palette.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// A single highlighted run of text within a line.
///
/// A line is a `Vec<Piece>`; the whole window is a `Vec<Vec<Piece>>`. The text
/// is owned rather than borrowed because it outlives the parse — `ui` clips it
/// horizontally before turning it into spans.
pub type Piece = (Style, String);

/// Owns the loaded syntax definitions and the active theme.
///
/// Construction loads syntect's full default syntax set, which is not cheap,
/// so `App` builds exactly one and keeps it. It is rebuilt only when
/// `syntax_theme` changes.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

/// Files past this many lines are shown unhighlighted — the parse cost stops
/// being worth it and plain text still edits fine.
const MAX_HIGHLIGHT_LINES: usize = 20_000;

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
        let themes = ThemeSet::load_defaults();
        let (theme, warning) = match themes.themes.get(name) {
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
        };
        (
            Self {
                syntaxes: SyntaxSet::load_defaults_newlines(),
                theme,
            },
            warning,
        )
    }

    /// The no-op syntax, used for unknown file types and unlabelled fences.
    fn plain(&self) -> &SyntaxReference {
        self.syntaxes.find_syntax_plain_text()
    }

    /// Pick a syntax from a file path, falling back to sniffing the first line
    /// (which catches `#!/usr/bin/env python` on extensionless scripts).
    pub fn syntax_for_path(&self, path: &Path, first_line: &str) -> &SyntaxReference {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(|e| self.syntaxes.find_syntax_by_extension(e))
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

    /// Highlight `count` lines starting at `start`, parsing from the top of the
    /// file so multi-line constructs resolve correctly.
    ///
    /// The loop runs from line 0 but skips pushing anything until it reaches
    /// `start`: those early lines are parsed purely to build up state, so a
    /// line 400 deep inside a docstring is styled as a string rather than as
    /// code. Cost therefore scales with how far you have scrolled, not with
    /// how long the file is.
    ///
    /// Two escape hatches drop to unstyled text rather than failing: files
    /// past [`MAX_HIGHLIGHT_LINES`], and any line syntect refuses to parse.
    /// Neither should stop you editing.
    pub fn highlight_window(
        &self,
        lines: &[String],
        syntax: &SyntaxReference,
        start: usize,
        count: usize,
    ) -> Vec<Vec<Piece>> {
        let end = (start + count).min(lines.len());
        if start >= end {
            return Vec::new();
        }
        if lines.len() > MAX_HIGHLIGHT_LINES {
            return lines[start..end]
                .iter()
                .map(|l| vec![(Style::default(), l.clone())])
                .collect();
        }

        let mut h = HighlightLines::new(syntax, &self.theme);
        let mut out = Vec::with_capacity(end - start);
        for (i, line) in lines.iter().enumerate().take(end) {
            // syntect's syntaxes are the `_newlines` variants, so each line
            // needs its terminator to parse the same way it would in a file.
            let with_nl = format!("{line}\n");
            let Ok(ranges) = h.highlight_line(&with_nl, &self.syntaxes) else {
                // Bail out to plain text rather than dropping the rest.
                return lines[start..end]
                    .iter()
                    .map(|l| vec![(Style::default(), l.clone())])
                    .collect();
            };
            if i < start {
                continue; // parsed for state only
            }
            out.push(
                ranges
                    .into_iter()
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
