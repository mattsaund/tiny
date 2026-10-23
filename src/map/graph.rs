//! The link graph: what in this project points at what.
//!
//! Two kinds of connection, built the same way and drawn together:
//!
//!   - **Notes.** `[[wikilinks]]` and relative markdown links, resolved
//!     against the files in the project.
//!   - **Code.** Calls, matched by name: if one file calls `load_config` and
//!     another defines it, that is an edge. Definitions and references come
//!     from tree-sitter's tags queries, so this works the same way for every
//!     language that ships one.
//!
//! Both are connections you can point at in the text. An `import` on its own
//! is not: it says a file is *available*, not that anything in it is used, and
//! wiring every module to every module it merely mentions is what turned this
//! picture into a hairball. Import a module and call nothing from it and the
//! map stays quiet.
//!
//! Call matching is a heuristic and behaves like one. A name defined in many
//! files is ambiguous (`new`, `main`, `get`) so past `max_ambiguity`
//! definitions a symbol stops producing edges rather than wiring the project
//! to itself.
//!
//! # How a graph gets built
//!
//! [`build`] is one pass over the project, then one pass over what it found:
//!
//! 1. **Walk.** `search::walk` lists the files: the same traversal search
//!    uses, so the two always agree on what the project contains.
//! 2. **Collect.** Each file becomes a [`Node`] plus a private `Facts` record:
//!    what it defines, what it calls, and what it links to. Notes are
//!    scanned for links; source is parsed for symbols. Files are independent
//!    of each other here, so this runs on every core (see `parallel`).
//! 3. **Index.** Two lookup tables are built: `by_rel` (path to node) and
//!    `by_stem` (bare filename to nodes), plus `definers`, mapping each
//!    symbol to the files that define it.
//! 4. **Resolve.** Every recorded link and call is turned into an
//!    [`Edge`] where a target can be found, per file, on every core again.
//!    Identical edges are merged and counted, which is why the map is keyed
//!    by `(from, to, kind, label)`.
//!
//! # Exact edges and guessed ones
//!
//! Wikilinks and markdown links are *resolved*: they name a target, and
//! either it exists or no edge is drawn. Call edges are *guessed*: they
//! match a called name against every file that defines that name, with no
//! scope analysis, no type information, and no import following.
//!
//! Three guards keep that heuristic honest, and all three matter:
//!
//! - `max_ambiguity`: a name defined in more than N files says nothing about
//!   which one was meant, so it produces no edge at all. Without this, `new`
//!   and `main` connect the entire project to itself.
//! - [`is_test_file`](super::scan::is_test_file) and
//!   [`strip_tests`](super::scan::strip_tests): test helpers are named
//!   `fixture`, `render`, `setup`, and collide with real code everywhere.
//! - Only tags typed `call` become edges, and only definitions of callable
//!   things become definitions. A `mod x;` is a definition to tree-sitter but
//!   nothing ever calls it.
//!
//! When you see a wrong edge in the graph, one of those three is where to
//! look.
//!
//! # Adding a language
//!
//! Three steps: add the `tree-sitter-*` crate, add a `LazyLock<Option<...>>`
//! beside `scan::PYTHON`, and add a `scan::Lang` variant with its extensions in
//! `scan::lang_of` and a `scan::tags_for` arm. Nothing on screen lists the
//! languages, so there is no fourth place to remember.
//!
//! A language with no tags query still appears in the graph as a node; it
//! just has no call edges. That is what `scan::is_sourcelike` is for.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::scan::{Facts, Index, facts_of, node_kind, resolve_link};
use tree_sitter_tags::TagsContext;

use crate::parallel::par_map;
use crate::text::search;

/// What a file is, which decides how it is scanned and how it is labeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Markdown. Scanned for both wikilinks and `[text](path)` links.
    Note,
    /// Other prose: `.txt`, `.rst`, `.org`. Wikilinks only; a `[a](b)` in a
    /// text file is not a link.
    Prose,
    /// Source. Parsed for symbols where a grammar exists.
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
    /// What connects them: the link target as written, or the symbol called.
    pub label: String,
    /// How many times, so a heavily-used call reads as a stronger line.
    pub count: usize,
}

