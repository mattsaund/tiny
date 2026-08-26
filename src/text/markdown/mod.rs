//! Markdown -> styled terminal lines.
//!
//! pulldown-cmark gives us the event stream; this module turns it into wrapped,
//! styled `Line`s sized to the preview pane. Wrapping happens here rather than
//! via ratatui's `Paragraph` because each line carries per-span styling that a
//! naive re-wrap would smear.
//!
//! `[[wikilinks]]` are not CommonMark, so they are scanned out of text events
//! and styled separately. The same scanner feeds the link graph.
//!
//! # Two entry points
//!
//! - [`render()`] — full markdown. Headings get rules, lists get bullets, tables
//!   get drawn, fenced code is syntax-highlighted via `highlight`.
//! - [`render_plain`] — `.txt` and friends. Wraps long lines and picks out
//!   wikilinks and URLs, and does nothing else. A `#` at the start of a line in
//!   a text file is a hash, not a heading; a plain-text file that silently
//!   reformatted itself would be a bug, not a feature.
//!
//! # Why wrapping is done by hand
//!
//! ratatui's `Paragraph` can wrap, but it wraps a finished `Line` — and by
//! then every span already carries its own style. Re-wrapping smears those
//! styles across the break, so a bolded word split over two lines loses its
//! bold on the second half. [`wrap()`](wrap::wrap) instead breaks the styled run into
//! style-carrying tokens and reassembles them, which is also what makes the
//! per-context indentation (list continuations, blockquote gutters) possible.
//!
//! # The rendering pipeline
//!
//! ```text
//! source ──▶ pulldown-cmark events ──▶ Renderer ──▶ Vec<Line>
//!                                        │
//!                                        ├─ spans:      the inline run being built
//!                                        ├─ style_stack: nested emphasis/strong/link
//!                                        ├─ list_stack:  one entry per nesting level
//!                                        └─ out:        finished lines
//! ```
//!
//! The `Renderer` is a small state machine. Inline text accumulates into
//! `spans` until something block-level ends, at which point `flush_inline` or
//! `flush_item` wraps it into `out` and clears it. Almost every bug in here
//! comes from a missing flush before a block boundary.
//!
//! Everything is measured in *display width* (`unicode-width`), not character
//! count, so CJK text and emoji do not overflow the pane.//!
//! # Where things are
//!
//! - here — the public entry points, and the block splitter that lets a
//!   document be rendered a piece at a time.
//! - [`mod@render`] — the state machine that turns pulldown-cmark's events into
//!   styled rows.
//! - [`mod@wrap`] — fitting those rows into the pane without smearing their
//!   styles.

#[cfg(test)]
mod testing;

mod render;
mod wrap;

use pulldown_cmark::{Options, Parser};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use self::render::Renderer;
use self::wrap::wrap;
use crate::config::Palette;
use crate::text::highlight::Highlighter;

/// Extract every `[[target]]` in document order, with any `|alias` stripped.
/// Duplicates are kept — callers that want unique edges can dedupe.
///
/// This is how `graph` finds the wikilinks in a file, so the renderer and the
/// project map agree on what counts as one.
pub fn wikilinks(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (_, _, target) in scan_wikilinks(source) {
        if let Some(t) = target {
            out.push(t);
        }
    }
    out
}

/// Split `text` into `(start, end, Option<target>)` runs. `None` marks plain
/// text; `Some(target)` marks a wikilink whose displayed range is start..end.
///
/// The offsets are byte offsets into `text`, so callers can slice directly.
/// Returned runs are contiguous and cover the whole string, which is what lets
/// callers rebuild the text by concatenating them without losing anything.
///
/// Scanning is done over raw bytes for the `[[` and `]]` markers, which is
/// safe because both are ASCII and cannot appear inside a multi-byte UTF-8
/// sequence. An unterminated `[[` is left as ordinary text.
pub(super) fn scan_wikilinks(text: &str) -> Vec<(usize, usize, Option<String>)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut plain_start = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'['
            && bytes[i + 1] == b'['
            && let Some(close) = text[i + 2..].find("]]")
        {
            let inner_start = i + 2;
            let inner_end = inner_start + close;
            let inner = &text[inner_start..inner_end];
            // `[[target|alias]]` — the target is what the graph links to.
            let target = inner.split('|').next().unwrap_or(inner).trim();
            if !target.is_empty() {
                if plain_start < i {
                    out.push((plain_start, i, None));
                }
                out.push((i, inner_end + 2, Some(target.to_string())));
                i = inner_end + 2;
                plain_start = i;
                continue;
            }
        }
        i += 1;
    }
    if plain_start < text.len() {
        out.push((plain_start, text.len(), None));
    }
    out
}

