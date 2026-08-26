//! The project tree: a lazily-loaded directory model plus the flattening
//! step that turns it into the list of rows the left pane draws.
//!
//! Children are read from disk only when a directory is first expanded, so
//! opening a project with a deep `node_modules` in it stays instant.
//!
//! # The two shapes
//!
//! There are deliberately two representations of the same thing:
//!
//! - [`Node`] is the **tree**: nested, mutable, and the source of truth for
//!   which directories are open and which have been read.
//! - [`Row`] is the **list**: one flat entry per visible line, produced by
//!   [`Tree::flatten`], carrying a precomputed `depth` for indentation.
//!
//! `app` holds the flattened rows and indexes into them with a single `usize`
//! cursor; `ui` draws a window of them. Nothing outside this module walks the
//! nested form. That means every structural change — expand, collapse,
//! refresh — has to be followed by a re-flatten, which `App::rebuild_rows`
//! does while preserving the cursor by path rather than by index.
//!
//! # There is no file watcher
//!
//! The tree only re-reads the disk when something asks it to: `R`/`F5`, a
//! create/rename/delete, or a `.`-toggle. Files changed by another program do
//! not appear until then. That is a deliberate simplification — an inotify
//! watcher would mean a background thread and a redraw signal, and the event
//! loop is built to block until a key arrives (see `main`).

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

/// One entry in the nested tree. Directories own their `children`, which stay
/// empty until the directory is first expanded.
#[derive(Debug, Clone)]
pub struct Node {
    /// Absolute path. Canonicalized at the root, so it is safe to compare with
    /// `==` and to `strip_prefix` against `Tree::root_path`.
    pub path: PathBuf,
    /// Just the final component, cached so drawing does not re-derive it.
    pub name: String,
    pub is_dir: bool,
    /// Whether the user has opened this directory. Independent of `loaded`: a
    /// directory can be read but collapsed, which is what happens when you
    /// expand something and then close it again.
    pub expanded: bool,
    /// Whether `children` has been read from disk yet.
    pub loaded: bool,
    /// Set when the directory exists but could not be read (permissions, etc).
    pub unreadable: bool,
    /// Populated only once `loaded` is true. Empty for files, and for
    /// directories that have never been opened.
    pub children: Vec<Node>,
}

impl Node {
    fn new(path: PathBuf, is_dir: bool) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            // A root like `/` has no file_name; show the path itself.
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self {
            path,
            name,
            is_dir,
            expanded: false,
            loaded: false,
            unreadable: false,
            children: Vec::new(),
        }
    }
}

/// One visible line in the tree pane.
///
/// A flattened snapshot of a [`Node`], not a reference to one — `app` keeps a
/// `Vec<Row>` and indexes it by cursor position, so rows have to outlive any
/// borrow of the tree. Cloning the path and name on every flatten is the price
/// of that, and it is cheap next to the disk read that produced them.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// Nesting level, with the project root at 0. Drawing multiplies this by
    /// the indent width; nothing else uses it.
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub unreadable: bool,
}

/// The directory model. Owns the root [`Node`] and the dotfile setting that
/// governs every read.
pub struct Tree {
    root: Node,
    show_hidden: bool,
}

impl Tree {
    /// Build a tree rooted at `path` with its top level already loaded.
    ///
    /// The root is forced to `is_dir` and `expanded` regardless of what is on
    /// disk: the project root is always a directory and is always open, so
    /// there is no state in which the tree pane can be empty of context.
    pub fn new(path: PathBuf, show_hidden: bool) -> Self {
        let mut root = Node::new(path, true);
        root.is_dir = true;
        root.expanded = true;
        let mut tree = Self { root, show_hidden };
        tree.load_children_at(&tree.root.path.clone());
        tree
    }

