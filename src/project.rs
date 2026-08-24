//! Working out what `tiny <thing>` meant, and making a project if there
//! isn't one.
//!
//! A project is just a directory with a `.tiny/` folder in it. That folder
//! holds an optional per-project `tiny.conf` and, later, the link-graph
//! cache. Nothing about a project lives outside the directory it describes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{Config, PROJECT_DIR};

/// What to open: a directory for the tree, and optionally a file to land on.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub root: PathBuf,
    /// Set when the argument named a file — the tree opens on its directory
    /// and the cursor starts on the file itself.
    pub file: Option<PathBuf>,
    /// True when this run created the project.
    pub created: bool,
}

pub fn is_project(dir: &Path) -> bool {
    dir.join(PROJECT_DIR).is_dir()
}

/// Walk up from `start` looking for a directory that is already a project.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if is_project(dir) {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Turn the command-line argument into something to open, creating the
/// project if it does not exist yet.
///
/// - no argument: the working directory, or the project it sits inside
/// - a file: its directory, with the cursor on the file
/// - a directory: itself
/// - a path that does not exist: created, then treated as a directory
pub fn resolve(arg: Option<&str>, cfg: &Config) -> Result<Target> {
    let (mut root, file, mut created) = match arg {
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| cfg.default_root.clone());
            // Running bare inside a project opens the project, not the
            // subdirectory you happen to be standing in.
            (find_root(&cwd).unwrap_or(cwd), None, false)
        }
        Some(a) => {
            let path = expand(a);
            if path.is_file() {
                let dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                (dir, Some(path), false)
            } else if path.is_dir() {
                (path, None, false)
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
    let file = file.map(|f| f.canonicalize().unwrap_or(f));

    if !is_project(&root) {
        if cfg.auto_init {
            // Scaffold only into a directory tiny just made or found empty.
            // Opening an existing folder full of work should add a marker,
            // not a README and a welcome note.
            let empty = is_effectively_empty(&root);
            init(&root, created || empty)?;
            created = created || empty;
        } else {
            created = false;
        }
    }

    Ok(Target {
        root,
        file,
        created,
    })
}

/// Mark a directory as a project. With `scaffold`, also lay down a starting
/// note so a brand-new project is not an empty screen.
pub fn init(root: &Path, scaffold: bool) -> Result<()> {
    let meta = root.join(PROJECT_DIR);
    fs::create_dir_all(&meta).with_context(|| format!("cannot create {}", meta.display()))?;

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());

    let marker = meta.join("project.conf");
    if !marker.exists() {
        fs::write(
            &marker,
            format!("name = \"{name}\"\ncreated = \"{}\"\n", today()),
        )?;
    }

    if scaffold {
        let notes = root.join("notes");
        if !notes.exists() {
            fs::create_dir_all(&notes)?;
            fs::write(notes.join("welcome.md"), WELCOME)?;
        }
        let readme = root.join("README.md");
        if !readme.exists() {
            fs::write(
                &readme,
                format!("# {name}\n\nA tiny project. Start in [[welcome]].\n"),
            )?;
        }
    }
    Ok(())
}

/// Empty enough to scaffold into: no visible files, ignoring dotfiles.
fn is_effectively_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(entries) => !entries
            .flatten()
            .any(|e| !e.file_name().to_string_lossy().starts_with('.')),
        Err(_) => false,
    }
}

/// Expand a leading `~`, which the shell leaves alone inside quotes.
fn expand(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Path::new(&home).join(rest);
    }
    if s == "~"
        && let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    PathBuf::from(s)
}

/// `YYYY-MM-DD` from the system clock, without pulling in a date crate.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Civil-from-days, Howard Hinnant's algorithm.
    let z = secs / 86_400 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

pub const WELCOME: &str = r#"# Welcome

This is a **tiny** project — a folder of markdown files, nothing more.
Delete this note whenever you like.

## Moving around

| key        | does                                   |
|------------|----------------------------------------|
| `up` `down`| move the cursor in the tree            |
| `right`    | open a folder, or edit a file          |
| `left`     | close a folder, or go up to its parent |
| `/`        | search the whole project               |
| `:`        | settings and find-replace              |
| `?`        | the full keymap                        |

## Editing

Hovering a `.md` file renders it here. Press `e` to edit the raw source,
and `Ctrl+S` to save — the same key micro uses.

Code files skip the rendered view and open straight in the editor:

