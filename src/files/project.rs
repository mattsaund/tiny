//! Working out what `tiny <thing>` meant, and what to write if it is not
//! there yet.
//!
//! A project is a directory. That is the whole definition — no marker folder,
//! no registry, no database, and no state anywhere else on the machine. Point
//! tiny at a folder and it opens the folder; move the folder and nothing
//! breaks, because nothing ever knew where it was.
//!
//! # Nothing is written into your project
//!
//! tiny creates exactly one file, ever: a starter `README.md`, and only in a
//! directory it just made or found empty. No dotfiles, no caches, no
//! per-project settings — how the program behaves comes from one config file
//! per user (see [`crate::config`]), never from the folder being opened. An
//! existing folder of someone's work is left exactly as it was found.
//!
//! # Why there is no `tiny new`
//!
//! Naming a path that does not exist creates it. That collapses "make a
//! project" and "open a project" into the same gesture, which is the whole
//! reason there is no separate subcommand. The cost is that a typo in a path
//! silently creates a directory instead of erroring — a trade the design
//! accepts, because the alternative is a `new` command nobody remembers.
//!
//! # Naming a file
//!
//! `tiny todo.txt` opens that one file in the editor with the tree beside it,
//! which is how tiny stands in for a plain text editor when a whole project is
//! more than you wanted. A name that is not on disk yet is created either way;
//! the extension is what decides which — see [`names_a_file`].

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{self, Config};

/// What to open: a directory for the tree, and optionally a file to land on.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub root: PathBuf,
    /// Set when the argument named a file — the tree opens on its directory
    /// and the cursor starts on the file itself.
    pub file: Option<PathBuf>,
    /// True when this run made the directory, or found it empty and wrote a
    /// starter README into it.
    pub created: bool,
}

/// Turn the command-line argument into something to open, creating whatever
/// is named if it does not exist yet.
///
/// - no argument: the working directory
/// - a directory: itself
/// - a file: its folder, with the file in the editor
/// - a name that does not exist, with an extension: an empty file, opened
/// - a name that does not exist, without one: a new folder
///
/// There is no walking up to find anything, because there is nothing to find:
/// `tiny` opens where you are standing, and `tiny some/folder` opens that
/// folder. Nothing on disk marks one directory as more of a project than
/// another.
///
/// Everything is canonicalized before it is returned, so the rest of the
/// program can compare paths with `==` and use `strip_prefix` against the root
/// without worrying about `./`, symlinks, or relative segments. `tree`,
/// `search` and `graph` all rely on that being true.
pub fn resolve(arg: Option<&str>, cfg: &Config) -> Result<Target> {
    let (mut root, file, mut created) = match arg {
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| cfg.default_root.clone());
            (cwd, None, false)
        }
        Some(a) => {
            let path = expand(a);
            if path.is_file() {
                (folder_of(&path), Some(path), false)
            } else if path.is_dir() {
                (path, None, false)
            } else if names_a_file(a, &path) {
                // `tiny todo.txt` on a name that is not there yet writes the
                // file and edits it, rather than making a folder called
                // `todo.txt` — which is what a `.` in the name always meant.
                touch(&path)?;
                (folder_of(&path), Some(path), false)
            } else {
                // Naming somewhere that isn't there yet is how you start a
                // project now that there is no separate `new` command.
                fs::create_dir_all(&path)
                    .with_context(|| format!("cannot create {}", path.display()))?;
                (path, None, true)
            }
        }
    };

    root = root
        .canonicalize()
        .with_context(|| format!("cannot open {}", root.display()))?;
    let mut file = file.map(|f| f.canonicalize().unwrap_or(f));

    // The one file tiny ever writes, and only into a folder it just made or
    // found empty. An existing folder of work is left exactly as it was — and
    // a folder that just gained the file named on the command line is no
    // longer empty, so `tiny new/todo.txt` gets the file it asked for and
    // nothing besides.
    if cfg.starter_readme {
        created = created || is_effectively_empty(&root);
        if created {
            write_readme(&root)?;
        }
    }

    // A brand-new folder opens on its README, so the first frame is the page
    // explaining where you are rather than an empty pane.
    if file.is_none() && created {
        let readme = root.join(README);
        if readme.is_file() {
            file = Some(readme);
        }
    }

    Ok(Target {
        root,
        file,
        created,
    })
}