/// Render plain text: wrap long lines to the pane, keep the line structure
/// the author chose, and pick out `[[wikilinks]]` and bare URLs.
///
/// Deliberately not markdown — a `.txt` file with a `#` at the start of a line
/// means a hash, not a heading. Each source line wraps on its own, and
/// continuations keep its indentation, so hand-made lists and tables survive.
pub fn render_plain(source: &str, width: usize, pal: &Palette) -> Vec<Line<'static>> {
    let width = width.max(8);
    let mut out = Vec::new();
    for line in source.lines() {
        if line.trim().is_empty() {
            out.push(Line::from(""));
            continue;
        }
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let indent_w = indent.width().min(width.saturating_sub(4));
        let body = &line[indent.len()..];

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (s, e, target) in scan_wikilinks(body) {
            let slice = &body[s..e];
            if target.is_some() {
                spans.push(Span::styled(slice.to_string(), pal.link));
            } else {
                spans.extend(scan_urls(slice, pal));
            }
        }
        let first = vec![Span::styled(indent.clone(), pal.text)];
        let cont = vec![Span::raw(" ".repeat(indent_w))];
        out.extend(wrap(&spans, width, first, cont));
    }
    while out.last().is_some_and(is_blank) {
        out.pop();
    }
    out
}

/// Split a run of plain text so bare `http(s)://` URLs get the link style.
///
/// Only used by [`render_plain`] — real markdown has `Tag::Link` for this.
/// Trailing sentence punctuation is trimmed off the URL, which is what makes
/// `see https://example.com.` underline the address and not the full stop.
pub(super) fn scan_urls(text: &str, pal: &Palette) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        if !rest[at..].starts_with("http://") && !rest[at..].starts_with("https://") {
            // Not a scheme, just the letters. Emit up to here and carry on.
            let (before, after) = rest.split_at(at + 4);
            out.push(Span::styled(before.to_string(), pal.text));
            rest = after;
            continue;
        }
        if at > 0 {
            out.push(Span::styled(rest[..at].to_string(), pal.text));
        }
        let tail = &rest[at..];
        // A URL ends at whitespace; trailing sentence punctuation is not part
        // of it, which is what makes "see https://x.com." behave.
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(['.', ',', ')', ']', '>', ';', ':']);
        out.push(Span::styled(url.to_string(), pal.link));
        rest = &tail[url.len()..];
    }
    if !rest.is_empty() {
        out.push(Span::styled(rest.to_string(), pal.text));
    }
    out
}

/// One markdown block: a run of source lines that renders as a unit.
///
/// The unit the live editor swaps between formatted and raw. `end` is
/// exclusive, and the blocks of a file tile it completely — every source line
/// belongs to exactly one — so a cursor line always names a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

impl Block {
    pub fn contains(&self, line: usize) -> bool {
        line >= self.start && line < self.end
    }
}

/// Split markdown source into the blocks the live editor formats one at a time.
///
/// Four rules, in order of precedence:
///
/// 1. A fenced code block runs from its opening fence to the matching closing
///    one, blank lines and all. Splitting inside a fence would render half a
///    code block, which is not a code block.
/// 2. A blank line is its own block, so the gap between paragraphs survives.
/// 3. An ATX heading is its own block. CommonMark lets a heading interrupt a
///    paragraph, so `text / # Heading / text` is three blocks even with no
///    blank lines around it — otherwise editing the heading would unformat the
///    paragraphs either side of it.
/// 4. Anything else groups with the non-blank lines next to it.
///
/// Deliberately *not* a rule: a `---` under a line of text stays with it.
/// That is a setext heading, and pulling the underline into its own block
/// would turn a title into a paragraph plus a horizontal rule.
pub fn blocks(lines: &[String]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut start: Option<usize> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;

    let close = |out: &mut Vec<Block>, start: &mut Option<usize>, at: usize| {
        if let Some(s) = start.take() {
            out.push(Block { start: s, end: at });
        }
    };

    while i < lines.len() {
        let line = &lines[i];
        if let Some((ch, len)) = fence {
            if closes_fence(line, ch, len) {
                fence = None;
                out.push(Block {
                    start: start.take().expect("a fence opened a block"),
                    end: i + 1,
                });
            }
            i += 1;
            continue;
        }
        if let Some((ch, len)) = opens_fence(line) {
            close(&mut out, &mut start, i);
            fence = Some((ch, len));
            start = Some(i);
            i += 1;
            continue;
        }
        if line.trim().is_empty() {
            close(&mut out, &mut start, i);
            out.push(Block {
                start: i,
                end: i + 1,
            });
            i += 1;
            continue;
        }
        if is_atx_heading(line) {
            close(&mut out, &mut start, i);
            out.push(Block {
                start: i,
                end: i + 1,
            });
            i += 1;
            continue;
        }
        start.get_or_insert(i);
        i += 1;
    }
    // An unclosed fence, or the last paragraph, runs to the end of the file.
    close(&mut out, &mut start, lines.len());
    out
}

