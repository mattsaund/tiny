//! Project-wide search, and the find-replace behind `*replace`.
//!
//! Deliberately plain: a literal substring scan over the text files in the
//! project, no index and no regex. For a folder of notes and source that is
//! fast enough to run on every keystroke, and it never goes stale. The files
//! are read on every core at once, in batches taken in order, so a big
//! project costs its disk rather than one core's patience, see [`search`].
//!
//! Matching is smart-cased: an all-lowercase query ignores case, a query
//! with any capital in it does not. Replace is always case-sensitive, because
//! rewriting files on a loose match is not a mistake worth being clever about.
//!
//! # Why no index
//!
//! An index is the obvious optimization and the wrong one here. It would need
//! building on startup, invalidating on every save, and persisting somewhere
//! on disk, which tiny does not do, since it writes nothing into a project.
//! It can also go stale, which means search can start lying.
//! A linear scan over a folder of notes and source finishes in single-digit
//! milliseconds and is correct by construction. The size caps below
//! ([`MAX_FILE_BYTES`], `max_results`) are what keep it that way; if a project
//! ever outgrows them, that is the moment to reconsider, not before.
//!
//! # Smart case
//!
//! `widget` matches `Widget`; `Widget` matches only `Widget`. This applies to
//! [`search`] and to the `goto` commands. [`count`] and [`replace_all`] are
//! always case-sensitive and always literal (a [`Needle`]) because a
//! find-replace that guessed at your intent across a whole project would be a
//! very bad afternoon. The one thing a needle may ask for is whole words, so
//! that replacing `hi` leaves `this` alone.
//!
//! # Shared with the graph
//!
//! [`walk`] is `pub` because `graph::build` uses the same traversal, and the
//! two must agree on what counts as part of the project; otherwise a file
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
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::parallel::{self, par_map};

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

/// Hits one search will collect before it stops looking.
///
/// Five hundred is far more than anyone reads and little enough to gather
/// between keystrokes. It was a setting; nobody wants to be asked how much of
/// their own project to search, and the two answers that differ from this one
/// are "slower" and "less".
pub const MAX_RESULTS: usize = 500;

/// Traversal limits, built from the live config by `App::search_opts` so
/// `*set search_ignore ...` takes effect on the next keystroke.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Stop collecting past this many hits. The search box runs on every
    /// keystroke, so an unbounded result set on a one-letter query would
    /// stall the UI.
    pub max_results: usize,
    /// Directory names never walked into, matched exactly: `target`, not
    /// `*/target/*`.
    pub ignore: Vec<String>,
    /// Whether dotfiles and dot-directories are searched at all.
    pub show_hidden: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            max_results: MAX_RESULTS,
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

/// Version control's own folders, never walked whatever `ignore` says:
/// nothing in them is a file of the project, and a replace across the project
/// must never rewrite a repository's insides.
const NEVER_WALKED: &[&str] = &[".git", ".hg", ".svn"];

/// Files above this are skipped: a search box should not stall on a dump.
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

    // Read a batch on every core, then go through it in order exactly as a
    // plain loop would, so the results are the same, and the cap still stops
    // the reading one batch after it is reached rather than at the end of the
    // project. A file only comes back when the query is somewhere in it, so
    // splitting into lines happens only where there is something to find.
    let batch = parallel::threads() * 32;
    'files: for chunk in files.chunks(batch) {
        let texts = par_map(
            chunk,
            || (),
            |_, _, path| read_text(path).ok().filter(|text| matcher.is_in(text)),
        );
        for (path, text) in chunk.iter().zip(texts) {
            if names.len() + contents.len() >= opts.max_results {
                break 'files;
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
            let Some(text) = text else { continue };
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
    }

    names.extend(contents);
    names
}

