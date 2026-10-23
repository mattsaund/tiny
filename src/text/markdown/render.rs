//! The event stream, turned into lines.
//!
//! pulldown-cmark hands over a flat sequence of events (start-of-heading,
//! text, end-of-heading) and [`Renderer`] is the state machine that turns it
//! into styled rows. Almost everything it holds is there because markdown is
//! nested and the event stream is not: a stack of styles, a stack of the
//! containers a line is inside, the table being accumulated.
//!
//! Inline content is buffered rather than emitted as it arrives, because a
//! line cannot be wrapped until it is finished. [`Renderer::flush`] is where a
//! paragraph becomes rows.
//!
//! # Containers
//!
//! A quote can hold a list whose item holds a code block. Each container puts
//! its own piece in front of every line drawn inside it, outermost first: a
//! quote its gutter bar, an item as many spaces as its marker is wide. An
//! item's marker takes the place of those spaces on the first line drawn in
//! the item, whatever kind of block that line belongs to. One stack of them
//! ([`Nest`]) is what lines a code block in a list item up under the item's
//! text, keeps the gutter on a heading inside a quote, and puts the bullet on
//! the line it belongs to.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::scan_wikilinks;
use super::wrap::{clip, spans_width, wrap};
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
    /// How each column's cells sit in it: what the delimiter row's colons
    /// said, `:--`, `:-:` or `--:`.
    align: Vec<Alignment>,
}