```python
def hello(name):
    return f"hi {name}"
```

## Linking

Write `[[wikilinks]]` to point one note at another. A link graph that draws
these connections — and the ones between functions in your code — is the
next thing being built.

> Everything here is a plain file. Nothing is locked in a database.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn a_missing_path_becomes_a_new_scaffolded_project() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("project1");
        let t = resolve(Some(path.to_str().unwrap()), &cfg()).unwrap();

        assert!(t.created);
        assert!(is_project(&t.root));
        assert!(t.root.join("README.md").is_file());
        assert!(t.root.join("notes").join("welcome.md").is_file());
        assert_eq!(t.file, None);
    }

    #[test]
    fn an_existing_folder_of_work_gets_a_marker_but_no_scaffolding() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("existing");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("thesis.txt"), "words").unwrap();

        let t = resolve(Some(root.to_str().unwrap()), &cfg()).unwrap();
        assert!(is_project(&t.root), "it is a project now");
        assert!(
            !t.root.join("README.md").exists(),
            "someone else's folder must not gain a README"
        );
        assert!(!t.root.join("notes").exists());
        assert!(!t.created);
    }

    #[test]
    fn naming_a_file_opens_its_directory_with_the_cursor_on_it() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("code");
        fs::create_dir(&root).unwrap();
        let file = root.join("main.py");
        fs::write(&file, "print(1)").unwrap();

        let t = resolve(Some(file.to_str().unwrap()), &cfg()).unwrap();
        assert_eq!(t.root, root.canonicalize().unwrap());
        assert_eq!(t.file, Some(file.canonicalize().unwrap()));
        assert!(is_project(&t.root), "its folder becomes a project");
    }

    #[test]
    fn an_existing_project_is_opened_not_re_scaffolded() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("proj");
        fs::create_dir(&root).unwrap();
        init(&root, true).unwrap();
        fs::write(root.join("README.md"), "# mine\n").unwrap();

        let t = resolve(Some(root.to_str().unwrap()), &cfg()).unwrap();
        assert!(!t.created);
        assert_eq!(
            fs::read_to_string(t.root.join("README.md")).unwrap(),
            "# mine\n",
            "an existing README is never overwritten"
        );
    }

    #[test]
    fn auto_init_off_leaves_the_directory_untouched() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("hands-off");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "x").unwrap();

        let mut c = cfg();
        c.auto_init = false;
        let t = resolve(Some(root.to_str().unwrap()), &c).unwrap();
        assert!(!is_project(&t.root), "no .tiny/ appears");
        assert_eq!(fs::read_dir(&t.root).unwrap().count(), 1);
    }

    #[test]
    fn find_root_walks_up_to_the_project() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("proj");
        let deep = root.join("a").join("b").join("c");
        fs::create_dir_all(&deep).unwrap();
        init(&root, false).unwrap();

        assert_eq!(find_root(&deep), Some(root.clone()));
        assert_eq!(find_root(&root), Some(root));
    }

    #[test]
    fn find_root_gives_up_outside_any_project() {
        let td = tempfile::tempdir().unwrap();
        // A temp dir under /tmp is not inside a project unless one is made.
        let deep = td.path().join("x").join("y");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_root(&deep), None);
    }

    #[test]
    fn init_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().to_path_buf();
        init(&root, true).unwrap();
        let first = fs::read_to_string(root.join("README.md")).unwrap();
        fs::write(root.join("README.md"), "edited by hand\n").unwrap();
        init(&root, true).unwrap();
        assert_ne!(fs::read_to_string(root.join("README.md")).unwrap(), first);
    }

    #[test]
    fn tilde_expands_to_the_home_directory() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand("~/notes"), Path::new(&home).join("notes"));
        assert_eq!(expand("~"), PathBuf::from(&home));
        assert_eq!(expand("/tmp/x"), PathBuf::from("/tmp/x"));
        assert_eq!(expand("relative/x"), PathBuf::from("relative/x"));
    }

    #[test]
    fn today_is_a_plausible_iso_date() {
        let d = today();
        let parts: Vec<&str> = d.split('-').collect();
        assert_eq!(parts.len(), 3, "{d}");
        let y: i32 = parts[0].parse().unwrap();
        assert!((2024..2100).contains(&y), "{d}");
        assert!((1..=12).contains(&parts[1].parse::<u32>().unwrap()));
        assert!((1..=31).contains(&parts[2].parse::<u32>().unwrap()));
    }
}
