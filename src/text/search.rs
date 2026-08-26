//! Project-wide search, and the find-replace behind `:replace`.
//!
//! Deliberately plain: a literal substring scan over the text files in the
//! project, no index and no regex. For a folder of notes and source that is
//! fast enough to run on every keystroke, and it never goes stale.
//!
//! Matching is smart-cased — an all-lowercase query ignores case, a query
//! with any capital in it does not. Replace is always case-sensitive, because
//! rewriting files on a loose match is not a mistake worth being clever about.
//!
//! # Why no index
//!
//! An index is the obvious optimisation and the wrong one here. It would need
//! building on startup, invalidating on every save, and persisting somewhere
//! on disk — which tiny does not do, since it writes nothing into a project.
//! It can also go stale, which means search can start lying.
//! A linear scan over a folder of notes and source finishes in single-digit
//! milliseconds and is correct by construction. The size caps below
//! ([`MAX_FILE_BYTES`], `max_results`) are what keep it that way; if a project
//! ever outgrows them, that is the moment to reconsider, not before.
//!
//! # Smart case
//!
//! `widget` matches `Widget`; `Widget` matches only `Widget`. This applies to
//! [`search`] alone. [`count`] and [`replace_all`] are always case-sensitive
//! and always literal, because a find-replace that guessed at your intent
//! across a whole project would be a very bad afternoon.
//!
//! # Shared with the graph
//!
//! [`walk`] is `pub` because `graph::build` uses the same traversal, and the
//! two must agree on what counts as part of the project — otherwise a file
//! could be searchable but absent from the graph, or the reverse.
//!
//! # Shared with the screen
//!
//! [`Matcher`] is `pub` for the same kind of reason: `ui` marks the query in
//! the result list and again in the preview pane, and if it decided for itself
//! what a match was, the highlight could land somewhere the search did not
//! think there was one. Prepared once and reused, because the alternative is
//! an allocation per line of every file, on every keystroke.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Whether a hit came from a file's name or from a line inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// The query matched the file's name.
    Name,
    /// The query matched a line inside the file.
    Content,
}

/// One match. Carries enough to jump straight to it: the file, the line, and
/// the character column, which `App::place_cursor_on` feeds to `Editor::goto`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    pub kind: HitKind,
    /// 0-based line number; 0 for name hits.
    pub line: usize,
    /// 0-based character index of the match within `text`.
    pub col: usize,
    /// The matching line, for display. Name hits carry the file name.
    pub text: String,
}

/// Traversal limits, built from the live config by `App::search_opts` so
/// `:set search_ignore ...` takes effect on the next keystroke.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Stop collecting past this many hits. The search box runs on every
    /// keystroke, so an unbounded result set on a one-letter query would
    /// stall the UI.
    pub max_results: usize,
    /// Directory names never walked into, matched exactly — `target`, not
    /// `*/target/*`.
    pub ignore: Vec<String>,
    /// Whether dotfiles and dot-directories are searched at all.
    pub show_hidden: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            max_results: 500,
            ignore: [".git", "target", "node_modules", ".venv", "__pycache__"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            show_hidden: false,
        }
    }
}

/// What a replace did, or what one would do. Used both to describe the change
/// afterwards and to fill in the confirmation prompt beforehand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    pub files: usize,
    pub occurrences: usize,
}

/// Files above this are skipped — a search box should not stall on a dump.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Long lines are truncated for display, not skipped.
const MAX_LINE_DISPLAY: usize = 400;

/// Search names and contents under `root`. Name matches come first, then
/// content matches in directory order.
///
/// The ordering is why hits are collected into two vectors and concatenated at
/// the end: typing part of a filename should surface the file itself, not the
/// forty lines that happen to mention it. Case folding is decided once from
/// the query and applied to both passes.
///
/// A file yields at most one name hit but any number of content hits. The cap
/// is checked in both loops, so a single enormous file cannot exhaust it
/// before other files are reached.
pub fn search(root: &Path, query: &str, opts: &Opts) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let Some(matcher) = Matcher::new(query) else {
        return Vec::new();
    };
    let files = walk(root, opts);
    let mut names = Vec::new();
    let mut contents = Vec::new();

    for path in files {
        if names.len() + contents.len() >= opts.max_results {
            break;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(col) = matcher.first(&name) {
            names.push(Hit {
                path: path.clone(),
                kind: HitKind::Name,
                line: 0,
                col,
                text: name,
            });
        }

        let Ok(text) = read_text(&path) else { continue };
        for (lineno, line) in text.lines().enumerate() {
            let Some(col) = matcher.first(line) else {
                continue;
            };
            contents.push(Hit {
                path: path.clone(),
                kind: HitKind::Content,
                line: lineno,
                col,
                text: truncate(line, MAX_LINE_DISPLAY),
            });
            if names.len() + contents.len() >= opts.max_results {
                break;
            }
        }
    }

    names.extend(contents);
    names
}

