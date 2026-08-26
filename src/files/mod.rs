//! The filesystem, as tiny sees it.
//!
//! Three separate questions about things on disk:
//!
//! - [`project`] — what `tiny <thing>` meant, answered once at startup.
//! - [`tree`] — the directory model the left pane walks, loaded lazily and
//!   refreshed on demand rather than watched.
//! - [`media`] — what a picture or a video is, and how to hand it to the
//!   desktop's own viewer.
//!
//! Nothing here writes to disk except [`project`], and only on the one path
//! where it creates the folder you asked for. File operations the user
//! triggers — new, rename, delete, copy — live in `app::fileops`,
//! because each one has a status message and an undo story attached and those
//! belong with the state they change.

pub mod media;
pub mod project;
pub mod tree;
