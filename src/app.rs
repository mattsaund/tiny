//! Application state and input handling.
//!
//! Rendering lives in `ui`; this module owns everything else. The two panes
//! share one selection: moving the tree cursor changes what the preview shows,
//! and focusing the preview starts editing that same file.
//!
//! Open buffers are kept in a map rather than reloaded per selection, so
//! arrowing away from a file with unsaved edits and back does not lose them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Config, Markers, Palette};
use crate::editor::Editor;
use crate::graph;
use crate::highlight::Highlighter;
use crate::media;
use crate::project;
use crate::search::{self, Hit, HitKind};
use crate::tree::{Row, Tree};
use crate::web;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    NewFile,
    NewDir,
    Rename,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub label: String,
    pub input: String,
    pub cursor: usize,
    pub base: PathBuf,
}

/// The bar across the top: project search, or a command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    Search,
    Command,
}

#[derive(Debug, Clone)]
pub struct Bar {
    pub kind: BarKind,
    pub input: String,
    pub cursor: usize,
    pub results: Vec<Hit>,
    pub selected: usize,
    pub scroll: usize,
    /// Set when a search found nothing, so the bar can say so.
    pub searched: bool,
}

impl Bar {
    fn new(kind: BarKind) -> Self {
        Self {
            kind,
            input: String::new(),
            cursor: 0,
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            searched: false,
        }
    }
}

/// The in-program settings area.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub selected: usize,
    pub scroll: usize,
    /// Present while a value is being typed.
    pub editing: Option<String>,
    pub cursor: usize,
}

#[derive(Debug, Clone)]
pub enum ConfirmKind {
    Delete(PathBuf),
    QuitUnsaved,
    Replace { find: String, replace: String },
}

#[derive(Debug, Clone)]
pub struct Confirm {
    pub kind: ConfirmKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum Mode {
    Normal,
    Prompt(Prompt),
    Confirm(Confirm),
    /// The keymap, with a scroll offset for terminals too short for it.
    Help(usize),
    Bar(Bar),
    Settings(Settings),
}

/// How a text file wants to be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// Rendered markdown.
    Markdown,
    /// Wrapped prose — notes, licences, logs. Read first, edited on demand.
    Prose,
    /// Source. Straight into the editor, with line numbers.
    Code,
}

impl TextKind {
    /// Prose and markdown open to be read; code opens to be typed into.
    pub fn reads_first(self) -> bool {
        self != TextKind::Code
    }
}

/// Extensionless files that are prose by convention rather than by suffix.
const PROSE_NAMES: &[&str] = &[
    "LICENCE",
    "LICENSE",
    "COPYING",
    "NOTICE",
    "AUTHORS",
    "CONTRIBUTORS",
    "CHANGELOG",
    "CHANGES",
    "README",
    "TODO",
];

pub fn text_kind(path: &Path, prose_exts: &[String]) -> TextKind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
    {
        // Markdown is checked first: it is also in the prose list by default,
        // but it has a renderer of its own.
        Some(e) if matches!(e.as_str(), "md" | "markdown" | "mdown" | "mkd") => TextKind::Markdown,
        Some(e) if prose_exts.iter().any(|p| p.eq_ignore_ascii_case(&e)) => TextKind::Prose,
        Some(_) => TextKind::Code,
        None => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            if PROSE_NAMES.contains(&name.as_str()) {
                TextKind::Prose
            } else {
                TextKind::Code
            }
        }
    }
}

/// What the preview pane is showing for the current selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// A text file with an open buffer. Notes and prose render when the tree
    /// has focus and show raw source while being edited.
    Buffer {
        path: PathBuf,
        kind: TextKind,
    },
    /// A picture or a video, drawn as half-blocks.
    Media {
        path: PathBuf,
        kind: media::Kind,
        size: u64,
    },
    Directory {
        path: PathBuf,
        entries: usize,
    },
    Binary {
        path: PathBuf,
        size: u64,
        kind: String,
    },
    Unreadable(String),
    Empty,
}

/// A drawn media preview, kept so it is not re-decoded on every keystroke.
pub struct MediaCache {
    pub path: PathBuf,
    pub cols: usize,
    pub rows: usize,
    pub result: std::result::Result<media::Preview, String>,
}

/// Files larger than this are not loaded into the editor.
const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;

pub struct App {
    pub tree: Tree,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub tree_scroll: usize,
    pub focus: Focus,
    pub mode: Mode,
    pub preview: Preview,
    pub preview_scroll: usize,
    pub preview_len: usize,
    /// Markdown opens rendered; `e` drops into raw editing.
    pub read_mode: bool,
    pub buffers: HashMap<PathBuf, Editor>,
    pub config: Config,
    pub palette: Palette,
    pub highlighter: Highlighter,
    pub media: Option<MediaCache>,
    /// The web view's server, once it has been opened at least once.
    pub web: Option<web::Server>,
    pub status: String,
    pub should_quit: bool,
    pub last_edit_height: usize,
    pub last_tree_height: usize,
}

impl App {
    pub fn new(target: project::Target, config: Config, warning: Option<String>) -> Result<Self> {
        let root = target.root.clone();
        if !root.is_dir() {
            return Err(anyhow!("{} is not a directory", root.display()));
        }
        let (highlighter, theme_warning) = Highlighter::with_theme(&config.syntax_theme);
        let tree = Tree::new(root, config.show_hidden);
        let rows = tree.flatten();
        let palette = Palette::from_theme(&config.theme);

        let opening = if target.created {
            "new project — ? for help".to_string()
        } else {
            "? for help".to_string()
        };
        let mut app = Self {
            tree,
            rows,
            selected: 0,
            tree_scroll: 0,
            focus: Focus::Tree,
            mode: Mode::Normal,
            preview: Preview::Empty,
            preview_scroll: 0,
            preview_len: 0,
            read_mode: true,
            buffers: HashMap::new(),
            config,
            palette,
            highlighter,
            media: None,
            web: None,
            status: warning.or(theme_warning).unwrap_or(opening),
            should_quit: false,
            last_edit_height: 20,
            last_tree_height: 20,
        };
        app.sync_preview();

        // `tiny <file>` lands on that file with the editor already open, which
        // is the whole point of naming a file instead of a folder.
        if let Some(file) = target.file {
            app.reveal(&file);
            if matches!(app.preview, Preview::Buffer { .. }) {
                app.focus_editor();
            }
        }
        Ok(app)
    }

    pub fn root(&self) -> &Path {
        self.tree.root_path()
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected_row().map(|r| r.path.as_path())
    }

    pub fn active_buffer(&self) -> Option<&Editor> {
        match &self.preview {
            Preview::Buffer { path, .. } => self.buffers.get(path),
            _ => None,
        }
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut Editor> {
        match &self.preview {
            Preview::Buffer { path, .. } => {
                let path = path.clone();
                self.buffers.get_mut(&path)
            }
            _ => None,
        }
    }

    pub fn dirty_buffers(&self) -> Vec<&Path> {
        self.buffers
            .values()
            .filter(|e| e.dirty)
            .map(|e| e.path.as_path())
            .collect()
    }

    pub fn is_dirty(&self, path: &Path) -> bool {
        self.buffers.get(path).is_some_and(|e| e.dirty)
    }

    /// Glyphs for an open folder, a closed folder, and a file.
    pub fn tree_markers(&self) -> (&'static str, &'static str, &'static str) {
        match self.config.markers {
            Markers::Arrows => ("▾ ", "▸ ", "  "),
            Markers::Ascii => ("- ", "+ ", "  "),
        }
    }

    /// Settings the graph is built with, derived from the live config so
    /// `:set show_hidden true` changes the web view too.
    pub fn graph_options(&self) -> graph::Options {
        graph::Options {
            ignore: self.config.search_ignore.clone(),
            show_hidden: self.config.show_hidden,
            prose_extensions: self.config.prose_extensions.clone(),
            max_ambiguity: self.config.graph_max_ambiguity,
        }
    }

    /// Start the web view if it is not already up, and point a browser at it.
    fn open_web(&mut self) {
        if self.web.is_none() {
            let root = self.root().to_path_buf();
            let options = self.graph_options();
            match web::Server::start(root, options, self.config.web_port) {
                Ok(s) => self.web = Some(s),
                Err(e) => {
                    self.status = format!("web view: {e:#}");
                    return;
                }
            }
        }
        let url = self.web.as_ref().expect("just started").url();
        self.status = if web::open_in_browser(&url) {
            format!("web view at {url}")
        } else {
            // No desktop to hand it to; the address is still the answer.
            format!("web view at {url} — open it yourself")
        };
    }

    /// A file the web view asked tiny to open, if any.
    pub fn take_web_open(&self) -> Option<PathBuf> {
        self.web.as_ref()?.take_open_request()
    }

    /// Move the cursor to a path and start editing it. Used by the web view.
    pub fn open_path(&mut self, path: &Path) {
        if self.reveal(path) {
            if matches!(self.preview, Preview::Buffer { .. }) {
                self.focus_editor();
            }
            self.status = format!("opened {}", display_name(path));
        } else {
            self.status = format!("{} is not in this project", display_name(path));
        }
    }

    fn search_opts(&self) -> search::Opts {
        search::Opts {
            max_results: self.config.max_search_results,
            ignore: self.config.search_ignore.clone(),
            show_hidden: self.config.show_hidden,
        }
    }

    // ---- selection & preview ---------------------------------------------

    fn rebuild_rows(&mut self) {
        let want = self.selected_path().map(Path::to_path_buf);
        self.rows = self.tree.flatten();
        if let Some(want) = want {
            if let Some(i) = self.rows.iter().position(|r| r.path == want) {
                self.selected = i;
            } else {
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            }
        }
        self.sync_preview();
    }

    /// Open every folder on the way to `path` and put the cursor on it.
    fn reveal(&mut self, path: &Path) -> bool {
        let root = self.tree.root_path().to_path_buf();
        let Ok(rel) = path.strip_prefix(&root) else {
            return false;
        };
        let mut cur = root;
        let parts: Vec<_> = rel.components().collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            cur.push(part);
            self.tree.expand(&cur);
        }
        self.rows = self.tree.flatten();
        match self.rows.iter().position(|r| r.path == path) {
            Some(i) => {
                self.selected = i;
                self.sync_preview();
                true
            }
            None => {
                self.sync_preview();
                false
            }
        }
    }

