//! Following a link: a click on one, see [`super::mouse`].
//!
//! A web address goes to the desktop's browser, and a `mailto:` to its mail
//! program. A link to a file opens the file here: a markdown link's path is
//! found the way the map finds it, beside the note and then from the top of
//! the project, and a `[[wikilink]]` is the note of that name wherever it is.
//! A heading after a `#` is where the file opens.

use std::path::{Path, PathBuf};

use super::App;
use super::mouse::LinkHit;
use super::parts::display_name;
use crate::map::scan::{normalize, percent_decoded};
use crate::text::markdown::Target;
use crate::text::search;

/// What a link to a note may leave off.
const NOTE_EXTENSIONS: &[&str] = &["md", "markdown", "txt"];

impl App {
    /// Go where `link` goes.
    pub(super) fn follow(&mut self, link: &LinkHit) {
        let from = link.from.parent().map(Path::to_path_buf);
        let (found, heading, written) = match &link.target {
            Target::Web(url) => {
                self.status = match crate::files::media::open_url(url) {
                    Ok(()) => format!("opened {url}"),
                    Err(e) => format!("{e:#}"),
                };
                return;
            }
            Target::Path(dest) => {
                let (file, heading) = split_heading(dest);
                let file = percent_decoded(file.split('?').next().unwrap_or(file));
                let found = if file.is_empty() {
                    Some(link.from.clone())
                } else {
                    self.find_path(&file, from.as_deref())
                };
                (found, heading, dest.clone())
            }
            Target::Wiki(name) => {
                let (note, heading) = split_heading(name);
                let found = if note.is_empty() {
                    Some(link.from.clone())
                } else {
                    self.find_note(note.trim(), from.as_deref())
                };
                (found, heading, format!("[[{name}]]"))
            }
        };
        let Some(path) = found else {
            self.status = format!("nothing here is {written}");
            return;
        };
        self.open_path(&path);
        if !heading.is_empty() {
            self.go_to_heading(&path, heading);
        }
    }

    /// The file a markdown link's path names: beside the note, then from the
    /// top of the project, where a path starting with `/` begins too, and
    /// with a note's extension if it was left off. A `..` is gone from what
    /// comes back: the browser's rows are spelled without one, and a path
    /// with one in it would match none of them.
    fn find_path(&self, file: &str, from: Option<&Path>) -> Option<PathBuf> {
        let root = self.root().to_path_buf();
        let mut places = Vec::new();
        if let Some(dir) = from
            && !file.starts_with('/')
        {
            places.push(dir.join(file));
        }
        places.push(root.join(file.trim_start_matches('/')));
        places
            .into_iter()
            .map(|p| normalize(&p))
            .flat_map(with_note_extensions)
            .find(|p| p.is_file())
    }

    /// The note a wikilink names: beside the linking note, from the top of
    /// the project, and then anywhere in it by name, the one nearest the
    /// linking note when there are several.
    fn find_note(&self, name: &str, from: Option<&Path>) -> Option<PathBuf> {
        if let Some(path) = self.find_path(name, from) {
            return Some(path);
        }
        let wanted = name.to_lowercase();
        let ends = format!("/{wanted}");
        let walked = search::walk_all(self.root(), &self.search_opts_for_links());
        let mut found: Vec<PathBuf> = walked
            .files
            .into_iter()
            .filter(|p| {
                let rel = p
                    .strip_prefix(self.root())
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_lowercase();
                let bare = rel.rsplit_once('.').map_or(rel.as_str(), |(stem, ext)| {
                    if NOTE_EXTENSIONS.contains(&ext) {
                        stem
                    } else {
                        rel.as_str()
                    }
                });
                bare == wanted || bare.ends_with(&ends)
            })
            .collect();
        // Nearest the note the link is in: the most folders in common, then
        // the shortest path.
        let near = |p: &PathBuf| {
            let common = from.map_or(0, |dir| {
                p.components()
                    .zip(dir.components())
                    .take_while(|(a, b)| a == b)
                    .count()
            });
            (std::cmp::Reverse(common), p.components().count())
        };
        found.sort_by_key(near);
        found.into_iter().next()
    }

    fn search_opts_for_links(&self) -> search::Opts {
        search::Opts {
            max_results: usize::MAX,
            ignore: self.config.search_ignore.clone(),
            show_hidden: self.config.show_hidden,
        }
    }

    /// Put the cursor on the heading in `path` called `heading`, however
    /// a link spelled it: `#Getting Started` and `#getting-started` both.
    fn go_to_heading(&mut self, path: &Path, heading: &str) {
        let wanted = slug(heading);
        let Some(ed) = self.buffers.get_mut(path) else {
            return;
        };
        let line = ed.lines().iter().position(|l| {
            let t = l.trim_start();
            t.starts_with('#') && slug(t.trim_start_matches('#')) == wanted
        });
        match line {
            Some(n) => {
                ed.goto(n, 0);
                self.status = format!("{}, at {heading}", display_name(path));
            }
            None => self.status = format!("{} has no heading {heading}", display_name(path)),
        }
    }
}

/// A link's file, and the heading after its `#`, if any.
fn split_heading(dest: &str) -> (&str, &str) {
    dest.split_once('#').unwrap_or((dest, ""))
}

/// `path`, and with each of a note's extensions when it has none of its own.
fn with_note_extensions(path: PathBuf) -> Vec<PathBuf> {
    let mut out = vec![path.clone()];
    if path.extension().is_none() {
        out.extend(NOTE_EXTENSIONS.iter().map(|e| path.with_extension(e)));
    }
    out
}

/// A heading as a link names it: lowercase, spaces as dashes, and nothing
/// but letters, digits and dashes.
fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' => Some('-'),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}
