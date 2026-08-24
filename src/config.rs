//! Configuration.
//!
//! Two places, both optional:
//!   - `~/.config/tiny/tiny.conf`     user defaults
//!   - `<project>/.tiny/tiny.conf`    per-project overrides
//!
//! Every field has a default, so a partial or malformed file never stops the
//! program starting — it falls back and says so in the status bar.
//!
//! Colours are written as style specs rather than raw colours, so a line can
//! carry weight as well as hue: `"bold"`, `"underline"`, `"white on black"`,
//! `"#7dcfff bold"`. The shipped defaults are deliberately monochrome and use
//! the terminal's own palette, so tiny looks like the terminal it runs in.
//!
//! # Layering
//!
//! The project file **replaces** the user file rather than merging into it.
//! Fields a project omits fall back to the library defaults, not to the user's
//! settings. That is a real trade: it means a project config has to restate
//! anything it wants kept, but it also means opening someone else's project
//! gives you exactly what they specified, with no half-understood blend of two
//! files to reason about. See [`Config::load_from`].
//!
//! # Adding a setting
//!
//! Four places have to agree, and there is no macro tying them together, so
//! all four need editing by hand:
//!
//! 1. A field on [`Config`], with a doc comment.
//! 2. A default in `impl Default for Config`.
//! 3. A row in [`Config::settings_index`] — this drives both `:set` completion
//!    and the in-program settings area.
//! 4. Arms in [`Config::get`] and [`Config::set`].
//!
//! Miss (3) and the setting works from a config file but is invisible in the
//! UI; miss (4) and `:set` reports it as unknown. The
//! `every_setting_round_trips` test in this file catches most of this.
//!
//! # Nothing here fails hard
//!
//! A malformed file falls back to defaults and returns a warning string for
//! the status bar; an unknown word in a style spec is ignored; out-of-range
//! numbers are clamped by [`Config::sanitized`]. A typo in a config file
//! should cost you an underline, never the program.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

/// Directory name marking a folder as a tiny project.
pub const PROJECT_DIR: &str = ".tiny";
/// Config file name, used identically at both layers: `~/.config/tiny/tiny.conf`
/// and `<project>/.tiny/tiny.conf`.
pub const CONF_NAME: &str = "tiny.conf";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Which side of the window the tree pane occupies.
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Vertical placement for the bar and the status line, independently settable.
#[serde(rename_all = "lowercase")]
pub enum Position {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Glyph set for the tree's expand/collapse indicators.
#[serde(rename_all = "lowercase")]
pub enum Markers {
    /// `▾ ▸` — geometric shapes, present in every terminal font.
    Arrows,
    /// `- +` — for terminals with no Unicode at all.
    Ascii,
}

/// Every setting, in one flat struct.
///
/// `#[serde(default)]` is what makes a partial file legal: any key the user
/// left out is filled from [`Config::default`], so a two-line `tiny.conf` is
/// as valid as a complete one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Fallback when the working directory cannot be read.
    pub default_root: PathBuf,
    /// Initialise a project in any directory opened without one.
    pub auto_init: bool,
    pub show_hidden: bool,
    pub tab_width: usize,

    /// Which side the project tree sits on.
    pub tree_side: Side,
    /// Share of the window given to the tree, 0.1 - 0.6.
    pub tree_width: f32,
    /// Where the search bar appears when open.
    pub search_position: Position,
    /// Where the status line sits.
    pub status_position: Position,

    pub line_numbers: bool,
    pub markers: Markers,
    /// Draw boxes around the panes. Off gives a plainer, quieter screen.
    pub borders: bool,

    /// syntect theme used for code. Chrome stays monochrome regardless.
    pub syntax_theme: String,
    /// Cap on hits collected by one search.
    pub max_search_results: usize,
    /// Directories never walked by search, by exact name.
    pub search_ignore: Vec<String>,

    /// Extensions treated as prose: wrapped, read first, edited on demand.
    /// Everything else that is text opens straight in the editor.
    pub prose_extensions: Vec<String>,

    /// A symbol defined in more files than this is too ambiguous to draw a
    /// call edge for. Names like `new` and `main` are everywhere.
    pub graph_max_ambiguity: usize,