/// Every line in the project where `at` finds something, as content hits in
/// walk order, stopping at `cap`.
///
/// What `goto word` and `goto text` look through when no file is named.
/// `in_file` is asked first, of the whole file, so a file with nothing in it
/// is never split into lines; `at` gives the character column of the first
/// match in a line. Read in parallel batches taken in order, like [`search`].
pub fn lines_where(
    root: &Path,
    opts: &Opts,
    cap: usize,
    in_file: impl Fn(&str) -> bool + Sync,
    at: impl Fn(&str) -> Option<usize>,
) -> Vec<Hit> {
    let files = walk(root, opts);
    let mut out = Vec::new();
    let batch = parallel::threads() * 32;
    for chunk in files.chunks(batch) {
        let texts = par_map(
            chunk,
            || (),
            |_, _, path| read_text(path).ok().filter(|text| in_file(text)),
        );
        for (path, text) in chunk.iter().zip(texts) {
            let Some(text) = text else { continue };
            for (line, col) in lines_in(&text, &at) {
                out.push(Hit {
                    path: path.clone(),
                    kind: HitKind::Content,
                    line,
                    col,
                    text: truncate(text.lines().nth(line).unwrap_or(""), MAX_LINE_DISPLAY),
                });
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

/// `(line, column)` of the first match on every line of `text` that has one.
pub fn lines_in(text: &str, at: impl Fn(&str) -> Option<usize>) -> Vec<(usize, usize)> {
    text.lines()
        .enumerate()
        .filter_map(|(n, line)| at(line).map(|col| (n, col)))
        .collect()
}

/// What a replace looks for: exact text, case and all, and whether it has to
/// stand as a word of its own.
///
/// Whole words is what a sentence means: `replace hi with hello` is about the
/// word, and leaves `this` and `high` alone. A needle that starts or ends with
/// something that is not a letter, digit or underscore only checks the ends
/// that are, so `replace -> with →` still finds every arrow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    pub text: String,
    pub whole_words: bool,
}

impl Needle {
    pub fn new(text: &str, whole_words: bool) -> Self {
        Self {
            text: text.to_string(),
            whole_words,
        }
    }

    /// Byte ranges of every place it sits in `hay`, left to right, never
    /// overlapping.
    pub fn find_in(&self, hay: &str) -> Vec<Range<usize>> {
        if self.text.is_empty() {
            return Vec::new();
        }
        hay.match_indices(&self.text)
            .map(|(at, m)| at..at + m.len())
            .filter(|r| !self.whole_words || self.stands_alone(hay, r))
            .collect()
    }

    /// `hay` with every occurrence replaced, and how many there were.
    pub fn replace_in(&self, hay: &str, with: &str) -> (String, usize) {
        let found = self.find_in(hay);
        let mut out = String::with_capacity(hay.len());
        let mut from = 0;
        for r in &found {
            out.push_str(&hay[from..r.start]);
            out.push_str(with);
            from = r.end;
        }
        out.push_str(&hay[from..]);
        (out, found.len())
    }

    /// Whether the match at `r` is a word on its own: nothing word-like
    /// running into it at either end that is itself word-like.
    fn stands_alone(&self, hay: &str, r: &Range<usize>) -> bool {
        let starts_word = self.text.chars().next().is_some_and(is_word_char);
        let ends_word = self.text.chars().next_back().is_some_and(is_word_char);
        let before = hay[..r.start].chars().next_back();
        let after = hay[r.end..].chars().next();
        !(starts_word && before.is_some_and(is_word_char))
            && !(ends_word && after.is_some_and(is_word_char))
    }
}

/// Letters, digits and the underscore: what a word, or a name in code, is
/// made of.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// How many occurrences a `*replace` would rewrite, without touching anything.
///
/// Runs before the confirmation prompt so the user is told the real scale of
/// what they are about to do. Deliberately a separate pass rather than a
/// dry-run flag on [`replace_all`]: the two are read by different code paths
/// and keeping them apart makes it impossible to accidentally write during a
/// count.
///
/// Files `skip` names are left out: the caller has a newer copy of them than
/// the disk does (a buffer with unsaved edits) and counts that instead.
pub fn count(root: &Path, needle: &Needle, opts: &Opts, skip: &dyn Fn(&Path) -> bool) -> Report {
    if needle.text.is_empty() {
        return Report::default();
    }
    let mut files = walk(root, opts);
    files.retain(|path| !skip(path));
    let counts = par_map(
        &files,
        || (),
        |_, _, path| read_text(path).map_or(0, |text| needle.find_in(&text).len()),
    );
    let mut report = Report::default();
    for n in counts.into_iter().filter(|n| *n > 0) {
        report.files += 1;
        report.occurrences += n;
    }
    report
}

/// Rewrite every occurrence across the project. Case-sensitive, literal.
/// Files that fail to write are reported rather than silently skipped.
///
/// There is no undo for this; it edits files on disk, not buffers. The
/// question a replace puts up first (see `app::plan`) is the only guard,
/// which is why it quotes the exact counts from [`count`].
///
/// On partial failure it keeps going and returns an error naming every file it
/// could not write, so the user knows exactly what state the project is in.
/// `App::do_replace` then brings clean buffers up to date with what it wrote.
///
/// Files `skip` names are not touched: a buffer with unsaved edits is the
/// newer copy of its file, and writing under it would be undone by its next
/// save. The caller replaces in those buffers instead.
pub fn replace_all(
    root: &Path,
    needle: &Needle,
    replacement: &str,
    opts: &Opts,
    skip: &dyn Fn(&Path) -> bool,
) -> Result<Report> {
    if needle.text.is_empty() {
        return Ok(Report::default());
    }
    let mut report = Report::default();
    let mut failures: Vec<String> = Vec::new();
    for path in walk(root, opts).into_iter().filter(|path| !skip(path)) {
        let Ok(text) = read_text(&path) else { continue };
        let (changed, n) = needle.replace_in(&text, replacement);
        if n == 0 {
            continue;
        }
        match fs::write(&path, changed) {
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

/// A prepared literal query.
///
/// Built once per search and reused for every line. That is the whole reason
/// it is a type rather than a function: `search` runs over every line of every
/// file in the project on every keystroke, so anything allocated per call is
/// allocated tens of thousands of times per keypress.
///
/// Smart case is decided once, at construction, from the query, see
/// [`folds_case`].
pub struct Matcher {
    /// The query as characters, for the case-folding scan.
    pat: Vec<char>,
    /// The query as it was written, for the case-sensitive path, where
    /// `str::find` beats anything character-by-character and allocates nothing.
    needle: String,
    /// The query as bytes, when it is ASCII (nearly always). Folding case is
    /// then a byte comparison rather than a Unicode lowercasing per
    /// character, which was most of what a lowercase search cost.
    ascii: Option<Vec<u8>>,
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
            ascii: query.is_ascii().then(|| query.as_bytes().to_vec()),
            fold: folds_case(query),
        })
    }

    /// Character index of the first match, if there is one.
    ///
    /// A *character* index, not a byte offset, because it ends up as an
    /// `Editor` cursor column, see the note on character indexing in
    /// `editor`.
    pub fn first(&self, haystack: &str) -> Option<usize> {
        let byte = match self.folded_ascii(haystack) {
            _ if !self.fold => haystack.find(&self.needle)?,
            Some(pat) => find_folded(haystack.as_bytes(), pat)?,
            None => {
                return haystack
                    .char_indices()
                    .position(|(byte, _)| self.at(haystack, byte));
            }
        };
        Some(haystack[..byte].chars().count())
    }

    /// Character column of the first place the query stands as a word of its
    /// own: the same match as [`Matcher::first`], with the ones inside a
    /// longer word passed over. What `goto word` looks for.
    pub fn first_word(&self, haystack: &str) -> Option<usize> {
        let chars: Vec<char> = haystack.chars().collect();
        let starts_word = self.pat.first().is_some_and(|c| is_word_char(*c));
        let ends_word = self.pat.last().is_some_and(|c| is_word_char(*c));
        self.all(haystack).into_iter().find_map(|(start, end)| {
            let before = start.checked_sub(1).map(|i| chars[i]);
            let after = chars.get(end).copied();
            let alone = !(starts_word && before.is_some_and(is_word_char))
                && !(ends_word && after.is_some_and(is_word_char));
            alone.then_some(start)
        })
    }

    /// Whether the query is anywhere in `haystack`: [`Matcher::first`]
    /// without the counting of characters up to it. How a whole file is
    /// checked before it is split into lines.
    pub fn is_in(&self, haystack: &str) -> bool {
        match self.folded_ascii(haystack) {
            _ if !self.fold => haystack.contains(&self.needle),
            Some(pat) => find_folded(haystack.as_bytes(), pat).is_some(),
            None => haystack
                .char_indices()
                .any(|(byte, _)| self.at(haystack, byte)),
        }
    }

    /// The query's bytes, when folding case can be done on bytes.
    ///
    /// That takes an ASCII query, and nothing else: an ASCII byte never
    /// appears inside a longer UTF-8 character, so a byte-wise match is a
    /// character-wise one, whatever else the text has in it. The one thing
    /// outside ASCII that lowercases *into* it is the Kelvin sign, `K` → `k`,
    /// and text with one in it takes the long way round so it still matches.
    fn folded_ascii(&self, haystack: &str) -> Option<&[u8]> {
        self.ascii
            .as_deref()
            .filter(|_| !haystack.contains('\u{212A}'))
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

/// Byte offset of the first place `pat` sits in `hay`, ignoring ASCII case.
/// `pat` is lowercase already: a query that folds has no capitals in it.
fn find_folded(hay: &[u8], pat: &[u8]) -> Option<usize> {
    let (&first, rest) = pat.split_first()?;
    let upper = first.to_ascii_uppercase();
    let last = hay.len().checked_sub(pat.len())?;
    (0..=last).find(|&i| {
        (hay[i] == first || hay[i] == upper) && hay[i + 1..i + pat.len()].eq_ignore_ascii_case(rest)
    })
}

/// Whether a query is matched case-insensitively.
///
/// Smart case, in one place: all-lowercase ignores case, any capital does not.
/// Every match in the program goes through [`Matcher`], which asks here, so
/// the preview pane cannot disagree with the result list about what counts.
fn folds_case(query: &str) -> bool {
    !query.chars().any(char::is_uppercase)
}

/// Cut a line down for display. Long lines are shortened, never skipped: the
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
/// uses. It is what stops a `.png` or a compiled object from being scanned,
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
    walk_all(root, opts).files
}

/// Everything [`walk`] finds, with the folders it went through as well.
#[derive(Debug, Default)]
pub struct Walked {
    pub files: Vec<PathBuf>,
    /// Every folder under the root, empty ones included, in the same sorted
    /// order. The root itself is not one of them.
    pub folders: Vec<PathBuf>,
    /// Whether the walk stopped at its limit with more still to see.
    pub truncated: bool,
}

/// [`walk`], keeping the folders: what a command looks a name up in, since
/// `goto folder notes` has to find a folder with nothing in it too.
pub fn walk_all(root: &Path, opts: &Opts) -> Walked {
    walk_up_to(root, opts, usize::MAX)
}

/// [`walk_all`], stopping once it has found `max_files` files: for the map,
/// which reads every one of them while the user waits, and opened on a home
/// folder would otherwise read everything in it.
pub fn walk_up_to(root: &Path, opts: &Opts, max_files: usize) -> Walked {
    let mut out = Walked::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir != root {
            out.folders.push(dir.clone());
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if opts.ignore.iter().any(|i| i == &name) || NEVER_WALKED.contains(&name.as_str()) {
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
        out.files.extend(files);
        if out.files.len() >= max_files {
            out.truncated = out.files.len() > max_files || !stack.is_empty() || !dirs.is_empty();
            out.files.truncate(max_files);
            break;
        }
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
        let report = count(
            td.path(),
            &Needle::new("widget", false),
            &Opts::default(),
            &|_| false,
        );
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
        let report = replace_all(
            td.path(),
            &Needle::new("widget", false),
            "gadget",
            &Opts::default(),
            &|_| false,
        )
        .unwrap();
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
        replace_all(
            td.path(),
            &Needle::new("widget", false),
            "gadget",
            &Opts::default(),
            &|_| false,
        )
        .unwrap();
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
        let report = replace_all(
            td.path(),
            &Needle::new("", false),
            "x",
            &Opts::default(),
            &|_| false,
        )
        .unwrap();
        assert_eq!(report, Report::default());
        let report = replace_all(
            td.path(),
            &Needle::new("absent-string", false),
            "x",
            &Opts::default(),
            &|_| false,
        )
        .unwrap();
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

    // ---- whole words -------------------------------------------------------

    #[test]
    fn a_whole_word_needle_leaves_longer_words_alone() {
        let n = Needle::new("hi", true);
        let (out, count) = n.replace_in("hi there, this is high. Hi! (hi)", "hello");
        assert_eq!(out, "hello there, this is high. Hi! (hello)");
        assert_eq!(count, 2, "case-sensitive, and only where it stands alone");
    }

    #[test]
    fn a_needle_of_symbols_still_matches_between_words() {
        let n = Needle::new("->", true);
        let (out, count) = n.replace_in("a->b and c -> d", "→");
        assert_eq!(out, "a→b and c → d");
        assert_eq!(count, 2, "only the word-like ends are held to a boundary");
    }

    #[test]
    fn a_text_needle_matches_inside_words() {
        let n = Needle::new("ing", false);
        assert_eq!(n.replace_in("sing a song of singing", "ung").1, 3);
    }

    #[test]
    fn a_whole_word_count_and_replace_agree() {
        let td = fixture();
        fs::write(td.path().join("notes/words.md"), "hi this hi\nhigh hi\n").unwrap();
        let n = Needle::new("hi", true);
        let report = count(td.path(), &n, &Opts::default(), &|_| false);
        assert_eq!((report.files, report.occurrences), (1, 3));
        replace_all(td.path(), &n, "hey", &Opts::default(), &|_| false).unwrap();
        assert_eq!(
            fs::read_to_string(td.path().join("notes/words.md")).unwrap(),
            "hey this hey\nhigh hey\n"
        );
    }

    #[test]
    fn a_word_is_found_on_its_own_and_not_inside_another() {
        let m = Matcher::new("test").unwrap();
        assert_eq!(m.first_word("testing the test"), Some(12));
        assert_eq!(m.first_word("attest, contest"), None);
        assert_eq!(m.first_word("Test it"), Some(0), "smart case still applies");
    }

    #[test]
    fn walking_with_folders_lists_empty_ones_too() {
        let td = fixture();
        fs::create_dir_all(td.path().join("empty/inner")).unwrap();
        let walked = walk_all(td.path(), &Opts::default());
        let names: Vec<String> = walked
            .folders
            .iter()
            .map(|p| {
                p.strip_prefix(td.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(names.contains(&"empty".to_string()), "{names:?}");
        assert!(names.contains(&"empty/inner".to_string()), "{names:?}");
        assert!(
            !names.contains(&"target".to_string()),
            "ignored stays ignored"
        );
    }

    #[test]
    fn lines_where_finds_every_line_with_a_match() {
        let td = fixture();
        let m = Matcher::new("widget").unwrap();
        let hits = lines_where(
            td.path(),
            &Opts::default(),
            100,
            |t| m.is_in(t),
            |l| m.first(l),
        );
        let lines: Vec<(String, usize)> = hits
            .iter()
            .map(|h| {
                (
                    h.path.file_name().unwrap().to_string_lossy().into_owned(),
                    h.line,
                )
            })
            .collect();
        assert!(lines.contains(&("design.md".to_string(), 2)), "{lines:?}");
        assert!(lines.contains(&("design.md".to_string(), 3)), "{lines:?}");
    }
}