/// The fence character and its length, for a line that opens a code fence.
/// Up to three leading spaces, then three or more backticks or tildes.
fn opens_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|c| *c == ch).count();
    (len >= 3).then_some((ch, len))
}

/// Whether a line closes an open fence: the same character, at least as many
/// of them, and nothing else on the line.
fn closes_fence(line: &str, ch: char, len: usize) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= len && trimmed.chars().all(|c| c == ch) && trimmed.starts_with(ch)
}

/// `# Heading` through `###### Heading`, and a bare `#` on its own.
fn is_atx_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes)
        && trimmed
            .chars()
            .nth(hashes)
            .is_none_or(|c| c.is_whitespace())
}

/// Render one block of source, as [`blocks`] carved it out.
///
/// A blank-line block renders to nothing — [`render()`] trims trailing blanks —
/// so it comes back as one empty line instead, because the gap between two
/// paragraphs is a row on the screen and has to stay one.
pub fn render_block(
    lines: &[String],
    width: usize,
    pal: &Palette,
    hl: &Highlighter,
) -> Vec<Line<'static>> {
    let made = render(&lines.join("\n"), width, pal, hl);
    if made.is_empty() {
        return vec![Line::default()];
    }
    made
}

/// Render markdown to styled lines fitted to `width`.
///
/// The enabled extensions are strikethrough, tables, task lists and footnotes
/// — GitHub-flavoured markdown minus the parts with no sensible terminal
/// representation. `hl` is threaded through only for fenced code blocks.
///
/// Trailing blank lines are trimmed so a note does not open with dead space at
/// the bottom of the pane.
pub fn render(source: &str, width: usize, pal: &Palette, hl: &Highlighter) -> Vec<Line<'static>> {
    let width = width.max(8);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);

    let mut r = Renderer::new(width, pal, hl);
    for event in Parser::new_ext(source, opts) {
        r.event(event);
    }
    let mut out = r.finish();
    // Trim trailing blank lines so the pane does not open with dead space.
    while out.last().is_some_and(is_blank) {
        out.pop();
    }
    out
}