/// The finished graph. Immutable once built: `ProjectMap` filters what is
/// drawn without ever changing this.
#[derive(Debug, Clone)]
pub struct Graph {
    /// Every file in the project, in walk order. Index is identity.
    pub nodes: Vec<Node>,
    /// Merged and counted connections.
    pub edges: Vec<Edge>,
    /// Whether the project had more files than [`MAX_FILES`], and the map
    /// is of the first of them.
    pub truncated: bool,
}

/// How many files may define a name before a call to it links to nothing.
///
/// Three. It was a setting, and a setting is a question; this one asked the
/// reader to tune a heuristic they cannot see the workings of, and every
/// answer but a small number makes the map worse. A name defined in four
/// places is not evidence about any of them.
pub const MAX_AMBIGUITY: usize = 3;

/// The most files the map reads. A project that big is a picture nobody can
/// read either, and the map is built while you wait: opened on a home folder
/// it would read everything in it first.
pub const MAX_FILES: usize = 10_000;

/// Build settings, derived from the live config by `App::graph_options` so a
/// `*set` is reflected the next time the map is opened.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory names never walked into. Shared with search.
    pub ignore: Vec<String>,
    pub show_hidden: bool,
    /// Extensions treated as [`NodeKind::Prose`].
    pub prose_extensions: Vec<String>,
}

impl Options {
    /// The settings a config asks for.
    ///
    /// Logs are left out of prose here, though they read as prose: one is
    /// rarely where a link is, and often the biggest file in a folder, read
    /// whole on every open of the map for a `[[` it does not have.
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            ignore: config.search_ignore.clone(),
            show_hidden: config.show_hidden,
            prose_extensions: config
                .prose_extensions
                .iter()
                .filter(|ext| !ext.eq_ignore_ascii_case("log"))
                .cloned()
                .collect(),
        }
    }
}

/// What the tests build with: the shipped config's settings, so a test map
/// is the one a user gets.
#[cfg(test)]
impl Default for Options {
    fn default() -> Self {
        Self::from_config(&crate::config::Config::default())
    }
}

