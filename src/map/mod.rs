//! What links to what, and the screen that shows it.
//!
//! Two layers, deliberately apart:
//!
//! - [`graph`] — the facts. Which files exist, and which of them reach which
//!   others, by wikilink, by markdown link, or by calling a function defined
//!   somewhere else. Built by scanning the project; knows nothing about
//!   drawing.
//! - [`view`] — the thing you look at. Takes a [`graph::Graph`], decides
//!   what is on screen, where each box sits, and what the cursor is on.
//!
//! The split is what keeps the layout testable: [`view`] can be asked where
//! it put a box without a terminal existing. Actually painting the boxes and
//! the lines between them is `ui::map`'s job, one layer further out
//! again.

#[cfg(test)]
pub(crate) mod testing;

pub mod graph;
pub mod layout;
pub mod scan;
pub mod view;
