//! What a command would do, here, now, and then doing it.
//!
//! [`super::sentence`] reads a line into words with parts; this is the half
//! that knows the project. It finds the files a sentence names, counts what a
//! replace would touch, and settles every question into a [`Plan`], which
//! the bar describes while you type, and which Enter carries out. The two are
//! one thing on purpose: the line under the bar says what Enter will do
//! because it is reading the very plan Enter will follow.
//!
//! # Names
//!
//! A sentence says `notes`, not `projects/2026/notes`. [`App::find`] looks a
//! name up the way a person would mean it: as a path from the folder you are
//! in, then from the top of the project, then as the name of any one file or
//! folder anywhere in it. When several answer to a name the plan stops and
//! lists them rather than picking one: copying into the wrong `notes` is
//! worse than being asked which.
//!
//! The project's files are listed once while the bar is open, so looking a
//! name up as you type is a lookup and not a walk; running the command lists
//! them afresh, so what Enter acts on is the disk as it is.

use std::path::{Path, PathBuf};

use anyhow::anyhow;

use crate::map::scan;
use crate::parallel::par_map;
use crate::text::editor::Editor;
use crate::text::search::{self, Hit, HitKind, Matcher, Needle, Report};

use super::App;
use super::mode::{Confirm, ConfirmKind, Focus, Mode, Window as AppWindow};
use super::parts::{display_name, plural, safe_join};
use super::preview::{Preview, read_editable};
use super::sentence::{self, Place, Sentence, Target, Window};

/// Most places a `goto` collects before it stops counting.
const MAX_SPOTS: usize = 1000;

/// Past this many code files, the bar does not read them all to say how many
/// functions answer to a name: it says it will look, and Enter looks.
const MAX_PREVIEW_CODE_FILES: usize = 2000;

/// Every file and folder in the project, listed once for looking names up in,
/// and, once a `goto` has asked, every definition and heading in it, so
/// typing the rest of a name filters a list rather than reading the project
/// again.
#[derive(Debug, Default)]
pub struct Names {
    files: Vec<PathBuf>,
    folders: Vec<PathBuf>,
    /// Every function-like definition, by name.
    definitions: Option<Vec<(String, Hit)>>,
    /// Every markdown heading, by its title in lower case.
    headings: Option<Vec<(String, Hit)>>,
}

impl Names {
    /// Every file and folder name, each once, sorted: what Tab offers. A
    /// folder's ends with a separator, the way a typed path to one would, so
    /// Tab can carry straight on into it.
    pub fn all_names(&self) -> Vec<String> {
        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned());
        let mut out: Vec<String> = self
            .files
            .iter()
            .filter_map(name)
            .chain(
                self.folders
                    .iter()
                    .filter_map(name)
                    .map(|n| format!("{n}{}", std::path::MAIN_SEPARATOR)),
            )
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// What a name may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    File,
    Folder,
    Either,
}

/// A command with every name in it found and every count counted.
#[derive(Debug)]
pub(super) enum Plan {
    /// Go to one of several places: `which` is the one this run goes to.
    Jump {
        spots: Vec<Hit>,
        which: usize,
        what: String,
    },
    /// A line of a file; `None` for the one open now.
    Line {
        path: Option<PathBuf>,
        line: usize,
    },
    Open(PathBuf),
    Reveal(PathBuf),
    Window(Window),
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Delete(PathBuf),
    New {
        path: PathBuf,
        folder: bool,
    },
    ReplaceProject {
        needle: Needle,
        with: String,
        report: Report,
    },
    ReplaceFile {
        path: PathBuf,
        needle: Needle,
        with: String,
        count: usize,
    },
    Set(Vec<String>),
    Commit(String),
    Reload,
    Update,
    Plugin {
        name: String,
        args: Vec<String>,
    },
}

impl App {
    // ---- names ------------------------------------------------------------

    /// The project's files and folders, listed if they are not already.
    fn names(&mut self) -> &Names {
        if self.names.is_none() {
            let walked = search::walk_all(self.tree.root_path(), &self.search_opts());
            self.names = Some(Names {
                files: walked.files,
                folders: walked.folders,
                ..Names::default()
            });
        }
        self.names.as_ref().expect("listed just above")
    }

    /// Forget the listing, so the next lookup sees the disk as it is now.
    pub(super) fn forget_names(&mut self) {
        self.names = None;
    }

