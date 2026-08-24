//! The link graph: what in this project points at what.
//!
//! Two kinds of connection, built the same way and drawn together:
//!
//!   - **Notes.** `[[wikilinks]]` and relative markdown links, resolved
//!     against the files in the project.
//!   - **Code.** Imports, which are exact, and calls, which are matched by
//!     name: if one file calls `load_config` and another defines it, that is
//!     an edge. Definitions and references come from tree-sitter's tags
//!     queries, so this works the same way for every language that ships one.
//!
//! Call matching is a heuristic and behaves like one. A name defined in many
//! files is ambiguous — `new`, `main`, `get` — so past `max_ambiguity`
//! definitions a symbol stops producing edges rather than wiring the project
//! to itself.
//!
//! # How a graph gets built
//!
//! [`build`] is one pass over the project, then one pass over what it found:
//!
//! 1. **Walk.** `search::walk` lists the files — the same traversal search
//!    uses, so the two always agree on what the project contains.
//! 2. **Collect.** Each file becomes a [`Node`] plus a private `Facts` record:
//!    what it defines, what it calls, what it links to, what it imports.
//!    Notes are scanned for links; source is parsed for symbols and imports.
//! 3. **Index.** Two lookup tables are built — `by_rel` (path to node) and
//!    `by_stem` (bare filename to nodes) — plus `definers`, mapping each
//!    symbol to the files that define it.
//! 4. **Resolve.** Every recorded link, import and call is turned into an
//!    [`Edge`] where a target can be found. Identical edges are merged and
//!    counted, which is why the map is keyed by `(from, to, kind, label)`.
//!
//! # Exact edges and guessed ones
//!
//! Wikilinks, markdown links and imports are *resolved*: they name a target,
//! and either it exists or no edge is drawn. Call edges are *guessed*: they
//! match a called name against every file that defines that name, with no
//! scope analysis, no type information, and no import following.
//!
//! Three guards keep that heuristic honest, and all three matter:
//!
//! - `max_ambiguity` — a name defined in more than N files says nothing about
//!   which one was meant, so it produces no edge at all. Without this, `new`
//!   and `main` connect the entire project to itself.
//! - [`is_test_file`] and [`strip_tests`] — test helpers are named `fixture`,
//!   `render`, `setup`, and collide with real code everywhere.
//! - Only tags typed `call` become edges, and only definitions of callable
//!   things become definitions. A `mod x;` is a definition to tree-sitter but
//!   nothing ever calls it.
//!
//! When you see a wrong edge in the graph, one of those three is where to
//! look.
//!
//! # Adding a language
//!
//! Four steps: add the `tree-sitter-*` crate, add a `LazyLock<Option<...>>`
//! beside [`PYTHON`], add a [`Lang`] variant with its extensions in
//! [`lang_of`] and a [`tags_for`] arm, and add an arm to [`collect_imports`]
//! for its import syntax. Then add the name to [`supported_languages`], which
//! is what the graph view tells the user it can trace.
//!
//! A language with no tags query still appears in the graph as a node — it
//! just has no call edges. That is what [`is_sourcelike`] is for.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::search;

/// What a file is, which decides how it is scanned and how it is labelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Markdown. Scanned for both wikilinks and `[text](path)` links.
    Note,
    /// Other prose — `.txt`, `.rst`, `.org`. Wikilinks only; a `[a](b)` in a
    /// text file is not a link.
    Prose,
    /// Source. Parsed for symbols and imports where a grammar exists.
    Code,
    /// Everything else. Drawn as a node, never scanned.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeKind {
    /// `[[wikilink]]` between notes.
    Wikilink,
    /// A relative markdown link, `[text](../notes/x.md)`.
    Link,
    /// One source file importing another.
    Import,
    /// One source file calling a function another defines.
    Call,
}

/// One file in the graph. Nodes are identified by their index into
/// `Graph::nodes`, which is what [`Edge::from`] and [`Edge::to`] hold.
#[derive(Debug, Clone)]
pub struct Node {
    /// Path relative to the project root, which is also its display name.
    /// Always uses forward slashes, including on Windows, so link targets
    /// written in notes resolve the same way everywhere.
    pub rel: String,
    /// Just the filename, used for the label drawn beside the dot.
    pub name: String,
    pub kind: NodeKind,
    /// Symbols this file defines, for the tooltip.
    pub defines: Vec<String>,
    pub path: PathBuf,
}