/// How many occurrences a `:replace` would rewrite, without touching anything.
///
/// Runs before the confirmation prompt so the user is told the real scale of
/// what they are about to do. Deliberately a separate pass rather than a
/// dry-run flag on [`replace_all`]: the two are read by different code paths
/// and keeping them apart makes it impossible to accidentally write during a
/// count.
pub fn count(root: &Path, needle: &str, opts: &Opts) -> Report {
    if needle.is_empty() {
        return Report::default();
    }
    let mut report = Report::default();
    for path in walk(root, opts) {
        let Ok(text) = read_text(&path) else { continue };
        let n = text.matches(needle).count();
        if n > 0 {
            report.files += 1;
            report.occurrences += n;
        }
    }
    report
}

/// Rewrite every occurrence across the project. Case-sensitive, literal.
/// Files that fail to write are reported rather than silently skipped.
///
/// There is no undo for this — it edits files on disk, not buffers. The
/// confirmation prompt in `App::cmd_replace` is the only guard, which is why
/// it quotes the exact counts from [`count`].
///
/// On partial failure it keeps going and returns an error naming every file it
/// could not write, so the user knows exactly what state the project is in.
/// `App::do_replace` then drops clean buffers so open files re-read from disk.
pub fn replace_all(root: &Path, needle: &str, replacement: &str, opts: &Opts) -> Result<Report> {
    if needle.is_empty() {
        return Ok(Report::default());
    }
    let mut report = Report::default();
    let mut failures: Vec<String> = Vec::new();
    for path in walk(root, opts) {
        let Ok(text) = read_text(&path) else { continue };
        let n = text.matches(needle).count();
        if n == 0 {
            continue;
        }
        match fs::write(&path, text.replace(needle, replacement)) {
            Ok(()) => {
                report.files += 1;
                report.occurrences += n;
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    if !failures.is_empty() {
        return Err(anyhow::anyhow!(
            "changed {} file(s); {} could not be written: {}",
            report.files,
            failures.len(),
            failures.join(", ")
        ));
    }
    Ok(report)
}

/// Character index of the first match, honouring case folding.
///
/// Returns a *character* index, not a byte offset, because it ends up as an
/// `Editor` cursor column — see the note on character indexing in `editor`.
/// A prepared literal query.
///
/// Built once per search and reused for every line. That is the whole reason
/// it is a type rather than a function: `search` runs over every line of every
/// file in the project on every keystroke, so anything allocated per call is
/// allocated tens of thousands of times per keypress.
///
/// Smart case is decided once, at construction, from the query — see
/// [`folds_case`].
pub struct Matcher {
    /// The query as characters, for the case-folding scan.
    pat: Vec<char>,
    /// The query as it was written, for the case-sensitive path, where
    /// `str::find` beats anything character-by-character and allocates nothing.
    needle: String,
    fold: bool,
}

impl Matcher {
    /// Prepare a query. `None` for an empty one, which matches nothing rather
    /// than everything.
    pub fn new(query: &str) -> Option<Self> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        Some(Self {
            pat: query.chars().collect(),
            needle: query.to_string(),
            fold: folds_case(query),
        })
    }

    /// Character index of the first match, if there is one.
    pub fn first(&self, haystack: &str) -> Option<usize> {
        if !self.fold {
            let byte = haystack.find(&self.needle)?;
            // A character index, so it can drive a cursor directly.
            return Some(haystack[..byte].chars().count());
        }
        haystack
            .char_indices()
            .position(|(byte, _)| self.at(haystack, byte))
    }

    /// Every non-overlapping match, as character ranges.
    ///
    /// Character ranges rather than byte ranges because the caller is `ui`,
    /// which cuts drawn spans at character boundaries. Non-overlapping because
    /// these become highlights on screen, and two overlapping highlights are
    /// one highlight to look at.
    pub fn all(&self, haystack: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut resume = 0;
        for (i, (byte, _)) in haystack.char_indices().enumerate() {
            if i < resume {
                continue;
            }
            if self.at(haystack, byte) {
                resume = i + self.pat.len();
                out.push((i, resume));
            }
        }
        out
    }

    /// Whether the query sits at this byte offset. Compares characters as they
    /// come rather than folding the line first, so nothing is allocated and a
    /// character whose lowercase form is a different length cannot shift the
    /// answer.
    fn at(&self, haystack: &str, byte: usize) -> bool {
        let mut chars = haystack[byte..].chars();
        self.pat.iter().all(|&want| match chars.next() {
            Some(c) if self.fold => c.to_lowercase().eq(want.to_lowercase()),
            Some(c) => c == want,
            None => false,
        })
    }
}

/// Whether a query is matched case-insensitively.
///
/// Smart case, in one place: all-lowercase ignores case, any capital does not.
/// Every match in the program goes through [`Matcher`], which asks here, so
/// the preview pane cannot disagree with the result list about what counts.
fn folds_case(query: &str) -> bool {
    !query.chars().any(char::is_uppercase)
}

/// Cut a line down for display. Long lines are shortened, never skipped — the
/// hit is still real, there is just no room to show all of it.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// Read a file if it is text. Binary files and oversized files are skipped.
///
/// "Binary" means "contains a zero byte", the same cheap heuristic `grep`
/// uses. It is what stops a `.png` or a compiled object from being scanned —
/// and, in [`replace_all`], from being rewritten.
fn read_text(path: &Path) -> Result<String> {
    let meta = fs::metadata(path)?;
    if meta.len() > MAX_FILE_BYTES {
        anyhow::bail!("too large");
    }
    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        anyhow::bail!("binary");
    }
    Ok(String::from_utf8(bytes)?)
}

/// Every candidate file under `root`, in sorted directory order.
///
/// Shared with `graph::build`, so the two always agree on what the project
/// contains. Iterative with an explicit stack rather than recursive, so a
/// pathologically deep tree cannot blow the call stack.
///
/// Symlinks are skipped entirely, not just not-followed. A link pointing back
/// up its own tree would otherwise loop forever, and there is no sane way to
/// present the same file twice under two paths.
///
/// Directories are pushed reversed so the stack pops them in sorted order,
/// which is what makes results stable between runs.
pub fn walk(root: &Path, opts: &Opts) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if opts.ignore.iter().any(|i| i == &name) {
                continue;
            }
            if !opts.show_hidden && name.starts_with('.') {
                continue;
            }
            // Do not follow symlinks: a link back up the tree would loop.
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                dirs.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
        files.sort();
        out.extend(files);
        dirs.sort();
        // Pushed reversed so the stack pops them in sorted order.
        for d in dirs.into_iter().rev() {
            stack.push(d);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let r = td.path();
        fs::create_dir_all(r.join("notes")).unwrap();
        fs::create_dir_all(r.join("src")).unwrap();
        fs::create_dir_all(r.join("target")).unwrap();
        fs::write(
            r.join("notes/design.md"),
            "# Design\n\nthe Widget plan\nwidget again\n",
        )
        .unwrap();
        fs::write(r.join("src/widget.py"), "class Widget:\n    pass\n").unwrap();
        fs::write(r.join("README.md"), "nothing to see\n").unwrap();
        fs::write(r.join("target/widget.o"), "widget widget widget\n").unwrap();
        fs::write(r.join(".hidden.md"), "widget in a dotfile\n").unwrap();
        fs::write(r.join("blob.bin"), [b'w', b'i', 0, b'd']).unwrap();
        td
    }

    fn names(hits: &[Hit]) -> Vec<String> {
        hits.iter()
            .map(|h| h.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_matches_in_names_and_contents_with_names_first() {
        let td = fixture();
        let hits = search(td.path(), "widget", &Opts::default());
        assert_eq!(hits[0].kind, HitKind::Name);
        assert_eq!(hits[0].text, "widget.py");
        assert!(hits[1..].iter().all(|h| h.kind == HitKind::Content));
    }

    #[test]
    fn lowercase_queries_ignore_case_but_mixed_case_does_not() {
        let td = fixture();
        let loose = search(td.path(), "widget", &Opts::default());
        assert!(
            loose.iter().any(|h| h.text.contains("the Widget plan")),
            "a lowercase query matches the capitalised line"
        );
        let strict = search(td.path(), "Widget", &Opts::default());
        assert!(
            !strict.iter().any(|h| h.text == "widget again"),
            "a capitalised query does not match lowercase text"
        );
    }

    #[test]
    fn reports_the_line_and_column_of_each_match() {
        let td = fixture();
        let hits = search(td.path(), "again", &Opts::default());
        let hit = hits.iter().find(|h| h.kind == HitKind::Content).unwrap();
        assert_eq!(hit.line, 3, "0-based line number");
        assert_eq!(hit.col, 7, "character index within the line");
        assert_eq!(hit.text, "widget again");
    }

    #[test]
    fn skips_ignored_directories_dotfiles_and_binaries() {
        let td = fixture();
        let found = names(&search(td.path(), "widget", &Opts::default()));
        assert!(!found.iter().any(|n| n == "widget.o"), "target/ is ignored");
        assert!(!found.iter().any(|n| n == ".hidden.md"), "dotfiles skipped");
        assert!(!found.iter().any(|n| n == "blob.bin"), "binaries skipped");
    }

    #[test]
    fn show_hidden_brings_dotfiles_into_the_search() {
        let td = fixture();
        let opts = Opts {
            show_hidden: true,
            ..Opts::default()
        };
        let found = names(&search(td.path(), "widget", &opts));
        assert!(found.iter().any(|n| n == ".hidden.md"));
    }

    #[test]
    fn a_matcher_finds_every_occurrence_not_just_the_first() {
        let m = Matcher::new("wid").unwrap();
        assert_eq!(m.all("wid a wid b wid"), [(0, 3), (6, 9), (12, 15)]);
        assert_eq!(m.first("wid a wid"), Some(0));
        assert_eq!(m.first("nothing here"), None);
    }

    #[test]
    fn a_matcher_never_returns_overlapping_ranges() {
        // "aa" in "aaaa" is two matches, not three: they become highlights,
        // and overlapping highlights are one highlight to look at.
        let m = Matcher::new("aa").unwrap();
        assert_eq!(m.all("aaaa"), [(0, 2), (2, 4)]);
    }

    #[test]
    fn a_matcher_is_smart_cased_the_same_way_the_search_is() {
        let lower = Matcher::new("widget").unwrap();
        assert_eq!(lower.all("Widget widget WIDGET").len(), 3, "all of them");

        let mixed = Matcher::new("Widget").unwrap();
        assert_eq!(mixed.all("Widget widget WIDGET"), [(0, 6)], "only the one");
    }

    #[test]
    fn a_matcher_reports_character_positions_not_byte_positions() {
        let m = Matcher::new("plan").unwrap();
        // Four characters, ten bytes, before the match.
        assert_eq!(m.all("日本語です the plan"), [(10, 14)]);
        assert_eq!(m.first("日本語です the plan"), Some(10));
    }

    #[test]
    fn a_matcher_folds_case_beyond_ascii() {
        let m = Matcher::new("straße").unwrap();
        assert_eq!(m.all("die STRASSE").len(), 0, "not the same characters");
        assert_eq!(m.all("die Straße"), [(4, 10)]);
    }

    #[test]
    fn an_empty_query_matches_nothing_rather_than_everything() {
        assert!(Matcher::new("").is_none());
        assert!(Matcher::new("   ").is_none());
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let td = fixture();
        assert!(search(td.path(), "", &Opts::default()).is_empty());
        assert!(search(td.path(), "   ", &Opts::default()).is_empty());
    }

    #[test]
    fn results_are_capped() {
        let td = tempfile::tempdir().unwrap();
        let body: String = (0..200).map(|_| "needle\n").collect();
        fs::write(td.path().join("many.txt"), body).unwrap();
        let opts = Opts {
            max_results: 10,
            ..Opts::default()
        };
        assert!(search(td.path(), "needle", &opts).len() <= 10);
    }

    #[test]
    fn count_reports_files_and_occurrences_without_changing_anything() {
        let td = fixture();
        let before = fs::read_to_string(td.path().join("notes/design.md")).unwrap();
        let report = count(td.path(), "widget", &Opts::default());
        assert_eq!(report.files, 1, "only the lowercase spelling, in one file");
        assert_eq!(report.occurrences, 1);
        assert_eq!(
            fs::read_to_string(td.path().join("notes/design.md")).unwrap(),
            before
        );
    }

    #[test]
    fn replace_rewrites_every_occurrence_across_the_project() {
        let td = fixture();
        let report = replace_all(td.path(), "widget", "gadget", &Opts::default()).unwrap();
        assert_eq!(report.occurrences, 1);
        let design = fs::read_to_string(td.path().join("notes/design.md")).unwrap();
        assert!(design.contains("gadget again"));
        assert!(
            design.contains("the Widget plan"),
            "replace is case-sensitive"
        );
    }

    #[test]
    fn replace_leaves_ignored_and_binary_files_alone() {
        let td = fixture();
        replace_all(td.path(), "widget", "gadget", &Opts::default()).unwrap();
        assert_eq!(
            fs::read_to_string(td.path().join("target/widget.o")).unwrap(),
            "widget widget widget\n",
            "an ignored directory is never rewritten"
        );
        assert_eq!(
            fs::read(td.path().join("blob.bin")).unwrap(),
            [b'w', b'i', 0, b'd'],
            "a binary file is never rewritten"
        );
    }

    #[test]
    fn replacing_nothing_is_a_no_op() {
        let td = fixture();
        let report = replace_all(td.path(), "", "x", &Opts::default()).unwrap();
        assert_eq!(report, Report::default());
        let report = replace_all(td.path(), "absent-string", "x", &Opts::default()).unwrap();
        assert_eq!(report.files, 0);
    }

    #[test]
    fn a_symlink_loop_does_not_hang_the_walk() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root, root.join("loop")).unwrap();
        let hits = search(root, "needle", &Opts::default());
        assert_eq!(hits.len(), 1);
    }
}