    /// A path as the project spells it: from the root, forward slashes.
    pub(super) fn rel(&self, path: &Path) -> String {
        let root = self.tree.root_path();
        if path == root {
            return format!("{}/", display_name(root));
        }
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// The file, or folder, a sentence means by `place`.
    ///
    /// In order: what the cursor is on, for `this`; a path from the folder
    /// the cursor is in; a path from the top of the project; and then the
    /// name of any one thing in the project. Several things with the name is
    /// an error that lists them, so it can be said again more exactly.
    fn find(&mut self, place: &Place, want: Want) -> Result<PathBuf, String> {
        let name = match place {
            Place::This => return self.this(want),
            Place::Named(name) => name
                .trim()
                .trim_start_matches("./")
                .trim_end_matches(['/', '\\']),
        };
        if name.is_empty() {
            return self.this(want);
        }
        let root = self.tree.root_path().to_path_buf();
        let fits = |p: &Path| match want {
            Want::File => p.is_file(),
            Want::Folder => p.is_dir(),
            Want::Either => p.exists(),
        };
        // A path that would leave the project from both places says so,
        // rather than being looked up as a name and not found.
        let mut escaped = None;
        for base in [self.creation_base(), root.clone()] {
            match safe_join(&base, name, &root) {
                Ok(p) if fits(&p) => return Ok(p),
                Ok(_) => escaped = None,
                Err(e) => escaped = escaped.or(Some(format!("{e:#}"))),
            }
        }
        if let Some(e) = escaped {
            return Err(e);
        }

        let wanted = name.replace('\\', "/").to_lowercase();
        let rel_of = |p: &Path| {
            p.strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase()
        };
        self.names();
        let names = self.names.as_ref().expect("listed just above");
        let pool: Vec<&PathBuf> = match want {
            Want::File => names.files.iter().collect(),
            Want::Folder => names.folders.iter().collect(),
            Want::Either => names.files.iter().chain(&names.folders).collect(),
        };
        // The whole name first; then, for a file, the name without its
        // extension: `goto file readme` is plainly README.md.
        let by_name: Vec<&PathBuf> = pool
            .iter()
            .copied()
            .filter(|p| {
                let rel = rel_of(p);
                if wanted.contains('/') {
                    rel == wanted || rel.ends_with(&format!("/{wanted}"))
                } else {
                    rel.rsplit('/').next() == Some(wanted.as_str())
                }
            })
            .collect();
        let found = if by_name.is_empty() && !wanted.contains('/') {
            pool.iter()
                .copied()
                .filter(|p| {
                    p.is_file()
                        && p.file_stem()
                            .is_some_and(|s| s.to_string_lossy().to_lowercase() == wanted)
                })
                .collect()
        } else {
            by_name
        };

        let what = match want {
            Want::File => "file",
            Want::Folder => "folder",
            Want::Either => "file or folder",
        };
        match found.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(format!("no {what} called {name} in this project")),
            many => {
                let listed: Vec<String> = many.iter().take(3).map(|p| self.rel(p)).collect();
                let more = if many.len() > 3 {
                    format!(" and {} more", many.len() - 3)
                } else {
                    String::new()
                };
                Err(format!(
                    "{} things are called {name}: {}{more}. Say which, like {}",
                    many.len(),
                    listed.join(", "),
                    listed[0]
                ))
            }
        }
    }

    /// A file in the project called exactly `name`, or at exactly that path
    /// from a folder: unlike [`App::find`], never by its name without the
    /// extension, so `archive` is not `archive.md`.
    fn file_named(&mut self, name: &str) -> Option<PathBuf> {
        let wanted = name
            .trim()
            .trim_start_matches("./")
            .replace('\\', "/")
            .to_lowercase();
        let root = self.tree.root_path().to_path_buf();
        self.names();
        let names = self.names.as_ref()?;
        names
            .files
            .iter()
            .find(|p| {
                let rel = p
                    .strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_lowercase();
                if wanted.contains('/') {
                    rel == wanted || rel.ends_with(&format!("/{wanted}"))
                } else {
                    rel.rsplit('/').next() == Some(wanted.as_str())
                }
            })
            .cloned()
    }