/// A connection between two files. Directed: `from` points at `to`.
#[derive(Debug, Clone)]
pub struct Edge {
    /// Index into `Graph::nodes`.
    pub from: usize,
    /// Index into `Graph::nodes`.
    pub to: usize,
    pub kind: EdgeKind,
    /// What connects them: the link target, the module, or the symbol.
    pub label: String,
    /// How many times, so a heavily-used call reads as a stronger line.
    pub count: usize,
}

/// The finished graph. Immutable once built — `GraphView` filters what is
/// drawn without ever changing this.
#[derive(Debug, Clone)]
pub struct Graph {
    /// Every file in the project, in walk order. Index is identity.
    pub nodes: Vec<Node>,
    /// Merged and counted connections.
    pub edges: Vec<Edge>,
    /// Files that were unreachable from anything and reached nothing.
    pub orphans: usize,
    /// Languages whose calls tiny can follow, so the view can say so rather
    /// than leaving you wondering why your Go files have no edges.
    pub languages: Vec<String>,
}

/// Build settings, derived from the live config by `App::graph_options` so a
/// `:set` is reflected the next time the graph is opened.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory names never walked into. Shared with search.
    pub ignore: Vec<String>,
    pub show_hidden: bool,
    /// Extensions treated as [`NodeKind::Prose`].
    pub prose_extensions: Vec<String>,
    /// A symbol defined in more files than this is too ambiguous to link.
    pub max_ambiguity: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ignore: [".git", "target", "node_modules", ".venv", "__pycache__"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            show_hidden: false,
            prose_extensions: ["md", "txt", "rst", "org"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_ambiguity: 3,
        }
    }
}

/// Source files past this size are not parsed for symbols.
const MAX_PARSE_BYTES: u64 = 1024 * 1024;

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
/// [`collect_imports`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Python,
    Rust,
    JavaScript,
}

/// Map a file extension to a parseable language, or `None` for everything
/// else. TypeScript is absent deliberately — it needs its own grammar, and
/// falling back to the JavaScript one mis-parses type annotations.
fn lang_of(path: &Path) -> Option<Lang> {
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
fn tags_for(lang: Lang) -> Option<&'static TagsConfiguration> {
    match lang {
        Lang::Python => PYTHON.as_ref(),
        Lang::Rust => RUST.as_ref(),
        Lang::JavaScript => JAVASCRIPT.as_ref(),
    }
}

/// Languages tiny can trace calls through, for the "what is supported" line.
pub fn supported_languages() -> &'static [&'static str] {
    &["Python", "Rust", "JavaScript"]
}

// ---- one file's contribution ----------------------------------------------

/// What one file contributes to the graph, before anything is resolved.
///
/// Kept separate from [`Node`] because it is scratch: the resolution pass
/// consumes it and only `defines` survives onto the node, for the tooltip.
#[derive(Debug, Default)]
struct Facts {
    /// Symbols defined here.
    defines: BTreeSet<String>,
    /// Symbols called here, with how many times.
    calls: BTreeMap<String, usize>,
    /// Wikilink and markdown-link targets, as written.
    links: Vec<(String, EdgeKind)>,
    /// Import targets, as written in the source.
    imports: Vec<String>,
}

