//! Spreading one job over every core.
//!
//! The two things tiny does to a whole project at once (search it, and map
//! it) are a loop over files where no file needs another's answer. Reading
//! and parsing them one at a time leaves every core but one idle while the
//! user waits, so both hand the loop to [`par_map`] instead.
//!
//! Plain scoped threads rather than a thread-pool crate: the work arrives as
//! one batch, the threads live exactly as long as it, and nothing outlives the
//! call, which is all a pool would be for.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `work` over every item on every core, each answer kept at its item's index
/// so the result is the same as a plain loop would give.
///
/// Items are handed out one at a time from a shared counter rather than split
/// into fixed chunks up front, because what they cost is wildly uneven (one
/// generated parser is worth a thousand notes) and a fixed split leaves
/// cores idle behind whichever chunk drew the expensive ones. Each thread
/// gets its own `state`, for things like a tree-sitter context that cannot be
/// shared.
pub fn par_map<T: Sync, R: Send, S>(
    items: &[T],
    state: impl Fn() -> S + Sync,
    work: impl Fn(&mut S, usize, &T) -> R + Sync,
) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut st = state();
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        done.push((i, work(&mut st, i, item)));
                    }
                    done
                })
            })
            .collect();
        for worker in workers {
            for (i, r) in worker.join().expect("a worker thread panicked") {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every item is taken by some worker"))
        .collect()
}

/// How many threads [`par_map`] will use, for callers that size their batches
/// by it.
pub fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_come_back_in_the_order_of_the_items() {
        let items: Vec<usize> = (0..1000).collect();
        let out = par_map(&items, || (), |_, i, x| (i, x * 2));
        for (n, (i, doubled)) in out.into_iter().enumerate() {
            assert_eq!((i, doubled), (n, n * 2));
        }
    }

    #[test]
    fn nothing_to_do_is_nothing_done() {
        let out: Vec<u8> = par_map(&[] as &[u8], || (), |_, _, x| *x);
        assert!(out.is_empty());
    }
}