    /// What `this` is: the file being edited, or what the cursor is on.
    fn this(&self, want: Want) -> Result<PathBuf, String> {
        if let (Want::File | Want::Either, Preview::Buffer { path, .. }) = (want, &self.preview)
            && self.focus == Focus::Editor
        {
            return Ok(path.clone());
        }
        let row = self
            .selected_row()
            .ok_or_else(|| "nothing is selected".to_string())?;
        match want {
            Want::Folder if !row.is_dir => Ok(row
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root().to_path_buf())),
            Want::File if row.is_dir => Err(format!("{} is a folder", row.name)),
            _ => Ok(row.path.clone()),
        }
    }

    /// The text of a file: the open buffer's, unsaved edits and all, or the
    /// disk's.
    fn text_of(&self, path: &Path) -> Option<String> {
        match self.buffers.get(path) {
            Some(ed) => Some(ed.to_text()),
            None => read_editable(path),
        }
    }

    /// Every file open with edits not yet saved.
    pub(super) fn unsaved_paths(&self) -> std::collections::HashSet<PathBuf> {
        self.dirty_buffers()
            .into_iter()
            .map(Path::to_path_buf)
            .collect()
    }

    // ---- planning ---------------------------------------------------------

    /// Turn a sentence into a plan, or say why it cannot be one.
    ///
    /// `line` is the command as typed, which is how running the same `goto`
    /// twice knows to go on to the next place rather than back to the first.
    pub(super) fn plan(&mut self, said: &Sentence, line: &str) -> Result<Plan, String> {
        match said {
            Sentence::Empty => Err("type a command, ? lists them".into()),
            Sentence::Incomplete(how) => Err(how.clone()),
            Sentence::Goto { target, within } => self.plan_goto(target, within.as_ref(), line),
            Sentence::Copy { from, to } => {
                let from = self.find(from, Want::Either)?;
                let to = self.destination(&from, to, true)?;
                Ok(Plan::Copy { from, to })
            }
            Sentence::Move { from, to, rename } => {
                let from = self.find(from, Want::Either)?;
                if from == self.tree.root_path() {
                    let verb = if *rename { "rename" } else { "move" };
                    return Err(format!("cannot {verb} the project folder itself"));
                }
                let to = self.destination(&from, to, !rename)?;
                Ok(Plan::Move { from, to })
            }
            Sentence::Delete(what) => {
                let path = self.find(what, Want::Either)?;
                if path == self.tree.root_path() {
                    return Err("cannot delete the project folder itself".into());
                }
                Ok(Plan::Delete(path))
            }
            Sentence::New {
                name,
                folder,
                within,
            } => {
                let base = match within {
                    Some(place) => self.find(place, Want::Folder)?,
                    None => self.creation_base(),
                };
                let path =
                    safe_join(&base, name, self.tree.root_path()).map_err(|e| format!("{e:#}"))?;
                if path.exists() {
                    return Err(format!("{} already exists", self.rel(&path)));
                }
                Ok(Plan::New {
                    path,
                    folder: *folder,
                })
            }
            Sentence::Replace {
                find,
                with,
                within,
                whole_words,
            } => self.plan_replace(find, with, within.as_ref(), *whole_words),
            Sentence::Set(args) => Ok(Plan::Set(args.clone())),
            Sentence::Commit(message) => Ok(Plan::Commit(message.clone())),
            Sentence::Reload => Ok(Plan::Reload),
            Sentence::Update => Ok(Plan::Update),
            Sentence::Other { name, args } => {
                // A plugin's own command comes first; failing that, a word on
                // its own is a name to go to.
                if self.plugins.commands().iter().any(|c| c == name) {
                    return Ok(Plan::Plugin {
                        name: name.clone(),
                        args: args.clone(),
                    });
                }
                let whole = std::iter::once(name.as_str())
                    .chain(args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                // While it is being typed, a word tiny does not know is most
                // likely a verb half-written. Only a file or folder that
                // answers to it is looked for (reading the whole project for
                // `cop` on the way to `copy` would be a waste) and Enter
                // looks for the rest.
                if self.planning_for_preview {
                    return match self.find(&Place::Named(whole.clone()), Want::Either) {
                        Ok(p) if p.is_dir() => Ok(Plan::Reveal(p)),
                        Ok(p) => Ok(Plan::Open(p)),
                        Err(e) if !e.starts_with("no ") => Err(e),
                        Err(_) => Err(format!(
                            "`{whole}` is not a command. Tab completes one, ? lists them"
                        )),
                    };
                }
                self.plan_goto(&Target::Anything(whole.clone()), None, line)
                    .map_err(|_| {
                        format!(
                            "`{whole}` is not a command or a name in this project. ? lists them"
                        )
                    })
            }
        }
    }

    /// Where something copied or moved to `to` ends up.
    ///
    /// A folder that answers to the name is gone *into*, when `into` allows
    /// it; otherwise `to` is the new name: beside the original if it is a
    /// bare name, from the top of the project if it has a slash in it.
    ///
    /// An existing path is refused. So, for a copy or a move, is naming a
    /// file the project already has anywhere: that file is almost certainly
    /// the one meant, and writing a second beside the original would be a
    /// surprise. A rename names the new name and nothing else, so only its
    /// own path counts.
    fn destination(&mut self, from: &Path, to: &str, into: bool) -> Result<PathBuf, String> {
        let name = from
            .file_name()
            .ok_or_else(|| "that has no name to keep".to_string())?
            .to_os_string();
        if into {
            match self.find(&Place::Named(to.to_string()), Want::Folder) {
                Ok(folder) => {
                    let path = folder.join(name);
                    if path.exists() {
                        return Err(format!("{} already exists", self.rel(&path)));
                    }
                    return Ok(path);
                }
                // Several folders called that is a question, not a new name.
                Err(e) if !e.starts_with("no ") => return Err(e),
                Err(_) => {}
            }
            if let Some(there) = self.file_named(to) {
                return Err(format!("{} already exists", self.rel(&there)));
            }
        }
        let root = self.tree.root_path().to_path_buf();
        let base = if to.contains(['/', '\\']) {
            root.clone()
        } else {
            from.parent().map(Path::to_path_buf).unwrap_or(root.clone())
        };
        let path = safe_join(&base, to, &root).map_err(|e| format!("{e:#}"))?;
        // On a disk that ignores case, a rename to `readme.md` finds
        // `README.md` already there, being itself.
        if path.exists() && !super::fileops::same_file(&path, from) {
            return Err(format!("{} already exists", self.rel(&path)));
        }
        Ok(path)
    }

    fn plan_goto(
        &mut self,
        target: &Target,
        within: Option<&Place>,
        line: &str,
    ) -> Result<Plan, String> {
        // Which file to look in, when one was named. Folders and windows are
        // not looked for inside anything.
        let file = match (target, within) {
            (Target::File(_) | Target::Folder(_) | Target::Window(_), _) | (_, None) => None,
            (_, Some(place)) => Some(self.find(place, Want::File)?),
        };
        let spots = match target {
            Target::Line(n) => {
                let path = file;
                if path.is_none() && !matches!(self.preview, Preview::Buffer { .. }) {
                    return Err(format!("no file open, say which: line {n} in notes.md"));
                }
                if *n == 0 {
                    return Err("lines are counted from 1".into());
                }
                return Ok(Plan::Line { path, line: *n });
            }
            Target::File(name) => {
                return self
                    .find(&Place::Named(name.clone()), Want::File)
                    .map(Plan::Open);
            }
            Target::Folder(name) => {
                return self
                    .find(&Place::Named(name.clone()), Want::Folder)
                    .map(Plan::Reveal);
            }
            Target::Window(w) => return Ok(Plan::Window(*w)),
            Target::Function(name) => self.definitions(name, file)?,
            Target::Heading(name) => self.headings(name, file),
            Target::Word(word) => self.occurrences(word, file, true),
            Target::Text(text) => self.occurrences(text, file, false),
            Target::Anything(name) => {
                // A file or folder by that name first, since that is what a
                // bare name most often is.
                match self.find(&Place::Named(name.clone()), Want::Either) {
                    Ok(p) if p.is_dir() => return Ok(Plan::Reveal(p)),
                    Ok(p) => return Ok(Plan::Open(p)),
                    Err(e) if !e.starts_with("no ") => return Err(e),
                    Err(_) => {}
                }
                let mut spots = self.definitions(name, file.clone()).unwrap_or_default();
                if spots.is_empty() {
                    spots = self.headings(name, file.clone());
                }
                if spots.is_empty() {
                    spots = self.occurrences(name, file, true);
                }
                spots
            }
        };
        if spots.is_empty() {
            let where_ = match within {
                Some(Place::Named(n)) => format!(" in {n}"),
                Some(Place::This) => " in this file".into(),
                None => String::new(),
            };
            return Err(format!("nothing{where_} matches {}", what_of(target)));
        }
        // The same command again goes on to the next place.
        let which = match &self.last_jump {
            Some((said, at)) if said == line.trim() => (at + 1) % spots.len(),
            _ => 0,
        };
        Ok(Plan::Jump {
            spots,
            which,
            what: what_of(target),
        })
    }

    /// Where functions called `name` are defined, in one file or the whole
    /// project. An exact match wins; failing any, the same name in another
    /// case.
    fn definitions(&mut self, name: &str, file: Option<PathBuf>) -> Result<Vec<Hit>, String> {
        let all = match file {
            Some(f) => self.read_definitions(&[f]),
            None => match self.names().definitions.clone() {
                Some(all) => all,
                None => {
                    let files = self.names().files.clone();
                    let code = files.iter().filter(|p| scan::lang_of(p).is_some()).count();
                    // Too many to read on a pause in typing; Enter reads them.
                    if code > MAX_PREVIEW_CODE_FILES && self.planning_for_preview {
                        return Err(format!(
                            "Enter looks for function {name} in {code} code files"
                        ));
                    }
                    let all = self.read_definitions(&files);
                    if let Some(names) = self.names.as_mut() {
                        names.definitions = Some(all.clone());
                    }
                    all
                }
            },
        };
        let exact = all.iter().any(|(n, _)| n == name);
        Ok(all
            .into_iter()
            .filter(|(n, _)| {
                if exact {
                    n == name
                } else {
                    n.eq_ignore_ascii_case(name)
                }
            })
            .map(|(_, h)| h)
            .take(MAX_SPOTS)
            .collect())
    }

    /// Every definition in `files`, by tree-sitter (the same reading the map
    /// does) from the open buffer where there is one, unsaved edits and all.
    fn read_definitions(&self, files: &[PathBuf]) -> Vec<(String, Hit)> {
        let code: Vec<&PathBuf> = files
            .iter()
            .filter(|p| scan::lang_of(p).is_some())
            .collect();
        let buffers = &self.buffers;
        par_map(&code, tree_sitter_tags::TagsContext::new, |ctx, _, path| {
            let Some(lang) = scan::lang_of(path) else {
                return Vec::new();
            };
            let text = match buffers.get(*path) {
                Some(ed) => ed.to_text(),
                None => match scan::read_text(path) {
                    Some(t) => t,
                    None => return Vec::new(),
                },
            };
            let lines: Vec<&str> = text.lines().collect();
            scan::definitions(ctx, lang, &text)
                .into_iter()
                .map(|(n, line, col)| {
                    let hit = spot(path, line, col, lines.get(line).copied().unwrap_or(""));
                    (n, hit)
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect()
    }

    /// Markdown headings with `name` in them, ignoring case, in one file or
    /// the whole project: the ones that say exactly that first.
    fn headings(&mut self, name: &str, file: Option<PathBuf>) -> Vec<Hit> {
        let all = match file {
            Some(f) => self.read_headings(&[f]),
            None => match self.names().headings.clone() {
                Some(all) => all,
                None => {
                    let files = self.names().files.clone();
                    let all = self.read_headings(&files);
                    if let Some(names) = self.names.as_mut() {
                        names.headings = Some(all.clone());
                    }
                    all
                }
            },
        };
        let wanted = name.to_lowercase();
        let mut found: Vec<(bool, Hit)> = all
            .into_iter()
            .filter(|(title, _)| title.contains(&wanted))
            .map(|(title, h)| (title == wanted, h))
            .collect();
        // Exact titles first; otherwise the project's own order.
        found.sort_by_key(|(exact, _)| !*exact);
        found.into_iter().map(|(_, h)| h).take(MAX_SPOTS).collect()
    }

    /// Every heading in the markdown among `files`, by its title in lower
    /// case. Lines inside a fenced code block are not headings, whatever
    /// they start with.
    fn read_headings(&self, files: &[PathBuf]) -> Vec<(String, Hit)> {
        let notes: Vec<&PathBuf> = files.iter().filter(|p| is_markdown(p)).collect();
        let buffers = &self.buffers;
        par_map(
            &notes,
            || (),
            |_, _, path| {
                let text = match buffers.get(*path) {
                    Some(ed) => ed.to_text(),
                    None => scan::read_text(path).unwrap_or_default(),
                };
                let mut fenced = false;
                let mut out = Vec::new();
                for (n, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("```") {
                        fenced = !fenced;
                        continue;
                    }
                    if let Some(title) = heading_text(line).filter(|_| !fenced) {
                        out.push((title.to_lowercase(), spot(path, n, 0, line)));
                    }
                }
                out
            },
        )
        .into_iter()
        .flatten()
        .collect()
    }

    /// Every line `what` is on, as a whole word or anywhere, in one file, or
    /// across the project.
    fn occurrences(&mut self, what: &str, file: Option<PathBuf>, whole: bool) -> Vec<Hit> {
        let Some(m) = Matcher::new(what) else {
            return Vec::new();
        };
        let at = |line: &str| {
            if whole {
                m.first_word(line)
            } else {
                m.first(line)
            }
        };
        match file {
            Some(path) => {
                let text = self.text_of(&path).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                search::lines_in(&text, at)
                    .into_iter()
                    .take(MAX_SPOTS)
                    .map(|(n, col)| spot(&path, n, col, lines[n]))
                    .collect()
            }
            None => search::lines_where(
                self.tree.root_path(),
                &self.search_opts(),
                MAX_SPOTS,
                |text| m.is_in(text),
                at,
            ),
        }
    }

    fn plan_replace(
        &mut self,
        find: &str,
        with: &str,
        within: Option<&Place>,
        whole_words: bool,
    ) -> Result<Plan, String> {
        let needle = Needle::new(find, whole_words);
        let as_what = if whole_words {
            ""
        } else {
            " (inside words too)"
        };
        match within {
            None => {
                // A file open with unsaved edits is counted as it is on
                // screen, which is the copy the replace will change.
                let unsaved = self.unsaved_paths();
                let mut report = search::count(
                    self.tree.root_path(),
                    &needle,
                    &self.search_opts(),
                    &|path| unsaved.contains(path),
                );
                for path in &unsaved {
                    let n = needle.find_in(&self.buffers[path].to_text()).len();
                    if n > 0 {
                        report.files += 1;
                        report.occurrences += n;
                    }
                }
                if report.occurrences == 0 {
                    return Err(format!("`{find}` is not in this project{as_what}"));
                }
                Ok(Plan::ReplaceProject {
                    needle,
                    with: with.to_string(),
                    report,
                })
            }
            Some(place) => {
                let path = self.find(place, Want::File)?;
                let text = self
                    .text_of(&path)
                    .ok_or_else(|| format!("{} is not a text file", self.rel(&path)))?;
                let count = needle.find_in(&text).len();
                if count == 0 {
                    return Err(format!("`{find}` is not in {}{as_what}", self.rel(&path)));
                }
                Ok(Plan::ReplaceFile {
                    path,
                    needle,
                    with: with.to_string(),
                    count,
                })
            }
        }
    }

    // ---- saying what it will do --------------------------------------------

    /// What Enter would do, in a few words: the line under the bar.
    pub(super) fn describe(&self, plan: &Plan) -> String {
        match plan {
            Plan::Jump { spots, which, what } => {
                let h = &spots[*which];
                let place = format!("{}:{}", self.rel(&h.path), h.line + 1);
                if spots.len() == 1 {
                    format!("goes to {what} at {place}")
                } else {
                    let files = distinct_files(spots);
                    format!(
                        "goes to {what} at {place}: {} of {}{} in {files} file{}; again for the next",
                        which + 1,
                        spots.len(),
                        if spots.len() >= MAX_SPOTS { "+" } else { "" },
                        plural(files)
                    )
                }
            }
            Plan::Line { path, line } => match path {
                Some(p) => format!("goes to line {line} of {}", self.rel(p)),
                None => format!("goes to line {line}"),
            },
            Plan::Open(p) => format!("opens {}", self.rel(p)),
            Plan::Reveal(p) => format!("shows the folder {}/", self.rel(p)),
            Plan::Window(w) => match w {
                Window::Browser => "goes back to the browser".into(),
                Window::Map => "opens the project map".into(),
                Window::Git => "opens git".into(),
                Window::Help => "shows every key and command".into(),
                Window::Settings => "opens the settings".into(),
            },
            Plan::Copy { from, to } => {
                format!("copies {} to {}", self.rel(from), self.rel(to))
            }
            Plan::Move { from, to } => {
                format!("moves {} to {}", self.rel(from), self.rel(to))
            }
            Plan::Delete(p) => format!("deletes {}, asks first", self.rel(p)),
            Plan::New { path, folder } => format!(
                "makes the {} {}",
                if *folder { "folder" } else { "file" },
                self.rel(path)
            ),
            Plan::ReplaceProject {
                needle,
                with,
                report,
            } => format!(
                "replaces {} occurrence{} of `{}` with `{with}` across {} file{}, asks first",
                report.occurrences,
                plural(report.occurrences),
                needle.text,
                report.files,
                plural(report.files)
            ),
            Plan::ReplaceFile {
                path,
                needle,
                with,
                count,
            } => format!(
                "replaces {count} occurrence{} of `{}` with `{with}` in {}, Ctrl+Z undoes it",
                plural(*count),
                needle.text,
                self.rel(path)
            ),
            Plan::Set(args) => match args.first() {
                Some(key) => format!("sets {key}"),
                None => "set what? set tab_width to 2".into(),
            },
            Plan::Commit(m) if m.is_empty() => "commit with what message?".into(),
            Plan::Commit(m) => format!("commits what is staged: {m}"),
            Plan::Reload => "reads the disk and the plugins again".into(),
            Plan::Update => "asks GitHub for a newer tiny".into(),
            Plan::Plugin { name, .. } => {
                let owner = self.plugins.command_owner(name).unwrap_or_default();
                format!("runs {owner}'s *{name}")
            }
        }
    }

    // ---- doing it -----------------------------------------------------------

    /// Carry a plan out, returning the status line to show, empty when the
    /// work set its own.
    pub(super) fn carry_out(&mut self, plan: Plan, line: &str) -> anyhow::Result<String> {
        match plan {
            Plan::Jump { spots, which, what } => {
                let hit = &spots[which];
                self.jump_to(hit);
                self.last_jump = Some((line.trim().to_string(), which));
                Ok(if spots.len() == 1 {
                    format!("{what} at {}:{}", self.rel(&hit.path), hit.line + 1)
                } else {
                    format!(
                        "{what}: {} of {}, run it again for the next",
                        which + 1,
                        spots.len()
                    )
                })
            }
            // A line, a file or a folder is a place in the main window,
            // wherever the command was typed from.
            Plan::Line { path, line: n } => {
                self.window = AppWindow::Main;
                if let Some(p) = path
                    && !self.reveal(&p)
                {
                    return Err(anyhow!("{} is not in this project", self.rel(&p)));
                }
                let Some(ed) = self.active_buffer_mut() else {
                    return Err(anyhow!("that file cannot be opened as text"));
                };
                let line = (n - 1).min(ed.line_count().saturating_sub(1));
                ed.goto(line, 0);
                self.focus = Focus::Editor;
                Ok(format!("line {}", line + 1))
            }
            Plan::Open(p) => {
                self.window = AppWindow::Main;
                if !self.reveal(&p) {
                    return Err(anyhow!("{} is not in this project", self.rel(&p)));
                }
                if matches!(self.preview, Preview::Buffer { .. }) {
                    self.focus_editor();
                }
                Ok(format!("opened {}", self.rel(&p)))
            }
            Plan::Reveal(p) => {
                self.window = AppWindow::Main;
                if !self.reveal(&p) {
                    return Err(anyhow!("{} is not in this project", self.rel(&p)));
                }
                self.tree.expand(&p);
                self.rebuild_rows();
                self.focus_tree();
                Ok(format!("{}/", self.rel(&p)))
            }
            Plan::Window(w) => {
                match w {
                    Window::Browser => self.show_window(AppWindow::Main),
                    Window::Map => self.show_window(AppWindow::Map),
                    Window::Git => self.show_window(AppWindow::Source),
                    Window::Help => self.mode = Mode::Help(0),
                    Window::Settings => self.open_settings(),
                }
                Ok(String::new())
            }
            Plan::Copy { from, to } => self.copy_entry(&from, &to),
            Plan::Move { from, to } => self.rename_path(&from, &to),
            Plan::Delete(p) => {
                self.arm_delete(&p)?;
                Ok(String::new())
            }
            Plan::New { path, folder } => {
                let root = self.tree.root_path().to_path_buf();
                let rel = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
                self.create_entry(&root, &rel.to_string_lossy(), folder)
            }
            Plan::ReplaceProject {
                needle,
                with,
                report,
            } => {
                // Rewriting many files on disk cannot be undone, so it asks.
                self.mode = Mode::Confirm(Confirm {
                    message: format!(
                        "Replace {} occurrence{} of `{}` with `{with}` across {} file{}?  (y/n)",
                        report.occurrences,
                        plural(report.occurrences),
                        needle.text,
                        report.files,
                        plural(report.files),
                    ),
                    kind: ConfirmKind::Replace {
                        find: needle,
                        replace: with,
                    },
                });
                Ok(String::new())
            }
            Plan::ReplaceFile {
                path,
                needle,
                with,
                count,
            } => self.replace_in_file(&path, &needle, &with, count),
            Plan::Set(args) => self.cmd_set(&args),
            Plan::Commit(m) => self.commit(&m),
            Plan::Reload => self.cmd_reload(),
            Plan::Update => self.cmd_update(),
            Plan::Plugin { name, args } => self
                .run_plugin_command(&name, &args)
                .unwrap_or_else(|| Err(anyhow!("no plugin has *{name} any more"))),
        }
    }

    /// Replace in one file, in its buffer, as one step `Ctrl+Z` takes back,
    /// then open it, so that `Ctrl+Z` is right there.
    ///
    /// A file with no unsaved edits is saved straight after, so the disk says
    /// what the screen does, the same as a replace across the project. One
    /// that had edits of its own is left unsaved: saving it would write those
    /// too, and nobody asked for that.
    fn replace_in_file(
        &mut self,
        path: &Path,
        needle: &Needle,
        with: &str,
        count: usize,
    ) -> anyhow::Result<String> {
        if !self.buffers.contains_key(path) {
            let text = read_editable(path)
                .ok_or_else(|| anyhow!("cannot open {} as text", self.rel(path)))?;
            self.buffers.insert(
                path.to_path_buf(),
                Editor::from_str(path.to_path_buf(), &text),
            );
            self.note_file(path);
        }
        let ed = self.buffers.get_mut(path).expect("opened just above");
        let was_clean = !ed.dirty;
        let (text, _) = needle.replace_in(&ed.to_text(), with);
        ed.replace_all(&text);
        let saved = was_clean && ed.save().is_ok();
        if saved {
            self.note_file(path);
        }
        self.highlight_cache.clear();
        self.reveal(path);
        if matches!(self.preview, Preview::Buffer { .. }) {
            self.focus_editor();
        }
        Ok(format!(
            "replaced {count} occurrence{} of `{}` in {}{}, Ctrl+Z undoes it",
            plural(count),
            needle.text,
            self.rel(path),
            if saved { "" } else { " (not saved yet)" }
        ))
    }
}

/// A place in a file, as a search hit so the editor can jump to it.
fn spot(path: &Path, line: usize, col: usize, text: &str) -> Hit {
    Hit {
        path: path.to_path_buf(),
        kind: HitKind::Content,
        line,
        col,
        text: text.to_string(),
    }
}

/// How many files a list of places is spread over.
fn distinct_files(spots: &[Hit]) -> usize {
    let mut paths: Vec<&Path> = spots.iter().map(|h| h.path.as_path()).collect();
    paths.dedup();
    paths.len()
}

/// The words for what a `goto` looks for, as the status line says them.
fn what_of(target: &Target) -> String {
    match target {
        Target::Function(n) => format!("function {n}"),
        Target::Heading(n) => format!("heading {n}"),
        Target::Word(n) => format!("word {n}"),
        Target::Text(n) => format!("\"{n}\""),
        Target::Anything(n) => n.clone(),
        Target::Line(n) => format!("line {n}"),
        Target::File(n) | Target::Folder(n) => n.clone(),
        Target::Window(_) => String::new(),
    }
}

/// Whether a path is a markdown note, which is where headings are.
fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
    })
}

/// The title of a markdown heading line (`## Install` is `Install`) or
/// `None` for any other line.
fn heading_text(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // `#tag`, not a heading
    }
    Some(rest.trim().trim_end_matches('#').trim())
}

/// Read a command line into a sentence. Here so the rest of `app` asks one
/// module for both halves.
pub(super) fn read(line: &str) -> Sentence {
    sentence::parse(line)
}
