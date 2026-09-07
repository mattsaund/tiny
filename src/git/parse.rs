//! Turning git's output into values, with no process anywhere near it.
//!
//! Every function here is text in and values out, which is the whole point:
//! the cases that are awkward to produce in a real repository — a rename whose
//! two halves arrive as separate records, a conflicted file, a diff with no
//! trailing newline — are tested as strings.

use super::{Branch, Change, Code, Diff, DiffLine, LineKind, Status};

/// Parse `git status --porcelain=v1 --branch -z`.
///
/// The `-z` form is the one worth parsing: records are NUL-separated and paths
/// are never quoted or escaped, so a file with a space, a quote or a newline in
/// its name arrives intact. The readable form escapes those, and unescaping it
/// correctly is more work than this.
///
/// Each record is `XY <path>`, where `X` is what the index has and `Y` is what
/// the working tree has. A rename or copy spends two records: the new name,
/// then the old one.
pub fn status(out: &str) -> Status {
    let mut status = Status::default();
    let mut records = out.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(head) = record.strip_prefix("## ") {
            status.branch = branch(head);
            continue;
        }
        let (codes, path) = record.split_at(record.len().min(2));
        let path = path.trim_start().to_string();
        let mut chars = codes.chars();
        let (x, y) = (chars.next().unwrap_or(' '), chars.next().unwrap_or(' '));

        // A rename or a copy is followed by the name it came from, in a record
        // of its own. Take it now, whichever side the change lands on.
        let from = if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            records.next().map(str::to_string)
        } else {
            None
        };

        // Untracked and ignored are both spelled with a `?` or a `!` twice
        // over, and neither is a change to the index.
        if x == '?' || y == '?' {
            status.unstaged.push(Change {
                code: Code::Untracked,
                path,
                from: None,
            });
            continue;
        }
        if x == '!' || y == '!' {
            continue;
        }
        // Unmerged, in all its spellings: either side is a `U`, or both sides
        // agree on a letter that only a conflict produces.
        if x == 'U' || y == 'U' || (x == y && (x == 'A' || x == 'D')) {
            status.unstaged.push(Change {
                code: Code::Conflict,
                path,
                from,
            });
            continue;
        }
        if let Some(code) = code_of(x) {
            status.staged.push(Change {
                code,
                path: path.clone(),
                from: from.clone(),
            });
        }
        if let Some(code) = code_of(y) {
            status.unstaged.push(Change { code, path, from });
        }
    }
    status
}

/// The `## main...origin/main [ahead 1, behind 2]` line.
///
/// Everything after the name is optional: a branch with no upstream has no
/// `...`, and one that is level has no bracket.
fn branch(head: &str) -> Branch {
    let (names, counts) = match head.split_once(" [") {
        Some((n, c)) => (n, c.trim_end_matches(']')),
        None => (head, ""),
    };
    let (name, upstream) = match names.split_once("...") {
        Some((n, u)) => (n, Some(u.to_string())),
        None => (names, None),
    };
    let count = |word: &str| -> usize {
        counts
            .split(", ")
            .find_map(|part| part.strip_prefix(word)?.trim().parse().ok())
            .unwrap_or(0)
    };
    Branch {
        // A repository with no commits yet says `No commits yet on main`.
        name: name.rsplit(' ').next().unwrap_or(name).to_string(),
        upstream,
        ahead: count("ahead "),
        behind: count("behind "),
    }
}

/// One status letter, or `None` for "nothing on this side".
fn code_of(c: char) -> Option<Code> {
    match c {
        'A' => Some(Code::Added),
        'M' => Some(Code::Modified),
        'D' => Some(Code::Deleted),
        'R' => Some(Code::Renamed),
        'C' => Some(Code::Copied),
        'T' => Some(Code::Modified), // a type change is a change
        _ => None,
    }
}

/// Parse a unified diff into two columns that stay level with each other.
///
/// With `-U1000000` git emits the whole file as one hunk, so this walks a
/// complete before and after rather than fragments. A removed line puts a real
/// line on the left and a [`LineKind::Filler`] on the right; an added line does
/// the reverse; a context line goes on both. The two sides therefore always
/// have the same length, which is what makes reading across them mean anything.
///
/// Runs of removals followed by additions — the ordinary shape of an edit — are
/// paired up rather than stacked, so a changed line sits opposite the line it
/// replaced instead of below a column of blanks.
pub fn diff(out: &str) -> Diff {
    let mut diff = Diff::default();
    let mut removed: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    let mut in_hunk = false;

    // Nothing has changed: `git diff` says nothing at all.
    if out.trim().is_empty() {
        return diff;
    }

    let flush = |diff: &mut Diff, removed: &mut Vec<String>, added: &mut Vec<String>| {
        for i in 0..removed.len().max(added.len()) {
            diff.before.push(match removed.get(i) {
                Some(text) => DiffLine {
                    text: text.clone(),
                    kind: LineKind::Changed,
                },
                None => filler(),
            });
            diff.after.push(match added.get(i) {
                Some(text) => DiffLine {
                    text: text.clone(),
                    kind: LineKind::Changed,
                },
                None => filler(),
            });
        }
        removed.clear();
        added.clear();
    };

    for line in out.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'-') => removed.push(line[1..].to_string()),
            Some(b'+') => added.push(line[1..].to_string()),
            // `\ No newline at end of file` is a note about the line above, not
            // a line of the file.
            Some(b'\\') => {}
            _ => {
                flush(&mut diff, &mut removed, &mut added);
                let text = line.strip_prefix(' ').unwrap_or(line).to_string();
                diff.before.push(DiffLine {
                    text: text.clone(),
                    kind: LineKind::Same,
                });
                diff.after.push(DiffLine {
                    text,
                    kind: LineKind::Same,
                });
            }
        }
    }
    flush(&mut diff, &mut removed, &mut added);
    diff
}

