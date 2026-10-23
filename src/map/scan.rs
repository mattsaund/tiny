//! Reading one file and saying what is in it.
//!
//! Everything [`super::graph::build`] needs to know about a single file, and
//! nothing about the graph it ends up in. Split out because these are two
//! different kinds of work: this half is parsers and heuristics, and knows
//! what a link and a call look like in each language; the other half is
//! bookkeeping over the answers.
//!
//! # The budget
//!
//! Building the graph reads every file it can find something in, while the
//! user waits, see the event loop's docs in `main`. So every read here is
//! capped at [`MAX_PARSE_BYTES`], and a file over the line is skipped rather
//! than truncated: half a parse produces confidently wrong symbols, where no
//! parse produces none. A file with nothing to find (a picture, a binary, a
//! language with no grammar here) is never opened at all; see [`facts_of`].
//!
//! # Test code is not the project
//!
//! [`strip_tests`] takes a module's tests out before its calls are collected,
//! and [`is_test_file`] leaves out a file that is all test. A test file calls
//! everything, so leaving them in makes every file look connected to every
//! other, and the map's whole value is in the connections that are not there.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::map::graph::{EdgeKind, NodeKind};

/// Source files past this size are not parsed for symbols.
pub(super) const MAX_PARSE_BYTES: u64 = 1024 * 1024;

// ---- language support -----------------------------------------------------

/// Tags configurations are expensive to compile, so each is built once.
/// A grammar that fails to compile its query is simply skipped: one bad
/// language should not take the whole graph down with it.
static PYTHON: LazyLock<Option<TagsConfiguration>> = LazyLock::new(|| {
    TagsConfiguration::new(
        tree_sitter_python::LANGUAGE.into(),
        tree_sitter_python::TAGS_QUERY,
        "",
    )
    .ok()
});
static RUST: LazyLock<Option<TagsConfiguration>> = LazyLock::new(|| {
    TagsConfiguration::new(
        tree_sitter_rust::LANGUAGE.into(),
        tree_sitter_rust::TAGS_QUERY,
        "",
    )
    .ok()
});
static JAVASCRIPT: LazyLock<Option<TagsConfiguration>> = LazyLock::new(|| {
    TagsConfiguration::new(
        tree_sitter_javascript::LANGUAGE.into(),
        tree_sitter_javascript::TAGS_QUERY,
        tree_sitter_javascript::LOCALS_QUERY,
    )
    .ok()
});

/// Languages with a tree-sitter grammar compiled in. Adding one means adding
/// a variant here and wiring it through [`lang_of`] and [`tags_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    Python,
    Rust,
    JavaScript,
}

/// Map a file extension to a parseable language, or `None` for everything
/// else. TypeScript is absent deliberately; it needs its own grammar, and
/// falling back to the JavaScript one mis-parses type annotations.
pub(crate) fn lang_of(path: &Path) -> Option<Lang> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("py" | "pyi") => Some(Lang::Python),
        Some("rs") => Some(Lang::Rust),
        Some("js" | "jsx" | "mjs" | "cjs") => Some(Lang::JavaScript),
        _ => None,
    }
}

/// The compiled tags query for a language, or `None` if its query failed to
/// compile at first use.
pub(super) fn tags_for(lang: Lang) -> Option<&'static TagsConfiguration> {
    match lang {
        Lang::Python => PYTHON.as_ref(),
        Lang::Rust => RUST.as_ref(),
        Lang::JavaScript => JAVASCRIPT.as_ref(),
    }
}

// ---- one file's contribution ----------------------------------------------

/// What one file contributes to the graph, before anything is resolved.
///
/// Kept separate from [`super::graph::Node`] because it is scratch: the resolution pass
/// consumes it and only `defines` survives onto the node, for the tooltip.
#[derive(Debug, Default)]
pub(super) struct Facts {
    /// Symbols defined here.
    pub(super) defines: BTreeSet<String>,
    /// Symbols called here: how many times, and how.
    pub(super) calls: BTreeMap<String, Call>,
    /// Wikilink and markdown-link targets, as written.
    pub(super) links: Vec<(String, EdgeKind)>,
    /// Test modules this file declares in files of their own, `#[cfg(test)]
    /// mod testing;`: those files are tests too.
    pub(super) test_modules: Vec<String>,
}