    /// Preview images and video poster frames as coloured half-blocks.
    pub media_preview: bool,
    /// Rows of terminal cells a media preview may use.
    pub media_height: usize,

    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_root: home(),
            auto_init: true,
            show_hidden: false,
            tab_width: 4,
            tree_side: Side::Left,
            tree_width: 0.30,
            search_position: Position::Top,
            status_position: Position::Bottom,
            line_numbers: true,
            markers: Markers::Arrows,
            borders: true,
            syntax_theme: "base16-ocean.dark".into(),
            max_search_results: 500,
            search_ignore: [".git", "target", "node_modules", ".venv", "__pycache__"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            prose_extensions: [
                "md", "markdown", "mdown", "mkd", "txt", "text", "rst", "org", "adoc", "asciidoc",
                "log",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            graph_max_ambiguity: 3,
            media_preview: true,
            media_height: 24,
            theme: Theme::default(),
        }
    }
}

impl Config {
    /// `~/.config/tiny/tiny.conf`.
    pub fn user_path() -> Option<PathBuf> {
        config_dir().map(|d| d.join(CONF_NAME))
    }

    /// `<project>/.tiny/tiny.conf`.
    pub fn project_path(root: &Path) -> PathBuf {
        root.join(PROJECT_DIR).join(CONF_NAME)
    }

    /// Load user config, then let the project's own file override it.
    /// Returns the config plus anything worth telling the user about.
    ///
    /// Called twice during startup — once with `None` before the project is
    /// known, then again with the resolved root. See `main::real_main`.
    pub fn load(project_root: Option<&Path>) -> (Self, Option<String>) {
        Self::load_from(Self::user_path().as_deref(), project_root)
    }

    /// The body of `load`, with both paths passed in. Tests use this so they
    /// never depend on — or touch — whatever config the machine really has.
    pub fn load_from(
        user_conf: Option<&Path>,
        project_root: Option<&Path>,
    ) -> (Self, Option<String>) {
        let mut warning = None;
        let mut cfg = match user_conf {
            Some(p) if p.exists() => match Self::read(p) {
                Ok(c) => c,
                Err(e) => {
                    warning = Some(format!("{CONF_NAME}: {e} (using defaults)"));
                    Self::default()
                }
            },
            _ => Self::default(),
        };
        if let Some(root) = project_root {
            let p = Self::project_path(root);
            if p.exists() {
                match Self::read(&p) {
                    // A project file is a full config; fields it omits fall
                    // back to library defaults, not to the user file. Keeping
                    // it simple beats a half-understood merge.
                    Ok(c) => cfg = c,
                    Err(e) => warning = Some(format!("project {CONF_NAME}: {e}")),
                }
            }
        }
        (cfg.sanitized(), warning)
    }

