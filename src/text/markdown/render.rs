//! The event stream, turned into lines.
//!
//! pulldown-cmark hands over a flat sequence of events (start-of-heading,
//! text, end-of-heading) and [`Renderer`] is the state machine that turns it
//! into styled rows. Almost everything it holds is there because markdown is
//! nested and the event stream is not: a stack of styles, a stack of list
//! numbers, the table being accumulated, the quote depth.
//!
//! Inline content is buffered rather than emitted as it arrives, because a
//! line cannot be wrapped until it is finished. [`Renderer::flush_inline`] is
//! where a paragraph becomes rows.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::wrap::{clip, spans_width, wrap};
use super::{is_blank, scan_wikilinks};
use crate::config::Palette;
use crate::text::highlight::Highlighter;
use crate::text::tabs;

/// A table being accumulated. Cells cannot be drawn as they arrive, because
/// column widths depend on the widest cell in the whole table, so everything
/// is buffered until `TagEnd::Table` and laid out at once by
/// [`Renderer::emit_table`].
struct TableState {
    /// `rows[row][col]` is one cell's styled content.
    rows: Vec<Vec<Vec<Span<'static>>>>,
    in_head: bool,
}

/// The event-stream state machine. Lives only for the duration of one
/// `markdown::render` call.
pub(super) struct Renderer<'a> {
    pal: &'a Palette,
    hl: &'a Highlighter,
    /// Target width in cells. Everything wraps or clips to this.
    width: usize,
    /// Finished lines, in order.
    pub(super) out: Vec<Line<'static>>,
    /// The inline run being built up, not yet wrapped into `out`. Flushed at
    /// every block boundary.
    spans: Vec<Span<'static>>,
    /// Nested inline styles. The base style sits at the bottom and is never
    /// popped, so [`Renderer::style`] always has something to return.
    style_stack: Vec<Style>,
    /// One entry per nesting level; `Some(n)` is the next number in an
    /// ordered list, `None` is a bullet list.
    list_stack: Vec<Option<u64>>,
    /// Blockquote nesting, drawn as one `│ ` gutter per level.
    quote_depth: usize,
    /// `Some(info)` while inside a fenced block; the info string names the
    /// language. Its presence is also how `Event::Text` knows to buffer rather
    /// than emit.
    code_fence: Option<String>,
    /// Raw code accumulated inside the current fence, highlighted on close.
    code_buf: String,
    table: Option<TableState>,
    /// Bullet/number to place on the first line of the current list item.
    pending_item_prefix: Option<String>,
}

/// How a heading of each level is drawn.
///
/// Each level is one step quieter than the one above it, and each step
/// changes one thing. H1 and H2 are the heading style, told apart by the rules
/// under them. Below those: fainter (H3), the dim color (H4), the weight gone
/// (H5), fainter again (H6). Measured in kitty, the brightest pixel of each
/// goes 220, 220, 146, 96, 96, 63, where it used to drop from 220 straight to
/// 96 at H4 and stay there.
///
/// Faint is the one attribute here a terminal may not draw, and one that
/// does not just shows that level a step brighter, as tiny used to.
///
/// Headings used to keep their `###` prefix instead, which left the markup
/// showing through a rendered view, and in the editor's live preview a line
/// that never looked finished. Weight and brightness say the same thing and
/// are drawn by every terminal, which an escape for real text size is not.
fn heading_style(pal: &Palette, level: HeadingLevel) -> Style {
    let faint = |style: Style| style.add_modifier(Modifier::DIM);
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => pal.heading,
        HeadingLevel::H3 => faint(pal.heading),
        HeadingLevel::H4 => pal.heading.patch(pal.dim),
        HeadingLevel::H5 => pal.dim,
        HeadingLevel::H6 => faint(pal.dim),
    }
}

impl<'a> Renderer<'a> {
    /// A renderer with nothing rendered yet, sized to the pane.
    pub(super) fn new(width: usize, pal: &'a Palette, hl: &'a Highlighter) -> Self {
        Self {
            pal,
            hl,
            width,
            out: Vec::new(),
            spans: Vec::new(),
            style_stack: vec![pal.text],
            list_stack: Vec::new(),
            quote_depth: 0,
            code_fence: None,
            code_buf: String::new(),
            table: None,
            pending_item_prefix: None,
        }
    }

