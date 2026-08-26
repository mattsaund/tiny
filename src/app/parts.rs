//! Small helpers more than one part of `app` needs.
//!
//! Nothing here touches [`App`](super::App). They are the shared answers to questions that
//! come up in several places — how a path is named in a message, how a list
//! cursor moves, how a command line is split into arguments — kept in one
//! place so the answer cannot differ depending on which file asked.
//!
//! [`safe_join`] is the one that matters. Every path that arrives as text goes
//! through it, and it fails closed twice: once if `..` pops past the start,
//! and again if the result does not sit under the project root.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Filename for a status message, falling back to the full path.
pub(super) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `""` or `"s"`, for building status messages that read properly at n = 1.
pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Move a list cursor the way every list in tiny moves: an arrow or `i`/`k`,
/// Shift or `I`/`K` for the ends, and a page at a time.
///
/// `None` for anything else, which is how each caller keeps its own keys —
/// Enter, Esc, Delete — to itself.
///
/// These are not rebindable, and deliberately: an overlay you cannot get out
/// of because you rebound its keys is a trap, and `keys` says so.
pub(super) fn list_move(key: KeyEvent, at: usize, last: usize, page: usize) -> Option<usize> {
    // A chord is never a movement: `Ctrl+K` is a chord the window may want for
    // something else, not the letter k with a modifier attached.
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    Some(match key.code {
        KeyCode::Up if shift => 0,
        KeyCode::Down if shift => last,
        KeyCode::Char('I') | KeyCode::Home => 0,
        KeyCode::Char('K') | KeyCode::End => last,
        KeyCode::Up | KeyCode::Char('i') => at.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('k') => (at + 1).min(last),
        KeyCode::PageUp => at.saturating_sub(page),
        KeyCode::PageDown => (at + page).min(last),
        _ => return None,
    })
}

/// Byte offset of character index `ci`.
pub(super) fn char_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

/// Split a command line on whitespace, keeping double-quoted runs together so
/// `:replace "old thing" "new thing"` works.
///
/// The `any` flag tracks whether the current argument was *started*, which is
/// what lets `""` produce a deliberate empty argument rather than being
/// dropped as whitespace. There is no escaping — a literal quote cannot be
/// passed, which is a known limit rather than an oversight.
pub(super) fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.trim().chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Join a user-typed name onto `base`, refusing anything that would escape the
/// project root — a typed `../../etc/passwd` must not create a file there.
///
/// **This is a security boundary.** Every path that comes from user input and
/// is then created, renamed to, or written must pass through here. The check
/// is lexical, done by walking components and popping on `..`, so it does not
/// depend on the target existing — and it fails closed twice over: once if
/// `..` pops past the start, and again if the normalised result does not sit
/// under `root`.
///
/// It relies on `root` being canonicalized, which `project::resolve`
/// guarantees. Symlinks inside the project are not resolved, so a symlink
/// pointing outside is not caught here.
pub(super) fn safe_join(base: &Path, name: &str, root: &Path) -> Result<PathBuf> {
    let candidate = base.join(name);
    let mut normalized = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(anyhow!("path escapes the project"));
                }
            }
            std::path::Component::CurDir => {}
            c => normalized.push(c.as_os_str()),
        }
    }
    if !normalized.starts_with(root) {
        return Err(anyhow!("path escapes the project"));
    }
    Ok(normalized)
}
