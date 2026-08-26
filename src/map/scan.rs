//! Reading one file and saying what is in it.
//!
//! Everything [`super::graph::build`] needs to know about a single file, and
//! nothing about the graph it ends up in. Split out because these are two
//! different kinds of work: this half is parsers and heuristics and has to
//! know about Python's import syntax; the other half is bookkeeping over the
//! answers.
//!
//! # The budget
//!
//! Building the graph reads every file in the project, synchronously, while
//! the user waits — see the event loop's docs in `main`. So every read here
//! is capped at [`MAX_PARSE_BYTES`], and a file over the line is skipped
//! rather than truncated: half a parse produces confidently wrong symbols,
//! where no parse produces none.
//!
//! # Test code is not the project
//!
//! [`strip_tests`] takes a module's tests out before its calls are collected.
//! A test file calls everything, so leaving them in makes every file look
//! connected to every other, and the map's whole value is in the connections
//! that are not there.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::map::graph::{EdgeKind, NodeKind};

/// Source files past this size are not parsed for symbols.
pub(super) const MAX_PARSE_BYTES: u64 = 1024 * 1024;

// ---- language support -----------------------------------------------------

/// Tags configurations are expensive to compile, so each is built once.
/// A grammar that fails to compile its query is simply skipped — one bad
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
/// a variant here and wiring it through [`lang_of`], [`tags_for`] and
/// [`tags_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lang {
    Python,
    Rust,
    JavaScript,
}

/// Map a file extension to a parseable language, or `None` for everything
/// else. TypeScript is absent deliberately — it needs its own grammar, and
/// falling back to the JavaScript one mis-parses type annotations.
pub(super) fn lang_of(path: &Path) -> Option<Lang> {
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

/// Languages tiny can trace calls through, for the "what is supported" line.
pub(super) fn supported_languages() -> &'static [&'static str] {
    &["Python", "Rust", "JavaScript"]
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
    /// Symbols called here, with how many times.
    pub(super) calls: BTreeMap<String, usize>,
    /// Wikilink and markdown-link targets, as written.
    pub(super) links: Vec<(String, EdgeKind)>,
}

/// Classify a file. Order matters: markdown is checked before the prose list
/// because it is usually *in* that list but has a link syntax of its own.
pub(super) fn node_kind(path: &Path, prose_exts: &[String]) -> NodeKind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(e) if matches!(e.as_str(), "md" | "markdown" | "mdown" | "mkd") => NodeKind::Note,
        Some(e) if prose_exts.iter().any(|p| p.eq_ignore_ascii_case(&e)) => NodeKind::Prose,
        Some(_) if lang_of(path).is_some() => NodeKind::Code,
        // Source in a language tiny cannot parse still shows, it just has no
        // outgoing call edges.
        Some(e) if is_sourcelike(&e) => NodeKind::Code,
        _ => NodeKind::Other,
    }
}

/// Extensions that are recognisably source without a grammar to parse them.
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

/// Read a file if it is small enough and looks like text. Deliberately a
/// local copy of `search::read_text` rather than a shared helper: the two have
/// different size limits, and the graph swallows errors as `None` where search
/// wants a `Result`.
pub(super) fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_PARSE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Wikilinks, plus relative markdown links to files inside the project.
pub(super) fn collect_links(text: &str, kind: NodeKind, f: &mut Facts) {
    for target in crate::text::markdown::wikilinks(text) {
        f.links.push((target, EdgeKind::Wikilink));
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
            {
                let dest = text[i + 2..i + 2 + end].trim();
                // Titles and anchors are not part of the path.
                let dest = dest.split_whitespace().next().unwrap_or(dest);
                let dest = dest.split('#').next().unwrap_or(dest);
                if !dest.is_empty() && !dest.contains("://") && !dest.starts_with("mailto:") {
                    f.links.push((dest.to_string(), EdgeKind::Link));
                }
                i += 2 + end;
                continue;
            }
        }
        i += 1;
    }
}

/// Whether a file exists to test other files. Its helpers are not part of how
/// the project fits together, and they collide with real names constantly.
///
/// Covers the common conventions across languages: a `test_` prefix (Python),
/// `_test.` (Go, Rust), `.test.` and `.spec.` (JavaScript), and any path
/// component named `tests` or `__tests__`.
pub(super) fn is_test_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || path
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some("tests" | "__tests__")))
}

/// Drop a trailing test module. Rust puts its tests in the same file behind
/// `#[cfg(test)]`, and those helpers — `fixture`, `render`, `plain` — shadow
/// real names across the whole project if they are counted.
pub(super) fn strip_tests(lang: Lang, text: &str) -> &str {
    match lang {
        Lang::Rust => match text.find("\n#[cfg(test)]") {
            Some(at) => &text[..=at],
            None => text,
        },
        _ => text,
    }
}