/// Build the whole graph for a project. See the module docs for the four
/// phases; the code below follows them in order.
///
/// Cost is one read and one parse per file, so this is the single most
/// expensive thing tiny does. It runs synchronously when you press `w`, which
/// is why [`MAX_PARSE_BYTES`] and the `ignore` list exist.
///
/// Edges accumulate into a `BTreeMap` keyed by `(from, to, kind, label)` so
/// repeats merge into a count rather than stacking up as duplicate lines —
/// and so the output order is deterministic, which keeps the layout stable
/// between runs.
pub fn build(root: &Path, opts: &Options) -> Graph {
    let walk_opts = search::Opts {
        max_results: usize::MAX,
        ignore: opts.ignore.clone(),
        show_hidden: opts.show_hidden,
    };
    let files = search::walk(root, &walk_opts);

    let mut nodes: Vec<Node> = Vec::new();
    let mut facts: Vec<Facts> = Vec::new();
    let mut by_rel: HashMap<String, usize> = HashMap::new();
    // Several files can share a stem (`mod.rs`, `index.js`), so this is a list.
    let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();

    let mut ctx = TagsContext::new();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel.clone());
        let kind = node_kind(&path, &opts.prose_extensions);

        let mut f = Facts::default();
        if let Some(text) = read_text(&path) {
            match kind {
                NodeKind::Note | NodeKind::Prose => collect_links(&text, kind, &mut f),
                NodeKind::Code => {
                    if let Some(lang) = lang_of(&path)
                        && !is_test_file(&path)
                    {
                        let body = strip_tests(lang, &text);
                        collect_symbols(&mut ctx, lang, body, &mut f);
                        collect_imports(lang, body, &mut f);
                    }
                }
                NodeKind::Other => {}
            }
        }

        let id = nodes.len();
        by_rel.insert(rel.clone(), id);
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            by_stem
                .entry(stem.to_ascii_lowercase())
                .or_default()
                .push(id);
        }
        nodes.push(Node {
            rel,
            name,
            kind,
            defines: f.defines.iter().cloned().collect(),
            path,
        });
        facts.push(f);
    }

    // Which files define each symbol.
    let mut definers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (id, f) in facts.iter().enumerate() {
        for sym in &f.defines {
            definers.entry(sym.as_str()).or_default().push(id);
        }
    }

    let mut edges: BTreeMap<(usize, usize, EdgeKind, String), usize> = BTreeMap::new();
    for (from, f) in facts.iter().enumerate() {
        let from_dir = nodes[from].path.parent().map(Path::to_path_buf);

        for (target, kind) in &f.links {
            if let Some(to) = resolve_link(target, root, from_dir.as_deref(), &by_rel, &by_stem)
                && to != from
            {
                *edges.entry((from, to, *kind, target.clone())).or_insert(0) += 1;
            }
        }

        for target in &f.imports {
            if let Some(to) = resolve_link(target, root, from_dir.as_deref(), &by_rel, &by_stem)
                && to != from
            {
                *edges
                    .entry((from, to, EdgeKind::Import, target.clone()))
                    .or_insert(0) += 1;
            }
        }

        for (sym, count) in &f.calls {
            let Some(defs) = definers.get(sym.as_str()) else {
                continue;
            };
            // Too many definitions means the name says nothing about which
            // file was meant, so it produces no edge at all.
            if defs.len() > opts.max_ambiguity {
                continue;
            }
            for &to in defs {
                if to == from {
                    continue; // calling your own function is not a link
                }
                *edges
                    .entry((from, to, EdgeKind::Call, sym.clone()))
                    .or_insert(0) += count;
            }
        }
    }

    let edges: Vec<Edge> = edges
        .into_iter()
        .map(|((from, to, kind, label), count)| Edge {
            from,
            to,
            kind,
            label,
            count,
        })
        .collect();

    let mut connected = vec![false; nodes.len()];
    for e in &edges {
        connected[e.from] = true;
        connected[e.to] = true;
    }
    let orphans = connected.iter().filter(|c| !**c).count();

    Graph {
        nodes,
        edges,
        orphans,
        languages: supported_languages()
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Classify a file. Order matters: markdown is checked before the prose list
/// because it is usually *in* that list but has a link syntax of its own.
fn node_kind(path: &Path, prose_exts: &[String]) -> NodeKind {
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
fn is_sourcelike(ext: &str) -> bool {
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
fn read_text(path: &Path) -> Option<String> {
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
fn collect_links(text: &str, kind: NodeKind, f: &mut Facts) {
    for target in crate::markdown::wikilinks(text) {
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
fn is_test_file(path: &Path) -> bool {
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
fn strip_tests(lang: Lang, text: &str) -> &str {
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
fn collect_symbols(ctx: &mut TagsContext, lang: Lang, text: &str, f: &mut Facts) {
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

/// Whether a string is a bare identifier. Guards the import scanners against
/// picking up braces, wildcards and other syntax as a module name.
fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Imports, read line by line. Exact where it matters and quiet where it is
/// not sure — an unresolvable target simply produces no edge.
///
/// Text scanning rather than tree-sitter, because import *statements* are
/// trivially recognisable per language while extracting a usable path from the
/// parse tree differs wildly between grammars. The cost is that an import
/// inside a comment or a string counts; the benefit is about thirty lines
/// instead of three more queries.
///
/// Each language keeps only what can name a file in this project:
///
/// - **Python** — the module path from `import a.b` or `from a.b import c`,
///   with dots turned into slashes and leading relative dots stripped.
/// - **Rust** — `mod name;` declares the file, `use crate::name` reaches into
///   it. Only the first path segment after the prefix is taken, since that is
///   the part that names a sibling module.
/// - **JavaScript** — only relative specifiers. A bare `from 'react'` is a
///   package, not a file here.
fn collect_imports(lang: Lang, text: &str, f: &mut Facts) {
    for raw in text.lines() {
        let line = raw.trim();
        match lang {
            Lang::Python => {
                // `import a.b` / `from a.b import c`
                let module = if let Some(rest) = line.strip_prefix("from ") {
                    rest.split_whitespace().next()
                } else if let Some(rest) = line.strip_prefix("import ") {
                    rest.split([',', ' ']).next()
                } else {
                    None
                };
                if let Some(m) = module {
                    let m = m.trim_start_matches('.');
                    if !m.is_empty() {
                        f.imports.push(m.replace('.', "/"));
                    }
                }
            }
            Lang::Rust => {
                let rest = line.strip_prefix("pub ").unwrap_or(line);
                // `mod name;` declares the file; `use crate::name` reaches into
                // it. Both name a sibling, and most files only do the second.
                if let Some(rest) = rest.strip_prefix("mod ")
                    && let Some(name) = rest.strip_suffix(';')
                {
                    if is_ident(name.trim()) {
                        f.imports.push(name.trim().to_string());
                    }
                } else if let Some(rest) = rest.strip_prefix("use ") {
                    for prefix in ["crate::", "super::", "self::"] {
                        if let Some(tail) = rest.strip_prefix(prefix) {
                            let first = tail
                                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                                .next()
                                .unwrap_or("");
                            if is_ident(first) {
                                f.imports.push(first.to_string());
                            }
                            break;
                        }
                    }
                }
            }
            Lang::JavaScript => {
                // Only relative specifiers name a file in this project.
                for marker in ["from ", "require("] {
                    let Some(at) = line.find(marker) else {
                        continue;
                    };
                    let rest = &line[at + marker.len()..];
                    let rest = rest.trim_start();
                    let Some(quote) = rest.chars().next() else {
                        continue;
                    };
                    if quote != '\'' && quote != '"' {
                        continue;
                    }
                    if let Some(end) = rest[1..].find(quote) {
                        let spec = &rest[1..1 + end];
                        if spec.starts_with('.') {
                            f.imports.push(spec.to_string());
                        }
                    }
                }
            }
        }
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
fn resolve_link(
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
fn normalize(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn graph(td: &tempfile::TempDir) -> Graph {
        build(td.path(), &Options::default())
    }

    /// Edges as `("from", "to", kind, "label")`, for readable assertions.
    fn edges(g: &Graph) -> Vec<(String, String, EdgeKind, String)> {
        g.edges
            .iter()
            .map(|e| {
                (
                    g.nodes[e.from].rel.clone(),
                    g.nodes[e.to].rel.clone(),
                    e.kind,
                    e.label.clone(),
                )
            })
            .collect()
    }

    fn has(g: &Graph, from: &str, to: &str, kind: EdgeKind) -> bool {
        edges(g)
            .iter()
            .any(|(f, t, k, _)| f == from && t == to && *k == kind)
    }

    // ---- notes ------------------------------------------------------------

    #[test]
    fn a_wikilink_connects_two_notes() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "notes/design.md",
            "see [[architecture]] for why\n",
        );
        write(td.path(), "notes/architecture.md", "# Architecture\n");
        let g = graph(&td);
        assert!(
            has(
                &g,
                "notes/design.md",
                "notes/architecture.md",
                EdgeKind::Wikilink
            ),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_wikilink_finds_a_note_in_another_folder_by_name() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a/one.md", "[[deep]]\n");
        write(td.path(), "b/c/deep.md", "hi\n");
        let g = graph(&td);
        assert!(has(&g, "a/one.md", "b/c/deep.md", EdgeKind::Wikilink));
    }

    #[test]
    fn an_ambiguous_wikilink_links_to_nothing() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "start.md", "[[notes]]\n");
        write(td.path(), "a/notes.md", "one\n");
        write(td.path(), "b/notes.md", "two\n");
        let g = graph(&td);
        assert!(
            g.edges.is_empty(),
            "two files answer to that name, so it means nothing: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_relative_markdown_link_is_an_edge_but_a_url_is_not() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "docs/index.md",
            "[spec](../notes/spec.md) and [site](https://example.com)\n",
        );
        write(td.path(), "notes/spec.md", "# Spec\n");
        let g = graph(&td);
        assert_eq!(edges(&g).len(), 1, "{:?}", edges(&g));
        assert!(has(&g, "docs/index.md", "notes/spec.md", EdgeKind::Link));
    }

    #[test]
    fn a_relative_link_prefers_the_neighbouring_file() {
        let td = tempfile::tempdir().unwrap();
        // Two files share a stem; the relative path has to win.
        write(td.path(), "a/index.md", "[x](./target.md)\n");
        write(td.path(), "a/target.md", "the right one\n");
        write(td.path(), "b/target.md", "the wrong one\n");
        let g = graph(&td);
        assert!(has(&g, "a/index.md", "a/target.md", EdgeKind::Link));
        assert!(!has(&g, "a/index.md", "b/target.md", EdgeKind::Link));
    }

    #[test]
    fn wikilinks_work_in_plain_text_too() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "log.txt", "today: see [[design]]\n");
        write(td.path(), "design.md", "# Design\n");
        let g = graph(&td);
        assert!(has(&g, "log.txt", "design.md", EdgeKind::Wikilink));
    }

    // ---- python -----------------------------------------------------------

    #[test]
    fn python_imports_and_calls_both_become_edges() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "utils.py",
            "def load_config(path):\n    return {}\n\ndef parse(raw):\n    return []\n",
        );
        write(
            td.path(),
            "main.py",
            "import utils\n\ndef main():\n    cfg = utils.load_config('x')\n    return utils.parse(cfg)\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "main.py", "utils.py", EdgeKind::Import),
            "import edge missing: {:?}",
            edges(&g)
        );
        let calls: Vec<String> = g
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Call)
            .map(|e| e.label.clone())
            .collect();
        assert!(calls.contains(&"load_config".to_string()), "{calls:?}");
        assert!(calls.contains(&"parse".to_string()), "{calls:?}");
    }

    #[test]
    fn a_file_calling_its_own_function_is_not_an_edge() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "solo.py",
            "def helper():\n    return 1\n\ndef main():\n    return helper()\n",
        );
        let g = graph(&td);
        assert!(g.edges.is_empty(), "{:?}", edges(&g));
        assert_eq!(g.orphans, 1);
    }

    #[test]
    fn from_imports_resolve_through_a_package_path() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "pkg/core.py", "def run():\n    pass\n");
        write(td.path(), "app.py", "from pkg.core import run\nrun()\n");
        let g = graph(&td);
        assert!(
            has(&g, "app.py", "pkg/core.py", EdgeKind::Import),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_call_count_is_carried_on_the_edge() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "lib.py", "def tick():\n    pass\n");
        write(
            td.path(),
            "run.py",
            "import lib\nlib.tick()\nlib.tick()\nlib.tick()\n",
        );
        let g = graph(&td);
        let call = g
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Call)
            .expect("a call edge");
        assert_eq!(call.count, 3, "three calls, one edge");
    }

    // ---- rust and javascript ---------------------------------------------

    #[test]
    fn rust_mod_declarations_and_calls_become_edges() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "src/main.rs",
            "mod helpers;\n\nfn main() {\n    let _ = helpers::compute();\n}\n",
        );
        write(
            td.path(),
            "src/helpers.rs",
            "pub fn compute() -> i32 {\n    1\n}\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "src/main.rs", "src/helpers.rs", EdgeKind::Import),
            "mod edge missing: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn javascript_relative_imports_become_edges() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "lib.js",
            "export function greet() { return 1; }\n",
        );
        write(
            td.path(),
            "app.js",
            "import { greet } from './lib.js';\ngreet();\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "app.js", "lib.js", EdgeKind::Import),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_bare_package_import_is_not_an_edge() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "app.js", "import React from 'react';\n");
        write(td.path(), "react.js", "// not the real one\n");
        let g = graph(&td);
        assert!(
            !has(&g, "app.js", "react.js", EdgeKind::Import),
            "a bare specifier means node_modules, not this file"
        );
    }

    // ---- keeping test code out of the picture ----------------------------

    #[test]
    fn a_rust_test_module_does_not_shadow_names_across_the_project() {
        let td = tempfile::tempdir().unwrap();
        // `render` is a real function in one file and a test helper in another.
        write(
            td.path(),
            "src/render.rs",
            "pub fn render() -> u8 {\n    1\n}\n",
        );
        write(
            td.path(),
            "src/media.rs",
            "pub fn draw() {}\n\n#[cfg(test)]\nmod tests {\n    fn render() -> u8 { 2 }\n    #[test]\n    fn t() { let _ = render(); }\n}\n",
        );
        let g = graph(&td);
        assert!(
            !has(&g, "src/media.rs", "src/render.rs", EdgeKind::Call),
            "a call inside `#[cfg(test)]` is not project structure: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_test_file_contributes_nothing_to_the_graph() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "core.py", "def compute():\n    return 1\n");
        write(td.path(), "test_core.py", "import core\ncompute()\n");
        let g = graph(&td);
        assert!(
            g.edges.is_empty(),
            "a test file is about the code, not part of it: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_module_declaration_is_not_something_you_can_call() {
        let td = tempfile::tempdir().unwrap();
        // main.rs declares `mod parser;`. A call to `parser()` elsewhere must
        // not resolve to that declaration.
        write(td.path(), "src/main.rs", "mod parser;\n\nfn main() {}\n");
        write(td.path(), "src/parser.rs", "pub fn parse() {}\n");
        write(
            td.path(),
            "src/other.rs",
            "fn go() {\n    let _ = parser();\n}\n",
        );
        let g = graph(&td);
        assert!(
            !has(&g, "src/other.rs", "src/main.rs", EdgeKind::Call),
            "`mod parser;` is a declaration, not a function: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn rust_use_statements_are_import_edges_too() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "src/main.rs",
            "mod app;\nmod search;\nfn main() {}\n",
        );
        write(
            td.path(),
            "src/search.rs",
            "pub fn count() -> u8 {\n    0\n}\n",
        );
        write(
            td.path(),
            "src/app.rs",
            "use crate::search::{self, Foo};\n\npub fn go() {\n    let _ = search::count();\n}\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "src/app.rs", "src/search.rs", EdgeKind::Import),
            "`use crate::search` names a sibling: {:?}",
            edges(&g)
        );
    }

    // ---- the ambiguity guard ---------------------------------------------

    #[test]
    fn a_name_defined_everywhere_stops_producing_edges() {
        let td = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(td.path(), &format!("m{i}.py"), "def run():\n    pass\n");
        }
        write(td.path(), "caller.py", "run()\n");
        let g = graph(&td);
        assert!(
            !g.edges.iter().any(|e| e.label == "run"),
            "`run` is defined five times, so it names nothing: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_name_defined_twice_still_links_to_both() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a.py", "def once():\n    pass\n");
        write(td.path(), "b.py", "def once():\n    pass\n");
        write(td.path(), "caller.py", "once()\n");
        let g = graph(&td);
        assert!(has(&g, "caller.py", "a.py", EdgeKind::Call));
        assert!(has(&g, "caller.py", "b.py", EdgeKind::Call));
    }

    // ---- shape ------------------------------------------------------------

    #[test]
    fn every_file_becomes_a_node_and_unconnected_ones_are_counted() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a.md", "[[b]]\n");
        write(td.path(), "b.md", "hi\n");
        write(td.path(), "lonely.md", "nothing here\n");
        fs::write(td.path().join("picture.png"), [0x89u8, b'P', b'N', b'G']).unwrap();
        let g = graph(&td);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.orphans, 2, "lonely.md and picture.png");
        assert_eq!(
            g.nodes
                .iter()
                .find(|n| n.rel == "picture.png")
                .unwrap()
                .kind,
            NodeKind::Other
        );
        assert_eq!(
            g.nodes.iter().find(|n| n.rel == "a.md").unwrap().kind,
            NodeKind::Note
        );
    }

    #[test]
    fn definitions_are_listed_on_the_node() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "lib.py",
            "def alpha():\n    pass\n\ndef beta():\n    pass\n",
        );
        let g = graph(&td);
        let node = &g.nodes[0];
        assert!(
            node.defines.contains(&"alpha".to_string()),
            "{:?}",
            node.defines
        );
        assert!(node.defines.contains(&"beta".to_string()));
    }

    #[test]
    fn ignored_folders_never_reach_the_graph() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a.md", "hello\n");
        write(td.path(), "node_modules/dep.js", "export function x() {}\n");
        write(td.path(), "target/build.rs", "fn main() {}\n");
        let g = graph(&td);
        assert_eq!(
            g.nodes.len(),
            1,
            "{:?}",
            g.nodes.iter().map(|n| &n.rel).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_empty_project_produces_an_empty_graph() {
        let td = tempfile::tempdir().unwrap();
        let g = graph(&td);
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
        assert_eq!(g.orphans, 0);
    }
}