/// The calls a file makes to one name.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Call {
    pub(super) count: usize,
    /// Whether any of them said where the name lives (`obj.load()`,
    /// `helpers::load()`), which a bare `load()` does not: a file that calls
    /// its own `load` that way means its own.
    pub(super) qualified: bool,
}

impl Facts {
    fn call(&mut self, name: &str, qualified: bool) {
        let c = self.calls.entry(name.to_string()).or_default();
        c.count += 1;
        c.qualified |= qualified;
    }
}

/// Classify a file. Order matters: markdown is checked before the prose list
/// because it is usually *in* that list but has a link syntax of its own.
pub(super) fn node_kind(path: &Path, prose_exts: &[String]) -> NodeKind {
    if crate::text::markdown::is_markdown(path) {
        return NodeKind::Note;
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(e) if prose_exts.iter().any(|p| p.eq_ignore_ascii_case(&e)) => NodeKind::Prose,
        Some(_) if lang_of(path).is_some() => NodeKind::Code,
        // Source in a language tiny cannot parse still shows, it just has no
        // outgoing call edges.
        Some(e) if is_sourcelike(&e) => NodeKind::Code,
        _ => NodeKind::Other,
    }
}

/// Extensions that are recognizably source without a grammar to parse them.
/// These become [`NodeKind::Code`] nodes with no outgoing call edges, so a Go
/// or C++ project still draws as something rather than as a pile of orphans.
pub(super) fn is_sourcelike(ext: &str) -> bool {
    matches!(
        ext,
        "c" | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "go"
            | "java"
            | "rb"
            | "php"
            | "sh"
            | "bash"
            | "fish"
            | "ts"
            | "tsx"
            | "lua"
            | "zig"
            | "swift"
            | "kt"
            | "cs"
    )
}

/// What one file contributes, read only when there is something to find in
/// it: links in prose, definitions and calls in a language with a grammar.
/// Everything else is a node and nothing more, so it is never opened:
/// reading every picture in a folder to throw it away was most of what a
/// media-heavy project cost the map.
///
/// `rel` is the path inside the project, which is what says whether a file
/// is a test: a project kept in a folder called `tests` is not all tests.
pub(super) fn facts_of(ctx: &mut TagsContext, path: &Path, rel: &str, kind: NodeKind) -> Facts {
    let mut f = Facts::default();
    match kind {
        NodeKind::Note | NodeKind::Prose => {
            if let Some(text) = read_text(path) {
                collect_links(&text, kind, &mut f);
            }
        }
        NodeKind::Code => {
            if let Some(lang) = lang_of(path)
                && !is_test_file(rel)
                && let Some(text) = read_text(path)
            {
                let (body, test_modules) = strip_tests(lang, &text);
                f.test_modules = test_modules;
                collect_symbols(ctx, lang, &body, &mut f);
                if lang == Lang::Rust {
                    collect_path_calls(&body, &mut f);
                }
            }
        }
        NodeKind::Other => {}
    }
    f
}