    fn read(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str::<Config>(&text)?)
    }

    /// Write to the user config path, creating the directory if needed.
    ///
    /// Writes the *whole* config, including everything left at its default, so
    /// the generated file doubles as documentation of what can be set. Bound
    /// to `Ctrl+S` in the settings area, and run once on first launch.
    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::user_path().ok_or_else(|| anyhow!("no config directory"))?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Clamp every numeric field into a range that cannot break the layout.
    ///
    /// Run after loading a file and after every `:set`, so there is no path by
    /// which a hand-edited `tree_width = 40` produces a tree wider than the
    /// window or a `tab_width = 0` produces an infinite loop.
    fn sanitized(mut self) -> Self {
        self.tree_width = self.tree_width.clamp(0.10, 0.60);
        self.tab_width = self.tab_width.clamp(1, 16);
        self.media_height = self.media_height.clamp(4, 200);
        self.graph_max_ambiguity = self.graph_max_ambiguity.clamp(1, 100);
        self.max_search_results = self.max_search_results.clamp(1, 100_000);
        self
    }

    /// Every key `:set` accepts, with a one-line description. Also what the
    /// in-program settings area lists, so the two can never drift apart.
    pub fn settings_index() -> &'static [(&'static str, &'static str)] {
        &[
            ("auto_init", "initialise any folder opened"),
            ("show_hidden", "list dotfiles in the tree"),
            ("tab_width", "spaces inserted by Tab"),
            ("tree_side", "tree side: left, right"),
            ("tree_width", "share of width for the tree"),
            ("search_position", "search bar: top, bottom"),
            ("status_position", "status line: top, bottom"),
            ("line_numbers", "line numbers while editing"),
            ("markers", "tree glyphs: arrows, ascii"),
            ("borders", "draw boxes around the panes"),
            ("syntax_theme", "syntect theme used for code"),
            ("max_search_results", "cap on hits from one search"),
            ("prose_extensions", "wrapped, read-first file types"),
            ("search_ignore", "folders search never walks"),
            (
                "graph_max_ambiguity",
                "max definitions before a name is ignored",
            ),
            ("media_preview", "draw images and video frames"),
            ("media_height", "rows a media preview may use"),
            ("theme.text", "body text style"),
            ("theme.dim", "secondary text style"),
            ("theme.border", "pane border style"),
            ("theme.border_focus", "focused pane border style"),
            ("theme.selection", "selected row style"),
            ("theme.directory", "folder name style"),
            ("theme.heading", "markdown heading style"),
            ("theme.link", "link and wikilink style"),
            ("theme.code", "inline code style"),
            ("theme.marker", "unsaved marker and warnings"),
        ]
    }

    /// Current value of a key, as it would be written to the file.
    ///
    /// Round-trips with [`Config::set`]: whatever comes out of `get` is
    /// accepted by `set` and produces the same value again. The settings area
    /// depends on that, since it seeds its edit field from `get` and feeds the
    /// result back to `set`.
    ///
    /// `None` means the key does not exist, which the caller reports as an
    /// unknown setting.
    pub fn get(&self, key: &str) -> Option<String> {
        Some(match key {
            "auto_init" => self.auto_init.to_string(),
            "show_hidden" => self.show_hidden.to_string(),
            "tab_width" => self.tab_width.to_string(),
            "tree_side" => side_name(self.tree_side).into(),
            "tree_width" => format!("{:.2}", self.tree_width),
            "search_position" => pos_name(self.search_position).into(),
            "status_position" => pos_name(self.status_position).into(),
            "line_numbers" => self.line_numbers.to_string(),
            "markers" => match self.markers {
                Markers::Arrows => "arrows".into(),
                Markers::Ascii => "ascii".into(),
            },
            "borders" => self.borders.to_string(),
            "syntax_theme" => self.syntax_theme.clone(),
            "max_search_results" => self.max_search_results.to_string(),
            "prose_extensions" => self.prose_extensions.join(" "),
            "search_ignore" => self.search_ignore.join(" "),
            "graph_max_ambiguity" => self.graph_max_ambiguity.to_string(),
            "media_preview" => self.media_preview.to_string(),
            "media_height" => self.media_height.to_string(),
            "theme.text" => self.theme.text.clone(),
            "theme.dim" => self.theme.dim.clone(),
            "theme.border" => self.theme.border.clone(),
            "theme.border_focus" => self.theme.border_focus.clone(),
            "theme.selection" => self.theme.selection.clone(),
            "theme.directory" => self.theme.directory.clone(),
            "theme.heading" => self.theme.heading.clone(),
            "theme.link" => self.theme.link.clone(),
            "theme.code" => self.theme.code.clone(),
            "theme.marker" => self.theme.marker.clone(),
            _ => return None,
        })
    }

    /// Apply one `:set key value`. Rejects unknown keys and unparseable
    /// values rather than silently doing nothing.
    ///
    /// Note that theme entries are stored as raw strings without validation —
    /// [`parse_style`] ignores words it does not recognise, so a misspelled
    /// colour is accepted here and simply has no effect when drawn. Callers
    /// must follow a successful `set` with `App::apply_config` to rebuild the
    /// palette and highlighter; the config alone is just data.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let v = value.trim();
        match key {
            "auto_init" => self.auto_init = parse_bool(v)?,
            "show_hidden" => self.show_hidden = parse_bool(v)?,
            "tab_width" => self.tab_width = parse_num(v)?,
            "tree_side" => {
                self.tree_side = match v.to_ascii_lowercase().as_str() {
                    "left" => Side::Left,
                    "right" => Side::Right,
                    _ => return Err(anyhow!("tree_side must be left or right")),
                }
            }
            "tree_width" => {
                self.tree_width = v
                    .parse::<f32>()
                    .map_err(|_| anyhow!("tree_width must be a number like 0.3"))?
            }
            "search_position" => self.search_position = parse_pos(v)?,
            "status_position" => self.status_position = parse_pos(v)?,
            "line_numbers" => self.line_numbers = parse_bool(v)?,
            "markers" => {
                self.markers = match v.to_ascii_lowercase().as_str() {
                    "arrows" => Markers::Arrows,
                    "ascii" => Markers::Ascii,
                    _ => return Err(anyhow!("markers must be arrows or ascii")),
                }
            }
            "borders" => self.borders = parse_bool(v)?,
            "syntax_theme" => self.syntax_theme = v.to_string(),
            "max_search_results" => self.max_search_results = parse_num(v)?,
            "prose_extensions" => self.prose_extensions = parse_list(v),
            "search_ignore" => self.search_ignore = parse_list(v),
            "graph_max_ambiguity" => self.graph_max_ambiguity = parse_num(v)?,
            "media_preview" => self.media_preview = parse_bool(v)?,
            "media_height" => self.media_height = parse_num(v)?,
            "theme.text" => self.theme.text = v.to_string(),
            "theme.dim" => self.theme.dim = v.to_string(),
            "theme.border" => self.theme.border = v.to_string(),
            "theme.border_focus" => self.theme.border_focus = v.to_string(),
            "theme.selection" => self.theme.selection = v.to_string(),
            "theme.directory" => self.theme.directory = v.to_string(),
            "theme.heading" => self.theme.heading = v.to_string(),
            "theme.link" => self.theme.link = v.to_string(),
            "theme.code" => self.theme.code = v.to_string(),
            "theme.marker" => self.theme.marker = v.to_string(),
            _ => return Err(anyhow!("unknown setting `{key}`")),
        }
        *self = std::mem::take(self).sanitized();
        Ok(())
    }
}

