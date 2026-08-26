//! Text, and what can be done to it.
//!
//! Everything here works on strings and knows nothing about panes, keys, or
//! the filesystem. That is the whole reason the folder exists: these are the
//! four pieces with real algorithms in them, and keeping them free of `App`
//! is what lets each one be tested against plain input and plain output.
//!
//! - [`editor`] — the buffer: lines, a cursor, and undo.
//! - [`markdown`] — markdown to styled terminal lines.
//! - [`highlight`] — syntax colors, and the parser-state cache that keeps
//!   them cheap.
//! - [`search`] — finding a string, in one file or in a whole project.
//!
//! [`search::Matcher`] is the one piece shared sideways: both the search
//! itself and the marking of hits in the preview go through it, so "what
//! counts as a match" is decided in exactly one place.

pub mod editor;
pub mod highlight;
pub mod markdown;
pub mod search;