/// Read a file if it is small enough and looks like text. Deliberately a
/// local copy of `search::read_text` rather than a shared helper: the two have
/// different size limits, and the graph swallows errors as `None` where search
/// wants a `Result`.
///
/// One open, and the size taken from it rather than from a second lookup by
/// path. The read stops one byte past the limit, so a file that grew after it
/// was measured is still refused rather than read whole.
pub(crate) fn read_text(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > MAX_PARSE_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(MAX_PARSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PARSE_BYTES || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Wikilinks, plus relative markdown links to files inside the project.
pub(super) fn collect_links(text: &str, kind: NodeKind, f: &mut Facts) {
    for target in crate::text::markdown::wikilinks(text) {
        // `[[design#Goals]]` and `[[design#^block]]` link the note; what
        // follows the `#` is a place in it. `[[#Goals]]` is a place in this
        // one.
        let note = target.split('#').next().unwrap_or_default().trim();
        if !note.is_empty() {
            f.links.push((note.to_string(), EdgeKind::Wikilink));
        }
    }
    if kind != NodeKind::Note {
        return; // `[a](b)` only means a link in markdown
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        // `](` is the seam of an inline link; find its closing paren.
        if bytes[i] == b']'
            && bytes[i + 1] == b'('
            && let Some(end) = text[i + 2..].find(')')
        {
            if let Some(dest) = link_destination(&text[i + 2..i + 2 + end]) {
                f.links.push((dest, EdgeKind::Link));
            }
            i += 2 + end;
            continue;
        }
        i += 1;
    }
}

/// The file an inline link's parentheses point at, or `None` for a web
/// address, a mail address or nothing at all.
///
/// A path with spaces in it is written one of two ways, and both are read:
/// in angle brackets, `[a](<My note.md>)`, and with the spaces encoded,
/// `[a](My%20note.md)`, which is what Obsidian writes. A title after the path
/// and an anchor or a query on the end of it are not part of it.
fn link_destination(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let dest = match raw.strip_prefix('<') {
        Some(inside) => inside.split('>').next()?,
        None => raw.split_whitespace().next()?,
    };
    let dest = dest.split(['#', '?']).next()?;
    if dest.is_empty() || dest.contains("://") || dest.starts_with("mailto:") {
        return None;
    }
    Some(percent_decoded(dest))
}

/// `%20` and its like turned back into the bytes they stand for; anything that
/// does not decode to text is left as it was written.
fn percent_decoded(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let hex = |b: u8| (b as char).to_digit(16);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (
            b[i],
            b.get(i + 1).copied().and_then(hex),
            b.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (byte, ..) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Whether a file exists to test other files. Its helpers are not part of how
/// the project fits together, and they collide with real names constantly.
///
/// Covers the common conventions across languages: a `test_` prefix (Python),
/// `_test.` (Go, Rust), `.test.` and `.spec.` (JavaScript), a file called
/// `tests` or `testing`, and any folder named `tests` or `__tests__`, all of
/// it read from `rel`, the path inside the project: a project that lives in
/// a folder called `tests` is not therefore all tests.
pub(super) fn is_test_file(rel: &str) -> bool {
    let (folders, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let name = name.to_lowercase();
    let stem = name.split('.').next().unwrap_or_default();
    name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || matches!(stem, "tests" | "testing")
        || folders
            .split('/')
            .any(|folder| matches!(folder, "tests" | "__tests__"))
}

/// A Rust file with its tests taken out: every item behind `#[cfg(test)]`,
/// wherever it is, and the names of the test modules it declares in files of
/// their own (`#[cfg(test)] mod testing;`). Those helpers (`fixture`,
/// `render`, `plain`) shadow real names across the whole project if they are
/// counted.
///
/// Each item goes and nothing after it: a `#[cfg(test)] mod tests;` near the
/// top of a file used to take the whole file with it.
pub(super) fn strip_tests(lang: Lang, text: &str) -> (Cow<'_, str>, Vec<String>) {
    const ATTR: &str = "#[cfg(test)]";
    if lang != Lang::Rust || !text.contains(ATTR) {
        return (Cow::Borrowed(text), Vec::new());
    }
    let mut out = String::with_capacity(text.len());
    let mut modules = Vec::new();
    let mut from = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find(ATTR) {
        let attr = search + found;
        let line = text[..attr].rfind('\n').map_or(0, |i| i + 1);
        // Only the attribute itself, starting a line: the words in a string
        // or a comment are not one.
        if !text[line..attr].trim().is_empty() {
            search = attr + ATTR.len();
            continue;
        }
        let end = item_end(text, attr + ATTR.len());
        if let Some(name) = declared_module(&text[attr + ATTR.len()..end]) {
            modules.push(name);
        }
        out.push_str(&text[from..line]);
        // Its line breaks stay, so every line after it keeps its number.
        out.extend(text[line..end].chars().filter(|c| *c == '\n'));
        from = end;
        search = end;
    }
    out.push_str(&text[from..]);
    (Cow::Owned(out), modules)
}

/// Where the Rust item starting at `from` ends: past its `;`, or past the `}`
/// that closes the brace its body opened, whichever comes first.
///
/// Brackets of every kind are counted, so the `;` in `[u8; 3]` is not taken
/// for the end, and strings, characters and comments are stepped over whole,
/// so a `'{'` or a `"}"` in a test does not count either.
fn item_end(text: &str, from: usize) -> usize {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(b.len(), |n| i + n);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                continue;
            }
            b'"' => {
                i = string_end(b, i + 1, 0);
                continue;
            }
            // A raw string: `r"..."` or `r#"..."#`, with as many hashes at
            // the end as at the start.
            b'r' if !b
                .get(i.wrapping_sub(1))
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') =>
            {
                let hashes = b[i + 1..].iter().take_while(|c| **c == b'#').count();
                if b.get(i + 1 + hashes) == Some(&b'"') {
                    i = string_end(b, i + 2 + hashes, hashes);
                    continue;
                }
            }
            b'\'' => {
                i = char_end(text, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Just past the end of a string whose text starts at `i`, closed by a `"`
/// followed by `hashes` hashes. Escapes count only in a plain string.
fn string_end(b: &[u8], mut i: usize, hashes: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'\\' if hashes == 0 => i += 2,
            b'"' if b[i + 1..]
                .iter()
                .take(hashes)
                .filter(|c| **c == b'#')
                .count()
                == hashes =>
            {
                return i + 1 + hashes;
            }
            _ => i += 1,
        }
    }
    b.len()
}

/// Just past a character literal starting at the `'` at `i`, or just past the
/// quote alone when it is a lifetime or a label (`'a`, `'outer:`).
fn char_end(text: &str, i: usize) -> usize {
    let rest = &text[i + 1..];
    if let Some(escaped) = rest.strip_prefix('\\') {
        // `'\n'`, `'\''`, `'\u{1F600}'`: up to the quote after the escaped
        // character, which is stepped over whole, however many bytes it is.
        let Some(first) = escaped.chars().next() else {
            return text.len();
        };
        let skip = first.len_utf8();
        return escaped[skip..]
            .find('\'')
            .map_or(text.len(), |n| i + 2 + skip + n + 1);
    }
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(c), Some('\'')) => i + 1 + c.len_utf8() + 1,
        _ => i + 1,
    }
}

/// The name in a `mod name;` declaration, with nothing in braces after it:
/// a module whose code is in a file of its own.
fn declared_module(item: &str) -> Option<String> {
    let mut words = item
        .split(|c: char| c.is_whitespace() || c == ';')
        .filter(|w| !w.is_empty());
    let mut word = words.next()?;
    while word.starts_with("#[") || word.starts_with("pub") {
        word = words.next()?;
    }
    let name = (word == "mod").then(|| words.next())??;
    (!item.contains('{') && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| name.to_string())
}

/// Definitions and calls, via the language's tags query.
///
/// tree-sitter's tags queries emit a stream of tagged ranges with a syntax
/// type name: `function`, `call`, `reference.implementation`, and so on. Two
/// filters narrow that to something useful:
///
/// - **Definitions** are kept only for callable things. `mod x;` and `X = 1`
///   are definitions too, but nothing calls them, and counting them makes
///   every `mod` line look like a function.
/// - **References** are kept only when typed `call`, or `class` for the
///   `new Parser()` JavaScript tags as one: building something is calling it.
///   The others describe type relationships, not one file reaching into
///   another.
///
/// A call is *qualified* when the text just before its name says where it
/// lives (`obj.load()`, `helpers::load()`), see [`Call`].
///
/// Parse failures return quietly: a file with a syntax error contributes
/// nothing rather than taking the graph down.
pub(super) fn collect_symbols(ctx: &mut TagsContext, lang: Lang, text: &str, f: &mut Facts) {
    let Some(config) = tags_for(lang) else { return };
    let Ok((tags, _)) = ctx.generate_tags(config, text.as_bytes(), None) else {
        return;
    };
    for tag in tags.flatten() {
        let Some(name) = text.get(tag.name_range.clone()) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let syntax = config.syntax_type_name(tag.syntax_type_id);
        if tag.is_definition {
            if is_callable(syntax) {
                f.defines.insert(name.to_string());
            }
        } else if syntax == "call" || syntax == "class" {
            let before = &text[..tag.name_range.start];
            f.call(name, before.ends_with('.') || before.ends_with("::"));
        }
    }
}

/// Whether a definition is of something you can call or build: what the
/// map links calls to, and what `goto function` goes to.
///
/// `mod x;` and `X = 1` are definitions too, but nothing calls them, and
/// counting them makes every `mod` line look like a function.
fn is_callable(syntax: &str) -> bool {
    matches!(
        syntax,
        "function" | "method" | "class" | "interface" | "macro" | "struct" | "enum"
    )
}

/// Every callable thing defined in `text`, as its name, line and character
/// column: where `goto function` lands.
///
/// The same definitions [`collect_symbols`] counts for the map, so the two
/// agree on what a function is. Test code is included here: jumping to a test
/// is a perfectly good thing to want, where linking to one was not.
pub(crate) fn definitions(
    ctx: &mut TagsContext,
    lang: Lang,
    text: &str,
) -> Vec<(String, usize, usize)> {
    let Some(config) = tags_for(lang) else {
        return Vec::new();
    };
    let Ok((tags, _)) = ctx.generate_tags(config, text.as_bytes(), None) else {
        return Vec::new();
    };
    tags.flatten()
        .filter(|t| t.is_definition && is_callable(config.syntax_type_name(t.syntax_type_id)))
        .filter_map(|t| {
            let name = text.get(t.name_range.clone())?;
            // `span` is the name's own position; its column is in bytes.
            let line_start = t.name_range.start.checked_sub(t.span.start.column)?;
            let col = text.get(line_start..t.name_range.start)?.chars().count();
            Some((name.to_string(), t.span.start.row, col))
        })
        .collect()
}

/// Rust calls written through a path: `helpers::compute()`, `Config::load()`,
/// `parse::<u8>()`.
///
/// A gap in the shipped Rust tags query, and not a small one: it tags a bare
/// `compute()` as a call and says nothing about `helpers::compute()`, which is
/// how a Rust file nearly always reaches into another one. Without this, the
/// map of a Rust project is almost empty.
///
/// Through a type, `Config::new()`, the type counts as called too: `new` is
/// defined in every other file of a Rust project, too common to say which,
/// where `Config` names the one file that defines it.
///
/// Text scanning rather than a second tree-sitter query, for the same reason
/// the language list is short: this is thirty lines against a whole query to
/// compile and keep in step with the grammar. The cost is that a call written
/// inside a comment or a string counts. It is bounded by what happens next:
/// a name only becomes an edge if some file in the project defines it, and
/// `max_ambiguity` throws out the names too common to mean anything.
pub(super) fn collect_path_calls(text: &str, f: &mut Facts) {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while let Some(at) = text[i..].find("::") {
        let colons = i + at;
        let start = colons + 2;
        let mut end = start;
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
        i = start;
        if end == start {
            continue; // `::<T>`, `::*`, or a stray pair of colons
        }
        // Only a call, not a `use` path or a type: the parenthesis is what
        // says something is being invoked. A turbofish may sit between the
        // two, `parse::<u8>()`, and is stepped over.
        let mut after = end;
        if bytes[after..].starts_with(b"::<") {
            after = generics_end(bytes, after + 2);
        } else if bytes[after..].starts_with(b"::") {
            continue; // a longer path; the last segment is the call
        }
        while bytes.get(after) == Some(&b' ') {
            after += 1;
        }
        if bytes.get(after) == Some(&b'(') {
            f.call(&text[start..end], true);
            // The type the call goes through, when there is one.
            // Past the whole of the character before it, which is not always
            // one byte: `// 调用Config::load()`.
            let owner_start = text[..colons]
                .char_indices()
                .rev()
                .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
                .map_or(0, |(n, c)| n + c.len_utf8());
            let owner = &text[owner_start..colons];
            if owner.starts_with(|c: char| c.is_ascii_uppercase()) && owner != "Self" {
                f.call(owner, true);
            }
        }
        i = end;
    }
}

/// Just past the `>` that closes the `<` at `open`, angle brackets counted.
fn generics_end(bytes: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (n, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return n + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

/// The files of a project, indexed the ways a link can name one.
pub(super) struct Index {
    /// By path from the project root, forward-slashed, as written.
    pub(super) by_rel: HashMap<String, usize>,
    /// By file name, lowercased: `[[design]]` from anywhere.
    pub(super) by_name: HashMap<String, Vec<usize>>,
    /// Every path from the root, lowercased, in node order, for a link that
    /// names the end of one: `[[notes/index]]`.
    pub(super) rels: Vec<String>,
}

/// What a link written without an extension may leave off: a note's.
const NOTE_EXTENSIONS: &[&str] = &["md", "markdown", "txt"];

/// Turn a written target into a file. Tries, in order: the path as given,
/// with a note's extension if it has none, relative to the linking file, then
/// relative to the root; then, for a wikilink or a bare name, the file of that
/// name anywhere in the project, a note before anything else, and the end of
/// a path for a target with a folder in it.
///
/// The order is the whole design. Relative-to-the-linking-file comes first so
/// `../utils` means the neighbor you meant, not something with the same name
/// on the other side of the project. The anywhere fallback comes last and is
/// accepted *only when exactly one file answers to the name*, so an ambiguous
/// `[[index]]` in a project with five `index.md` files draws no edge rather
/// than a wrong one. A markdown link that names a folder is a path, and a
/// path that leads nowhere links nowhere: it is never guessed onto a file
/// with the same name somewhere else.
///
/// Returns `None` for anything it cannot place: a link to a file that does
/// not exist simply produces no edge, silently, which is correct for a graph
/// but does mean a typo'd wikilink is invisible here.
pub(super) fn resolve_link(
    target: &str,
    kind: EdgeKind,
    root: &Path,
    from_dir: Option<&Path>,
    index: &Index,
) -> Option<usize> {
    let target = target.trim().trim_start_matches("./");
    if target.is_empty() {
        return None;
    }

    // A name with a dot in it is not therefore one with an extension: a note
    // called `v1.2 notes` or `2024.03.01` is still a note, and the `.md` is
    // what its link left off.
    let lower = target.to_lowercase();
    let mut candidates: Vec<String> = vec![target.to_string()];
    if !NOTE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
    {
        candidates.extend(NOTE_EXTENSIONS.iter().map(|ext| format!("{target}.{ext}")));
    }

    for cand in &candidates {
        // Relative to the linking file first: `../utils` should mean the
        // neighbor, not something with the same name elsewhere.
        if let Some(dir) = from_dir {
            let joined = normalize(&dir.join(cand));
            if let Ok(rel) = joined.strip_prefix(root) {
                let key = rel.to_string_lossy().replace('\\', "/");
                if let Some(&id) = index.by_rel.get(&key) {
                    return Some(id);
                }
            }
        }
        if let Some(&id) = index.by_rel.get(cand.as_str()) {
            return Some(id);
        }
    }

    let path_like = target.contains('/');
    if kind == EdgeKind::Link && path_like {
        return None;
    }
    // Anywhere in the project, by name or by the end of a path, only when
    // one file answers. Every candidate is asked in turn, so `[[diagram]]`
    // finds `diagram.md` even beside a `diagram.png`.
    for cand in &candidates {
        let cand = cand.to_lowercase();
        let found: Vec<usize> = if path_like {
            let end = format!("/{cand}");
            index
                .rels
                .iter()
                .enumerate()
                .filter(|(_, rel)| **rel == cand || rel.ends_with(&end))
                .map(|(id, _)| id)
                .collect()
        } else {
            index.by_name.get(&cand).cloned().unwrap_or_default()
        };
        match found.as_slice() {
            [one] => return Some(*one),
            [] => {}
            _ => return None,
        }
    }
    None
}

/// Resolve `.` and `..` without touching the filesystem.
///
/// Purely lexical, unlike `canonicalize`: link targets routinely name files
/// that do not exist, and a syscall per candidate per link would be slow. Note
/// that `pop` on an empty buffer is a no-op, so `../..` from the root cannot
/// escape upwards; it just flattens.
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