/// Style specs, kept as strings so the file stays readable and hand-editable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub text: String,
    pub dim: String,
    pub border: String,
    pub border_focus: String,
    pub selection: String,
    pub directory: String,
    pub heading: String,
    pub link: String,
    pub code: String,
    pub marker: String,
}

impl Default for Theme {
    fn default() -> Self {
        // Monochrome by design. Nothing here names a colour, so tiny inherits
        // whatever palette the terminal is already using — weight, underline
        // and reverse video carry the meaning instead.
        Self {
            text: "default".into(),
            dim: "darkgray".into(),
            border: "darkgray".into(),
            border_focus: "white".into(),
            selection: "reverse".into(),
            directory: "bold".into(),
            heading: "bold".into(),
            link: "underline".into(),
            code: "dim".into(),
            marker: "bold".into(),
        }
    }
}

/// A resolved theme: specs parsed once at startup so drawing never re-parses.
///
/// `Copy`, and deliberately so — `ui` passes it around by value on every line
/// it draws, and threading a reference through would add lifetimes to most of
/// that module for no benefit. Rebuilt by `App::apply_config` whenever the
/// theme changes, which is how `:set theme.heading cyan bold` repaints without
/// a restart.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub text: Style,
    pub dim: Style,
    pub border: Style,
    pub border_focus: Style,
    pub selection: Style,
    pub directory: Style,
    pub heading: Style,
    pub link: Style,
    pub code: Style,
    pub marker: Style,
}

impl Palette {
    pub fn from_theme(t: &Theme) -> Self {
        Self {
            text: parse_style(&t.text),
            dim: parse_style(&t.dim),
            border: parse_style(&t.border),
            border_focus: parse_style(&t.border_focus),
            selection: parse_style(&t.selection),
            directory: parse_style(&t.directory),
            heading: parse_style(&t.heading),
            link: parse_style(&t.link),
            code: parse_style(&t.code),
            marker: parse_style(&t.marker),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::from_theme(&Theme::default())
    }
}

/// Parse a style spec: colour names, `#rrggbb`, palette indices, `on <colour>`
/// for a background, and the modifiers bold / dim / italic / underline /
/// reverse / strike. Unknown words are ignored rather than failing — a typo in
/// a config file should cost you an underline, not the program.
pub fn parse_style(spec: &str) -> Style {
    let mut style = Style::default();
    let mut tokens = spec.split_whitespace();
    while let Some(tok) = tokens.next() {
        match tok.to_ascii_lowercase().as_str() {
            "on" => {
                if let Some(c) = tokens.next().and_then(parse_color) {
                    style = style.bg(c);
                }
            }
            "bold" => style = style.add_modifier(Modifier::BOLD),
            "dim" | "faint" => style = style.add_modifier(Modifier::DIM),
            "italic" => style = style.add_modifier(Modifier::ITALIC),
            "underline" | "underlined" => style = style.add_modifier(Modifier::UNDERLINED),
            "reverse" | "reversed" | "invert" => style = style.add_modifier(Modifier::REVERSED),
            "strike" | "crossed" => style = style.add_modifier(Modifier::CROSSED_OUT),
            other => {
                if let Some(c) = parse_color(other) {
                    style = style.fg(c);
                }
            }
        }
    }
    style
}

/// Parse one colour word: a name, `#rgb`, `#rrggbb`, or a 0-255 palette index.
///
/// Named colours and `default` map to ratatui's 16-colour constants, which the
/// terminal renders with its own palette — that is what lets tiny match the
/// theme a user already has. `#rrggbb` forces a true-colour value instead, and
/// needs a 24-bit terminal.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    let named = match s.to_ascii_lowercase().as_str() {
        // `default` keeps the terminal's own colour, which is how tiny stays
        // transparent against whatever theme the user already has.
        "default" | "none" | "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" | "brightred" => Some(Color::LightRed),
        "lightgreen" | "brightgreen" => Some(Color::LightGreen),
        "lightyellow" | "brightyellow" => Some(Color::LightYellow),
        "lightblue" | "brightblue" => Some(Color::LightBlue),
        "lightmagenta" | "brightmagenta" => Some(Color::LightMagenta),
        "lightcyan" | "brightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    };
    named
        .or_else(|| s.parse::<u8>().ok().map(Color::Indexed))
        .or_else(|| parse_hex(s))
}

