//! Project-wide search, and the find-replace behind `:replace`.
//!
//! Deliberately plain: a literal substring scan over the text files in the
//! project, no index and no regex. For a folder of notes and source that is
//! fast enough to run on every keystroke, and it never goes stale.
//!
//! Matching is smart-cased — an all-lowercase query ignores case, a query
//! with any capital in it does not. Replace is always case-sensitive, because
//! rewriting files on a loose match is not a mistake worth being clever about.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// The query matched the file's name.
    Name,
    /// The query matched a line inside the file.
    Content,
}

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

#[derive(Debug, Clone)]
pub struct Opts {
    pub max_results: usize,
    pub ignore: Vec<String>,
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
pub fn search(root: &Path, query: &str, opts: &Opts) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let fold = !query.chars().any(char::is_uppercase);
    let needle = if fold {
        query.to_lowercase()
    } else {
        query.to_string()
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
        if let Some(col) = find_in(&name, &needle, fold) {
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
            let Some(col) = find_in(line, &needle, fold) else {
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
fn find_in(haystack: &str, needle: &str, fold: bool) -> Option<usize> {
    let hay = if fold {
        haystack.to_lowercase()
    } else {
        haystack.to_string()
    };
    let byte = hay.find(needle)?;
    // Report a character index so it can drive a cursor directly.
    Some(hay[..byte].chars().count())
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// Read a file if it is text. Binary files and oversized files are skipped.
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
