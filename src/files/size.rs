//! How much disk a file or a folder takes.
//!
//! One number, for the right-hand end of the status line: what the cursor is
//! on, in KiB, MiB or GiB, see [`human`].
//!
//! # A folder is a walk, and a walk has to be allowed to give up
//!
//! A file's size is one `stat`. A folder's is every file under it, which for a
//! `node_modules` or a `target` is hundreds of thousands of them, far too slow
//! to do while someone holds an arrow key down. So the walk carries a deadline
//! and stops when it runs out, and what it reports is honest about that: a
//! measurement that stopped early is marked incomplete and reads as `>1.20 GiB`,
//! which is true, rather than as a total that is quietly wrong.
//!
//! Callers are expected to remember the answer for a folder rather than ask
//! again on every frame, see `App::note_size`.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long one measurement may take before it gives up and says so.
///
/// A number, not a count of files: what has to stay small is the pause between
/// pressing an arrow and seeing the next row, and that is measured in
/// milliseconds however many files happen to be behind it. Fifty of them is
/// below what anyone notices, and reaches every file of an ordinary project
/// folder: the ones it cannot finish are `target` and `node_modules`, whose
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

/// A size in binary units, to two decimal places: `812 B`, `9.50 KiB`,
/// `1.25 MiB`, `47.00 GiB`.
///
/// Each unit is 1024 of the one below, and named so: `KiB` says what `KB`
/// left to be guessed, since disks are sold in powers of ten. The decimals
/// are cut rather than rounded, so a size never reads as more than it is and
/// a byte short of a MiB is `1023.99 KiB`, not a `1024.00 KiB` that should
/// have been `1.00 MiB`. Bytes are counted whole.
pub fn human(size: Size) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1 << 40, "TiB"),
        (1 << 30, "GiB"),
        (1 << 20, "MiB"),
        (1 << 10, "KiB"),
    ];
    // A measurement that gave up early is a floor, not a total.
    let more = if size.complete { "" } else { ">" };
    for (scale, name) in UNITS {
        if size.bytes >= scale {
            let whole = size.bytes / scale;
            let hundredths = (size.bytes % scale) * 100 / scale;
            return format!("{more}{whole}.{hundredths:02} {name}");
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
    fn sizes_are_written_in_binary_units_to_two_places() {
        assert_eq!(human(whole(0)), "0 B");
        assert_eq!(human(whole(812)), "812 B");
        assert_eq!(human(whole(1024)), "1.00 KiB");
        assert_eq!(human(whole(9 * 1024 + 512)), "9.50 KiB");
        assert_eq!(human(whole(12 * 1024)), "12.00 KiB");
        assert_eq!(human(whole(1024 + 52)), "1.05 KiB", "the zero is kept");
        assert_eq!(human(whole(3 * 1024 * 1024 / 2)), "1.50 MiB");
        assert_eq!(human(whole(47 * 1024 * 1024 * 1024)), "47.00 GiB");
        assert_eq!(human(whole(5 << 40)), "5.00 TiB");
        // Cut, not rounded: a byte short of a MiB is not a MiB.
        assert_eq!(human(whole((1 << 20) - 1)), "1023.99 KiB");
    }

    #[test]
    fn a_measurement_that_gave_up_says_it_is_a_floor() {
        let partial = Size {
            bytes: 5 * 1024 * 1024,
            complete: false,
        };
        assert_eq!(human(partial), ">5.00 MiB");
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