    fn sync_preview(&mut self) {
        self.preview_scroll = 0;
        let Some(row) = self.selected_row().cloned() else {
            self.preview = Preview::Empty;
            return;
        };
        if row.is_dir {
            if row.unreadable {
                self.preview = Preview::Unreadable(format!("cannot read {}", row.name));
                return;
            }
            let entries = fs::read_dir(&row.path).map(|d| d.count()).unwrap_or(0);
            self.preview = Preview::Directory {
                path: row.path,
                entries,
            };
            return;
        }
        self.preview = self.load_file_preview(&row.path);
    }

    fn load_file_preview(&mut self, path: &Path) -> Preview {
        // A buffer already open, possibly with unsaved edits, wins over disk.
        if self.buffers.contains_key(path) {
            return Preview::Buffer {
                path: path.to_path_buf(),
                kind: text_kind(path, &self.config.prose_extensions),
            };
        }
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => return Preview::Unreadable(format!("{}: {e}", display_name(path))),
        };
        let size = meta.len();

        let kind = media::classify(path);
        if kind != media::Kind::Other && self.config.media_preview {
            return Preview::Media {
                path: path.to_path_buf(),
                kind,
                size,
            };
        }
        if size > MAX_EDIT_BYTES {
            return Preview::Binary {
                path: path.to_path_buf(),
                size,
                kind: "large file".into(),
            };
        }
        match fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => {
                    self.buffers.insert(
                        path.to_path_buf(),
                        Editor::from_str(path.to_path_buf(), &text),
                    );
                    Preview::Buffer {
                        path: path.to_path_buf(),
                        kind: text_kind(path, &self.config.prose_extensions),
                    }
                }
                _ => Preview::Binary {
                    path: path.to_path_buf(),
                    size,
                    kind: binary_kind(path).into(),
                },
            },
            Err(e) => Preview::Unreadable(format!("{}: {e}", display_name(path))),
        }
    }

    /// Decode a picture or video frame for the pane, reusing the last one when
    /// nothing about the request has changed.
    pub fn ensure_media(&mut self, path: &Path, kind: media::Kind, cols: usize, rows: usize) {
        let fresh = self
            .media
            .as_ref()
            .is_some_and(|c| c.path == path && c.cols == cols && c.rows == rows);
        if fresh {
            return;
        }
        let result = media::render(path, kind, cols, rows).map_err(|e| format!("{e:#}"));
        self.media = Some(MediaCache {
            path: path.to_path_buf(),
            cols,
            rows,
            result,
        });
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        if next != self.selected {
            self.selected = next;
            self.sync_preview();
        }
    }

    fn select_index(&mut self, i: usize) {
        let i = i.min(self.rows.len().saturating_sub(1));
        if i != self.selected {
            self.selected = i;
            self.sync_preview();
        }
    }

    // ---- key dispatch -----------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        // Take the mode out so handlers can own it without fighting the borrow
        // checker, then put back whatever they leave behind.
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Help(scroll) => self.on_help_key(scroll, key),
            Mode::Prompt(p) => self.on_prompt_key(p, key),
            Mode::Confirm(c) => self.on_confirm_key(c, key),
            Mode::Bar(b) => self.on_bar_key(b, key),
            Mode::Settings(s) => self.on_settings_key(s, key),
            Mode::Normal => match self.focus {
                Focus::Tree => self.on_tree_key(key),
                Focus::Editor => self.on_editor_key(key),
            },
        }
    }

    /// Arrows scroll the keymap; anything else puts it away.
    fn on_help_key(&mut self, scroll: usize, key: KeyEvent) {
        let page = self.last_tree_height.max(1);
        let next = match key.code {
            KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => scroll + 1,
            KeyCode::PageUp => scroll.saturating_sub(page),
            KeyCode::PageDown => scroll + page,
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => usize::MAX,
            _ => return,
        };
        self.mode = Mode::Help(next);
    }

    fn on_tree_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.last_tree_height.saturating_sub(1).max(1) as isize;
        match key.code {
            KeyCode::Char('s') if ctrl => self.save_active(),
            KeyCode::Char('q') if ctrl => self.request_quit(),
            KeyCode::Char('f') if ctrl => self.open_bar(BarKind::Search),
            KeyCode::Char('p') if ctrl => self.open_bar(BarKind::Command),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::Home | KeyCode::Char('g') => self.select_index(0),
            KeyCode::End | KeyCode::Char('G') => self.select_index(usize::MAX),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.activate(),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_or_parent(),
            KeyCode::Tab => self.focus_editor(),
            KeyCode::Char('/') => self.open_bar(BarKind::Search),
            KeyCode::Char(':') => self.open_bar(BarKind::Command),
            KeyCode::Char(',') | KeyCode::F(2) => self.open_settings(),
            KeyCode::Char('w') => self.open_web(),
            KeyCode::Char('n') => self.begin_prompt(PromptKind::NewFile),
            KeyCode::Char('N') => self.begin_prompt(PromptKind::NewDir),
            KeyCode::Char('r') => self.begin_prompt(PromptKind::Rename),
            KeyCode::Char('d') => self.begin_delete(),
            KeyCode::Char('.') => self.toggle_hidden(),
            KeyCode::Char('R') | KeyCode::F(5) => self.refresh(),
            KeyCode::Char('?') => self.mode = Mode::Help(0),
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            _ => {}
        }
    }

    fn on_editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let tab_width = self.config.tab_width;
        let page = self.last_edit_height.saturating_sub(1).max(1);

        // Keys that leave the pane or act on the file come first, so they can
        // never be swallowed as text input.
        match key.code {
            KeyCode::Char('s') if ctrl => return self.save_active(),
            KeyCode::Char('q') if ctrl => return self.request_quit(),
            KeyCode::Char('f') if ctrl => return self.open_bar(BarKind::Search),
            // Ctrl+P reaches the command bar without leaving the editor, where
            // a bare `:` is just a character being typed.
            KeyCode::Char('p') if ctrl => return self.open_bar(BarKind::Command),
            KeyCode::Esc => {
                self.focus = Focus::Tree;
                self.status = "back to tree".into();
                return;
            }
            _ => {}
        }

        if self.read_mode {
            return self.on_read_key(key, page);
        }

        let Some(ed) = self.active_buffer_mut() else {
            self.focus = Focus::Tree;
            return;
        };
        match key.code {
            KeyCode::Char('z') if ctrl => {
                if !ed.undo() {
                    self.status = "nothing to undo".into();
                }
            }
            KeyCode::Char('y') if ctrl => {
                if !ed.redo() {
                    self.status = "nothing to redo".into();
                }
            }
            KeyCode::Char('k') if ctrl => ed.delete_line(),
            KeyCode::Left if ctrl => ed.move_word_left(),
            KeyCode::Right if ctrl => ed.move_word_right(),
            KeyCode::Home if ctrl => ed.move_doc_start(),
            KeyCode::End if ctrl => ed.move_doc_end(),
            KeyCode::Left => ed.move_left(),
            KeyCode::Right => ed.move_right(),
            KeyCode::Up => ed.move_up(),
            KeyCode::Down => ed.move_down(),
            KeyCode::Home => ed.move_home(),
            KeyCode::End => ed.move_end(),
            KeyCode::PageUp => ed.page_up(page),
            KeyCode::PageDown => ed.page_down(page),
            KeyCode::Enter => ed.insert_newline(),
            KeyCode::Backspace => ed.backspace(),
            KeyCode::Delete => ed.delete_forward(),
            KeyCode::Tab => ed.insert_tab(tab_width),
            KeyCode::Char(c) if !ctrl => ed.insert_char(c),
            _ => {}
        }
    }

    /// Reading rendered markdown, or looking at a picture: arrows scroll.
    fn on_read_key(&mut self, key: KeyEvent, page: usize) {
        let max = self.preview_len.saturating_sub(1);
        match key.code {
            KeyCode::Char('e') | KeyCode::Char('i') => {
                if matches!(self.preview, Preview::Buffer { .. }) {
                    self.read_mode = false;
                    self.status = "editing — ^S save, Esc back".into();
                }
            }
            KeyCode::Char('/') => self.open_bar(BarKind::Search),
            KeyCode::Char(':') => self.open_bar(BarKind::Command),
            KeyCode::Up | KeyCode::Char('k') => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.preview_scroll = (self.preview_scroll + 1).min(max)
            }
            KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(page),
            KeyCode::PageDown => self.preview_scroll = (self.preview_scroll + page).min(max),
            KeyCode::Home | KeyCode::Char('g') => self.preview_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.preview_scroll = max,
            _ => {}
        }
    }

    // ---- the bar ----------------------------------------------------------

    fn open_bar(&mut self, kind: BarKind) {
        self.mode = Mode::Bar(Bar::new(kind));
        self.status = match kind {
            BarKind::Search => "search the project — Enter to jump, Esc to close".into(),
            BarKind::Command => "command — :set, :replace, :config, :help".into(),
        };
    }

    fn on_bar_key(&mut self, mut b: Bar, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            // Ctrl+Q means quit everywhere, including here. No other chord is
            // allowed to fall through and type its letter into the query.
            if key.code == KeyCode::Char('q') {
                self.request_quit();
            } else {
                self.mode = Mode::Bar(b);
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.status = "closed".into();
                return;
            }
            KeyCode::Enter => {
                match b.kind {
                    BarKind::Search => {
                        if let Some(hit) = b.results.get(b.selected).cloned() {
                            self.jump_to(&hit);
                            self.status = display_name(&hit.path);
                        } else {
                            self.status = "no matches".into();
                        }
                    }
                    BarKind::Command => self.run_command(&b.input.clone()),
                }
                return;
            }
            KeyCode::Up => {
                b.selected = b.selected.saturating_sub(1);
                self.preview_hit(&b);
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Down => {
                if !b.results.is_empty() {
                    b.selected = (b.selected + 1).min(b.results.len() - 1);
                }
                self.preview_hit(&b);
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Tab => {
                if b.kind == BarKind::Command {
                    complete_command(&mut b);
                }
                self.mode = Mode::Bar(b);
                return;
            }
            KeyCode::Backspace => {
                if b.cursor > 0 {
                    let s = char_byte(&b.input, b.cursor - 1);
                    let e = char_byte(&b.input, b.cursor);
                    b.input.replace_range(s..e, "");
                    b.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let n = b.input.chars().count();
                if b.cursor < n {
                    let s = char_byte(&b.input, b.cursor);
                    let e = char_byte(&b.input, b.cursor + 1);
                    b.input.replace_range(s..e, "");
                }
            }
            KeyCode::Left => b.cursor = b.cursor.saturating_sub(1),
            KeyCode::Right => b.cursor = (b.cursor + 1).min(b.input.chars().count()),
            KeyCode::Home => b.cursor = 0,
            KeyCode::End => b.cursor = b.input.chars().count(),
            KeyCode::Char(c) => {
                let byte = char_byte(&b.input, b.cursor);
                b.input.insert(byte, c);
                b.cursor += 1;
            }
            _ => {
                self.mode = Mode::Bar(b);
                return;
            }
        }

        if b.kind == BarKind::Search {
            self.run_search(&mut b);
            self.preview_hit(&b);
        }
        self.mode = Mode::Bar(b);
    }

    fn run_search(&mut self, b: &mut Bar) {
        let opts = self.search_opts();
        b.results = search::search(self.tree.root_path(), &b.input, &opts);
        b.searched = !b.input.trim().is_empty();
        b.selected = 0;
        b.scroll = 0;
    }

    /// Show the highlighted result in the preview pane without leaving the bar.
    fn preview_hit(&mut self, b: &Bar) {
        let Some(hit) = b.results.get(b.selected) else {
            return;
        };
        let hit = hit.clone();
        self.reveal(&hit.path);
        self.place_cursor_on(&hit);
    }

    /// Move to a result and hand focus to the pane it is in.
    fn jump_to(&mut self, hit: &Hit) {
        self.reveal(&hit.path);
        self.place_cursor_on(hit);
        if matches!(self.preview, Preview::Buffer { .. }) {
            self.focus = Focus::Editor;
        }
    }

    fn place_cursor_on(&mut self, hit: &Hit) {
        if hit.kind == HitKind::Content {
            // A match inside a note is easier to see as raw source with the
            // cursor sitting on it than as rendered prose.
            self.read_mode = false;
            let (line, col) = (hit.line, hit.col);
            if let Some(ed) = self.active_buffer_mut() {
                ed.goto(line, col);
            }
        } else {
            self.read_mode = matches!(self.preview, Preview::Buffer { kind, .. } if kind.reads_first())
                || matches!(self.preview, Preview::Media { .. });
        }
    }

    // ---- commands ---------------------------------------------------------

    fn run_command(&mut self, line: &str) {
        let args = split_args(line);
        let Some(cmd) = args.first().map(String::as_str) else {
            self.status = "no command".into();
            return;
        };
        let rest = &args[1..];
        let result = match cmd {
            "set" => self.cmd_set(rest),
            "replace" | "sub" => self.cmd_replace(rest),
            "config" | "settings" => {
                self.open_settings();
                Ok(String::new())
            }
            "graph" | "web" => {
                self.open_web();
                Ok(String::new())
            }
            "w" | "write" | "save" => {
                self.save_active();
                Ok(String::new())
            }
            "q" | "quit" => {
                self.request_quit();
                Ok(String::new())
            }
            "wq" => {
                self.save_active();
                self.request_quit();
                Ok(String::new())
            }
            "help" => {
                self.mode = Mode::Help(0);
                Ok(String::new())
            }
            "reload" | "refresh" => {
                self.refresh();
                Ok(String::new())
            }
            "new" => self.cmd_new(rest, false),
            "mkdir" => self.cmd_new(rest, true),
            "init" => {
                let root = self.root().to_path_buf();
                project::init(&root, false).map(|()| {
                    self.refresh();
                    "project initialised".to_string()
                })
            }
            other => Err(anyhow!("unknown command `{other}` — try :help")),
        };
        match result {
            Ok(msg) if !msg.is_empty() => self.status = msg,
            Ok(_) => {}
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn cmd_set(&mut self, args: &[String]) -> Result<String> {
        let key = args.first().ok_or_else(|| anyhow!(":set <key> <value>"))?;
        if args.len() < 2 {
            // With no value, report the current one rather than erroring.
            let value = self
                .config
                .get(key)
                .ok_or_else(|| anyhow!("unknown setting `{key}`"))?;
            return Ok(format!("{key} = {value}"));
        }
        let value = args[1..].join(" ");
        self.config.set(key, &value)?;
        self.apply_config();
        Ok(format!("{key} = {value}"))
    }

    fn cmd_replace(&mut self, args: &[String]) -> Result<String> {
        if args.len() < 2 {
            return Err(anyhow!(
                ":replace <find> <replace> — quote strings containing spaces"
            ));
        }
        let find = args[0].clone();
        let replace = args[1].clone();
        let report = search::count(self.tree.root_path(), &find, &self.search_opts());
        if report.occurrences == 0 {
            return Ok(format!("`{find}` is not in this project"));
        }
        // Rewriting many files at once is worth a second look first.
        self.mode = Mode::Confirm(Confirm {
            message: format!(
                "Replace {} occurrence{} of `{find}` with `{replace}` across {} file{}?  (y/n)",
                report.occurrences,
                plural(report.occurrences),
                report.files,
                plural(report.files),
            ),
            kind: ConfirmKind::Replace { find, replace },
        });
        Ok(String::new())
    }

    fn cmd_new(&mut self, args: &[String], is_dir: bool) -> Result<String> {
        let name = args
            .first()
            .ok_or_else(|| anyhow!("give a name: :new notes/today.md"))?;
        let base = self.creation_base();
        self.create_entry(&base, name, is_dir)
    }

    /// Re-derive everything a settings change can affect.
    fn apply_config(&mut self) {
        self.palette = Palette::from_theme(&self.config.theme);
        let (hl, _) = Highlighter::with_theme(&self.config.syntax_theme);
        self.highlighter = hl;
        if self.tree.show_hidden() != self.config.show_hidden {
            self.tree.set_show_hidden(self.config.show_hidden);
            self.rebuild_rows();
        }
        // A media size change invalidates whatever was drawn.
        self.media = None;
        if let Some(server) = &self.web {
            server.update_options(self.graph_options());
        }
    }

    // ---- settings area ----------------------------------------------------

    fn open_settings(&mut self) {
        self.mode = Mode::Settings(Settings::default());
        self.status = "settings — Enter to change, ^S to write tiny.conf, Esc to close".into();
    }

    fn on_settings_key(&mut self, mut s: Settings, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let index = Config::settings_index();
        let last = index.len().saturating_sub(1);

        // Editing a value: the keys go into the field.
        if let Some(mut buf) = s.editing.take() {
            if ctrl {
                if key.code == KeyCode::Char('s') {
                    match self.config.save() {
                        Ok(p) => self.status = format!("wrote {}", p.display()),
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                s.editing = Some(buf);
                self.mode = Mode::Settings(s);
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    self.status = "unchanged".into();
                }
                KeyCode::Enter => {
                    let key_name = index[s.selected.min(last)].0;
                    match self.config.set(key_name, &buf) {
                        Ok(()) => {
                            self.apply_config();
                            self.status = format!("{key_name} = {buf}");
                        }
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                KeyCode::Backspace => {
                    if s.cursor > 0 {
                        let a = char_byte(&buf, s.cursor - 1);
                        let b = char_byte(&buf, s.cursor);
                        buf.replace_range(a..b, "");
                        s.cursor -= 1;
                    }
                    s.editing = Some(buf);
                }
                KeyCode::Left => {
                    s.cursor = s.cursor.saturating_sub(1);
                    s.editing = Some(buf);
                }
                KeyCode::Right => {
                    s.cursor = (s.cursor + 1).min(buf.chars().count());
                    s.editing = Some(buf);
                }
                KeyCode::Char(c) => {
                    let byte = char_byte(&buf, s.cursor);
                    buf.insert(byte, c);
                    s.cursor += 1;
                    s.editing = Some(buf);
                }
                _ => s.editing = Some(buf),
            }
            self.mode = Mode::Settings(s);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.status = "closed".into();
                return;
            }
            KeyCode::Char('s') if ctrl => match self.config.save() {
                Ok(p) => self.status = format!("wrote {}", p.display()),
                Err(e) => self.status = format!("{e:#}"),
            },
            KeyCode::Up | KeyCode::Char('k') => s.selected = s.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => s.selected = (s.selected + 1).min(last),
            KeyCode::Home | KeyCode::Char('g') => s.selected = 0,
            KeyCode::End | KeyCode::Char('G') => s.selected = last,
            KeyCode::Enter => {
                let key_name = index[s.selected.min(last)].0;
                let current = self.config.get(key_name).unwrap_or_default();
                s.cursor = current.chars().count();
                s.editing = Some(current);
            }
            _ => {}
        }
        self.mode = Mode::Settings(s);
    }

    // ---- prompts & confirmations -----------------------------------------

    fn on_prompt_key(&mut self, mut p: Prompt, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode = Mode::Prompt(p);
            return;
        }
        match key.code {
            KeyCode::Esc => self.status = "cancelled".into(),
            KeyCode::Enter => self.commit_prompt(p),
            KeyCode::Backspace => {
                if p.cursor > 0 {
                    let b = char_byte(&p.input, p.cursor - 1);
                    let e = char_byte(&p.input, p.cursor);
                    p.input.replace_range(b..e, "");
                    p.cursor -= 1;
                }
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Delete => {
                let n = p.input.chars().count();
                if p.cursor < n {
                    let b = char_byte(&p.input, p.cursor);
                    let e = char_byte(&p.input, p.cursor + 1);
                    p.input.replace_range(b..e, "");
                }
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Left => {
                p.cursor = p.cursor.saturating_sub(1);
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Right => {
                p.cursor = (p.cursor + 1).min(p.input.chars().count());
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Home => {
                p.cursor = 0;
                self.mode = Mode::Prompt(p);
            }
            KeyCode::End => {
                p.cursor = p.input.chars().count();
                self.mode = Mode::Prompt(p);
            }
            KeyCode::Char(c) => {
                let b = char_byte(&p.input, p.cursor);
                p.input.insert(b, c);
                p.cursor += 1;
                self.mode = Mode::Prompt(p);
            }
            _ => self.mode = Mode::Prompt(p),
        }
    }

    fn on_confirm_key(&mut self, c: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => match c.kind {
                ConfirmKind::Delete(path) => self.do_delete(&path),
                ConfirmKind::QuitUnsaved => self.should_quit = true,
                ConfirmKind::Replace { find, replace } => self.do_replace(&find, &replace),
            },
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.status = "cancelled".into()
            }
            _ => self.mode = Mode::Confirm(c),
        }
    }

    fn do_replace(&mut self, find: &str, replace: &str) {
        let opts = self.search_opts();
        match search::replace_all(self.tree.root_path(), find, replace, &opts) {
            Ok(report) => {
                // Files changed underneath any open buffer, so drop the clean
                // ones and let them re-read.
                self.buffers.retain(|_, e| e.dirty);
                self.tree.refresh_all();
                self.rebuild_rows();
                self.status = format!(
                    "replaced {} occurrence{} in {} file{}",
                    report.occurrences,
                    plural(report.occurrences),
                    report.files,
                    plural(report.files)
                );
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    // ---- actions ----------------------------------------------------------

    fn activate(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir {
            if row.expanded {
                self.move_selection(1);
            } else {
                self.tree.expand(&row.path);
                self.rebuild_rows();
            }
        } else {
            self.focus_editor();
        }
    }

    fn focus_editor(&mut self) {
        match &self.preview {
            Preview::Buffer { kind, .. } => {
                // Markdown opens rendered; code goes straight into the editor,
                // since there is nothing else to show for it.
                self.read_mode = kind.reads_first();
                self.focus = Focus::Editor;
                self.status = if self.read_mode {
                    "reading — e to edit, Esc back".into()
                } else {
                    "editing — ^S save, Esc back".into()
                };
            }
            Preview::Media { .. } => {
                self.read_mode = true;
                self.focus = Focus::Editor;
                self.status = "viewing — Esc back".into();
            }
            Preview::Binary { kind, .. } => {
                self.status = format!("{kind} — not editable");
            }
            _ => {}
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir && row.expanded {
            self.tree.collapse(&row.path);
            self.rebuild_rows();
            return;
        }
        let Some(parent) = row.path.parent() else {
            return;
        };
        if let Some(i) = self.rows.iter().position(|r| r.path == parent) {
            self.select_index(i);
        }
    }

    fn toggle_hidden(&mut self) {
        let next = !self.tree.show_hidden();
        self.tree.set_show_hidden(next);
        self.config.show_hidden = next;
        self.rebuild_rows();
        self.status = if next {
            "showing hidden files".into()
        } else {
            "hiding hidden files".into()
        };
    }

    fn refresh(&mut self) {
        self.tree.refresh_all();
        // Drop clean buffers so they re-read from disk; keep unsaved work.
        self.buffers.retain(|_, e| e.dirty);
        self.media = None;
        self.rebuild_rows();
        self.status = "refreshed".into();
    }

    pub fn save_active(&mut self) {
        let Some(ed) = self.active_buffer_mut() else {
            self.status = "nothing to save".into();
            return;
        };
        if !ed.dirty {
            self.status = "no changes".into();
            return;
        }
        let name = display_name(&ed.path.clone());
        match ed.save() {
            Ok(()) => self.status = format!("saved {name}"),
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    fn request_quit(&mut self) {
        let dirty = self.dirty_buffers();
        if dirty.is_empty() {
            self.should_quit = true;
            return;
        }
        let names: Vec<String> = dirty.iter().map(|p| display_name(p)).collect();
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::QuitUnsaved,
            message: format!("Discard unsaved changes to {}?  (y/n)", names.join(", ")),
        });
    }

    fn creation_base(&self) -> PathBuf {
        match self.selected_row() {
            Some(r) if r.is_dir => r.path.clone(),
            Some(r) => r
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.tree.root_path().to_path_buf()),
            None => self.tree.root_path().to_path_buf(),
        }
    }

    fn begin_prompt(&mut self, kind: PromptKind) {
        let (label, input, base) = match kind {
            PromptKind::NewFile => ("New file".to_string(), String::new(), self.creation_base()),
            PromptKind::NewDir => (
                "New folder".to_string(),
                String::new(),
                self.creation_base(),
            ),
            PromptKind::Rename => {
                let Some(row) = self.selected_row() else {
                    return;
                };
                if row.path == self.tree.root_path() {
                    self.status = "cannot rename the project root".into();
                    return;
                }
                let parent = row
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.tree.root_path().to_path_buf());
                ("Rename".to_string(), row.name.clone(), parent)
            }
        };
        let cursor = input.chars().count();
        self.mode = Mode::Prompt(Prompt {
            kind,
            label,
            input,
            cursor,
            base,
        });
    }

    fn commit_prompt(&mut self, p: Prompt) {
        let name = p.input.trim().to_string();
        if name.is_empty() {
            self.status = "cancelled".into();
            return;
        }
        let result = match p.kind {
            PromptKind::NewFile => self.create_entry(&p.base, &name, false),
            PromptKind::NewDir => self.create_entry(&p.base, &name, true),
            PromptKind::Rename => self.rename_selected(&p.base, &name),
        };
        match result {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Create a file or directory. A name containing separators creates the
    /// intermediate directories too, so `notes/2026/today.md` works in one go.
    fn create_entry(&mut self, base: &Path, name: &str, is_dir: bool) -> Result<String> {
        let target = safe_join(base, name, self.tree.root_path())?;
        if target.exists() {
            return Err(anyhow!("{} already exists", display_name(&target)));
        }
        if is_dir {
            fs::create_dir_all(&target)
                .with_context(|| format!("cannot create {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            fs::write(&target, "")
                .with_context(|| format!("cannot create {}", target.display()))?;
        }
        self.tree.refresh_all();
        if is_dir {
            self.tree.expand(&target);
        }
        self.reveal(&target);
        Ok(format!("created {}", display_name(&target)))
    }

    fn rename_selected(&mut self, base: &Path, name: &str) -> Result<String> {
        let Some(row) = self.selected_row().cloned() else {
            return Err(anyhow!("nothing selected"));
        };
        let target = safe_join(base, name, self.tree.root_path())?;
        if target == row.path {
            return Ok("unchanged".into());
        }
        if target.exists() {
            return Err(anyhow!("{} already exists", display_name(&target)));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&row.path, &target)
            .with_context(|| format!("cannot rename {}", display_name(&row.path)))?;

        // Carry any open buffer, and its unsaved edits, to the new path.
        if let Some(mut ed) = self.buffers.remove(&row.path) {
            ed.path = target.clone();
            self.buffers.insert(target.clone(), ed);
        }
        self.tree.refresh_all();
        self.reveal(&target);
        Ok(format!("renamed to {}", display_name(&target)))
    }

    fn begin_delete(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.path == self.tree.root_path() {
            self.status = "cannot delete the project root".into();
            return;
        }
        // Say how much is at stake before asking — deleting a folder takes
        // everything under it.
        let message = if row.is_dir {
            let n = fs::read_dir(&row.path).map(|d| d.count()).unwrap_or(0);
            format!(
                "Delete folder {} and its {} entr{}?  (y/n)",
                row.name,
                n,
                if n == 1 { "y" } else { "ies" }
            )
        } else {
            format!("Delete {}?  (y/n)", row.name)
        };
        self.mode = Mode::Confirm(Confirm {
            kind: ConfirmKind::Delete(row.path),
            message,
        });
    }

    fn do_delete(&mut self, path: &Path) {
        let is_dir = path.is_dir();
        let result = if is_dir {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match result {
            Ok(()) => {
                self.buffers.retain(|p, _| !p.starts_with(path));
                self.media = None;
                let parent = path.parent().map(Path::to_path_buf);
                self.tree.refresh_all();
                self.rows = self.tree.flatten();
                if let Some(i) = parent.and_then(|p| self.rows.iter().position(|r| r.path == p)) {
                    self.selected = i;
                } else {
                    self.selected = self.selected.min(self.rows.len().saturating_sub(1));
                }
                self.sync_preview();
                self.status = format!("deleted {}", display_name(path));
            }
            Err(e) => self.status = format!("delete failed: {e}"),
        }
    }
}

// ---- free helpers ---------------------------------------------------------

fn binary_kind(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("pdf") => "PDF",
        Some("zip" | "gz" | "tar" | "xz" | "zst" | "7z") => "archive",
        Some("mp3" | "wav" | "flac" | "ogg") => "audio",
        Some("so" | "dll" | "dylib" | "o" | "a") => "object file",
        _ => "binary",
    }
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Byte offset of character index `ci`.
fn char_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

/// Split a command line on whitespace, keeping double-quoted runs together so
/// `:replace "old thing" "new thing"` works.
pub fn split_args(line: &str) -> Vec<String> {
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

/// Fill in the rest of a command name, or a setting name after `set`.
fn complete_command(b: &mut Bar) {
    const COMMANDS: &[&str] = &[
        "set", "replace", "config", "settings", "graph", "web", "write", "save", "quit", "help",
        "reload", "new", "mkdir", "init",
    ];
    let args = split_args(&b.input);
    let trailing_space = b.input.ends_with(' ');
    let (prefix, candidates): (String, Vec<String>) = match args.len() {
        0 => (
            String::new(),
            COMMANDS.iter().map(|s| s.to_string()).collect(),
        ),
        1 if !trailing_space => (
            args[0].clone(),
            COMMANDS.iter().map(|s| s.to_string()).collect(),
        ),
        _ if args[0] == "set" => {
            let typed = if trailing_space {
                String::new()
            } else {
                args.last().cloned().unwrap_or_default()
            };
            (
                typed,
                Config::settings_index()
                    .iter()
                    .map(|(k, _)| k.to_string())
                    .collect(),
            )
        }
        _ => return,
    };

    let matches: Vec<&String> = candidates
        .iter()
        .filter(|c| c.starts_with(&prefix))
        .collect();
    let Some(first) = matches.first() else { return };
    // With several options, fill in only what they all agree on.
    let common = matches.iter().skip(1).fold((*first).clone(), |acc, m| {
        acc.chars()
            .zip(m.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    if common.len() <= prefix.len() {
        return;
    }
    let base = &b.input[..b.input.len() - prefix.len()];
    b.input = format!("{base}{common}");
    b.cursor = b.input.chars().count();
}

/// Join a user-typed name onto `base`, refusing anything that would escape the
/// project root — a typed `../../etc/passwd` must not create a file there.
fn safe_join(base: &Path, name: &str, root: &Path) -> Result<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Markers, Position, Side};
    use crossterm::event::KeyEventKind;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    /// A small project with one of each thing the panes have to handle.
    fn build(dir: &Path) {
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("notes/design.md"),
            "# Design Notes\n\nThe core idea is a split view. See [[architecture]].\n\n- one\n- two\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/main.py"),
            "import utils\n\n\ndef main():\n    return utils.load()\n",
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Fixture\n\nhello widget\n").unwrap();
        fs::write(dir.join("logo.png"), [0x89u8, b'P', b'N', b'G', 0, 1, 2, 3]).unwrap();
    }

    fn target(root: &Path, file: Option<PathBuf>) -> project::Target {
        project::Target {
            root: root.to_path_buf(),
            file,
            created: false,
        }
    }

    fn fixture() -> (tempfile::TempDir, App) {
        fixture_with(Config::default())
    }

    fn fixture_with(cfg: Config) -> (tempfile::TempDir, App) {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let app = App::new(target(td.path(), None), cfg, None).unwrap();
        (td, app)
    }

    fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect::<String>()
            })
            .collect()
    }

    fn joined(app: &mut App) -> String {
        screen(app, 90, 24).join("\n")
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn ch(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..k(KeyCode::Char(c))
        }
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(ch(c));
        }
    }

    /// Run a command line through the bar, exactly as a user would.
    fn command(app: &mut App, line: &str) {
        app.on_key(ch(':'));
        type_str(app, line);
        app.on_key(k(KeyCode::Enter));
    }

    fn select(app: &mut App, name: &str) {
        for _ in 0..12 {
            let paths: Vec<PathBuf> = app
                .rows
                .iter()
                .filter(|r| r.is_dir && !r.expanded)
                .map(|r| r.path.clone())
                .collect();
            if paths.is_empty() {
                break;
            }
            for p in paths {
                app.tree.expand(&p);
            }
            app.rows = app.tree.flatten();
        }
        let i = app
            .rows
            .iter()
            .position(|r| r.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no row named {name} in {:?}",
                    app.rows.iter().map(|r| &r.name).collect::<Vec<_>>()
                )
            });
        app.selected = i;
        app.sync_preview();
    }

    // ---- layout ----------------------------------------------------------

    #[test]
    fn opens_with_the_project_tree_and_a_preview() {
        let (_td, mut app) = fixture();
        let out = joined(&mut app);
        assert!(out.contains("PROJECT"), "{out}");
        for name in ["notes", "src", "README.md", "logo.png"] {
            assert!(out.contains(name), "tree is missing {name}:\n{out}");
        }
    }

    #[test]
    fn the_tree_can_be_moved_to_the_right_hand_side() {
        let cfg = Config {
            tree_side: Side::Right,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        select(&mut app, "README.md");
        let rows = screen(&mut app, 90, 24);
        let header = &rows[0];
        let project_at = header.find("PROJECT").expect("tree pane is drawn");
        let file_at = header.find("README.md").expect("preview pane is drawn");
        assert!(
            project_at > file_at,
            "the tree should be on the right:\n{header}"
        );
    }

    #[test]
    fn borders_can_be_turned_off_for_a_quieter_screen() {
        let cfg = Config {
            borders: false,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        let out = joined(&mut app);
        assert!(!out.contains('┌'), "no box corners:\n{out}");
        assert!(!out.contains('│'), "no box sides:\n{out}");
        assert!(out.contains("PROJECT"), "the title still shows:\n{out}");
        assert!(out.contains("README.md"));
    }

    #[test]
    fn tree_markers_can_be_plain_ascii() {
        let cfg = Config {
            markers: Markers::Ascii,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        let out = joined(&mut app);
        assert!(!out.contains('▾'), "no geometric glyphs:\n{out}");
        assert!(out.contains("- "), "an open folder uses a dash:\n{out}");
    }

    #[test]
    fn the_status_line_can_move_to_the_top() {
        let cfg = Config {
            status_position: Position::Top,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        let rows = screen(&mut app, 90, 24);
        assert!(rows[0].contains("help"), "status is on row 0:\n{}", rows[0]);
        assert!(rows[1].contains("PROJECT"));
    }

    #[test]
    fn a_narrow_terminal_still_renders() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        for (w, h) in [(30u16, 10u16), (28, 6), (120, 40)] {
            let rows = screen(&mut app, w, h);
            assert_eq!(rows.len(), h as usize);
            assert!(rows.iter().all(|r| r.chars().count() == w as usize));
        }
    }

    // ---- the monochrome brief --------------------------------------------

    #[test]
    fn the_chrome_names_no_colours_at_all() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        let mut t = Terminal::new(TestBackend::new(90, 24)).unwrap();
        t.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();

        // The tree pane holds only chrome — no syntax highlighting to excuse
        // a colour. Everything in it should inherit the terminal's palette.
        for y in 0..buf.area.height {
            for x in 0..28 {
                let Some(cell) = buf.cell((x, y)) else {
                    continue;
                };
                let allowed = matches!(
                    cell.fg,
                    Color::Reset | Color::White | Color::DarkGray | Color::Gray | Color::Black
                );
                assert!(
                    allowed,
                    "chrome at {x},{y} uses {:?}; the design brief is white and black",
                    cell.fg
                );
            }
        }
    }

    #[test]
    fn no_emoji_or_pictorial_icons_are_drawn() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('e'));
        type_str(&mut app, "x");
        let out = joined(&mut app);
        for bad in ['●', '☐', '☑', '🖼', '📁', '📄'] {
            assert!(!out.contains(bad), "found {bad:?} in:\n{out}");
        }
        assert!(
            out.contains(" *"),
            "unsaved work is marked with an asterisk"
        );
    }

    // ---- plain text -------------------------------------------------------

    #[test]
    fn file_kinds_are_classified_by_extension_then_by_name() {
        let prose: Vec<String> = ["md", "txt", "rst"].iter().map(|s| s.to_string()).collect();
        let k = |p: &str| text_kind(Path::new(p), &prose);
        assert_eq!(k("a.md"), TextKind::Markdown);
        assert_eq!(k("a.MD"), TextKind::Markdown, "case does not matter");
        assert_eq!(k("a.txt"), TextKind::Prose);
        assert_eq!(k("a.rst"), TextKind::Prose);
        assert_eq!(k("a.py"), TextKind::Code);
        assert_eq!(k("a.csv"), TextKind::Code);
        // Extensionless files are code unless they are prose by convention.
        assert_eq!(k("LICENSE"), TextKind::Prose);
        assert_eq!(k("COPYING"), TextKind::Prose);
        assert_eq!(k("Makefile"), TextKind::Code);
        assert_eq!(k("script"), TextKind::Code);
    }

    #[test]
    fn a_text_file_opens_wrapped_and_readable_not_in_the_editor() {
        let (td, mut app) = fixture();
        let long = "a long line of prose that will certainly need to wrap because it keeps going";
        fs::write(td.path().join("notes.txt"), format!("{long}\n")).unwrap();
        app.on_key(ch('R'));
        select(&mut app, "notes.txt");

        let out = joined(&mut app);
        assert!(out.contains("READ"), "prose reads first:\n{out}");
        assert!(!out.contains(" 1 "), "no line numbers on prose:\n{out}");
        for line in screen(&mut app, 90, 24) {
            assert!(line.chars().count() == 90);
        }
        // Wrapped, not clipped: the last word of the line still made it.
        assert!(
            out.contains("keeps going"),
            "the tail of the line was lost:\n{out}"
        );
    }

    #[test]
    fn e_switches_plain_text_into_the_editor_and_back_out() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("notes.txt"), "first line\n").unwrap();
        app.on_key(ch('R'));
        select(&mut app, "notes.txt");
        app.on_key(k(KeyCode::Enter));
        assert!(app.read_mode);

        app.on_key(ch('e'));
        assert!(!app.read_mode);
        type_str(&mut app, "X");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('X'));
        app.on_key(ctrl('s'));
        assert_eq!(
            fs::read_to_string(td.path().join("notes.txt")).unwrap(),
            "Xfirst line\n"
        );
    }

    #[test]
    fn a_code_file_still_opens_straight_into_the_editor() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        assert!(!app.read_mode, "code is for typing into");
        assert!(
            joined(&mut app).contains(" 1 "),
            "and keeps its line numbers"
        );
    }

    #[test]
    fn prose_extensions_are_configurable() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("a.csv"), "x,y\n").unwrap();
        app.on_key(ch('R'));
        select(&mut app, "a.csv");
        assert!(joined(&mut app).contains("VIEW"), "csv is code by default");

        command(&mut app, "set prose_extensions md txt csv");
        app.on_key(ch('R'));
        select(&mut app, "a.csv");
        assert!(joined(&mut app).contains("READ"), "now it reads as prose");
    }

    // ---- preview dispatch -------------------------------------------------

    #[test]
    fn hovering_a_markdown_file_shows_it_rendered_not_raw() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        let out = joined(&mut app);
        assert!(out.contains("Design Notes"), "{out}");
        assert!(
            !out.contains("# Design Notes"),
            "the '#' is consumed:\n{out}"
        );
        assert!(out.contains("• one"), "list is rendered as bullets:\n{out}");
        assert!(out.contains("READ"));
    }

    #[test]
    fn hovering_a_code_file_shows_source_with_line_numbers() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        let out = joined(&mut app);
        assert!(out.contains("import utils"), "{out}");
        assert!(out.contains(" 1 "), "line numbers are shown:\n{out}");
    }

    #[test]
    fn hovering_a_picture_tries_to_draw_it() {
        let (_td, mut app) = fixture();
        select(&mut app, "logo.png");
        assert!(matches!(app.preview, Preview::Media { .. }));
        // The fixture's png is deliberately corrupt, so the pane should say
        // so by name rather than drawing nothing at all.
        let out = joined(&mut app);
        assert!(out.contains("logo.png"), "{out}");
    }

    #[test]
    fn a_real_picture_is_drawn_as_half_blocks() {
        let (td, mut app) = fixture();
        let mut img = image::RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 8) as u8, (y * 8) as u8, 200, 255]);
        }
        img.save(td.path().join("real.png")).unwrap();
        app.on_key(ch('R'));
        select(&mut app, "real.png");
        let out = joined(&mut app);
        assert!(out.contains('▀'), "expected half-blocks:\n{out}");
        assert!(out.contains("32x32"), "and a caption:\n{out}");
    }

    #[test]
    fn media_preview_can_be_turned_off() {
        let cfg = Config {
            media_preview: false,
            ..Config::default()
        };
        let (_td, mut app) = fixture_with(cfg);
        select(&mut app, "logo.png");
        assert!(matches!(app.preview, Preview::Binary { .. }));
    }

    #[test]
    fn hovering_a_folder_summarises_it() {
        let (_td, mut app) = fixture();
        select(&mut app, "notes");
        assert!(joined(&mut app).contains("1 entry"));
    }

    // ---- navigation & editing --------------------------------------------

    #[test]
    fn arrow_keys_move_the_cursor_and_open_folders() {
        let (_td, mut app) = fixture();
        assert_eq!(app.selected, 0, "starts on the project root");
        app.on_key(k(KeyCode::Down));
        assert_eq!(app.selected_row().unwrap().name, "notes");
        app.on_key(k(KeyCode::Right));
        assert!(app.selected_row().unwrap().expanded);
        assert_eq!(app.rows[2].name, "design.md");
        app.on_key(k(KeyCode::Left));
        assert!(!app.selected_row().unwrap().expanded);
    }

    #[test]
    fn the_cursor_cannot_run_off_either_end() {
        let (_td, mut app) = fixture();
        for _ in 0..50 {
            app.on_key(k(KeyCode::Up));
        }
        assert_eq!(app.selected, 0);
        for _ in 0..50 {
            app.on_key(k(KeyCode::Down));
        }
        assert_eq!(app.selected, app.rows.len() - 1);
    }

    #[test]
    fn enter_on_a_code_file_focuses_the_editor_and_typing_reaches_the_buffer() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        assert_eq!(app.focus, Focus::Editor);
        assert!(!app.read_mode, "code opens straight into editing");
        type_str(&mut app, "# hi");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("# hi"));
        assert!(app.active_buffer().unwrap().dirty);
    }

    #[test]
    fn q_types_a_letter_in_the_editor_instead_of_quitting() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('q'));
        assert!(!app.should_quit, "q must not quit while editing");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('q'));
    }

    #[test]
    fn slash_does_not_open_search_while_typing_code() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch('/'));
        assert!(
            matches!(app.mode, Mode::Normal),
            "it is a comment, not a key"
        );
        assert!(app.active_buffer().unwrap().lines()[0].starts_with('/'));
    }

    #[test]
    fn ctrl_s_writes_the_file_to_disk() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "x");
        app.on_key(ctrl('s'));
        assert!(!app.active_buffer().unwrap().dirty);
        let on_disk = fs::read_to_string(td.path().join("src/main.py")).unwrap();
        assert!(on_disk.starts_with("ximport utils"), "{on_disk:?}");
    }

    #[test]
    fn unsaved_edits_survive_navigating_away_and_back() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "MARKER");
        app.on_key(k(KeyCode::Esc));
        select(&mut app, "README.md");
        select(&mut app, "main.py");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("MARKER"));
    }

    #[test]
    fn markdown_opens_rendered_and_e_switches_to_raw_source() {
        let (_td, mut app) = fixture();
        select(&mut app, "design.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.read_mode);
        app.on_key(ch('e'));
        assert!(!app.read_mode);
        let out = joined(&mut app);
        assert!(out.contains("# Design Notes"), "raw source shows:\n{out}");
        assert!(out.contains("EDIT"));
    }

    // ---- opening a single file -------------------------------------------

    #[test]
    fn naming_a_file_opens_its_folder_with_the_editor_already_on_it() {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let file = td.path().join("src/main.py");
        let mut app = App::new(
            target(td.path(), Some(file.clone())),
            Config::default(),
            None,
        )
        .unwrap();

        assert_eq!(app.selected_row().unwrap().path, file);
        assert_eq!(app.focus, Focus::Editor, "it is ready to type into");
        assert!(!app.read_mode);
        let out = joined(&mut app);
        assert!(out.contains("main.py"), "{out}");
        assert!(out.contains("import utils"), "{out}");
        assert!(
            out.contains("README.md"),
            "the folder is still listed:\n{out}"
        );
    }

    #[test]
    fn naming_a_markdown_file_opens_it_rendered() {
        let td = tempfile::tempdir().unwrap();
        build(td.path());
        let file = td.path().join("notes/design.md");
        let mut app = App::new(target(td.path(), Some(file)), Config::default(), None).unwrap();
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.read_mode, "a note opens to be read, not edited");
        assert!(joined(&mut app).contains("Design Notes"));
    }

    // ---- search -----------------------------------------------------------

    #[test]
    fn slash_opens_the_search_bar_and_typing_finds_matches() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        assert!(matches!(app.mode, Mode::Bar(_)));
        type_str(&mut app, "widget");

        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert!(!b.results.is_empty(), "found nothing");
        assert!(b.results.iter().any(|h| h.text.contains("hello widget")));

        let out = joined(&mut app);
        assert!(out.contains("MATCH"), "{out}");
        assert!(out.contains("hello widget"), "{out}");
    }

    #[test]
    fn the_search_bar_shows_the_query_as_it_is_typed() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "design");
        let rows = screen(&mut app, 90, 24);
        assert!(
            rows[0].contains("design"),
            "the bar is on top:\n{}",
            rows[0]
        );
        assert!(rows[0].trim_start().starts_with('/'));
    }

    #[test]
    fn enter_jumps_to_the_selected_match_and_puts_the_cursor_on_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "utils.load");
        app.on_key(k(KeyCode::Enter));

        assert!(matches!(app.mode, Mode::Normal), "the bar closes");
        assert_eq!(app.selected_row().unwrap().name, "main.py");
        assert_eq!(app.focus, Focus::Editor);
        let ed = app.active_buffer().unwrap();
        assert_eq!(ed.cursor_line, 4, "landed on the matching line");
        assert_eq!(ed.cursor_col, 11, "and on the matching column");
    }

    #[test]
    fn moving_through_results_previews_each_one() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "e");

        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        let paths: Vec<PathBuf> = b.results.iter().map(|h| h.path.clone()).collect();
        assert_eq!(
            app.selected_path().unwrap(),
            paths[0],
            "starts on the first"
        );

        // Walk down to the first result in a different file.
        let other = paths
            .iter()
            .position(|p| *p != paths[0])
            .expect("fixture should match in more than one file");
        for _ in 0..other {
            app.on_key(k(KeyCode::Down));
        }
        assert!(matches!(app.mode, Mode::Bar(_)), "still searching");
        assert_eq!(
            app.selected_path().unwrap(),
            paths[other],
            "the preview follows the highlighted result"
        );
    }

    #[test]
    fn a_search_with_no_matches_says_so() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "zzzznothing");
        assert!(joined(&mut app).contains("no matches"));
    }

    #[test]
    fn control_chords_do_not_type_letters_into_the_search_bar() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "wid");
        app.on_key(ctrl('x'));
        let Mode::Bar(b) = &app.mode else {
            panic!("bar closed")
        };
        assert_eq!(b.input, "wid", "Ctrl+X must not append an x");
    }

    #[test]
    fn ctrl_q_quits_from_inside_the_search_bar() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        app.on_key(ctrl('q'));
        assert!(app.should_quit, "Ctrl+Q means quit everywhere");
    }

    #[test]
    fn control_chords_do_not_type_letters_into_a_prompt() {
        let (_td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "note");
        app.on_key(ctrl('s'));
        let Mode::Prompt(p) = &app.mode else {
            panic!("prompt closed")
        };
        assert_eq!(p.input, "note", "Ctrl+S must not append an s");
    }

    #[test]
    fn ctrl_s_while_editing_a_setting_does_not_type_an_s_into_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Enter));
        app.on_key(ctrl('s'));
        let Mode::Settings(s) = &app.mode else {
            panic!("settings closed")
        };
        assert_eq!(
            s.editing.as_deref(),
            Some("4"),
            "the field still holds tab_width's value, not \"4s\""
        );
    }

    #[test]
    fn escape_closes_the_search_bar() {
        let (_td, mut app) = fixture();
        app.on_key(ch('/'));
        type_str(&mut app, "widget");
        app.on_key(k(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(joined(&mut app).contains("PROJECT"), "the tree is back");
    }

    // ---- commands ---------------------------------------------------------

    #[test]
    fn set_changes_a_setting_and_it_takes_effect() {
        let (_td, mut app) = fixture();
        assert_eq!(app.config.tab_width, 4);
        command(&mut app, "set tab_width 2");
        assert_eq!(app.config.tab_width, 2);

        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(k(KeyCode::Tab));
        assert!(
            app.active_buffer().unwrap().lines()[0].starts_with("  import"),
            "the new tab width is what the editor actually inserts"
        );
    }

    #[test]
    fn set_with_no_value_reports_the_current_one() {
        let (_td, mut app) = fixture();
        command(&mut app, "set tree_side");
        assert_eq!(app.status, "tree_side = left");
    }

    #[test]
    fn set_can_repaint_the_theme_without_a_restart() {
        let (_td, mut app) = fixture();
        command(&mut app, "set theme.heading cyan bold");
        assert_eq!(app.config.theme.heading, "cyan bold");
        assert_eq!(app.palette.heading.fg, Some(Color::Cyan));
    }

    #[test]
    fn set_rejects_nonsense_without_changing_anything() {
        let (_td, mut app) = fixture();
        command(&mut app, "set tab_width banana");
        assert_eq!(app.config.tab_width, 4);
        assert!(app.status.contains("number"), "{}", app.status);

        command(&mut app, "set no_such_setting 1");
        assert!(app.status.contains("unknown setting"), "{}", app.status);
    }

    #[test]
    fn an_unknown_command_says_so() {
        let (_td, mut app) = fixture();
        command(&mut app, "frobnicate");
        assert!(app.status.contains("unknown command"), "{}", app.status);
    }

    #[test]
    fn replace_asks_first_then_rewrites_the_whole_project() {
        let (td, mut app) = fixture();
        fs::write(td.path().join("notes/more.md"), "widget widget\n").unwrap();
        app.on_key(ch('R'));

        command(&mut app, "replace widget gadget");
        assert!(matches!(app.mode, Mode::Confirm(_)), "it asks first");
        let out = joined(&mut app);
        assert!(
            out.contains("3 occurrences"),
            "counts them up front:\n{out}"
        );
        assert!(out.contains("2 files"), "{out}");

        app.on_key(ch('n'));
        assert!(
            fs::read_to_string(td.path().join("README.md"))
                .unwrap()
                .contains("widget"),
            "answering no changes nothing"
        );

        command(&mut app, "replace widget gadget");
        app.on_key(ch('y'));
        assert!(
            fs::read_to_string(td.path().join("README.md"))
                .unwrap()
                .contains("gadget"),
            "answering yes rewrites it"
        );
        assert_eq!(
            fs::read_to_string(td.path().join("notes/more.md")).unwrap(),
            "gadget gadget\n"
        );
        assert!(app.status.contains("replaced 3"), "{}", app.status);
    }

    #[test]
    fn replace_needs_both_halves_and_reports_a_miss() {
        let (_td, mut app) = fixture();
        command(&mut app, "replace onlyone");
        assert!(app.status.contains(":replace"), "{}", app.status);
        command(&mut app, "replace absent-word x");
        assert!(app.status.contains("not in this project"), "{}", app.status);
    }

    #[test]
    fn replace_takes_quoted_strings_with_spaces_in_them() {
        let (td, mut app) = fixture();
        command(&mut app, "replace \"hello widget\" \"goodbye gadget\"");
        app.on_key(ch('y'));
        assert!(
            fs::read_to_string(td.path().join("README.md"))
                .unwrap()
                .contains("goodbye gadget"),
            "quoted arguments keep their spaces"
        );
    }

    #[test]
    fn tab_completes_commands_and_setting_names() {
        let (_td, mut app) = fixture();
        app.on_key(ch(':'));
        type_str(&mut app, "repl");
        app.on_key(k(KeyCode::Tab));
        let Mode::Bar(b) = &app.mode else { panic!() };
        assert_eq!(b.input, "replace");

        app.on_key(k(KeyCode::Esc));
        app.on_key(ch(':'));
        type_str(&mut app, "set tree_w");
        app.on_key(k(KeyCode::Tab));
        let Mode::Bar(b) = &app.mode else { panic!() };
        assert_eq!(b.input, "set tree_width");
    }

    #[test]
    fn ctrl_p_reaches_the_command_bar_from_inside_the_editor() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "z");
        // A bare `:` here would be a character, so the editor needs its own way in.
        app.on_key(ctrl('p'));
        assert!(matches!(app.mode, Mode::Bar(_)));
        type_str(&mut app, "w");
        app.on_key(k(KeyCode::Enter));
        assert!(
            fs::read_to_string(td.path().join("src/main.py"))
                .unwrap()
                .starts_with('z')
        );
    }

    #[test]
    fn a_colon_in_the_editor_is_a_character_not_a_command() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        app.on_key(ch(':'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.active_buffer().unwrap().lines()[0].starts_with(':'));
    }

    #[test]
    fn commands_cover_saving_and_quitting() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "z");
        app.on_key(k(KeyCode::Esc));
        command(&mut app, "w");
        assert!(
            fs::read_to_string(td.path().join("src/main.py"))
                .unwrap()
                .starts_with('z')
        );

        command(&mut app, "q");
        assert!(app.should_quit);
    }

    // ---- settings area ----------------------------------------------------

    #[test]
    fn the_settings_area_lists_every_setting_with_its_value() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        assert!(matches!(app.mode, Mode::Settings(_)));
        let out = screen(&mut app, 100, 34).join("\n");
        assert!(out.contains("Settings"), "{out}");
        assert!(out.contains("tab_width"), "{out}");
        assert!(out.contains("tree_side"), "{out}");
        assert!(out.contains("left"), "values are shown:\n{out}");
    }

    #[test]
    fn a_setting_can_be_changed_from_the_settings_area() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        // Walk to tab_width, third in the index.
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Enter));

        // The field is prefilled with the current value.
        app.on_key(k(KeyCode::Backspace));
        type_str(&mut app, "7");
        app.on_key(k(KeyCode::Enter));
        assert_eq!(app.config.tab_width, 7, "status was: {}", app.status);
    }

    #[test]
    fn escape_while_editing_a_setting_leaves_it_alone() {
        let (_td, mut app) = fixture();
        app.on_key(ch(','));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Down));
        app.on_key(k(KeyCode::Enter));
        app.on_key(k(KeyCode::Backspace));
        type_str(&mut app, "9");
        app.on_key(k(KeyCode::Esc));
        assert_eq!(app.config.tab_width, 4);
        assert!(matches!(app.mode, Mode::Settings(_)), "still in settings");
    }

    #[test]
    fn the_settings_area_can_be_opened_by_command_too() {
        let (_td, mut app) = fixture();
        command(&mut app, "config");
        assert!(matches!(app.mode, Mode::Settings(_)));
    }

    // ---- file operations --------------------------------------------------

    #[test]
    fn n_creates_a_file_in_the_selected_folder_and_selects_it() {
        let (td, mut app) = fixture();
        select(&mut app, "notes");
        app.on_key(ch('n'));
        type_str(&mut app, "today.md");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("notes/today.md").is_file());
        assert_eq!(app.selected_row().unwrap().name, "today.md");
    }

    #[test]
    fn a_nested_name_creates_the_folders_along_the_way() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "journal/2026/aug.md");
        app.on_key(k(KeyCode::Enter));
        assert!(td.path().join("journal/2026/aug.md").is_file());
        assert_eq!(app.selected_row().unwrap().name, "aug.md");
    }

    #[test]
    fn creating_over_an_existing_name_is_refused() {
        let (_td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "README.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.status.contains("already exists"), "{}", app.status);
    }

    #[test]
    fn rename_moves_the_file_and_carries_its_open_buffer() {
        let (td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "EDITED");
        app.on_key(k(KeyCode::Esc));

        app.on_key(ch('r'));
        for _ in 0.."main.py".len() {
            app.on_key(k(KeyCode::Backspace));
        }
        type_str(&mut app, "app.py");
        app.on_key(k(KeyCode::Enter));

        assert!(td.path().join("src/app.py").is_file());
        assert!(!td.path().join("src/main.py").exists());
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("EDITED"));
    }

    #[test]
    fn delete_asks_before_removing_and_n_backs_out() {
        let (td, mut app) = fixture();
        select(&mut app, "README.md");
        app.on_key(ch('d'));
        assert!(joined(&mut app).contains("Delete README.md?"));
        app.on_key(ch('n'));
        assert!(td.path().join("README.md").exists());
        app.on_key(ch('d'));
        app.on_key(ch('y'));
        assert!(!td.path().join("README.md").exists());
    }

    #[test]
    fn the_project_root_cannot_be_deleted_or_renamed() {
        let (_td, mut app) = fixture();
        app.selected = 0;
        app.on_key(ch('d'));
        assert!(app.status.contains("cannot delete"));
        app.on_key(ch('r'));
        assert!(app.status.contains("cannot rename"));
    }

    #[test]
    fn a_typed_path_cannot_escape_the_project() {
        let (td, mut app) = fixture();
        app.on_key(ch('n'));
        type_str(&mut app, "../escaped.md");
        app.on_key(k(KeyCode::Enter));
        assert!(app.status.contains("escapes"), "{}", app.status);
        assert!(!td.path().parent().unwrap().join("escaped.md").exists());
    }

    // ---- quitting & misc --------------------------------------------------

    #[test]
    fn quitting_with_unsaved_work_asks_first() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "x");
        app.on_key(k(KeyCode::Esc));

        app.on_key(ch('q'));
        assert!(!app.should_quit);
        assert!(joined(&mut app).contains("Discard unsaved changes"));
        app.on_key(ch('n'));
        assert!(!app.should_quit);
        app.on_key(ch('q'));
        app.on_key(ch('y'));
        assert!(app.should_quit);
    }

    #[test]
    fn help_opens_and_any_key_closes_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        let out = joined(&mut app);
        assert!(out.contains("Keys"), "{out}");
        assert!(out.contains("search names and contents"), "{out}");
        assert!(out.contains("draw the project as a graph"), "{out}");
        app.on_key(ch('x'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn help_scrolls_on_a_terminal_too_short_for_it() {
        let (_td, mut app) = fixture();
        app.on_key(ch('?'));
        let top = screen(&mut app, 80, 16).join("\n");
        assert!(top.contains("TREE"), "starts at the top:\n{top}");

        for _ in 0..12 {
            app.on_key(k(KeyCode::Down));
        }
        assert!(
            matches!(app.mode, Mode::Help(_)),
            "arrows scroll, not close"
        );
        let lower = screen(&mut app, 80, 16).join("\n");
        assert_ne!(top, lower, "the view moved");

        // The point of scrolling: the end of the list is reachable at all.
        app.on_key(k(KeyCode::End));
        let bottom = screen(&mut app, 80, 16).join("\n");
        assert!(
            bottom.contains("quit"),
            "the last entry is reachable:\n{bottom}"
        );
        assert!(
            !bottom.contains("move the cursor"),
            "and the top has scrolled off"
        );

        // Anything that is not a scroll key still puts it away.
        app.on_key(ch('z'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn dot_toggles_hidden_files() {
        let (td, mut app) = fixture();
        fs::write(td.path().join(".secret"), "x").unwrap();
        app.on_key(ch('R'));
        assert!(!app.rows.iter().any(|r| r.name == ".secret"));
        app.on_key(ch('.'));
        assert!(app.rows.iter().any(|r| r.name == ".secret"));
    }

    #[test]
    fn refresh_keeps_buffers_that_have_unsaved_work() {
        let (_td, mut app) = fixture();
        select(&mut app, "main.py");
        app.on_key(k(KeyCode::Enter));
        type_str(&mut app, "KEEP");
        app.on_key(k(KeyCode::Esc));
        app.on_key(ch('R'));
        select(&mut app, "main.py");
        assert!(app.active_buffer().unwrap().lines()[0].starts_with("KEEP"));
    }

    #[test]
    fn split_args_keeps_quoted_runs_together() {
        assert_eq!(split_args("set tab_width 2"), ["set", "tab_width", "2"]);
        assert_eq!(
            split_args(r#"replace "old thing" "new thing""#),
            ["replace", "old thing", "new thing"]
        );
        assert_eq!(split_args("  spaced   out  "), ["spaced", "out"]);
        assert!(split_args("").is_empty());
        // An empty quoted string is a real argument, not nothing.
        assert_eq!(split_args(r#"replace "x" """#), ["replace", "x", ""]);
    }

    #[test]
    fn safe_join_rejects_paths_outside_the_root() {
        let root = Path::new("/project");
        assert!(safe_join(root, "notes/ok.md", root).is_ok());
        assert!(safe_join(root, "../outside.md", root).is_err());
        assert!(safe_join(root, "a/../../outside.md", root).is_err());
        assert!(safe_join(root, "/etc/passwd", root).is_err());
        assert_eq!(
            safe_join(root, "a/../b.md", root).unwrap(),
            PathBuf::from("/project/b.md")
        );
    }

    /// Not an assertion — a way to look at the panes.
    /// `TINY_SHOT=/path cargo test screenshot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn screenshot() {
        let dir = std::env::var("TINY_SHOT").expect("set TINY_SHOT");
        let file = std::env::var("TINY_SHOT_FILE").ok();
        let mut app = App::new(target(Path::new(&dir), None), Config::default(), None).unwrap();
        if let Some(f) = file {
            select(&mut app, &f);
        }
        if let Ok(keys) = std::env::var("TINY_SHOT_KEYS") {
            for c in keys.chars() {
                match c {
                    '\n' => app.on_key(k(KeyCode::Enter)),
                    '↓' => app.on_key(k(KeyCode::Down)),
                    _ => app.on_key(ch(c)),
                }
            }
        }
        for row in screen(&mut app, 100, 30) {
            println!("{row}");
        }
    }
}