    pub fn root_path(&self) -> &Path {
        &self.root.path
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Toggle dotfile visibility. Loaded directories are re-read so the
    /// newly-visible entries appear without a manual refresh.
    pub fn set_show_hidden(&mut self, show: bool) {
        if self.show_hidden != show {
            self.show_hidden = show;
            self.refresh_all();
        }
    }

    /// Depth-first walk producing the rows to draw, in display order.
    ///
    /// Called after every structural change, and its result is what the cursor
    /// indexes into. Cost is proportional to the number of *visible* rows, not
    /// to the size of the project, because collapsed directories are not
    /// descended into.
    pub fn flatten(&self) -> Vec<Row> {
        let mut out = Vec::new();
        // The root itself is row 0, so the project name is always on screen.
        out.push(Row {
            path: self.root.path.clone(),
            name: self.root.name.clone(),
            depth: 0,
            is_dir: true,
            expanded: self.root.expanded,
            unreadable: self.root.unreadable,
        });
        if self.root.expanded {
            push_rows(&self.root.children, 1, &mut out);
        }
        out
    }

    /// Locate a node by absolute path, walking down from the root one
    /// component at a time.
    ///
    /// Returns `None` for anything outside the root, and for anything inside a
    /// directory that has not been loaded yet — an unloaded directory has no
    /// children to find, even though the file exists on disk. Callers that
    /// need the node to exist have to `expand` the parents first; that is what
    /// `App::reveal` does.
    pub fn find(&self, path: &Path) -> Option<&Node> {
        let rel = path.strip_prefix(&self.root.path).ok()?;
        let mut node = &self.root;
        for part in rel.components() {
            let name = part.as_os_str();
            node = node
                .children
                .iter()
                .find(|c| c.path.file_name() == Some(name))?;
        }
        Some(node)
    }

    /// Mutable twin of [`Tree::find`]. Duplicated rather than generic because
    /// threading mutability through a shared walk needs either unsafe code or
    /// a macro, and neither is worth it for twelve lines.
    fn find_mut(&mut self, path: &Path) -> Option<&mut Node> {
        let rel = path.strip_prefix(&self.root.path).ok()?.to_path_buf();
        let mut node = &mut self.root;
        for part in rel.components() {
            let name = part.as_os_str().to_os_string();
            node = node
                .children
                .iter_mut()
                .find(|c| c.path.file_name() == Some(name.as_os_str()))?;
        }
        Some(node)
    }

    /// Expand a directory, reading it from disk if this is the first time.
    /// No-op for files. Returns true if anything changed.
    pub fn expand(&mut self, path: &Path) -> bool {
        let needs_load = match self.find(path) {
            Some(n) if n.is_dir => !n.loaded,
            _ => return false,
        };
        if needs_load {
            self.load_children_at(path);
        }
        match self.find_mut(path) {
            Some(n) => {
                let changed = !n.expanded;
                n.expanded = true;
                changed
            }
            None => false,
        }
    }

    /// Close a directory. Its children stay in memory, so reopening it is
    /// instant and does not touch the disk. Returns true if anything changed.
    pub fn collapse(&mut self, path: &Path) -> bool {
        match self.find_mut(path) {
            Some(n) if n.is_dir && n.expanded => {
                n.expanded = false;
                true
            }
            _ => false,
        }
    }

    /// Expand a collapsed directory or collapse an expanded one. This is what
    /// Enter does in the tree pane.
    pub fn toggle(&mut self, path: &Path) -> bool {
        match self.find(path) {
            Some(n) if n.is_dir && n.expanded => self.collapse(path),
            Some(n) if n.is_dir => self.expand(path),
            _ => false,
        }
    }

    /// Re-read every directory that has been loaded so far.
    ///
    /// The blunt instrument behind `R`, `:reload`, and every file operation.
    /// Cost is one `read_dir` per open directory — fine for a project you are
    /// looking at, and the reason unopened directories are skipped entirely.
    pub fn refresh_all(&mut self) {
        let dirs = self.loaded_dirs();
        for d in dirs {
            self.load_children_at(&d);
        }
    }

    /// Paths of every currently-loaded directory, parents before children so
    /// a refresh pass rebuilds the tree top-down.
    fn loaded_dirs(&self) -> Vec<PathBuf> {
        fn walk(n: &Node, out: &mut Vec<PathBuf>) {
            if n.is_dir && n.loaded {
                out.push(n.path.clone());
                for c in &n.children {
                    walk(c, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Read `dir` from disk and merge the result into the tree, carrying over
    /// the expansion state of any subdirectory that still exists.
    ///
    /// The merge is what makes refresh non-destructive. A naive re-read would
    /// collapse everything the user had opened; instead each fresh entry looks
    /// for its predecessor by name and inherits `expanded`, `loaded` and the
    /// whole subtree from it. Entries that vanished from disk are simply not
    /// in the fresh list, so they disappear; new ones arrive collapsed.
    ///
    /// The `prev.is_dir == fresh_child.is_dir` guard handles the case where a
    /// name was deleted and recreated as the other kind — inheriting a
    /// directory's children onto a file would be nonsense.
    ///
    /// A read failure is recorded as `unreadable` rather than propagated: a
    /// permission-denied directory should draw dimmed in the tree, not stop
    /// the program.
    fn load_children_at(&mut self, dir: &Path) {
        let show_hidden = self.show_hidden;
        let fresh = read_dir_sorted(dir, show_hidden);
        let Some(node) = self.find_mut(dir) else {
            return;
        };
        match fresh {
            Ok(entries) => {
                let old = std::mem::take(&mut node.children);
                node.children = entries
                    .into_iter()
                    .map(|mut fresh_child| {
                        if let Some(prev) = old.iter().find(|o| o.name == fresh_child.name) {
                            // Same entry as before: keep what the user had open.
                            if prev.is_dir == fresh_child.is_dir {
                                fresh_child.expanded = prev.expanded;
                                fresh_child.loaded = prev.loaded;
                                fresh_child.unreadable = prev.unreadable;
                                fresh_child.children = prev.children.clone();
                            }
                        }
                        fresh_child
                    })
                    .collect();
                node.loaded = true;
                node.unreadable = false;
            }
            Err(_) => {
                node.children.clear();
                node.loaded = true;
                node.unreadable = true;
            }
        }
    }
}

/// Recursive half of [`Tree::flatten`]: append `nodes` and, for any that are
/// open, their children one level deeper.
fn push_rows(nodes: &[Node], depth: usize, out: &mut Vec<Row>) {
    for n in nodes {
        out.push(Row {
            path: n.path.clone(),
            name: n.name.clone(),
            depth,
            is_dir: n.is_dir,
            expanded: n.expanded,
            unreadable: n.unreadable,
        });
        if n.is_dir && n.expanded {
            push_rows(&n.children, depth + 1, out);
        }
    }
}

/// Directories first, then case-insensitive name; ties broken by the raw name
/// so the order is stable between reads.
///
/// The tiebreak matters more than it looks: `read_dir` returns entries in
/// whatever order the filesystem feels like, and without a total ordering
/// `README` and `readme` could swap places between refreshes, moving rows out
/// from under the cursor.
///
/// Individual failing entries are skipped rather than failing the whole read,
/// so one bad inode does not hide a directory's other contents.
fn read_dir_sorted(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<Node>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        // `file_type` avoids following symlinks; a broken link still lists.
        let is_dir = match entry.file_type() {
            Ok(ft) if ft.is_symlink() => entry.path().is_dir(),
            Ok(ft) => ft.is_dir(),
            Err(_) => false,
        };
        out.push(Node::new(entry.path(), is_dir));
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a
            .name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name)),
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway project directory for a test.
    fn fixture() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let r = td.path();
        fs::create_dir(r.join("src")).unwrap();
        fs::create_dir(r.join("notes")).unwrap();
        fs::create_dir(r.join("notes").join("deep")).unwrap();
        fs::write(r.join("notes").join("deep").join("buried.md"), "x").unwrap();
        fs::write(r.join("src").join("main.py"), "print(1)").unwrap();
        fs::write(r.join("README.md"), "# hi").unwrap();
        fs::write(r.join(".hidden"), "secret").unwrap();
        td
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.name.clone()).collect()
    }

    #[test]
    fn root_loads_sorted_dirs_first_and_hides_dotfiles() {
        let td = fixture();
        let tree = Tree::new(td.path().to_path_buf(), false);
        let rows = tree.flatten();
        // Row 0 is the project root itself.
        assert_eq!(names(&rows)[1..], ["notes", "src", "README.md"]);
    }

    #[test]
    fn show_hidden_reveals_dotfiles() {
        let td = fixture();
        let mut tree = Tree::new(td.path().to_path_buf(), false);
        assert!(!names(&tree.flatten()).contains(&".hidden".to_string()));
        tree.set_show_hidden(true);
        assert!(names(&tree.flatten()).contains(&".hidden".to_string()));
    }

    #[test]
    fn children_load_lazily_on_expand() {
        let td = fixture();
        let mut tree = Tree::new(td.path().to_path_buf(), false);
        let src = td.path().join("src");
        assert!(!tree.find(&src).unwrap().loaded, "not read until expanded");
        assert_eq!(tree.flatten().len(), 4);

        tree.expand(&src);
        assert!(tree.find(&src).unwrap().loaded);
        assert_eq!(
            names(&tree.flatten())[1..],
            ["notes", "src", "main.py", "README.md"]
        );
    }

    #[test]
    fn toggle_expands_then_collapses() {
        let td = fixture();
        let mut tree = Tree::new(td.path().to_path_buf(), false);
        let notes = td.path().join("notes");
        tree.toggle(&notes);
        assert!(tree.find(&notes).unwrap().expanded);
        tree.toggle(&notes);
        assert!(!tree.find(&notes).unwrap().expanded);
        // Collapsed children stay out of the row list.
        assert_eq!(tree.flatten().len(), 4);
    }

    #[test]
    fn expanding_a_file_is_a_noop() {
        let td = fixture();
        let mut tree = Tree::new(td.path().to_path_buf(), false);
        assert!(!tree.expand(&td.path().join("README.md")));
        assert!(!tree.toggle(&td.path().join("README.md")));
    }

    #[test]
    fn refresh_picks_up_new_files_and_keeps_expansion_state() {
        let td = fixture();
        let mut tree = Tree::new(td.path().to_path_buf(), false);
        let notes = td.path().join("notes");
        let deep = notes.join("deep");
        tree.expand(&notes);
        tree.expand(&deep);
        assert!(names(&tree.flatten()).contains(&"buried.md".to_string()));

        fs::write(notes.join("new.md"), "fresh").unwrap();
        tree.refresh_all();

        let after = names(&tree.flatten());
        assert!(after.contains(&"new.md".to_string()), "new file appears");
        assert!(
            after.contains(&"buried.md".to_string()),
            "nested expansion survives a refresh of the parent"
        );
    }

    #[test]
    fn refresh_drops_deleted_entries() {
        let td = fixture();
        let mut tree = Tree::new(td.path().to_path_buf(), false);
        fs::remove_file(td.path().join("README.md")).unwrap();
        tree.refresh_all();
        assert!(!names(&tree.flatten()).contains(&"README.md".to_string()));
    }

    #[test]
    fn find_returns_none_for_paths_outside_the_root() {
        let td = fixture();
        let tree = Tree::new(td.path().to_path_buf(), false);
        assert!(tree.find(Path::new("/definitely/not/here")).is_none());
    }
}
