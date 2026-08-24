//! Markdown -> styled terminal lines.
//!
//! pulldown-cmark gives us the event stream; this module turns it into wrapped,
//! styled `Line`s sized to the preview pane. Wrapping happens here rather than
//! via ratatui's `Paragraph` because each line carries per-span styling that a
//! naive re-wrap would smear.
//!
//! `[[wikilinks]]` are not CommonMark, so they are scanned out of text events
//! and styled separately. The same scanner feeds the link graph.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::config::Palette;
use crate::highlight::Highlighter;

/// Extract every `[[target]]` in document order, with any `|alias` stripped.
/// Duplicates are kept — callers that want unique edges can dedupe.
///
/// Nothing calls this yet; it is the entry point the link graph will use.
#[allow(dead_code)]
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
fn scan_wikilinks(text: &str) -> Vec<(usize, usize, Option<String>)> {
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
fn scan_urls(text: &str, pal: &Palette) -> Vec<Span<'static>> {
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

pub fn render(source: &str, width: usize, pal: &Palette, hl: &Highlighter) -> Vec<Line<'static>> {
    let width = width.max(8);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);

    let mut r = Renderer {
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
    };
    for event in Parser::new_ext(source, opts) {
        r.event(event);
    }
    r.flush_inline(Vec::new(), Vec::new());
    // Trim trailing blank lines so the pane does not open with dead space.
    while r.out.last().is_some_and(is_blank) {
        r.out.pop();
    }
    r.out
}

fn is_blank(l: &Line<'_>) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

struct TableState {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    in_head: bool,
}

struct Renderer<'a> {
    pal: &'a Palette,
    hl: &'a Highlighter,
    width: usize,
    out: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    style_stack: Vec<Style>,
    /// One entry per nesting level; `Some(n)` is the next number in an
    /// ordered list, `None` is a bullet list.
    list_stack: Vec<Option<u64>>,
    quote_depth: usize,
    code_fence: Option<String>,
    code_buf: String,
    table: Option<TableState>,
    /// Bullet/number to place on the first line of the current list item.
    pending_item_prefix: Option<String>,
}

