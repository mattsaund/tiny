//! Helpers shared by the markdown tests.
//!
//! Every test here works on the rendered *text*, with the styling flattened
//! away, because what markdown rendering promises is a shape on the screen:
//! this line indented under that one, this table in columns. Asserting on
//! spans instead would tie the tests to how the rows happen to be cut up.

use crate::config::Palette;
use crate::text::highlight::Highlighter;
use crate::text::markdown::{blocks, render, render_plain};

pub(crate) fn plain(source: &str, width: usize) -> Vec<String> {
    let pal = Palette::default();
    let hl = Highlighter::new();
    render(source, width, &pal, &hl)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

/// Render as plain text and flatten to strings.
pub(crate) fn flat(source: &str, width: usize) -> Vec<String> {
    let pal = Palette::default();
    render_plain(source, width, &pal)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

pub(crate) fn lines_of(src: &str) -> Vec<String> {
    src.lines().map(str::to_string).collect()
}

/// Blocks as `(start, end)` pairs, for readable assertions.
pub(crate) fn split(src: &str) -> Vec<(usize, usize)> {
    blocks(&lines_of(src))
        .into_iter()
        .map(|b| (b.start, b.end))
        .collect()
}
