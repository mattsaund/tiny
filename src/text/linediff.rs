//! Which lines of a file are different from another version of it: what the
//! marks beside the line numbers are drawn from, against the last commit.
//!
//! A line-by-line diff, Myers' algorithm, on whatever is left once the lines
//! the two have in common at the start and at the end are set aside. An edit
//! is almost always in one place, so that leaves a handful of lines, and the
//! diff of those costs nothing. Past [`MOST_EDITS`] differences the middle is
//! taken as one changed stretch rather than searched for the fewest edits,
//! which keeps a file rewritten top to bottom from costing more than a
//! couple of milliseconds and a couple of megabytes.

/// What happened to one line of the new version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Change {
    #[default]
    Same,
    /// Not in the old version: green.
    Added,
    /// In place of different lines of the old version: blue.
    Modified,
}

/// The mark for one line of the new version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineMark {
    pub change: Change,
    /// Lines of the old version were deleted just after this one.
    pub deleted_below: bool,
    /// Lines of the old version were deleted just before this one, which is
    /// only ever said of the first line: anywhere else it is the line above
    /// that says it.
    pub deleted_above: bool,
}

/// Past this many lines taken out or put in, the middle is taken as one
/// changed stretch, see the module docs. A changed line is one of each.
const MOST_EDITS: usize = 1000;

/// Lines to compare: how many there are, and what each says.
pub trait Lines {
    fn count(&self) -> usize;
    fn line(&self, i: usize) -> &str;
}

impl<T: AsRef<str>> Lines for [T] {
    fn count(&self) -> usize {
        self.len()
    }

    fn line(&self, i: usize) -> &str {
        self[i].as_ref()
    }
}

/// Lines `from..from + len` of `all`, numbered from 0: what is left to
/// diff once the lines in common at either end are set aside.
struct Part<'a, L: ?Sized> {
    all: &'a L,
    from: usize,
    len: usize,
}

impl<L: Lines + ?Sized> Lines for Part<'_, L> {
    fn count(&self) -> usize {
        self.len
    }

    fn line(&self, i: usize) -> &str {
        self.all.line(self.from + i)
    }
}

/// One stretch where the two differ: old lines `[a0, a1)` gave way to new
/// lines `[b0, b1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
}

/// The mark for every line of `new`, against `old`.
#[cfg(test)]
pub fn marks(old: &(impl Lines + ?Sized), new: &(impl Lines + ?Sized)) -> Vec<LineMark> {
    let mut out = Vec::new();
    mark(old, new, &mut out);
    out
}

/// The mark for every line of `new`, against `old`, into `out`, whatever
/// was in it before: a list kept and filled again costs nothing, where a new
/// one on every key was a hundred thousand lines' worth of fresh memory for
/// a file that long.
pub fn mark(old: &(impl Lines + ?Sized), new: &(impl Lines + ?Sized), out: &mut Vec<LineMark>) {
    out.clear();
    out.resize(new.count(), LineMark::default());
    for h in hunks(old, new) {
        let (deleted, added) = (h.a1 > h.a0, h.b1 > h.b0);
        if !added {
            // Gone, from between two lines: said by the line above, or by the
            // first line when there is none above.
            match h.b0.checked_sub(1) {
                Some(above) => out[above].deleted_below = true,
                None => {
                    if let Some(first) = out.first_mut() {
                        first.deleted_above = true;
                    }
                }
            }
            continue;
        }
        let change = if deleted {
            Change::Modified
        } else {
            Change::Added
        };
        for mark in &mut out[h.b0..h.b1] {
            mark.change = change;
        }
    }
}