    /// Everything rendered so far, with the last inline run flushed out.
    ///
    /// Takes `self` because there is nothing sensible to do with a renderer
    /// afterwards: the buffers it was accumulating into are now the answer.
    pub(super) fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline(Vec::new(), Vec::new());
        self.out
    }
}

impl Renderer<'_> {
    /// The style currently in effect: the top of the stack.
    fn style(&self) -> Style {
        // The stack always holds the base style, but there is no reason for a
        // rendering bug to become a crash.
        self.style_stack.last().copied().unwrap_or_default()
    }

    /// Push a style derived from the current one, so emphasis nests: bold
    /// inside a link keeps the link's underline.
    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.style_stack.push(s);
    }

    /// Pop back to the enclosing style, never past the base entry.
    fn pop_style(&mut self) {
        if self.style_stack.len() > 1 {
            self.style_stack.pop();
        }
    }

    /// Emit a separating blank line, but never two in a row and never one at
    /// the very top. Called at block boundaries, so this collapsing is what
    /// keeps the spacing even without every caller having to check.
    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.last().is_some_and(is_blank) {
            self.out.push(Line::from(""));
        }
    }

    /// The gutter that marks blockquote depth, one bar per level.
    fn quote_prefix(&self) -> Vec<Span<'static>> {
        (0..self.quote_depth)
            .map(|_| Span::styled(self.pal.glyphs.gutter, self.pal.dim))
            .collect()
    }

    /// Emit the accumulated inline run as wrapped lines and clear it.
    pub(super) fn flush_inline(&mut self, first: Vec<Span<'static>>, cont: Vec<Span<'static>>) {
        if self.spans.is_empty() && first.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let q = self.quote_prefix();
        let mut first_prefix = q.clone();
        first_prefix.extend(first);
        let mut cont_prefix = q;
        cont_prefix.extend(cont);
        let lines = wrap(&spans, self.width, first_prefix, cont_prefix);
        self.out.extend(lines);
    }

    /// Flush the current inline run as a list-item line, applying the pending
    /// bullet and the continuation indent that aligns wrapped text under the
    /// item text rather than under its bullet. Outside a list this is just a
    /// paragraph flush.
    fn flush_item(&mut self) {
        if self.spans.is_empty() && self.pending_item_prefix.is_none() {
            return;
        }
        let in_list = !self.list_stack.is_empty();
        let indent = self.list_indent();
        let first = match self.pending_item_prefix.take() {
            Some(p) => vec![Span::styled(p, self.pal.dim)],
            None => vec![Span::raw(" ".repeat(indent))],
        };
        let cont_w = if in_list { indent + 2 } else { 0 };
        let cont = vec![Span::raw(" ".repeat(cont_w))];
        self.flush_inline(first, cont);
    }

    /// Indent for the current list depth, applied to both wrapped lines and
    /// nested blocks inside an item.
    fn list_indent(&self) -> usize {
        self.list_stack.len().saturating_sub(1) * 2
    }

    /// Handle one parser event. The dispatch point for the whole renderer:
    /// block starts and ends go to [`Renderer::start`] and [`Renderer::end`],
    /// and everything inline is handled here.
    ///
    /// Text is routed to `code_buf` while a fence is open and to
    /// [`Renderer::push_text`] otherwise; that branch is the only thing
    /// stopping code blocks from being scanned for wikilinks.
    pub(super) fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                // Tabs are taken to their stops here, at the one gate every
                // piece of document text comes through. Inside a fence that
                // keeps code lined up; in prose it is the difference between
                // `a\tb` reading as `a b` and reading as `ab`, since a tab
                // reaches the terminal as a cell holding nothing.
                let t = tabs::expand(&t, tabs::MARKDOWN, 0);
                if self.code_fence.is_some() {
                    self.code_buf.push_str(&t);
                } else {
                    self.push_text(&t);
                }
            }
            Event::Code(t) => {
                let style = self.style().patch(self.pal.code);
                self.spans.push(Span::styled(format!("`{t}`"), style));
            }
            Event::SoftBreak => self.spans.push(Span::styled(" ".to_string(), self.style())),
            Event::HardBreak => {
                let indent = self.list_indent();
                let pad = vec![Span::raw(" ".repeat(indent))];
                self.flush_inline(pad.clone(), pad);
            }
            Event::Rule => {
                self.flush_inline(Vec::new(), Vec::new());
                self.blank();
                self.out.push(Line::from(Span::styled(
                    self.pal.glyphs.rule.repeat(self.width),
                    self.pal.dim,
                )));
                self.blank();
            }
            Event::TaskListMarker(done) => {
                let glyph = if done { "[x] " } else { "[ ] " };
                let style = if done { self.pal.dim } else { self.pal.text };
                self.spans.push(Span::styled(glyph.to_string(), style));
            }
            Event::FootnoteReference(name) => {
                let style = self.style().patch(self.pal.link);
                self.spans.push(Span::styled(format!("[^{name}]"), style));
            }
            // Raw HTML in a note is shown as-is rather than silently dropped.
            Event::Html(t) | Event::InlineHtml(t) => {
                let style = self.pal.dim;
                self.spans
                    .push(Span::styled(t.trim_end().to_string(), style));
            }
            _ => {}
        }
    }

    /// Text events are where `[[wikilinks]]` hide.
    fn push_text(&mut self, text: &str) {
        let base = self.style();
        for (s, e, target) in scan_wikilinks(text) {
            let slice = &text[s..e];
            match target {
                Some(_) => {
                    let style = base.patch(self.pal.link);
                    self.spans.push(Span::styled(slice.to_string(), style));
                }
                None => self.spans.push(Span::styled(slice.to_string(), base)),
            }
        }
    }

    /// Open a block. Most cases flush the pending inline run first, since
    /// anything accumulated belongs to the block that is ending, not the one
    /// starting.
    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.blank();
                self.style_stack.push(heading_style(self.pal, level));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline(Vec::new(), Vec::new());
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline(Vec::new(), Vec::new());
                self.blank();
                self.code_fence = Some(match kind {
                    CodeBlockKind::Fenced(info) => info.to_string(),
                    CodeBlockKind::Indented => String::new(),
                });
                self.code_buf.clear();
            }
            Tag::List(start) => {
                // A nested list interrupts its parent item's text; flush that
                // text with its bullet before descending a level.
                self.flush_item();
                if self.list_stack.is_empty() {
                    self.blank();
                }
                self.list_stack.push(start);
            }
            Tag::Item => {
                let depth = self.list_stack.len().saturating_sub(1);
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        *n += 1;
                        s
                    }
                    // The bullet changes with depth, so nesting reads at a
                    // glance.
                    _ => self.pal.glyphs.bullets[depth % 3].to_string(),
                };
                self.pending_item_prefix = Some(format!("{}{marker}", " ".repeat(depth * 2)));
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => {
                let link = self.pal.link;
                self.push_style(move |s| s.patch(link));
            }
            Tag::Image { dest_url, .. } => {
                // Terminals cannot show it inline; name it so the note still reads.
                let style = self.pal.dim;
                self.spans
                    .push(Span::styled(format!("[image: {dest_url}] "), style));
            }
            Tag::Table(_) => {
                self.flush_inline(Vec::new(), Vec::new());
                self.blank();
                self.table = Some(TableState {
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = true;
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {}
            _ => {}
        }
    }

    /// Close a block: flush its text, undo whatever `start` pushed, and add
    /// the trailing rule or blank line the block calls for.
    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_item();
                if self.list_stack.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(level) => {
                self.flush_inline(Vec::new(), Vec::new());
                self.pop_style();
                // H1 and H2 get a rule under them; deeper levels fade
                // instead, so the page does not turn into lines.
                let rule = match level {
                    HeadingLevel::H1 => Some(self.pal.glyphs.rule_heavy),
                    HeadingLevel::H2 => Some(self.pal.glyphs.rule),
                    _ => None,
                };
                if let Some(ch) = rule {
                    // H1 spans the pane. H2 underlines only its own text, so a
                    // note with several sections does not turn into stripes.
                    let w = match level {
                        HeadingLevel::H1 => self.width,
                        _ => self
                            .out
                            .last()
                            .map(|l| spans_width(&l.spans))
                            .unwrap_or(self.width)
                            .clamp(4, self.width),
                    };
                    self.out
                        .push(Line::from(Span::styled(ch.repeat(w), self.pal.dim)));
                }
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline(Vec::new(), Vec::new());
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.blank();
            }
            TagEnd::CodeBlock => {
                let info = self.code_fence.take().unwrap_or_default();
                let body = std::mem::take(&mut self.code_buf);
                self.emit_code_block(&info, &body);
                self.blank();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.pending_item_prefix = None;
                if self.list_stack.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                // Tight list items carry their text straight here, with no
                // paragraph wrapper to flush it first.
                self.flush_item();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.pop_style()
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.spans);
                if let Some(t) = &mut self.table
                    && let Some(row) = t.rows.last_mut()
                {
                    row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = false;
                }
            }
            TagEnd::Table => self.emit_table(),
            _ => {}
        }
    }

    /// Draw a fenced block: a `│ ` gutter, then syntax-highlighted source.
    ///
    /// Code is never re-wrapped. Indentation is meaning in most languages, and
    /// a wrapped line of Python reads as broken. Overlong lines are clipped
    /// instead, so the pane never scrolls sideways on its own.
    fn emit_code_block(&mut self, info: &str, body: &str) {
        // Borrowed, never cloned: a cloned grammar has to deserialize itself
        // again before it can parse, which cost more than the highlighting;
        // a note with a few hundred code blocks took 28ms a frame.
        let hl = self.hl;
        let syntax = hl.syntax_for_token(info);
        let gutter = self.pal.dim;
        let avail = self.width.saturating_sub(2).max(1);
        for pieces in hl.highlight_snippet(body, syntax) {
            let mut spans = vec![Span::styled(self.pal.glyphs.gutter, gutter)];
            let mut w = 0usize;
            for (style, text) in pieces {
                // Code is not re-wrapped: indentation is meaning. Long lines
                // are clipped so the pane never scrolls sideways on its own.
                let tw = text.width();
                if w + tw <= avail {
                    w += tw;
                    spans.push(Span::styled(text, style));
                } else {
                    let room = avail.saturating_sub(w);
                    if room > 0 {
                        spans.push(Span::styled(clip(&text, room), style));
                    }
                    break;
                }
            }
            self.out.push(Line::from(spans));
        }
    }

    /// Lay out and draw the buffered table.
    ///
    /// Columns start at their natural width (the widest cell in each), then
    /// the widest column is shaved one cell at a time until the whole table
    /// fits the pane. Shrinking stops at three cells per column so a table
    /// squeezed into a narrow pane degrades into something still readable
    /// rather than a row of vertical bars.
    ///
    /// The first row is drawn bold and followed by a `─┼─` rule, whether or not
    /// the source actually marked it as a header.
    fn emit_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let rows: Vec<Vec<Vec<Span<'static>>>> =
            t.rows.into_iter().filter(|r| !r.is_empty()).collect();
        if rows.is_empty() {
            return;
        }
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut widths = vec![0usize; cols];
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(spans_width(cell));
            }
        }
        // Shrink the widest column until the table fits the pane.
        let sep_w = 3 * cols.saturating_sub(1);
        while widths.iter().sum::<usize>() + sep_w > self.width {
            let Some(widest) = (0..cols).max_by_key(|&i| widths[i]) else {
                break;
            };
            if widths[widest] <= 3 {
                break;
            }
            widths[widest] -= 1;
        }

        let dim = self.pal.dim;
        for (ri, row) in rows.iter().enumerate() {
            let mut spans: Vec<Span<'static>> = self.quote_prefix();
            // Indexing by column rather than iterating `widths`: the body
            // needs the matching cell from `row` at the same index.
            #[allow(clippy::needless_range_loop)]
            for ci in 0..cols {
                if ci > 0 {
                    spans.push(Span::styled(self.pal.glyphs.column, dim));
                }
                let empty = Vec::new();
                let cell = row.get(ci).unwrap_or(&empty);
                let bold = ri == 0;
                let mut w = 0usize;
                for s in cell {
                    let room = widths[ci].saturating_sub(w);
                    if room == 0 {
                        break;
                    }
                    let text = clip(&s.content, room);
                    w += text.width();
                    let style = if bold {
                        s.style.add_modifier(Modifier::BOLD)
                    } else {
                        s.style
                    };
                    spans.push(Span::styled(text, style));
                }
                if w < widths[ci] {
                    spans.push(Span::raw(" ".repeat(widths[ci] - w)));
                }
            }
            self.out.push(Line::from(spans));
            if ri == 0 {
                let mut rule: Vec<Span<'static>> = self.quote_prefix();
                let g = self.pal.glyphs;
                let bar: String = widths
                    .iter()
                    .map(|w| g.rule.repeat(*w))
                    .collect::<Vec<_>>()
                    .join(g.cross);
                rule.push(Span::styled(bar, dim));
                self.out.push(Line::from(rule));
            }
        }
        self.blank();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::markdown::testing::*;

    #[test]
    fn headings_get_a_rule_and_deeper_levels_fade_instead() {
        let out = plain("# Title\n\nbody\n", 20);
        assert_eq!(out[0], "Title");
        assert_eq!(out[1], "━".repeat(20), "H1 is underlined across the pane");

        let out = plain("### Deep\n", 20);
        assert_eq!(out[0], "Deep", "no rule, and no hashes left showing");
        assert_eq!(out.len(), 1, "and no rule under it either");
    }

    #[test]
    fn each_heading_below_a_rule_is_a_step_fainter() {
        // The one test here that looks at style rather than text. With no rule
        // under them and no hashes in front, style is the only thing left that
        // tells these levels apart, so it is the thing worth asserting.
        let pal = crate::config::Palette::default();
        let hl = crate::text::highlight::Highlighter::new();
        let style_of = |src: &str| {
            crate::text::markdown::render(src, 40, &pal, &hl)[0]
                .spans
                .iter()
                .find(|s| !s.content.trim().is_empty())
                .expect("a heading span")
                .style
        };

        let levels: Vec<Style> = (1..=6)
            .map(|n| style_of(&format!("{} level {n}\n", "#".repeat(n))))
            .collect();
        assert_eq!(levels[0], pal.heading, "H1 is the heading style");
        assert_eq!(levels[1], pal.heading, "and so is H2: their rules differ");
        // Below the rules, every level differs from the one above it, by
        // one thing at a time.
        let bold = |s: &Style| s.add_modifier.contains(Modifier::BOLD);
        let faint = |s: &Style| s.add_modifier.contains(Modifier::DIM);
        assert!(bold(&levels[2]) && faint(&levels[2]), "H3 is fainter");
        assert!(
            bold(&levels[3]) && !faint(&levels[3]) && levels[3].fg == pal.dim.fg,
            "H4 is in the dim color, still bold"
        );
        assert_eq!(levels[4], pal.dim, "H5 has lost the weight");
        assert!(
            !bold(&levels[5]) && faint(&levels[5]) && levels[5].fg == pal.dim.fg,
            "H6 is fainter again"
        );
        for pair in levels[1..].windows(2) {
            assert_ne!(pair[0], pair[1], "no two levels below H1 alike");
        }
    }

    #[test]
    fn an_h2_rule_is_only_as_wide_as_its_heading() {
        let out = plain("## Short\n\nbody\n", 40);
        assert_eq!(out[0], "Short");
        assert_eq!(out[1], "─".repeat(5), "not the full 40-column pane");
    }

    #[test]
    fn paragraphs_wrap_to_the_pane_width() {
        let src = "alpha bravo charlie delta echo foxtrot golf hotel india";
        let out = plain(src, 20);
        assert!(out.len() > 1, "long text wraps");
        for line in &out {
            assert!(line.width() <= 20, "no line exceeds the width: {line:?}");
        }
        // Wrapping must not lose or duplicate words.
        let joined = out.join(" ");
        let words: Vec<&str> = joined.split_whitespace().collect();
        assert_eq!(words, src.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn an_overlong_word_is_hard_broken_rather_than_overflowing() {
        let out = plain("https://example.com/a/very/long/path/that/never/ends", 15);
        assert!(out.len() > 1);
        for line in &out {
            assert!(line.width() <= 15, "{line:?}");
        }
    }

    #[test]
    fn bullet_lists_nest_with_distinct_glyphs() {
        let out = plain("- one\n- two\n  - nested\n", 40);
        let body: Vec<&String> = out.iter().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(body[0], "• one");
        assert_eq!(body[1], "• two");
        assert_eq!(
            body[2], "  ◦ nested",
            "depth shows in both indent and glyph"
        );
    }

    #[test]
    fn ordered_lists_number_sequentially() {
        let out = plain("1. first\n1. second\n1. third\n", 40);
        let body: Vec<&String> = out.iter().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(body[0], "1. first");
        assert_eq!(body[1], "2. second", "numbering is generated, not copied");
        assert_eq!(body[2], "3. third");
    }

    #[test]
    fn wrapped_list_items_align_under_their_text() {
        let out = plain("- alpha bravo charlie delta echo\n", 18);
        let body: Vec<&String> = out.iter().filter(|l| !l.trim().is_empty()).collect();
        assert!(body[0].starts_with("• "));
        assert!(
            body[1].starts_with("  "),
            "continuation indents past the bullet: {:?}",
            body[1]
        );
    }

    #[test]
    fn task_list_markers_render_as_checkboxes() {
        let out = plain("- [ ] todo\n- [x] done\n", 40);
        let body: Vec<&String> = out.iter().filter(|l| !l.trim().is_empty()).collect();
        assert!(body[0].contains("[ ] todo"), "{:?}", body[0]);
        assert!(body[1].contains("[x] done"), "{:?}", body[1]);
    }

    #[test]
    fn code_blocks_keep_their_text_and_indentation() {
        let out = plain("```python\ndef f():\n    return 1\n```\n", 40);
        let body: Vec<&String> = out.iter().filter(|l| l.contains('│')).collect();
        assert_eq!(body[0], "│ def f():");
        assert_eq!(body[1], "│     return 1", "leading whitespace is preserved");
    }

    #[test]
    fn long_code_lines_are_clipped_not_wrapped() {
        let long = "x".repeat(100);
        let out = plain(&format!("```\n{long}\n```\n"), 20);
        let code: Vec<&String> = out.iter().filter(|l| l.contains('│')).collect();
        assert_eq!(code.len(), 1, "one source line stays one rendered line");
        assert!(code[0].width() <= 20);
    }

    #[test]
    fn blockquotes_get_a_gutter() {
        let out = plain("> quoted text\n", 40);
        let body: Vec<&String> = out.iter().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(body[0], "│ quoted text");
    }

    #[test]
    fn tables_align_into_columns_with_a_header_rule() {
        let src = "| lang | use |\n|---|---|\n| rust | tui |\n";
        let out = plain(src, 40);
        let body: Vec<&String> = out.iter().filter(|l| !l.trim().is_empty()).collect();
        assert!(body[0].contains("lang") && body[0].contains("use"));
        assert!(body[1].contains('┼'), "header rule: {:?}", body[1]);
        assert!(body[2].contains("rust") && body[2].contains("tui"));
        // Columns line up: the separator sits at the same offset in both rows.
        assert_eq!(
            body[0].find('│'),
            body[2].find('│'),
            "column separators align"
        );
    }

    #[test]
    fn a_wide_table_is_squeezed_to_fit_the_pane() {
        let src = "| a very wide column indeed | another wide one |\n|---|---|\n| x | y |\n";
        let out = plain(src, 24);
        for line in &out {
            assert!(line.width() <= 24, "{line:?}");
        }
    }

    #[test]
    fn horizontal_rules_span_the_pane() {
        let out = plain("a\n\n---\n\nb\n", 12);
        assert!(out.iter().any(|l| *l == "─".repeat(12)));
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(plain("", 40).is_empty());
        assert!(plain("\n\n\n", 40).is_empty());
    }

    #[test]
    fn output_never_ends_in_blank_padding() {
        let out = plain("# Title\n\nsome body text\n\n\n", 40);
        assert!(!out.last().unwrap().trim().is_empty());
    }

    #[test]
    fn a_tiny_pane_still_renders_without_panicking() {
        for w in 0..6 {
            let _ = plain("# Heading\n\n- item with words\n\n> quote\n", w);
        }
    }
}