impl Renderer<'_> {
    fn style(&self) -> Style {
        *self.style_stack.last().unwrap()
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.style_stack.push(s);
    }

    fn pop_style(&mut self) {
        if self.style_stack.len() > 1 {
            self.style_stack.pop();
        }
    }

    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.last().is_some_and(is_blank) {
            self.out.push(Line::from(""));
        }
    }

    /// The `│ ` gutter that marks blockquote depth.
    fn quote_prefix(&self) -> Vec<Span<'static>> {
        (0..self.quote_depth)
            .map(|_| Span::styled("│ ".to_string(), self.pal.dim))
            .collect()
    }

    /// Emit the accumulated inline run as wrapped lines and clear it.
    fn flush_inline(&mut self, first: Vec<Span<'static>>, cont: Vec<Span<'static>>) {
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

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
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
                    "─".repeat(self.width),
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

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.blank();
                self.style_stack.push(self.pal.heading);
                if level >= HeadingLevel::H3 {
                    let hashes = "#".repeat(level as usize);
                    self.spans
                        .push(Span::styled(format!("{hashes} "), self.pal.dim));
                }
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
                    // Bullet glyph varies by depth so nesting reads at a glance.
                    _ => match depth % 3 {
                        0 => "• ".to_string(),
                        1 => "◦ ".to_string(),
                        _ => "▪ ".to_string(),
                    },
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
                // H1 and H2 get a rule under them; deeper levels use their
                // `###` prefix instead, so the page does not turn into lines.
                let rule = match level {
                    HeadingLevel::H1 => Some("━"),
                    HeadingLevel::H2 => Some("─"),
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

    fn emit_code_block(&mut self, info: &str, body: &str) {
        let syntax = self.hl.syntax_for_token(info).clone();
        let gutter = self.pal.dim;
        let avail = self.width.saturating_sub(2).max(1);
        for pieces in self.hl.highlight_snippet(body, &syntax) {
            let mut spans = vec![Span::styled("│ ".to_string(), gutter)];
            let mut w = 0usize;
            for (style, text) in pieces {
                // Code is not re-wrapped — indentation is meaning. Long lines
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
                    spans.push(Span::styled(" │ ".to_string(), dim));
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
                let bar: String = widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                rule.push(Span::styled(bar, dim));
                self.out.push(Line::from(rule));
            }
        }
        self.blank();
    }
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Truncate to a display width, respecting character boundaries.
fn clip(s: &str, max_w: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.to_string().width();
        if w + cw > max_w {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

/// Push a wrapped line, dropping whitespace that ran off the end of it.
fn push_trimmed(lines: &mut Vec<Line<'static>>, mut spans: Vec<Span<'static>>) {
    while spans.len() > 1
        && spans
            .last()
            .is_some_and(|s| s.content.trim_end().is_empty())
    {
        spans.pop();
    }
    if let Some(last) = spans.last_mut()
        && (last.content.ends_with(' ') || last.content.ends_with('\t'))
    {
        *last = Span::styled(last.content.trim_end().to_string(), last.style);
    }
    lines.push(Line::from(spans));
}

/// Wrap a styled inline run to `width`, prefixing the first line with
/// `first_prefix` and every continuation line with `cont_prefix`.
fn wrap(
    spans: &[Span<'static>],
    width: usize,
    first_prefix: Vec<Span<'static>>,
    cont_prefix: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    let first_w = spans_width(&first_prefix);
    let cont_w = spans_width(&cont_prefix);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur = first_prefix;
    let mut cur_w = first_w;
    let mut indent_w = first_w;

    // Flatten into whitespace/word tokens, carrying each token's style.
    let mut tokens: Vec<(Style, String)> = Vec::new();
    for s in spans {
        let mut buf = String::new();
        let mut buf_space = false;
        for c in s.content.chars() {
            let is_space = c == ' ' || c == '\t';
            if !buf.is_empty() && is_space != buf_space {
                tokens.push((s.style, std::mem::take(&mut buf)));
            }
            buf_space = is_space;
            buf.push(c);
        }
        if !buf.is_empty() {
            tokens.push((s.style, buf));
        }
    }

    for (style, tok) in tokens {
        let tw = tok.width();
        let is_space = tok.starts_with(' ') || tok.starts_with('\t');
        if is_space {
            // Never start a wrapped line with the space that caused the break,
            // and never let a trailing space push the line past the width.
            if cur_w > indent_w && cur_w + tw <= width {
                cur.push(Span::styled(tok, style));
                cur_w += tw;
            }
            continue;
        }
        if cur_w + tw > width && cur_w > indent_w {
            push_trimmed(&mut lines, std::mem::replace(&mut cur, cont_prefix.clone()));
            cur_w = cont_w;
            indent_w = cont_w;
        }
        if cur_w + tw > width && tw > width.saturating_sub(indent_w) {
            // A single token wider than the pane (a long URL): hard-break it.
            let mut rest = tok.as_str();
            while !rest.is_empty() {
                let room = width.saturating_sub(cur_w).max(1);
                let chunk = clip(rest, room);
                if chunk.is_empty() {
                    break;
                }
                cur_w += chunk.width();
                rest = &rest[chunk.len()..];
                cur.push(Span::styled(chunk, style));
                if !rest.is_empty() {
                    push_trimmed(&mut lines, std::mem::replace(&mut cur, cont_prefix.clone()));
                    cur_w = cont_w;
                    indent_w = cont_w;
                }
            }
        } else {
            cur.push(Span::styled(tok, style));
            cur_w += tw;
        }
    }
    push_trimmed(&mut lines, cur);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(source: &str, width: usize) -> Vec<String> {
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
    fn flat(source: &str, width: usize) -> Vec<String> {
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

    #[test]
    fn headings_get_a_rule_and_deeper_levels_get_hashes() {
        let out = plain("# Title\n\nbody\n", 20);
        assert_eq!(out[0], "Title");
        assert_eq!(out[1], "━".repeat(20), "H1 is underlined across the pane");

        let out = plain("### Deep\n", 20);
        assert_eq!(out[0], "### Deep");
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