/// `#abc` or `#aabbcc` to an RGB colour. Three-digit form expands each nibble
/// by multiplying by 17, so `#abc` and `#aabbcc` are the same colour.
fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        3 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            // 0xA -> 0xAA
            Some(Color::Rgb((r * 17) as u8, (g * 17) as u8, (b * 17) as u8))
        }
        6 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            Some(Color::Rgb(
                ((v >> 16) & 0xFF) as u8,
                ((v >> 8) & 0xFF) as u8,
                (v & 0xFF) as u8,
            ))
        }
        _ => None,
    }
}

fn parse_bool(v: &str) -> Result<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(anyhow!("expected true or false, got `{v}`")),
    }
}

/// A whitespace- or comma-separated list, as typed into `:set` or the
/// settings area. Leading dots on extensions are forgiven.
///
/// So `:set prose_extensions md,.txt rst` and `:set prose_extensions md txt
/// rst` mean the same thing. Never fails — an unparseable list is an empty
/// one, which is a legal value.
fn parse_list(v: &str) -> Vec<String> {
    v.split([' ', ',', '\t'])
        .map(|s| s.trim().trim_start_matches('.'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_num(v: &str) -> Result<usize> {
    v.parse()
        .map_err(|_| anyhow!("expected a number, got `{v}`"))
}

fn parse_pos(v: &str) -> Result<Position> {
    match v.to_ascii_lowercase().as_str() {
        "top" => Ok(Position::Top),
        "bottom" => Ok(Position::Bottom),
        _ => Err(anyhow!("expected top or bottom")),
    }
}

fn side_name(s: Side) -> &'static str {
    match s {
        Side::Left => "left",
        Side::Right => "right",
    }
}

fn pos_name(p: Position) -> &'static str {
    match p {
        Position::Top => "top",
        Position::Bottom => "bottom",
    }
}

/// `$XDG_CONFIG_HOME/tiny`, falling back to `~/.config/tiny`.
///
/// Returns `None` when neither variable is set, which is why `--config` can
/// fail and why first-run config writing is best-effort.
fn config_dir() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return Some(Path::new(&x).join("tiny"));
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| Path::new(&h).join(".config").join("tiny"))
}