/// The folder a file argument opens the tree on: the one the file is in.
fn folder_of(file: &Path) -> PathBuf {
    // `Path::new("todo.txt").parent()` is `Some("")`, not `None`, so a bare
    // filename has to be turned into the working directory by hand.
    match file.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Does a name that is not on disk yet mean a file, or a folder?
///
/// The extension decides: `todo.txt` and `scratch.py` are files, `thesis` is a
/// folder. A trailing slash always means a folder, since that is what it means
/// everywhere else. The rule mis-reads a dotted folder name like `site.v2`,
/// which is the price of not having a flag.
///
/// The same rule answers the question on the command line and at the `n`
/// prompt inside the app, which is why it lives here and is called from both:
/// one place to read, and no chance of the two disagreeing. Neither can make
/// an extensionless *file* — `*new LICENSE` is the way to do that.
pub fn names_a_file(arg: &str, path: &Path) -> bool {
    !arg.ends_with(std::path::is_separator) && path.extension().is_some()
}

/// Create an empty file, and any directories above it.
///
/// `create_new` rather than `create`: if something appeared at that path
/// between the `is_file` check in `resolve` and here, the right answer is an
/// error, not a truncated file.
fn touch(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    Ok(())
}

/// The only file tiny ever writes into a folder of yours.
pub const README: &str = "README.md";

/// Write the starter `README.md`, headed by the folder's own name, so a
/// brand-new project is not an empty screen.
///
/// Guarded by an `exists()` check, so it can never clobber an edited README —
/// which matters because "the folder was empty" is decided separately, and
/// getting that wrong should cost nothing.
fn write_readme(root: &Path) -> Result<()> {
    let readme = root.join(README);
    if readme.exists() {
        return Ok(());
    }
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    fs::write(&readme, welcome(&name)).with_context(|| format!("cannot write {}", readme.display()))
}

/// Empty enough to scaffold into: no visible files, ignoring dotfiles.
///
/// Dotfiles are ignored so a directory holding only `.git/` or `.gitignore` —
/// a freshly cloned or freshly `git init`-ed folder — still counts as empty
/// and gets the starter README. An unreadable directory returns `false`:
/// when in doubt, write nothing.
fn is_effectively_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(entries) => !entries
            .flatten()
            .any(|e| !e.file_name().to_string_lossy().starts_with('.')),
        Err(_) => false,
    }
}

/// Expand a leading `~`, which the shell leaves alone inside quotes.
///
/// Needed because `tiny "~/notes"` and `tiny '~/notes'` reach us with a
/// literal tilde — the shell only expands it unquoted, and cmd.exe never does.
/// Deliberately handles just `~` and `~/...`; `~otheruser` is not supported and
/// is left as a literal path, which will simply fail to open.
fn expand(s: &str) -> PathBuf {
    let tail = if s == "~" {
        Some("")
    } else {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    };
    match tail.zip(config::home_dir()) {
        Some(("", home)) => home,
        Some((rest, home)) => home.join(rest),
        None => PathBuf::from(s),
    }
}

/// The starter `README.md`, headed by the project's own name.
///
/// It doubles as the first thing that exercises the markdown renderer, so if
/// you change `markdown.rs`, this is a good page to look at.
fn welcome(name: &str) -> String {
    format!("# {name}\n{WELCOME}")
}

/// Everything below the heading in a scaffolded README.
///
/// Kept as one literal rather than assembled, so what a new user reads first
/// can be proofread in one place.
const WELCOME: &str = r#"
A **tiny** project — a folder of plain files, nothing more. This page is one of
them: edit it, or delete it, like anything else here.

## Moving around

| key         | does                                    |
|-------------|-----------------------------------------|
| `up` `down` | move the cursor in the tree             |
| `enter`     | open or close a folder, or edit a file  |
| `right`     | open a folder, or step inside an open one |
| `left`      | close a folder, or go up to its parent  |
| `/`         | search the whole project                |
| `:`         | settings and find-replace               |
| `?`         | the full keymap                         |

## Editing

Hovering a `.md` file renders it here. Press `e` to edit the raw source,
and `Ctrl+S` to save — the same key micro uses.

Code files skip the rendered view and open straight in the editor:

```python
def hello(name):
    return f"hi {name}"
```

## Linking

Write `[[wikilinks]]` to point one note at another. Press `m` for the project
map that draws them, along with the calls between your code files.