pub(super) fn is_blank(l: &Line<'_>) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::markdown::testing::*;

    #[test]
    fn every_source_line_belongs_to_exactly_one_block() {
        let src = "# Title\n\npara one\nstill one\n\n- a\n- b\n\n```rs\n\ncode\n```\n";
        let lines = lines_of(src);
        let bs = blocks(&lines);
        let mut covered = vec![0usize; lines.len()];
        for b in &bs {
            for c in covered.iter_mut().take(b.end).skip(b.start) {
                *c += 1;
            }
        }
        assert!(
            covered.iter().all(|c| *c == 1),
            "blocks must tile the file: {covered:?}"
        );
        for (n, _) in lines.iter().enumerate() {
            assert!(bs.iter().any(|b| b.contains(n)), "line {n} has no block");
        }
    }

    #[test]
    fn a_fence_is_one_block_however_many_blank_lines_are_in_it() {
        let src = "before\n\n```python\nx = 1\n\ny = 2\n```\n\nafter\n";
        let bs = split(src);
        assert!(
            bs.contains(&(2, 7)),
            "the fence and everything in it is one block: {bs:?}"
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end_of_the_file() {
        let src = "para\n\n```\nstill code\nand more\n";
        let bs = split(src);
        assert_eq!(bs.last(), Some(&(2, 5)), "{bs:?}");
    }

    #[test]
    fn a_heading_is_its_own_block_even_with_no_blank_line_round_it() {
        let bs = split("text above\n# Heading\ntext below\n");
        assert_eq!(bs, [(0, 1), (1, 2), (2, 3)]);
    }

    #[test]
    fn a_setext_underline_stays_with_the_line_it_underlines() {
        // `---` after text is an H2, not a horizontal rule. Splitting it off
        // would turn the title into a paragraph and a stray line.
        let bs = split("Title\n---\n\nbody\n");
        assert_eq!(bs[0], (0, 2), "{bs:?}");
    }

    #[test]
    fn a_hash_that_is_not_a_heading_does_not_split() {
        // Seven hashes is not a heading, and neither is one with no space.
        let bs = split("a\n####### seven\n#nospace\nb\n");
        assert_eq!(bs, [(0, 4)], "{bs:?}");
    }

    #[test]
    fn a_blank_line_block_still_renders_as_a_row() {
        let pal = Palette::default();
        let hl = Highlighter::new();
        let out = render_block(&[String::new()], 40, &pal, &hl);
        assert_eq!(out.len(), 1, "the gap between paragraphs is a row");
    }

    #[test]
    fn a_block_renders_the_same_alone_as_it_does_in_the_file() {
        let pal = Palette::default();
        let hl = Highlighter::new();
        let text = |ls: &[Line]| {
            ls.iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let whole = render("# Title\n\n- a\n- b\n", 40, &pal, &hl);
        let heading = render_block(&lines_of("# Title"), 40, &pal, &hl);
        let list = render_block(&lines_of("- a\n- b"), 40, &pal, &hl);
        assert_eq!(
            text(&heading),
            text(&whole)[..2],
            "the heading and its rule"
        );
        assert_eq!(text(&list), text(&whole)[3..], "the list");
    }

    #[test]
    fn plain_text_wraps_but_keeps_the_authors_line_breaks() {
        let src = "alpha bravo charlie delta echo foxtrot\n\nsecond paragraph\n";
        let out = flat(src, 20);
        for l in &out {
            assert!(l.width() <= 20, "{l:?}");
        }
        assert_eq!(out.iter().filter(|l| l.trim().is_empty()).count(), 1);
        assert!(out.last().unwrap().contains("second paragraph"));
    }

    #[test]
    fn plain_text_is_not_treated_as_markdown() {
        // A hash in a .txt file is a hash, not a heading.
        let out = flat("# not a heading\n- not a bullet\n", 40);
        assert_eq!(out[0], "# not a heading");
        assert_eq!(out[1], "- not a bullet");
    }

    #[test]
    fn wrapped_plain_lines_keep_their_indentation() {
        let out = flat(
            "    a deeply indented line that has to wrap somewhere\n",
            24,
        );
        assert!(out.len() > 1);
        assert!(out[0].starts_with("    "));
        assert!(
            out[1].starts_with("    "),
            "the continuation lines up under the first: {:?}",
            out[1]
        );
    }

    #[test]
    fn plain_text_picks_out_wikilinks_and_urls() {
        let pal = Palette::default();
        let lines = render_plain("see [[design]] at https://example.com/spec now", 80, &pal);
        let linked: Vec<&str> = lines[0]
            .spans
            .iter()
            .filter(|s| s.style == pal.link)
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(linked, ["[[design]]", "https://example.com/spec"]);
    }

    #[test]
    fn a_url_does_not_swallow_the_punctuation_after_it() {
        let pal = Palette::default();
        let lines = render_plain("go to https://example.com/x. then stop", 80, &pal);
        let linked: Vec<&str> = lines[0]
            .spans
            .iter()
            .filter(|s| s.style == pal.link)
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(linked, ["https://example.com/x"]);
        let all: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            all, "go to https://example.com/x. then stop",
            "nothing lost"
        );
    }

    #[test]
    fn the_word_http_alone_is_not_a_link() {
        let pal = Palette::default();
        let lines = render_plain("we discussed http and https at length", 80, &pal);
        assert!(lines[0].spans.iter().all(|s| s.style != pal.link));
    }

    #[test]
    fn empty_plain_input_renders_nothing() {
        assert!(flat("", 40).is_empty());
        assert!(flat("\n\n\n", 40).is_empty());
    }

    #[test]
    fn extracts_wikilinks_including_aliases() {
        let src = "See [[architecture]] and [[notes/daily|today]].";
        assert_eq!(wikilinks(src), ["architecture", "notes/daily"]);
    }

    #[test]
    fn ignores_malformed_or_empty_wikilinks() {
        assert!(wikilinks("an unclosed [[link").is_empty());
        assert!(wikilinks("empty [[]] one").is_empty());
        assert!(wikilinks("a single [bracket] link").is_empty());
    }

    #[test]
    fn wikilink_text_survives_rendering() {
        let out = plain("Go to [[design]] now.", 40);
        assert!(out.iter().any(|l| l.contains("[[design]]")));
    }
}
