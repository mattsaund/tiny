//! How much disk a file or a folder takes.
//!
//! One number, for the right-hand end of the status line: what the cursor is
//! on, in the units a person would say it in.
//!
//! # A folder is a walk, and a walk has to be allowed to give up
//!
//! A file's size is one `stat`. A folder's is every file under it, which for a
//! `node_modules` or a `target` is hundreds of thousands of them — far too slow
//! to do while someone holds an arrow key down. So the walk carries a deadline
//! and stops when it runs out, and what it reports is honest about that: a
//! measurement that stopped early is marked incomplete and reads as `>1.2 GB`,
//! which is true, rather than as a total that is quietly wrong.
//!
//! Callers are expected to remember the answer for a folder rather than ask
//! again on every frame — see `App::note_size`.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long one measurement may take before it gives up and says so.
///
/// A number, not a count of files: what has to stay small is the pause between
/// pressing an arrow and seeing the next row, and that is measured in
/// milliseconds however many files happen to be behind it. Fifty of them is
/// below what anyone notices, and reaches every file of an ordinary project
/// folder — the ones it cannot finish are `target` and `node_modules`, whose
/// exact size nobody is sitting there waiting for.
const DEADLINE: Duration = Duration::from_millis(50);

/// Entries to count between two readings of the clock.
///
/// Asking the time is cheap but not free, and a folder of a million files
/// would ask a million times. Once per few hundred `stat` calls keeps the
/// check itself off the measurement.
const CHECK_EVERY: usize = 512;

/// A measured size, and whether the measurement is the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    /// False when the walk ran out of time and stopped. The folder holds *at
    /// least* this much, and [`human`] says so with a `>`.
    pub complete: bool,
}

/// Measure one path: a file's own length, or everything under a folder.
///
/// Symlinks are counted as the nothing they are rather than followed: a link
/// into a folder that contains it would otherwise walk forever, and a link to
/// a file elsewhere would count bytes that are not in this folder at all.
pub fn of(path: &Path) -> Option<Size> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(Size {
            bytes: meta.len(),
            complete: true,
        });
    }
    let mut bytes = 0;
    let mut seen = 0;
    let started = Instant::now();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        // An unreadable folder is skipped rather than abandoning the whole
        // measurement: a total missing one locked subfolder is worth more than
        // no total at all.
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen % CHECK_EVERY == 0 && started.elapsed() > DEADLINE {
                return Some(Size {
                    bytes,
                    complete: false,
                });
            }
            // `DirEntry::metadata` does not follow symlinks, which is what
            // keeps a link out of the total and off the stack.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                bytes += meta.len();
            }
        }
    }
    Some(Size {
        bytes,
        complete: true,
    })
}

/// A size as someone would say it: `812 B`, `9.4 KB`, `1.2 MB`, `47 GB`.
///
/// One decimal below ten units and none above, because the decimal stops
/// carrying information once there are two digits in front of it, and the
/// status line is shared with everything else on it.
///
/// The units step by 1024 and are named the short way. Disks are sold in
/// powers of ten and file managers disagree with each other about this; what
/// matters here is that the number next to `MB` is the one the rest of the
/// system will call a megabyte too.
pub fn human(size: Size) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GB"), (1 << 20, "MB"), (1 << 10, "KB")];
    // A measurement that gave up early is a floor, not a total.
    let more = if size.complete { "" } else { ">" };
    for (scale, name) in UNITS {
        if size.bytes >= scale {
            let whole = size.bytes / scale;
            if whole < 10 {
                let tenths = (size.bytes % scale) * 10 / scale;
                return format!("{more}{whole}.{tenths} {name}");
            }
            return format!("{more}{whole} {name}");
        }
    }
    format!("{more}{} B", size.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn whole(bytes: u64) -> Size {
        Size {
            bytes,
            complete: true,
        }
    }

    #[test]
    fn sizes_are_written_the_way_people_say_them() {
        assert_eq!(human(whole(0)), "0 B");
        assert_eq!(human(whole(812)), "812 B");
        assert_eq!(human(whole(1024)), "1.0 KB");
        assert_eq!(human(whole(9 * 1024 + 512)), "9.5 KB");
        // Ten and over, the decimal has stopped telling anyone anything.
        assert_eq!(human(whole(12 * 1024)), "12 KB");
        assert_eq!(human(whole(3 * 1024 * 1024 / 2)), "1.5 MB");
        assert_eq!(human(whole(47 * 1024 * 1024 * 1024)), "47 GB");
    }

    #[test]
    fn a_measurement_that_gave_up_says_it_is_a_floor() {
        let partial = Size {
            bytes: 5 * 1024 * 1024,
            complete: false,
        };
        assert_eq!(human(partial), ">5.0 MB");
    }

    #[test]
    fn a_folder_counts_everything_under_it() {
        let td = tempfile::tempdir().unwrap();
        fs::write(td.path().join("a.txt"), vec![b'x'; 100]).unwrap();
        fs::create_dir(td.path().join("deep")).unwrap();
        fs::write(td.path().join("deep/b.txt"), vec![b'y'; 250]).unwrap();

        let file = of(&td.path().join("a.txt")).unwrap();
        assert_eq!((file.bytes, file.complete), (100, true));

        // The folder's own entry has a size of its own on every filesystem,
        // and it is not part of what is *in* the folder.
        let dir = of(td.path()).unwrap();
        assert_eq!((dir.bytes, dir.complete), (350, true), "100 + 250");
    }

    #[test]
    fn nothing_on_disk_measures_to_nothing_at_all() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(of(&td.path().join("nope")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_and_not_counted() {
        let td = tempfile::tempdir().unwrap();
        fs::write(td.path().join("real.txt"), vec![b'x'; 100]).unwrap();
        fs::create_dir(td.path().join("sub")).unwrap();
        // A link pointing back at the folder holding it: following this would
        // walk until the budget ran out.
        std::os::unix::fs::symlink(td.path(), td.path().join("sub/loop")).unwrap();

        let dir = of(td.path()).unwrap();
        assert_eq!(
            (dir.bytes, dir.complete),
            (100, true),
            "the real file, once, and the walk terminated"
        );
    }
}