/// Where `old` and `new` differ.
fn hunks(old: &(impl Lines + ?Sized), new: &(impl Lines + ?Sized)) -> Vec<Hunk> {
    let (n, m) = (old.count(), new.count());
    let same = |i: usize, j: usize| old.line(i) == new.line(j);
    let mut start = 0;
    while start < n && start < m && same(start, start) {
        start += 1;
    }
    let mut end = 0;
    while end < n - start && end < m - start && same(n - 1 - end, m - 1 - end) {
        end += 1;
    }
    let (a1, b1) = (n - end, m - end);
    if start == a1 && start == b1 {
        return Vec::new();
    }
    if start == a1 || start == b1 {
        return vec![Hunk {
            a0: start,
            a1,
            b0: start,
            b1,
        }];
    }
    let a = Part {
        all: old,
        from: start,
        len: a1 - start,
    };
    let b = Part {
        all: new,
        from: start,
        len: b1 - start,
    };
    match myers(&a, &b) {
        Some(found) => found
            .into_iter()
            .map(|h| Hunk {
                a0: h.a0 + start,
                a1: h.a1 + start,
                b0: h.b0 + start,
                b1: h.b1 + start,
            })
            .collect(),
        None => vec![Hunk {
            a0: start,
            a1,
            b0: start,
            b1,
        }],
    }
}