/// Build the whole graph for a project. See the module docs for the four
/// phases; the code below follows them in order.
///
/// Cost is one read and one parse per file that has anything to find in it,
/// so this is the single most expensive thing tiny does. It runs
/// synchronously when the map is opened, which is why it is spread over
/// every core, and why `scan::MAX_PARSE_BYTES`, [`MAX_FILES`] and the
/// `ignore` list exist.
///
/// Edges accumulate into a `BTreeMap` keyed by `(from, to, kind, label)` so
/// repeats merge into a count rather than stacking up as duplicate lines,
/// and so the output order is deterministic, which keeps the layout stable
/// between runs however the work was shared out.
pub fn build(root: &Path, opts: &Options) -> Graph {
    let walk_opts = search::Opts {
        max_results: usize::MAX,
        ignore: opts.ignore.clone(),
        show_hidden: opts.show_hidden,
    };
    let walked = search::walk_up_to(root, &walk_opts, MAX_FILES);
    let files = walked.files;
    let rels: Vec<String> = files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    let kinds: Vec<NodeKind> = files
        .iter()
        .map(|path| node_kind(path, &opts.prose_extensions))
        .collect();
    let mut facts: Vec<Facts> = par_map(&files, TagsContext::new, |ctx, i, path| {
        facts_of(ctx, path, &rels[i], kinds[i])
    });
    forget_test_modules(&rels, &mut facts);

    let mut nodes: Vec<Node> = Vec::with_capacity(files.len());
    let mut index = Index {
        by_rel: HashMap::with_capacity(files.len()),
        by_name: HashMap::new(),
        rels: rels.iter().map(|r| r.to_lowercase()).collect(),
    };
    for (((path, rel), kind), f) in files.into_iter().zip(rels).zip(kinds).zip(&facts) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel.clone());
        let id = nodes.len();
        index.by_rel.insert(rel.clone(), id);
        // Several files can share a name (`mod.rs`, `index.md`), so this is
        // a list.
        index
            .by_name
            .entry(name.to_lowercase())
            .or_default()
            .push(id);
        nodes.push(Node {
            rel,
            name,
            kind,
            defines: f.defines.iter().cloned().collect(),
            path,
        });
    }

    // Which files define each symbol.
    let mut definers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (id, f) in facts.iter().enumerate() {
        for sym in &f.defines {
            definers.entry(sym.as_str()).or_default().push(id);
        }
    }

    // Each file's edges on their own, then all of them merged.
    let found = par_map(
        &facts,
        || (),
        |_, from, f| {
            let mut out: Vec<(EdgeKind, usize, String, usize)> = Vec::new();
            let from_dir = nodes[from].path.parent();

            for (target, kind) in &f.links {
                if let Some(to) = resolve_link(target, *kind, root, from_dir, &index)
                    && to != from
                {
                    out.push((*kind, to, target.clone(), 1));
                }
            }

            for (sym, call) in &f.calls {
                // A bare call to a name the file defines itself is a call to
                // its own; only one that says where it goes can mean another.
                if !call.qualified && f.defines.contains(sym) {
                    continue;
                }
                let Some(defs) = definers.get(sym.as_str()) else {
                    continue;
                };
                // Too many definitions means the name says nothing about which
                // file was meant, so it produces no edge at all.
                if defs.len() > MAX_AMBIGUITY {
                    continue;
                }
                for &to in defs {
                    if to == from {
                        continue; // calling your own function is not a link
                    }
                    out.push((EdgeKind::Call, to, sym.clone(), call.count));
                }
            }
            out
        },
    );

    let mut edges: BTreeMap<(usize, usize, EdgeKind, String), usize> = BTreeMap::new();
    for (from, out) in found.into_iter().enumerate() {
        for (kind, to, label, count) in out {
            *edges.entry((from, to, kind, label)).or_insert(0) += count;
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

    Graph {
        nodes,
        edges,
        truncated: walked.truncated,
    }
}

/// Take what the test modules a file declares in files of their own say out
/// of the picture: `#[cfg(test)] mod testing;` in `map/mod.rs` makes
/// `map/testing.rs` a test, and `#[cfg(test)] mod tests;` in `app/mod.rs`
/// makes every file under `app/tests/` one.
fn forget_test_modules(rels: &[String], facts: &mut [Facts]) {
    let mut tests: Vec<String> = Vec::new();
    for (rel, f) in rels.iter().zip(facts.iter()) {
        // A module's children live beside `mod.rs`, `lib.rs` and `main.rs`,
        // and in a folder named after any other file.
        let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel));
        let stem = file.strip_suffix(".rs").unwrap_or(file);
        let parent = match (dir, stem) {
            (_, "mod" | "lib" | "main") => dir.to_string(),
            ("", stem) => stem.to_string(),
            (dir, stem) => format!("{dir}/{stem}"),
        };
        for name in &f.test_modules {
            let at = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            tests.push(at);
        }
    }
    if tests.is_empty() {
        return;
    }
    for (rel, f) in rels.iter().zip(facts.iter_mut()) {
        let rel = rel.strip_suffix(".rs").unwrap_or(rel);
        let is_test = tests.iter().any(|t| {
            rel == t
                || rel
                    .strip_prefix(t.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        if is_test {
            *f = Facts::default();
        }
    }
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
    fn a_relative_link_prefers_the_neighboring_file() {
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
    fn a_call_becomes_an_edge() {
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
    fn importing_a_module_without_calling_it_is_not_an_edge() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "utils.py",
            "def load_config(path):\n    return {}\n",
        );
        // The import says `utils` is available. Nothing here uses it, and
        // "available" is not a connection worth a line on the map.
        write(
            td.path(),
            "main.py",
            "import utils\n\ndef main():\n    return 1\n",
        );
        let g = graph(&td);
        assert!(g.edges.is_empty(), "{:?}", edges(&g));
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
    }

    #[test]
    fn a_call_reaches_into_a_package() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "pkg/core.py", "def run():\n    pass\n");
        write(td.path(), "app.py", "from pkg.core import run\nrun()\n");
        let g = graph(&td);
        assert!(
            has(&g, "app.py", "pkg/core.py", EdgeKind::Call),
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
    fn a_rust_call_through_a_module_path_is_an_edge() {
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
            has(&g, "src/main.rs", "src/helpers.rs", EdgeKind::Call),
            "`mod helpers` alone would not do it; calling `compute` does: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_javascript_call_across_a_relative_import_is_an_edge() {
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
            has(&g, "app.js", "lib.js", EdgeKind::Call),
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
            g.edges.is_empty(),
            "a bare specifier means node_modules, not this file: {:?}",
            edges(&g)
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
    fn a_use_statement_is_not_an_edge_but_the_call_through_it_is() {
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
            "use crate::text::search::{self, Foo};\n\npub fn go() {\n    let _ = search::count();\n}\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "src/app.rs", "src/search.rs", EdgeKind::Call),
            "`use crate::text::search` only points the way; `search::count()` is the link: {:?}",
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
    fn a_test_module_declared_near_the_top_does_not_take_the_file_with_it() {
        // `#[cfg(test)] mod tests;` sat above everything in the file, and
        // the file was cut there, so none of its calls were seen.
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "src/app.rs",
            "//! The app.\n#[cfg(test)]\nmod tests;\n\npub fn run() {\n    let _ = helpers::compute();\n}\n",
        );
        write(
            td.path(),
            "src/helpers.rs",
            "pub fn compute() -> i32 {\n    1\n}\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "src/app.rs", "src/helpers.rs", EdgeKind::Call),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn only_the_test_item_is_taken_out_wherever_it_is() {
        let text = "fn a() {}\n#[cfg(test)]\nfn helper() { let s = \"}\"; let c = '{'; }\nfn b() { real(); }\n// #[cfg(test)] in a comment\n";
        let (body, modules) = super::super::scan::strip_tests(super::super::scan::Lang::Rust, text);
        assert!(
            body.contains("fn a()") && body.contains("fn b() { real(); }"),
            "{body}"
        );
        assert!(!body.contains("helper"), "{body}");
        assert!(modules.is_empty());
        assert_eq!(
            body.lines().count(),
            text.lines().count(),
            "lines keep their numbers"
        );
    }

    #[test]
    fn a_wide_character_beside_a_call_or_in_a_char_is_read_past() {
        use super::super::scan::{Facts, Lang, collect_path_calls, strip_tests};
        // Each of these once cut a character in half and took tiny down.
        for text in [
            "// \u{8c03}\u{7528}Config::load()\n",
            "/// \u{2018}Config::load()\u{2019}\n",
            "let g = Gr\u{f6}\u{df}e::new();\n",
        ] {
            let mut f = Facts::default();
            collect_path_calls(text, &mut f);
            assert!(
                f.calls.contains_key("load") || f.calls.contains_key("new"),
                "{text}"
            );
        }
        let mut f = Facts::default();
        collect_path_calls("x \u{2018}Config::load()", &mut f);
        assert!(f.calls.contains_key("Config"), "the type is still credited");
        for text in [
            "#[cfg(test)]\nmod t { const C: char = '\\\u{e9}'; }\nfn real() {}\n",
            "#[cfg(test)]\nmod t { const C: char = '\\",
        ] {
            let (body, _) = strip_tests(Lang::Rust, text);
            assert!(!body.contains("const C"), "{body}");
        }
    }

    #[test]
    fn a_test_module_in_a_file_of_its_own_is_a_test() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "src/map/mod.rs",
            "#[cfg(test)]\nmod testing;\npub fn draw() {}\n",
        );
        write(td.path(), "src/map/testing.rs", "pub fn write() {}\n");
        write(
            td.path(),
            "src/app.rs",
            "fn go() {\n    write();\n}\npub fn write() {}\n",
        );
        let g = graph(&td);
        assert!(
            !has(&g, "src/app.rs", "src/map/testing.rs", EdgeKind::Call),
            "a test helper is not where a real call goes: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_project_inside_a_folder_called_tests_is_not_all_tests() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("tests").join("app");
        write(&root, "lib.py", "def compute():\n    return 1\n");
        write(&root, "main.py", "compute()\n");
        let g = build(&root, &Options::default());
        assert!(
            has(&g, "main.py", "lib.py", EdgeKind::Call),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_wikilink_to_a_heading_links_the_note() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "a.md",
            "[[design#Goals|the goals]] and [[design#^block]]\n",
        );
        write(td.path(), "design.md", "# Goals\n");
        let g = graph(&td);
        assert!(
            has(&g, "a.md", "design.md", EdgeKind::Wikilink),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_link_to_a_name_with_spaces_finds_it_either_way_it_is_written() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a.md", "[one](My%20note.md)\n");
        write(td.path(), "b.md", "[two](<My note.md>)\n");
        write(td.path(), "My note.md", "hi\n");
        let g = graph(&td);
        assert!(
            has(&g, "a.md", "My note.md", EdgeKind::Link),
            "{:?}",
            edges(&g)
        );
        assert!(
            has(&g, "b.md", "My note.md", EdgeKind::Link),
            "{:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_name_is_found_the_way_it_was_meant() {
        let td = tempfile::tempdir().unwrap();
        write(
            td.path(),
            "start.md",
            "[[v1.2 notes]] [[diagram]] [[a/index]] [[Über]]\n[gone](../../other/design.md)\n",
        );
        write(td.path(), "v1.2 notes.md", "a dot is not an extension\n");
        write(td.path(), "notes/diagram.md", "the note, not the picture\n");
        std::fs::write(td.path().join("diagram.png"), [0x89u8, b'P', b'N', b'G']).unwrap();
        write(td.path(), "a/index.md", "a\n");
        write(td.path(), "b/index.md", "b\n");
        write(td.path(), "über.md", "case folds past ASCII\n");
        write(td.path(), "design.md", "not where that path goes\n");
        let g = graph(&td);
        for to in ["v1.2 notes.md", "notes/diagram.md", "a/index.md", "über.md"] {
            assert!(
                has(&g, "start.md", to, EdgeKind::Wikilink),
                "{to}: {:?}",
                edges(&g)
            );
        }
        assert!(
            !has(&g, "start.md", "design.md", EdgeKind::Link),
            "a path that leads nowhere links nowhere: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn building_something_is_calling_it() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "parser.js", "export class Parser {}\n");
        write(td.path(), "app.js", "const p = new Parser();\n");
        write(td.path(), "config.rs", "pub struct Config;\n");
        write(
            td.path(),
            "main.rs",
            "fn main() {\n    let _ = Config::new();\n    let _ = helpers::parse::<u8>();\n}\n",
        );
        write(
            td.path(),
            "helpers.rs",
            "pub fn parse<T>() -> T { todo!() }\n",
        );
        let g = graph(&td);
        assert!(
            has(&g, "app.js", "parser.js", EdgeKind::Call),
            "{:?}",
            edges(&g)
        );
        assert!(
            has(&g, "main.rs", "config.rs", EdgeKind::Call),
            "{:?}",
            edges(&g)
        );
        assert!(
            has(&g, "main.rs", "helpers.rs", EdgeKind::Call),
            "the turbofish: {:?}",
            edges(&g)
        );
    }

    #[test]
    fn a_bare_call_to_a_files_own_function_stays_its_own() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a.py", "def load():\n    pass\n\nload()\n");
        write(td.path(), "b.py", "def load():\n    pass\n");
        let g = graph(&td);
        assert!(!has(&g, "a.py", "b.py", EdgeKind::Call), "{:?}", edges(&g));
    }

    #[test]
    fn every_file_becomes_a_node() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "a.md", "[[b]]\n");
        write(td.path(), "b.md", "hi\n");
        write(td.path(), "lonely.md", "nothing here\n");
        fs::write(td.path().join("picture.png"), [0x89u8, b'P', b'N', b'G']).unwrap();
        let g = graph(&td);
        assert_eq!(g.nodes.len(), 4);
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
    }
}
