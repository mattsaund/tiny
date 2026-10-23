//! Small helpers more than one part of `app` needs.
//!
//! Nothing here touches [`App`](super::App). They are the shared answers to questions that
//! come up in several places (how a path is named in a message, how a list
//! cursor moves, how a one-line field is typed into) kept in one place so
//! the answer cannot differ depending on which file asked.
//!
//! [`safe_join`] is the one that matters. Every path that arrives as text goes
//! through it, and it fails closed twice: once if `..` pops past the start,
//! and again if the result does not sit under the project root.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::text::editor::byte_of_char;

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
/// `None` for anything else, which is how each caller keeps its own keys
/// (Enter, Esc, Delete) to itself.
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
        // Saturating: the help stores "the end" as the largest number there
        // is, for the frame to bring back into range, and a key can arrive
        // before that frame does.
        KeyCode::Down | KeyCode::Char('k') => at.saturating_add(1).min(last),
        KeyCode::PageUp => at.saturating_sub(page),
        KeyCode::PageDown => at.saturating_add(page).min(last),
        _ => return None,
    })
}

/// One key typed into a one-line field, with its cursor counted in
/// characters and never put before `floor`. False for a key a field has no
/// use for, so the caller can give it a meaning of its own.
///
/// The bar and the settings fields type the same way, so Backspace cannot
/// mean one thing in one and something else in the other. What Enter, Esc
/// and Tab do stays with each of them.
pub(super) fn edit_field(
    text: &mut String,
    cursor: &mut usize,
    floor: usize,
    key: KeyEvent,
) -> bool {
    let len = text.chars().count();
    match key.code {
        // Backspace is the one key allowed under the floor: the bar's `*` is
        // there to be taken away, which turns a command back into a search.
        KeyCode::Backspace if *cursor > 0 => {
            let at = byte_of_char(text, *cursor - 1)..byte_of_char(text, *cursor);
            text.replace_range(at, "");
            *cursor -= 1;
        }
        KeyCode::Delete if *cursor < len => {
            let at = byte_of_char(text, *cursor)..byte_of_char(text, *cursor + 1);
            text.replace_range(at, "");
        }
        KeyCode::Backspace | KeyCode::Delete => {}
        KeyCode::Left => *cursor = cursor.saturating_sub(1).max(floor),
        KeyCode::Right => *cursor = (*cursor + 1).min(len),
        KeyCode::Home => *cursor = floor,
        KeyCode::End => *cursor = len,
        KeyCode::Char(c) => {
            text.insert(byte_of_char(text, *cursor), c);
            *cursor += 1;
        }
        _ => return false,
    }
    true
}

/// Join a user-typed name onto `base`, refusing anything that would escape the
/// project root: a typed `../../etc/passwd` must not create a file there.
///
/// **This is a security boundary.** Every path that comes from user input and
/// is then created, renamed to, or written must pass through here. The check
/// is lexical, done by walking components and popping on `..`, so it does not
/// depend on the target existing, and it fails closed twice over: once if
/// `..` pops past the start, and again if the normalized result does not sit
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