/// `$HOME`, or `.` when it is unset — used only as `default_root`, the
/// fallback for when the working directory cannot be read at all.
fn home() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_monochrome() {
        let p = Palette::default();
        // Nothing in the chrome names a colour: the terminal's own palette
        // shows through, and meaning is carried by weight and reverse video.
        assert_eq!(p.text.fg, Some(Color::Reset));
        assert_eq!(p.heading.fg, None);
        assert!(p.heading.add_modifier.contains(Modifier::BOLD));
        assert!(p.link.add_modifier.contains(Modifier::UNDERLINED));
        assert!(p.selection.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn style_specs_combine_colour_and_modifiers() {
        let s = parse_style("white bold underline");
        assert_eq!(s.fg, Some(Color::White));
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert!(s.add_modifier.contains(Modifier::UNDERLINED));

        let s = parse_style("#7dcfff on black italic");
        assert_eq!(s.fg, Some(Color::Rgb(0x7d, 0xcf, 0xff)));
        assert_eq!(s.bg, Some(Color::Black));
        assert!(s.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn a_typo_in_a_style_costs_an_underline_not_the_program() {
        let s = parse_style("bold nonsense-word underline");
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert!(s.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(s.fg, None, "the unknown word is ignored");
    }

    #[test]
    fn colours_accept_names_hex_and_palette_indices() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("default"), Some(Color::Reset));
        assert_eq!(parse_color("#f0a"), Some(Color::Rgb(255, 0, 170)));
        assert_eq!(parse_color("ffffff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_color("42"), Some(Color::Indexed(42)));
        assert_eq!(parse_color("banana"), None);
    }

    #[test]
    fn partial_config_keeps_defaults_for_everything_else() {
        let cfg: Config = toml::from_str("show_hidden = true\ntab_width = 2\n").unwrap();
        assert!(cfg.show_hidden);
        assert_eq!(cfg.tab_width, 2);
        assert_eq!(cfg.tree_side, Side::Left);
        assert_eq!(cfg.theme.heading, "bold");
    }

    #[test]
    fn set_applies_and_rejects() {
        let mut cfg = Config::default();
        cfg.set("tab_width", "8").unwrap();
        assert_eq!(cfg.tab_width, 8);
        cfg.set("tree_side", "right").unwrap();
        assert_eq!(cfg.tree_side, Side::Right);
        cfg.set("theme.heading", "cyan bold").unwrap();
        assert_eq!(cfg.theme.heading, "cyan bold");
        cfg.set("show_hidden", "yes").unwrap();
        assert!(cfg.show_hidden);

        assert!(cfg.set("no_such_key", "1").is_err());
        assert!(cfg.set("tab_width", "banana").is_err());
        assert!(cfg.set("tree_side", "sideways").is_err());
    }

    #[test]
    fn set_clamps_values_that_would_break_the_layout() {
        let mut cfg = Config::default();
        cfg.set("tree_width", "0.99").unwrap();
        assert_eq!(cfg.tree_width, 0.60);
        cfg.set("tab_width", "0").unwrap();
        assert_eq!(cfg.tab_width, 1);
    }

    #[test]
    fn every_indexed_setting_can_be_read_and_written() {
        let mut cfg = Config::default();
        for (key, _) in Config::settings_index() {
            let current = cfg
                .get(key)
                .unwrap_or_else(|| panic!("`{key}` is listed but has no value"));
            cfg.set(key, &current)
                .unwrap_or_else(|e| panic!("`{key}` cannot take back its own value: {e}"));
        }
    }

    #[test]
    fn a_user_conf_is_read_when_the_project_has_none() {
        let td = tempfile::tempdir().unwrap();
        let user = td.path().join(CONF_NAME);
        fs::write(&user, "tab_width = 3\n").unwrap();
        let (cfg, warning) = Config::load_from(Some(&user), None);
        assert_eq!(cfg.tab_width, 3);
        assert!(warning.is_none());
    }

    #[test]
    fn roundtrips_through_the_conf_file() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join(CONF_NAME);
        let mut cfg = Config::default();
        cfg.set("tree_side", "right").unwrap();
        cfg.set("theme.link", "cyan underline").unwrap();
        cfg.save_to(&path).unwrap();

        let back = Config::read(&path).unwrap();
        assert_eq!(back.tree_side, Side::Right);
        assert_eq!(back.theme.link, "cyan underline");
    }

    #[test]
    fn a_project_conf_overrides_the_user_one() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        fs::create_dir_all(root.join(PROJECT_DIR)).unwrap();
        fs::write(
            Config::project_path(root),
            "tab_width = 2\ntree_side = \"right\"\n",
        )
        .unwrap();

        let (cfg, warning) = Config::load_from(None, Some(root));
        assert_eq!(cfg.tab_width, 2);
        assert_eq!(cfg.tree_side, Side::Right);
        assert!(warning.is_none());
    }

    #[test]
    fn a_broken_conf_file_warns_instead_of_stopping_the_program() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        fs::create_dir_all(root.join(PROJECT_DIR)).unwrap();
        fs::write(Config::project_path(root), "tab_width = = =\n").unwrap();

        let (cfg, warning) = Config::load_from(None, Some(root));
        assert_eq!(cfg.tab_width, 4, "falls back to the default");
        assert!(warning.is_some_and(|w| w.contains(CONF_NAME)));
    }
}