/// Something a line can be inside. See the module docs.
enum Nest {
    /// A blockquote: a gutter bar in front of every line.
    Quote,
    /// A list, and the next number when it is an ordered one. Puts nothing in
    /// front of a line itself; its items do.
    List(Option<u64>),
    /// A list item, or a footnote's definition: as many spaces as its marker
    /// is wide, and the marker itself until a line inside has taken it.
    Item {
        width: usize,
        marker: Option<Span<'static>>,
    },
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
    /// The containers the next line is inside, outermost first.
    nest: Vec<Nest>,
    /// `Some(info)` while inside a fenced block; the info string names the
    /// language. Its presence is also how `Event::Text` knows to buffer rather
    /// than emit.
    code_fence: Option<String>,
    /// Raw code accumulated inside the current fence, highlighted on close.
    code_buf: String,
    table: Option<TableState>,
    /// Inside a block of raw HTML, whose lines are drawn as they are written.
    in_html: bool,
    /// A blank line asked for and not yet drawn, and how many of the
    /// containers open now were open when it was asked for. It waits for the
    /// next line and is drawn inside the containers the two lines share:
    /// between two blocks in a quote it carries the gutter, before the
    /// quote's first line or after its last it does not, and at the very end
    /// it is never drawn at all.
    blank_due: Option<usize>,
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
            nest: Vec::new(),
            code_fence: None,
            code_buf: String::new(),
            table: None,
            in_html: false,
            blank_due: None,
        }
    }

    /// Everything rendered so far, with the last inline run flushed out.
    ///
    /// Takes `self` because there is nothing sensible to do with a renderer
    /// afterwards: the buffers it was accumulating into are now the answer.
    pub(super) fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        self.out
    }

    /// YAML front matter, drawn as it was written and dim: it is the note's
    /// settings for the program that published it, not part of what it says,
    /// and parsed as markdown its fences read as a rule and a heading.
    pub(super) fn front_matter(&mut self, yaml: &str) {
        for line in std::iter::once("---")
            .chain(yaml.lines())
            .chain(std::iter::once("---"))
        {
            let text = clip(&tabs::expand(line, tabs::MARKDOWN, 0), self.width);
            self.push_line(Line::from(Span::styled(text, self.pal.dim)));
        }
        self.blank();
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

    /// Whether the next line is inside a list.
    fn in_list(&self) -> bool {
        self.nest.iter().any(|n| matches!(n, Nest::List(_)))
    }

    /// What goes in front of the next line: every container's piece,
    /// outermost first. For the first line drawn in an item, `first` takes
    /// its marker in place of the spaces.
    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let open = self.nest.len();
        self.prefix_of(open, first)
    }

    /// [`Renderer::prefix`], of only the outermost `depth` containers.
    fn prefix_of(&mut self, depth: usize, first: bool) -> Vec<Span<'static>> {
        let gutter = Span::styled(self.pal.glyphs.gutter, self.pal.dim);
        let mut out = Vec::new();
        for n in &mut self.nest[..depth] {
            match n {
                Nest::Quote => out.push(gutter.clone()),
                Nest::List(_) => {}
                Nest::Item { width, marker } => match marker.take().filter(|_| first) {
                    Some(m) => out.push(m),
                    None => out.push(Span::raw(" ".repeat(*width))),
                },
            }
        }
        out
    }

    /// How wide [`Renderer::prefix`] is, which is the same for any line.
    fn prefix_width(&self) -> usize {
        let gutter = self.pal.glyphs.gutter.width();
        self.nest
            .iter()
            .map(|n| match n {
                Nest::Quote => gutter,
                Nest::List(_) => 0,
                Nest::Item { width, .. } => *width,
            })
            .sum()
    }

    /// Put a finished line out, after the blank line due before it, if one
    /// is.
    fn push_line(&mut self, line: Line<'static>) {
        if let Some(shared) = self.blank_due.take() {
            // Drawn inside the containers it shares with the line before it:
            // a quote's gutter stays, an item's spaces are trimmed away.
            let mut spans = self.prefix_of(shared, false);
            while spans.last().is_some_and(|s| s.content.trim().is_empty()) {
                spans.pop();
            }
            if let Some(last) = spans.last_mut() {
                *last = Span::styled(last.content.trim_end().to_string(), last.style);
            }
            self.out.push(Line::from(spans));
        }
        self.out.push(line);
    }

    /// Separate what comes next with a blank line, but never two in a row and
    /// never one at the very top. Called at block boundaries, so this
    /// collapsing is what keeps the spacing even without every caller having
    /// to check. See `blank_due` for when it is drawn.
    fn blank(&mut self) {
        if !self.out.is_empty() {
            let open = self.nest.len();
            self.blank_due = Some(self.blank_due.map_or(open, |k| k.min(open)));
        }
    }

    /// Close the innermost container. A blank line still waiting no longer
    /// shares it with whatever comes next.
    fn close(&mut self) {
        self.nest.pop();
        if let Some(k) = &mut self.blank_due {
            *k = (*k).min(self.nest.len());
        }
    }

    /// Emit the accumulated inline run as wrapped lines and clear it.
    pub(super) fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let first = self.prefix(true);
        let cont = self.prefix(false);
        for line in wrap(&spans, self.width, first, cont) {
            self.push_line(line);
        }
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
                let t = tabs::expand(&t, tabs::MARKDOWN, 0);
                // Drawn with the ticks it was written with, and with two, the
                // way markdown writes it, when the code has a tick of its own:
                // `a`b` would read as code that stopped at the a.
                let shown = if t.contains('`') {
                    format!("`` {t} ``")
                } else {
                    format!("`{t}`")
                };
                self.spans.push(Span::styled(shown, style));
            }
            Event::SoftBreak => self.spans.push(Span::styled(" ".to_string(), self.style())),
            // The line ends here; the rest of the paragraph goes on under it,
            // lined up with the text it continues.
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.blank();
                let width = self.width.saturating_sub(self.prefix_width());
                let mut spans = self.prefix(true);
                spans.push(Span::styled(
                    self.pal.glyphs.rule.repeat(width),
                    self.pal.dim,
                ));
                self.push_line(Line::from(spans));
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
            // Raw HTML is shown as written rather than silently dropped. A
            // block of it keeps its own lines, so a `<!-- toc -->` does not
            // run into the paragraph after it.
            Event::Html(t) if self.in_html => {
                for line in t.lines() {
                    let text = tabs::expand(line, tabs::MARKDOWN, 0);
                    let text = clip(&text, self.width.saturating_sub(self.prefix_width()));
                    let mut spans = self.prefix(true);
                    spans.push(Span::styled(text, self.pal.dim));
                    self.push_line(Line::from(spans));
                }
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                let style = self.pal.dim;
                self.spans
                    .push(Span::styled(t.trim_end().to_string(), style));
            }
            _ => {}
        }
    }

    /// Text events are where `[[wikilinks]]` hide. They arrive whole only
    /// because `render` merges the parser's adjacent text events: on its own
    /// it hands over each `[` and `]` as a separate one.
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
    /// starting: an item's own text, before the code block or the nested list
    /// that follows it.
    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                // A second paragraph in a list item, set apart from the first
                // the way paragraphs are, rather than reading as one line
                // wrapped.
                if matches!(self.nest.last(), Some(Nest::Item { marker: None, .. })) {
                    self.blank();
                }
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.blank();
                self.style_stack.push(heading_style(self.pal, level));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.nest.push(Nest::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.blank();
                self.code_fence = Some(match kind {
                    CodeBlockKind::Fenced(info) => info.to_string(),
                    CodeBlockKind::Indented => String::new(),
                });
                self.code_buf.clear();
            }
            Tag::HtmlBlock => {
                self.flush();
                self.blank();
                self.in_html = true;
            }
            Tag::List(start) => {
                self.flush();
                if !self.in_list() {
                    self.blank();
                }
                self.nest.push(Nest::List(start));
            }
            Tag::Item => {
                let depth = self
                    .nest
                    .iter()
                    .filter(|n| matches!(n, Nest::List(_)))
                    .count()
                    .saturating_sub(1);
                let marker = match self.nest.last_mut() {
                    Some(Nest::List(Some(n))) => {
                        let s = format!("{n}. ");
                        *n += 1;
                        s
                    }
                    // The bullet changes with depth, so nesting reads at a
                    // glance.
                    _ => self.pal.glyphs.bullets[depth % 3].to_string(),
                };
                self.nest.push(Nest::Item {
                    width: marker.width(),
                    marker: Some(Span::styled(marker, self.pal.dim)),
                });
            }
            Tag::FootnoteDefinition(label) => {
                self.flush();
                self.blank();
                let marker = format!("[^{label}]: ");
                self.nest.push(Nest::Item {
                    width: marker.width(),
                    marker: Some(Span::styled(marker, self.pal.link)),
                });
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
            Tag::Table(align) => {
                self.flush();
                self.blank();
                self.table = Some(TableState {
                    rows: Vec::new(),
                    align,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            _ => {}
        }
    }

    /// Close a block: flush its text, undo whatever `start` pushed, and add
    /// the trailing rule or blank line the block calls for.
    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                if !self.in_list() {
                    self.blank();
                }
            }
            TagEnd::Heading(level) => {
                self.flush();
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
                    let indent = self.prefix_width();
                    let room = self.width.saturating_sub(indent);
                    let w = match level {
                        HeadingLevel::H1 => room,
                        _ => self
                            .out
                            .last()
                            .map(|l| spans_width(&l.spans).saturating_sub(indent))
                            .unwrap_or(room)
                            .clamp(4.min(room), room),
                    };
                    let mut spans = self.prefix(false);
                    spans.push(Span::styled(ch.repeat(w), self.pal.dim));
                    self.push_line(Line::from(spans));
                }
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.close();
                self.blank();
            }
            TagEnd::CodeBlock => {
                let info = self.code_fence.take().unwrap_or_default();
                let body = std::mem::take(&mut self.code_buf);
                self.emit_code_block(&info, &body);
                self.blank();
            }
            TagEnd::HtmlBlock => {
                self.in_html = false;
                self.blank();
            }
            TagEnd::List(_) => {
                self.close();
                if !self.in_list() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                // Tight list items carry their text straight here, with no
                // paragraph wrapper to flush it first. An item with nothing
                // in it still shows its marker.
                self.flush();
                if matches!(
                    self.nest.last(),
                    Some(Nest::Item {
                        marker: Some(_),
                        ..
                    })
                ) {
                    let line = self.prefix(true);
                    self.push_line(Line::from(line));
                }
                self.close();
            }
            TagEnd::FootnoteDefinition => {
                self.flush();
                self.close();
                self.blank();
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
        let gutter = self.pal.glyphs.gutter;
        let avail = self
            .width
            .saturating_sub(self.prefix_width() + gutter.width())
            .max(1);
        for pieces in hl.highlight_snippet(body, syntax) {
            let mut spans = self.prefix(true);
            spans.push(Span::styled(gutter, self.pal.dim));
            let mut w = 0usize;
            for (style, text) in pieces {
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
            self.push_line(Line::from(spans));
        }
    }

    /// Lay out and draw the buffered table.
    ///
    /// Columns start at their natural width (the widest cell in each), then
    /// the widest column is shaved one cell at a time until the whole table
    /// fits the pane. Shrinking stops at three cells per column so a table
    /// squeezed into a narrow pane degrades into something still readable
    /// rather than a row of vertical bars. Each cell sits in its column the
    /// way the delimiter row asked: left, right, or centered.
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
        // Shrink the widest column until the table fits what the containers
        // around it leave of the pane.
        let room = self.width.saturating_sub(self.prefix_width());
        let sep_w = self.pal.glyphs.column.width() * cols.saturating_sub(1);
        while widths.iter().sum::<usize>() + sep_w > room {
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
            let mut spans = self.prefix(true);
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
                let mut text: Vec<Span<'static>> = Vec::new();
                let mut w = 0usize;
                for s in cell {
                    let room = widths[ci].saturating_sub(w);
                    if room == 0 {
                        break;
                    }
                    let clipped = clip(&s.content, room);
                    w += clipped.width();
                    let style = if bold {
                        s.style.add_modifier(Modifier::BOLD)
                    } else {
                        s.style
                    };
                    text.push(Span::styled(clipped, style));
                }
                let pad = widths[ci].saturating_sub(w);
                let (before, after) = match t.align.get(ci) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                if before > 0 {
                    spans.push(Span::raw(" ".repeat(before)));
                }
                spans.extend(text);
                if after > 0 {
                    spans.push(Span::raw(" ".repeat(after)));
                }
            }
            self.push_line(Line::from(spans));
            if ri == 0 {
                let mut rule = self.prefix(false);
                let g = self.pal.glyphs;
                let bar: String = widths
                    .iter()
                    .map(|w| g.rule.repeat(*w))
                    .collect::<Vec<_>>()
                    .join(g.cross);
                rule.push(Span::styled(bar, dim));
                self.push_line(Line::from(rule));
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
    fn the_gap_before_a_quote_is_outside_it() {
        // The blank line between a paragraph and a quote belongs to neither,
        // so it has no bar; one between two paragraphs in a quote does.
        let out = plain("para\n\n> quote\n>\n> more\n", 40);
        assert_eq!(out[1].trim(), "", "{out:?}");
        assert!(out[3].trim_start().starts_with('\u{2502}'), "{out:?}");
        let nested = plain("> a\n>\n> > b\n", 40);
        assert_eq!(nested[1].trim(), "\u{2502}", "one bar, not two: {nested:?}");
    }

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

    /// The rendered lines with nothing but blank ones dropped, for comparing
    /// layouts.
    fn rows(src: &str, width: usize) -> Vec<String> {
        plain(src, width)
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect()
    }

    #[test]
    fn a_block_in_a_list_item_sits_under_the_items_text() {
        // The item's number stays on the item's line, and the code it holds
        // lines up under its text rather than at the margin, with a lone
        // number left behind further down.
        let out = rows(
            "1. Install:\n   ```sh\n   cargo install tiny\n   ```\n2. Run it.\n",
            40,
        );
        assert_eq!(
            out,
            ["1. Install:", "   │ cargo install tiny", "2. Run it."]
        );

        let out = rows("- item text\n  > quote\n- next\n", 40);
        assert_eq!(out, ["• item text", "  │ quote", "• next"]);

        let out = rows("- item text\n  # Title\n", 40);
        assert_eq!(out[0], "• item text", "not run into the heading");
        assert_eq!(out[1], "  Title");
    }

    #[test]
    fn the_lines_of_a_list_item_line_up_under_its_text() {
        assert_eq!(
            rows("- one  \n  two\n", 40),
            ["• one", "  two"],
            "a hard break"
        );
        assert_eq!(
            rows("- first\n\n  second\n- next\n", 40),
            ["• first", "  second", "• next"],
            "a second paragraph"
        );
        let out = rows("10. a long item that wraps in a narrow pane\n", 20);
        assert!(
            out[1].starts_with("    ") && !out[1].starts_with("     "),
            "{out:?}"
        );
    }

    #[test]
    fn a_quote_keeps_its_gutter_on_everything_inside_it() {
        let out = plain("> # Title\n> text\n", 30);
        assert_eq!(out[0], "│ Title");
        assert!(out[1].starts_with("│ ━"), "the heading's rule: {out:?}");
        assert_eq!(out[2], "│", "the blank between them");
        assert_eq!(out[3], "│ text");
        assert_eq!(out.len(), 4, "and nothing hanging after it: {out:?}");
        assert_eq!(
            rows("> ```\n> x\n> ```\n", 30),
            ["│ │ x"],
            "code in a quote"
        );
    }

    #[test]
    fn a_block_of_html_keeps_its_own_lines() {
        assert_eq!(
            rows("<!-- toc -->\n\nIntro\n", 40),
            ["<!-- toc -->", "Intro"]
        );
        let out = rows(
            "<details>\n<summary>More</summary>\n\nhidden\n\n</details>\n\n# After\n",
            40,
        );
        assert_eq!(
            &out[..5],
            [
                "<details>",
                "<summary>More</summary>",
                "hidden",
                "</details>",
                "After"
            ]
        );
    }

    #[test]
    fn a_footnote_keeps_its_label() {
        assert_eq!(
            rows("Text[^1].\n\n[^1]: The note.\n", 40),
            ["Text[^1].", "[^1]: The note."]
        );
    }

    #[test]
    fn front_matter_is_drawn_as_written() {
        let out = rows("---\ntitle: Plan\ntags: [a]\n---\n\nBody\n", 40);
        assert_eq!(out, ["---", "title: Plan", "tags: [a]", "---", "Body"]);
        // A note that only starts with a rule keeps its rule and heading.
        let out = rows("---\n\nIntro\n", 40);
        assert_eq!(out[0], "─".repeat(40));
    }

    #[test]
    fn a_table_column_sits_where_its_colons_say() {
        let out = rows(
            "| a | b | c |\n|:--|--:|:-:|\n| left | right | mid |\n| l | r | m |\n",
            40,
        );
        assert_eq!(out[2], "left │ right │ mid");
        assert_eq!(out[3], "l    │     r │  m ");
    }

    #[test]
    fn a_wikilink_is_drawn_as_a_link() {
        let pal = crate::config::Palette::default();
        let hl = crate::text::highlight::Highlighter::new();
        let line = &crate::text::markdown::render("Go to [[design]] now\n", 40, &pal, &hl)[0];
        let linked: Vec<&str> = line
            .spans
            .iter()
            .filter(|s| s.style == pal.text.patch(pal.link))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(linked, ["[[design]]"], "{line:?}");
    }

    #[test]
    fn a_tiny_pane_still_renders_without_panicking() {
        for w in 0..6 {
            let _ = plain("# Heading\n\n- item with words\n\n> quote\n", w);
        }
    }
}