/// The fewest differences between `a` and `b`, as stretches, by Myers'
/// greedy algorithm, or `None` past [`MOST_EDITS`] of them.
fn myers(a: &impl Lines, b: &impl Lines) -> Option<Vec<Hunk>> {
    let (n, m) = (a.count() as isize, b.count() as isize);
    let most = (n + m).min(MOST_EDITS as isize);
    let offset = most + 1;
    // `v[offset + k]`: how far along `a` the furthest path on diagonal `k`
    // has reached, k being how many more lines of `a` it has passed than of
    // `b`. A round only reaches every other diagonal, so each round keeps
    // only those, to walk back along: round `d`'s are -d, -d + 2, up to d.
    let mut v = vec![0isize; (2 * offset + 1) as usize];
    let mut rounds: Vec<u32> = Vec::new();
    for d in 0..=most {
        for k in (-d..=d).step_by(2) {
            let i = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
                v[i + 1]
            } else {
                v[i - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a.line(x as usize) == b.line(y as usize) {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                return Some(walk_back(&rounds, d, n, m));
            }
        }
        rounds.extend((-d..=d).step_by(2).map(|k| v[(offset + k) as usize] as u32));
    }
    None
}

/// The stretches the path [`myers`] found in `last` rounds passes through,
/// read back from its end.
fn walk_back(rounds: &[u32], last: isize, n: isize, m: isize) -> Vec<Hunk> {
    // How far round `d` reached on diagonal `k`.
    let reached = |d: isize, k: isize| rounds[(d * (d + 1) / 2 + (k + d) / 2) as usize] as isize;
    // Each line taken out of `a` or put in from `b`, from the end: where in
    // each the step starts, and whether it put a line in.
    let mut steps: Vec<(usize, usize, bool)> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=last).rev() {
        let k = x - y;
        let put_in = k == -d || (k != d && reached(d - 1, k - 1) < reached(d - 1, k + 1));
        let before = if put_in { k + 1 } else { k - 1 };
        x = reached(d - 1, before);
        y = x - before;
        steps.push((x as usize, y as usize, put_in));
    }
    // Steps with nothing in common between them are one stretch.
    let mut out: Vec<Hunk> = Vec::new();
    for (x, y, put_in) in steps.into_iter().rev() {
        let (a1, b1) = if put_in { (x, y + 1) } else { (x + 1, y) };
        match out.last_mut() {
            Some(h) if h.a1 == x && h.b1 == y => {
                h.a1 = a1;
                h.b1 = b1;
            }
            _ => out.push(Hunk {
                a0: x,
                a1,
                b0: y,
                b1,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    /// The marks as letters, one per new line: `.` the same, `+` added,
    /// `~` modified, with `_` after one that has a deletion below it and `^`
    /// before one that has a deletion above.
    fn drawn(old: &str, new: &str) -> String {
        marks(lines(old).as_slice(), lines(new).as_slice())
            .iter()
            .map(|m| {
                let c = match m.change {
                    Change::Same => ".",
                    Change::Added => "+",
                    Change::Modified => "~",
                };
                format!(
                    "{}{c}{}",
                    if m.deleted_above { "^" } else { "" },
                    if m.deleted_below { "_" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_same_text_has_no_marks() {
        assert_eq!(drawn("a\nb\nc", "a\nb\nc"), ". . .");
        assert_eq!(drawn("", ""), "");
    }

    #[test]
    fn a_line_put_in_is_added() {
        assert_eq!(drawn("a\nc", "a\nb\nc"), ". + .");
        assert_eq!(drawn("a", "new\na"), "+ .");
        assert_eq!(drawn("a", "a\nb\nc"), ". + +");
    }

    #[test]
    fn a_line_changed_is_modified() {
        assert_eq!(drawn("a\nb\nc", "a\nB\nc"), ". ~ .");
    }

    #[test]
    fn a_line_taken_out_marks_the_line_above_it() {
        assert_eq!(drawn("a\nb\nc", "a\nc"), "._ .");
        assert_eq!(drawn("a\nb\nc", "a\nb"), ". ._");
    }

    #[test]
    fn the_first_line_taken_out_marks_the_new_first_line() {
        assert_eq!(drawn("a\nb\nc", "b\nc"), "^. .");
    }

    #[test]
    fn fewer_lines_in_place_of_more_are_modified() {
        assert_eq!(drawn("a\nb\nc\nd", "a\nX\nd"), ". ~ .");
        assert_eq!(drawn("a\nb\nd", "a\nX\nY\nZ\nd"), ". ~ ~ ~ .");
    }

    #[test]
    fn edits_in_several_places_are_each_found() {
        let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven";
        let new = "one\nTWO\nthree\nfour\nfour and a half\nfive\nseven";
        assert_eq!(drawn(old, new), ". ~ . . + ._ .");
    }

    #[test]
    fn a_line_moved_is_taken_out_in_one_place_and_put_in_another() {
        assert_eq!(drawn("a\nb\nc\nd", "b\nc\nd\na"), "^. . . +");
    }

    /// The fewest lines taken out and put in that make `b` of `a`, the
    /// slow way, to hold the diff to.
    fn fewest(a: &[String], b: &[String]) -> usize {
        let mut common = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                common[i][j] = if a[i] == b[j] {
                    common[i + 1][j + 1] + 1
                } else {
                    common[i + 1][j].max(common[i][j + 1])
                };
            }
        }
        a.len() + b.len() - 2 * common[0][0]
    }

    #[test]
    fn the_stretches_found_make_the_new_text_of_the_old_in_the_fewest_steps() {
        // Small texts from a small alphabet, so lines repeat and there is
        // more than one way to get from one to the other.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        for _ in 0..2000 {
            let mut text = |len: u64| -> Vec<String> {
                (0..next(len))
                    .map(|_| ["a", "b", "c", "d"][next(4) as usize].to_string())
                    .collect()
            };
            let (old, new) = (text(12), text(12));
            let found = hunks(old.as_slice(), new.as_slice());
            // Putting each stretch's new lines in place of its old ones
            // gives the new text.
            let mut made: Vec<String> = Vec::new();
            let mut at = 0;
            for h in &found {
                assert!(
                    h.a0 >= at && h.a0 <= h.a1 && h.b0 <= h.b1,
                    "{old:?} {new:?}"
                );
                assert!(h.a1 > h.a0 || h.b1 > h.b0, "an empty stretch");
                made.extend(old[at..h.a0].iter().cloned());
                made.extend(new[h.b0..h.b1].iter().cloned());
                at = h.a1;
            }
            made.extend(old[at..].iter().cloned());
            assert_eq!(made, new, "from {old:?}");
            let steps: usize = found.iter().map(|h| (h.a1 - h.a0) + (h.b1 - h.b0)).sum();
            assert_eq!(steps, fewest(&old, &new), "{old:?} to {new:?}");
        }
    }

    #[test]
    fn a_file_rewritten_top_to_bottom_is_one_changed_stretch() {
        let old: Vec<String> = (0..5000).map(|i| format!("old {i}")).collect();
        let new: Vec<String> = (0..5000).map(|i| format!("new {i}")).collect();
        let m = marks(old.as_slice(), new.as_slice());
        assert!(m.iter().all(|m| m.change == Change::Modified));
    }
}