/// A file git has never seen: nothing on the left, all of it on the right.
pub fn whole_file_added(text: &str) -> Diff {
    let mut diff = Diff::default();
    for line in text.lines() {
        diff.before.push(filler());
        diff.after.push(DiffLine {
            text: line.to_string(),
            kind: LineKind::Changed,
        });
    }
    diff
}

fn filler() -> DiffLine {
    DiffLine {
        text: String::new(),
        kind: LineKind::Filler,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records are NUL-separated, so a fixture reads better written with `\0`
    /// spelled out.
    fn z(records: &[&str]) -> String {
        records.iter().map(|r| format!("{r}\0")).collect::<String>()
    }

    #[test]
    fn a_branch_line_gives_up_its_name_and_its_distance() {
        let b = branch("main...origin/main [ahead 2, behind 1]");
        assert_eq!(b.name, "main");
        assert_eq!(b.upstream.as_deref(), Some("origin/main"));
        assert_eq!((b.ahead, b.behind), (2, 1));

        let plain = branch("wip");
        assert_eq!(plain.name, "wip");
        assert_eq!(plain.upstream, None);
        assert_eq!((plain.ahead, plain.behind), (0, 0));

        let one_way = branch("main...origin/main [behind 3]");
        assert_eq!((one_way.ahead, one_way.behind), (0, 3));
    }

    #[test]
    fn a_fresh_repository_still_names_its_branch() {
        // Before the first commit git writes the whole sentence out.
        assert_eq!(branch("No commits yet on main").name, "main");
    }

    #[test]
    fn the_two_columns_are_two_different_questions() {
        // `MM` is a file changed, staged, and then changed again.
        let s = status(&z(&[
            "## main",
            "MM both.rs",
            "M  staged.rs",
            " M dirty.rs",
        ]));
        assert_eq!(
            s.staged.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
            ["both.rs", "staged.rs"]
        );
        assert_eq!(
            s.unstaged
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            ["both.rs", "dirty.rs"]
        );
    }

    #[test]
    fn untracked_is_its_own_letter_and_never_staged() {
        let s = status(&z(&["## main", "?? scratch.txt"]));
        assert!(s.staged.is_empty());
        assert_eq!(s.unstaged[0].code, Code::Untracked);
        assert_eq!(s.unstaged[0].code.letter(), 'U');
    }

    #[test]
    fn an_ignored_file_is_not_a_change() {
        let s = status(&z(&["## main", "!! target/"]));
        assert!(s.is_clean(), "ignored files are not news");
    }

    #[test]
    fn a_rename_carries_the_name_it_came_from() {
        let s = status(&z(&["## main", "R  new.md", "old.md"]));
        assert_eq!(s.staged[0].code, Code::Renamed);
        assert_eq!(s.staged[0].path, "new.md");
        assert_eq!(s.staged[0].from.as_deref(), Some("old.md"));
        assert_eq!(s.staged[0].label(), "old.md → new.md");
    }

    #[test]
    fn a_path_with_a_space_in_it_survives() {
        let s = status(&z(&["## main", " M my notes.md"]));
        assert_eq!(s.unstaged[0].path, "my notes.md");
    }

    #[test]
    fn a_conflict_is_neither_staged_nor_a_plain_change() {
        for record in ["UU both.rs", "AA both.rs", "DD both.rs"] {
            let s = status(&z(&["## main", record]));
            assert!(s.staged.is_empty(), "{record} is not ready to commit");
            assert_eq!(s.unstaged[0].code, Code::Conflict, "{record}");
        }
    }

    #[test]
    fn a_diff_becomes_two_columns_of_the_same_height() {
        let out = "\
diff --git a/a.txt b/a.txt
index 111..222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
 first
-second
+SECOND
 third
";
        let d = diff(out);
        assert_eq!(d.before.len(), d.after.len(), "the sides stay level");
        assert_eq!(
            d.before.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert_eq!(
            d.after.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["first", "SECOND", "third"]
        );
        assert_eq!(d.before[1].kind, LineKind::Changed);
        assert_eq!(d.before[0].kind, LineKind::Same);
    }

    #[test]
    fn a_line_with_no_opposite_gets_a_blank_to_face() {
        let out = "\
@@ -1,1 +1,3 @@
 kept
+one
+two
";
        let d = diff(out);
        assert_eq!(d.before.len(), 3);
        assert_eq!(d.before[1].kind, LineKind::Filler);
        assert_eq!(d.before[2].kind, LineKind::Filler);
        assert_eq!(d.after[2].text, "two");
    }

    #[test]
    fn the_no_newline_note_is_not_a_line_of_the_file() {
        let out = "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let d = diff(out);
        assert_eq!(d.before.len(), 1, "one line each side");
        assert_eq!(d.before[0].text, "old");
        assert_eq!(d.after[0].text, "new");
    }

    #[test]
    fn nothing_changed_is_an_empty_diff_and_not_a_panic() {
        assert!(diff("").is_empty());
        assert!(diff("\n").is_empty());
    }

    #[test]
    fn an_untracked_file_is_all_right_hand_side() {
        let d = whole_file_added("one\ntwo\n");
        assert_eq!(d.before.len(), 2);
        assert!(d.before.iter().all(|l| l.kind == LineKind::Filler));
        assert_eq!(d.after[1].text, "two");
    }
}