/// Definitions and calls, via the language's tags query.
///
/// tree-sitter's tags queries emit a stream of tagged ranges with a syntax
/// type name — `function`, `call`, `reference.implementation`, and so on. Two
/// filters narrow that to something useful:
///
/// - **Definitions** are kept only for callable things. `mod x;` and `X = 1`
///   are definitions too, but nothing calls them, and counting them makes
///   every `mod` line look like a function.
/// - **References** are kept only when typed `call`. The others describe type
///   relationships, not one file reaching into another.
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
            // `mod x;` and `X = 1` are definitions, but nothing calls them.
            // Counting them makes every `mod` line look like a function.
            if matches!(
                syntax,
                "function" | "method" | "class" | "interface" | "macro" | "struct" | "enum"
            ) {
                f.defines.insert(name.to_string());
            }
        } else {
            // Only calls are followed. `reference.implementation` and friends
            // describe type relationships, not one file reaching into another.
            if syntax == "call" {
                *f.calls.entry(name.to_string()).or_insert(0) += 1;
            }
        }
    }
}

/// Rust calls written through a path — `helpers::compute()`, `Config::load()`.
///
/// A gap in the shipped Rust tags query, and not a small one: it tags a bare
/// `compute()` as a call and says nothing about `helpers::compute()`, which is
/// how a Rust file nearly always reaches into another one. Without this, the
/// map of a Rust project is almost empty.
///
/// Text scanning rather than a second tree-sitter query, for the same reason
/// the language list is short: this is thirty lines against a whole query to
/// compile and keep in step with the grammar. The cost is that a call written
/// inside a comment or a string counts. It is bounded by what happens next —
/// a name only becomes an edge if some file in the project defines it, and
/// `max_ambiguity` throws out the names too common to mean anything.
pub(super) fn collect_path_calls(text: &str, f: &mut Facts) {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while let Some(at) = text[i..].find("::") {
        let start = i + at + 2;
        let mut end = start;
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
        i = start.max(i + at + 1);
        if end == start {
            continue; // `::<T>`, `::*`, or a stray pair of colons
        }
        // Only a call, not a `use` path or a type: the parenthesis is what
        // says something is being invoked. Turbofish sits between the two.
        let mut after = end;
        if bytes.get(after) == Some(&b':') && bytes.get(after + 1) == Some(&b':') {
            continue; // a longer path; the last segment is the call
        }
        while bytes.get(after) == Some(&b' ') {
            after += 1;
        }
        if bytes.get(after) == Some(&b'(') {
            *f.calls.entry(text[start..end].to_string()).or_insert(0) += 1;
        }
        i = end;
    }
}

/// Turn a written target into a file. Tries, in order: the path as given
/// relative to the linking file, the same relative to the root, and finally
/// the bare stem — which is how `[[design]]` finds `notes/design.md`.
///
/// The order is the whole design. Relative-to-the-linking-file comes first so
/// `../utils` means the neighbour you meant, not something with the same name
/// on the other side of the project. The bare-stem fallback comes last and is
/// accepted *only when exactly one file answers to the name*, so an ambiguous
/// `[[index]]` in a project with five `index.js` files draws no edge rather
/// than a wrong one.
///
/// An extensionless target is tried against a list of likely suffixes, and
/// against directory-index filenames (`mod.rs`, `__init__.py`, `index.js`), so
/// `import utils` finds `utils/__init__.py`.
///
/// Returns `None` for anything it cannot place — a link to a file that does
/// not exist simply produces no edge, silently, which is correct for a graph
/// but does mean a typo'd wikilink is invisible here.
pub(super) fn resolve_link(
    target: &str,
    root: &Path,
    from_dir: Option<&Path>,
    by_rel: &HashMap<String, usize>,
    by_stem: &HashMap<String, Vec<usize>>,
) -> Option<usize> {
    let target = target.trim().trim_start_matches("./");
    if target.is_empty() {
        return None;
    }

    // Candidate suffixes for extensionless targets.
    let mut candidates: Vec<String> = vec![target.to_string()];
    if Path::new(target).extension().is_none() {
        for ext in ["md", "markdown", "txt", "rs", "py", "js", "ts"] {
            candidates.push(format!("{target}.{ext}"));
        }
        for tail in ["mod.rs", "__init__.py", "index.js"] {
            candidates.push(format!("{target}/{tail}"));
        }
    }

    for cand in &candidates {
        // Relative to the linking file first: `../utils` should mean the
        // neighbour, not something with the same name elsewhere.
        if let Some(dir) = from_dir {
            let joined = normalize(&dir.join(cand));
            if let Ok(rel) = joined.strip_prefix(root) {
                let key = rel.to_string_lossy().replace('\\', "/");
                if let Some(&id) = by_rel.get(&key) {
                    return Some(id);
                }
            }
        }
        if let Some(&id) = by_rel.get(cand.as_str()) {
            return Some(id);
        }
    }

    // Last resort: a bare name, matched against file stems. Only accepted
    // when exactly one file answers to it.
    let stem = Path::new(target)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(target)
        .to_ascii_lowercase();
    match by_stem.get(&stem) {
        Some(ids) if ids.len() == 1 => Some(ids[0]),
        _ => None,
    }
}

/// Resolve `.` and `..` without touching the filesystem.
///
/// Purely lexical, unlike `canonicalize`: link targets routinely name files
/// that do not exist, and a syscall per candidate per link would be slow. Note
/// that `pop` on an empty buffer is a no-op, so `../..` from the root cannot
/// escape upwards — it just flattens.
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