> Everything here is a plain file. Nothing is locked in a database.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    /// Every visible and hidden name directly inside `dir`.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_missing_path_becomes_a_new_folder_holding_only_a_readme() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("project1");
        let t = resolve(Some(path.to_str().unwrap()), &cfg()).unwrap();

        assert!(t.created);
        assert_eq!(listing(&t.root), ["README.md"], "one file, nothing hidden");
        assert_eq!(
            t.file,
            Some(t.root.join(README)),
            "a new project opens on its README"
        );
    }

    #[test]
    fn the_starter_readme_is_headed_by_the_folder_name() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("thesis");
        let t = resolve(Some(path.to_str().unwrap()), &cfg()).unwrap();

        let text = fs::read_to_string(t.root.join(README)).unwrap();
        assert!(text.starts_with("# thesis\n"), "{text:?}");
        assert!(text.contains("[[wikilinks]]"), "the welcome text is in it");
    }

    #[test]
    fn an_existing_folder_of_work_is_left_exactly_as_it_was() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("existing");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("thesis.txt"), "words").unwrap();

        let t = resolve(Some(root.to_str().unwrap()), &cfg()).unwrap();
        assert_eq!(
            listing(&t.root),
            ["thesis.txt"],
            "no README, and above all no dotfiles"
        );
        assert!(!t.created);
    }

    #[test]
    fn a_folder_holding_only_dotfiles_still_counts_as_empty() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("fresh-clone");
        fs::create_dir_all(root.join(".git")).unwrap();

        let t = resolve(Some(root.to_str().unwrap()), &cfg()).unwrap();
        assert!(t.created);
        assert_eq!(listing(&t.root), [".git", "README.md"]);
    }

    #[test]
    fn opening_the_same_new_folder_twice_writes_nothing_the_second_time() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("twice");
        resolve(Some(root.to_str().unwrap()), &cfg()).unwrap();
        fs::write(root.join(README), "# mine\n").unwrap();

        let t = resolve(Some(root.to_str().unwrap()), &cfg()).unwrap();
        assert!(!t.created, "it has a file in it now");
        assert_eq!(
            fs::read_to_string(t.root.join(README)).unwrap(),
            "# mine\n",
            "an edited README is never written over"
        );
    }

    #[test]
    fn starter_readme_off_writes_nothing_at_all() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("hands-off");

        let mut c = cfg();
        c.starter_readme = false;
        let t = resolve(Some(root.to_str().unwrap()), &c).unwrap();
        assert!(t.root.is_dir(), "the folder is still made");
        assert!(listing(&t.root).is_empty(), "and left completely empty");
        assert_eq!(t.file, None);
    }

    #[test]
    fn a_folder_is_opened_where_it_is_with_nothing_to_walk_up_to() {
        let td = tempfile::tempdir().unwrap();
        let deep = td.path().join("proj").join("src");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("main.py"), "print(1)").unwrap();

        let t = resolve(Some(deep.to_str().unwrap()), &cfg()).unwrap();
        assert_eq!(t.root, deep.canonicalize().unwrap());
    }

    #[test]
    fn naming_a_file_opens_its_folder_with_the_cursor_on_it() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("code");
        fs::create_dir(&root).unwrap();
        let file = root.join("main.py");
        fs::write(&file, "print(1)").unwrap();

        let t = resolve(Some(file.to_str().unwrap()), &cfg()).unwrap();
        assert_eq!(t.root, root.canonicalize().unwrap());
        assert_eq!(t.file, Some(file.canonicalize().unwrap()));
        assert_eq!(listing(&t.root), ["main.py"], "opening it wrote nothing");
    }

    #[test]
    fn naming_a_file_that_is_not_there_yet_writes_an_empty_one() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("scratch");
        fs::create_dir(&root).unwrap();
        let file = root.join("todo.txt");

        let t = resolve(Some(file.to_str().unwrap()), &cfg()).unwrap();
        assert!(file.is_file(), "the file is created, not a folder");
        assert_eq!(fs::read_to_string(&file).unwrap(), "");
        assert_eq!(t.root, root.canonicalize().unwrap());
        assert_eq!(t.file, Some(file.canonicalize().unwrap()));
        assert_eq!(
            listing(&t.root),
            ["todo.txt"],
            "asking for one file gets one file"
        );
    }

    #[test]
    fn a_new_file_brings_its_folders_with_it() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("a").join("b").join("notes.md");

        let t = resolve(Some(file.to_str().unwrap()), &cfg()).unwrap();
        assert!(file.is_file());
        assert_eq!(t.root, file.parent().unwrap().canonicalize().unwrap());
        assert_eq!(listing(&t.root), ["notes.md"]);
    }

    #[test]
    fn an_extension_is_what_separates_a_new_file_from_a_new_folder() {
        assert!(names_a_file("todo.txt", Path::new("todo.txt")));
        assert!(names_a_file(
            "~/code/scratch.py",
            Path::new("/home/x/scratch.py")
        ));
        assert!(!names_a_file("thesis", Path::new("thesis")));
        assert!(
            !names_a_file("notes.d/", Path::new("notes.d")),
            "a trailing slash means a folder whatever the name looks like"
        );
    }

    #[test]
    fn tilde_expands_to_the_home_directory() {
        let home = config::home_dir().unwrap();
        assert_eq!(expand("~/notes"), home.join("notes"));
        assert_eq!(expand("~"), home);
        assert_eq!(
            expand("~\\notes"),
            home.join("notes"),
            "cmd.exe never expands a tilde, so the backslash form has to work too"
        );
        assert_eq!(expand("/tmp/x"), PathBuf::from("/tmp/x"));
        assert_eq!(expand("relative/x"), PathBuf::from("relative/x"));
        assert_eq!(expand("~notes"), PathBuf::from("~notes"), "not a home path");
    }
}
