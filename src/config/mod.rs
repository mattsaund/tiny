//! Configuration.
//!
//! One file, and only one: `tiny.conf`, next to whatever else the platform
//! keeps a user's settings in (see [`config_dir`]). How the program behaves is
//! a property of the person using it, not of the folder they happen to have
//! open, so there is deliberately no per-project override: every project on a
//! machine draws the same tree the same way.
//!
//! Every field has a default, so a partial or malformed file never stops the
//! program starting; it falls back and says so in the status bar.
//!
//! Colors are written as style specs rather than raw colors, so a line can
//! carry weight as well as hue: `"bold"`, `"underline"`, `"white on black"`,
//! `"#7dcfff bold"`. The shipped defaults are deliberately monochrome and use
//! the terminal's own palette, so tiny looks like the terminal it runs in.
//!
//! # Adding a setting
//!
//! Four places have to agree, and there is no macro tying them together, so
//! all four need editing by hand:
//!
//! 1. A field on [`Config`], with a doc comment.
//! 2. A default in `impl Default for Config`.
//! 3. A row in [`Config::settings_index`]; this drives both `*set` completion
//!    and the in-program settings area.
//! 4. Arms in [`Config::get`] and [`Config::set`].
//!
//! Miss (3) and the setting works from a config file but is invisible in the
//! UI; miss (4) and `*set` reports it as unknown. The
//! `every_setting_round_trips` test in this file catches most of this.
//!
//! # Nothing here fails hard
//!
//! A malformed file falls back to defaults and returns a warning string for
//! the status bar; an unknown word in a style spec is ignored; out-of-range
//! numbers are clamped by [`Config::sanitized`]. A typo in a config file
//! should cost you an underline, never the program.

pub mod glyphs;
pub mod keys;
pub mod keyspec;
pub mod theme;

pub use self::glyphs::{GlyphSet, Glyphs};
pub use self::theme::{Palette, Theme};

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// The config file, in [`config_dir`]. There is exactly one of these.
pub const CONF_NAME: &str = "tiny.conf";

/// The plugins folder, beside the config file.
pub const PLUGINS_NAME: &str = "plugins";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Which side of the window the tree pane occupies.
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
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
    /// Key bindings that differ from the shipped ones, by action name, see
    /// [`crate::config::keys`]. Only what has been changed is kept, so an untouched
    /// config has no `[keys]` section and a binding added in a later version
    /// reaches everyone who has not overridden that action.
    pub keys: BTreeMap<String, String>,

    pub show_hidden: bool,
    pub tab_width: usize,
    /// Re-read files and folders that another program changed, without being
    /// asked. Off makes `*reload` the only way the disk is looked at again.
    pub auto_reload: bool,
    /// Ask GitHub, once per run, whether a newer tiny has been released, and
    /// offer to install it. Off leaves `*update` and `tiny --update` as the
    /// only ways to find out, see [`crate::update`].
    pub update_check: bool,

    /// Which side the project tree sits on.
    pub tree_side: Side,
    /// Share of the window given to the tree, 0.1 - 0.6.
    pub tree_width: f32,
    /// Number the lines of a file being edited.
    pub line_numbers: bool,
    /// Which characters tiny draws its lines and markers with. See
    /// [`glyphs`]: the font is the terminal's, so this is how tiny is made to
    /// look right in any of them. Read from `markers` too, its old name.
    #[serde(alias = "markers")]
    pub glyphs: GlyphSet,
    /// Draw boxes around the panes. Off gives a plainer, quieter screen.
    pub borders: bool,

    /// syntect theme used for code. Chrome stays monochrome regardless.
    pub syntax_theme: String,
    /// Directories never walked by search, by exact name.
    pub search_ignore: Vec<String>,

    /// Extensions treated as prose: wrapped, read first, edited on demand.
    /// Everything else that is text opens straight in the editor.
    pub prose_extensions: Vec<String>,

    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_root: home(),
            keys: BTreeMap::new(),
            show_hidden: false,
            tab_width: 4,
            auto_reload: true,
            update_check: true,
            tree_side: Side::Left,
            tree_width: 0.30,
            line_numbers: true,
            glyphs: GlyphSet::Unicode,
            borders: true,
            syntax_theme: "base16-ocean.dark".into(),
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
            theme: Theme::default(),
        }
    }
}

impl Config {
    /// `~/.config/tiny/tiny.conf`.
    pub fn user_path() -> Option<PathBuf> {
        config_dir().map(|d| d.join(CONF_NAME))
    }

    /// `~/.config/tiny/plugins`, see [`crate::plugins`]. Beside `tiny.conf`,
    /// so everything that is tiny's own is in one folder.
    pub fn plugins_dir() -> Option<PathBuf> {
        config_dir().map(|d| d.join(PLUGINS_NAME))
    }

    /// Read the one config file, if it is there. Returns the config plus
    /// anything worth telling the user about.
    ///
    /// Called once, before the project is even resolved; nothing about which
    /// folder is open can change the answer.
    pub fn load() -> (Self, Option<String>) {
        Self::load_from(Self::user_path().as_deref())
    }

    /// The body of `load`, with the path passed in. Tests use this so they
    /// never depend on (or touch) whatever config the machine really has.
    pub fn load_from(user_conf: Option<&Path>) -> (Self, Option<String>) {
        let mut warning = None;
        let cfg = match user_conf {
            Some(p) if p.exists() => match Self::read(p) {
                Ok(c) => c,
                Err(e) => {
                    warning = Some(format!("{CONF_NAME}: {e} (using defaults)"));
                    Self::default()
                }
            },
            _ => Self::default(),
        };
        (cfg.sanitized(), warning)
    }

    fn read(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str::<Config>(&text)?)
    }

    /// Write to the config path, creating the directory if needed.
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

    /// How many settings differ from the shipped ones.
    ///
    /// Counted through `get`, so it walks the same list the settings area
    /// shows and cannot fall out of step with it. Key bindings are not
    /// settings and are not counted; they have their own reset.
    pub fn changed_from_default(&self) -> usize {
        let shipped = Config::default();
        Self::settings_index()
            .iter()
            .filter(|(name, _)| self.get(name) != shipped.get(name))
            .count()
    }

    /// Clamp every numeric field into a range that cannot break the layout,
    /// and bring key names written by an older tiny up to date.
    ///
    /// Run after loading a file and after every `*set`, so there is no path by
    /// which a hand-edited `tree_width = 40` produces a tree wider than the
    /// window or a `tab_width = 0` produces an infinite loop.
    fn sanitized(mut self) -> Self {
        self.tree_width = self.tree_width.clamp(0.10, 0.60);
        self.tab_width = self.tab_width.clamp(1, 16);
        keys::migrate(&mut self.keys);
        self
    }

    /// Every key `*set` accepts, with a hint of a word or two. Also what the
    /// in-program settings area lists, so the two can never drift apart.
    ///
    /// A hint says what the name does not (the values a setting takes, or
    /// what it is for when the name is not enough) and is empty otherwise.
    /// The settings area shows it only beside the row under the cursor.
    pub fn settings_index() -> &'static [(&'static str, &'static str)] {
        &[
            ("show_hidden", "dotfiles"),
            ("tab_width", "spaces per Tab"),
            ("auto_reload", "pick up outside changes"),
            ("update_check", "ask when a new tiny is out"),
            ("tree_side", "left or right"),
            ("tree_width", "0.1 to 0.6"),
            ("line_numbers", ""),
            ("glyphs", "unicode or ascii"),
            ("borders", ""),
            ("syntax_theme", "colors for code"),
            ("prose_extensions", "wrapped, not code"),
            ("search_ignore", "folders search skips"),
            ("theme.text", ""),
            ("theme.dim", ""),
            ("theme.border", ""),
            ("theme.border_focus", ""),
            ("theme.selection", ""),
            ("theme.directory", ""),
            ("theme.heading", ""),
            ("theme.link", ""),
            ("theme.code", ""),
            ("theme.marker", "unsaved, and warnings"),
            ("theme.map_in", "map: what reaches it"),
            ("theme.map_out", "map: what it reaches"),
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
            "show_hidden" => self.show_hidden.to_string(),
            "tab_width" => self.tab_width.to_string(),
            "auto_reload" => self.auto_reload.to_string(),
            "update_check" => self.update_check.to_string(),
            "tree_side" => side_name(self.tree_side).into(),
            "tree_width" => format!("{:.2}", self.tree_width),
            "line_numbers" => self.line_numbers.to_string(),
            "glyphs" => self.glyphs.name().into(),
            "borders" => self.borders.to_string(),
            "syntax_theme" => self.syntax_theme.clone(),
            "prose_extensions" => self.prose_extensions.join(" "),
            "search_ignore" => self.search_ignore.join(" "),
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
            "theme.map_in" => self.theme.map_in.clone(),
            "theme.map_out" => self.theme.map_out.clone(),
            _ => return None,
        })
    }

    /// Apply one `*set key value`. Rejects unknown keys and unparseable
    /// values rather than silently doing nothing.
    ///
    /// Note that theme entries are stored as raw strings without validation:
    /// [`theme::parse_style`] ignores words it does not recognize, so a misspelled
    /// color is accepted here and simply has no effect when drawn. Callers
    /// must follow a successful `set` with `App::apply_config` to rebuild the
    /// palette and highlighter; the config alone is just data.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let v = value.trim();
        match key {
            "show_hidden" => self.show_hidden = parse_bool(v)?,
            "tab_width" => self.tab_width = parse_num(v)?,
            "auto_reload" => self.auto_reload = parse_bool(v)?,
            "update_check" => self.update_check = parse_bool(v)?,
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
            "line_numbers" => self.line_numbers = parse_bool(v)?,
            // `markers` was this setting's name before it covered more than
            // the browser, and is still what some people will type.
            "glyphs" | "markers" => {
                self.glyphs =
                    GlyphSet::parse(v).ok_or_else(|| anyhow!("glyphs must be unicode or ascii"))?
            }
            "borders" => self.borders = parse_bool(v)?,
            "syntax_theme" => self.syntax_theme = v.to_string(),
            "prose_extensions" => self.prose_extensions = parse_list(v),
            "search_ignore" => self.search_ignore = parse_list(v),
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
            "theme.map_in" => self.theme.map_in = v.to_string(),
            "theme.map_out" => self.theme.map_out = v.to_string(),
            _ => return Err(anyhow!("unknown setting `{key}`")),
        }
        *self = std::mem::take(self).sanitized();
        Ok(())
    }
}

fn parse_bool(v: &str) -> Result<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(anyhow!("expected true or false, got `{v}`")),
    }
}

/// A whitespace- or comma-separated list, as typed into `*set` or the
/// settings area. Leading dots on extensions are forgiven.
///
/// So `*set prose_extensions md,.txt rst` and `*set prose_extensions md txt
/// rst` mean the same thing. Never fails: an unparseable list is an empty
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

fn side_name(s: Side) -> &'static str {
    match s {
        Side::Left => "left",
        Side::Right => "right",
    }
}

/// The one directory `tiny.conf` lives in, in the order the platforms are
/// asked: `$XDG_CONFIG_HOME/tiny`, then `%APPDATA%\tiny` on Windows, then
/// `~/.config/tiny` on Linux and macOS.
///
/// `XDG_CONFIG_HOME` comes first everywhere because setting it is an explicit
/// choice, and `APPDATA` before the home directory because a Windows shell that
/// also defines `HOME` (git-bash, msys) should still put the file where the
/// rest of Windows keeps such things.
///
/// Returns `None` when none of them are set, which is why `--config` can fail
/// and why first-run config writing is best-effort.
fn config_dir() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return Some(Path::new(&x).join("tiny"));
    }
    if let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.is_empty()
    {
        return Some(Path::new(&appdata).join("tiny"));
    }
    home_dir().map(|h| h.join(".config").join("tiny"))
}

/// The user's home directory: `$HOME`, or `%USERPROFILE%` on Windows, which is
/// where Windows puts it and does not set `HOME`.
pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"].into_iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
    })
}

/// The home directory, or `.` when there isn't one, used only as
/// `default_root`, the fallback for when the working directory cannot be read
/// at all.
fn home() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_one_conf_file_is_what_gets_read() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join(CONF_NAME);
        fs::write(&path, "tab_width = 2\ntree_side = \"right\"\n").unwrap();

        let (cfg, warning) = Config::load_from(Some(&path));
        assert_eq!(cfg.tab_width, 2);
        assert_eq!(cfg.tree_side, Side::Right);
        assert!(warning.is_none());
    }

    #[test]
    fn rebindings_survive_the_round_trip_through_the_file() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join(CONF_NAME);
        let mut cfg = Config::default();
        cfg.keys.insert("tree.down".into(), "z".into());
        cfg.save_to(&path).unwrap();

        let (back, warning) = Config::load_from(Some(&path));
        assert!(warning.is_none());
        assert_eq!(back.keys.get("tree.down").map(String::as_str), Some("z"));
    }

    #[test]
    fn a_config_with_no_rebindings_has_none() {
        assert!(
            Config::default().keys.is_empty(),
            "the shipped keyboard is not written down twice"
        );
    }

    #[test]
    fn changed_from_default_counts_only_what_moved() {
        let mut cfg = Config::default();
        assert_eq!(cfg.changed_from_default(), 0);
        cfg.set("tab_width", "7").unwrap();
        cfg.set("tree_side", "right").unwrap();
        assert_eq!(cfg.changed_from_default(), 2);
        // A rebinding is not a setting, and has its own reset.
        cfg.keys.insert("tree.down".into(), "z".into());
        assert_eq!(cfg.changed_from_default(), 2);
    }

    #[test]
    fn a_broken_conf_file_warns_instead_of_stopping_the_program() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join(CONF_NAME);
        fs::write(&path, "tab_width = = =\n").unwrap();

        let (cfg, warning) = Config::load_from(Some(&path));
        assert_eq!(cfg.tab_width, 4, "falls back to the default");
        assert!(warning.is_some_and(|w| w.contains(CONF_NAME)));
    }

    #[test]
    fn a_config_from_before_glyphs_keeps_its_look() {
        // `markers` was the setting's old name, and `arrows` its old value.
        let cfg: Config = toml::from_str("markers = \"ascii\"\n").unwrap();
        assert_eq!(cfg.glyphs, GlyphSet::Ascii);
        let cfg: Config = toml::from_str("markers = \"arrows\"\n").unwrap();
        assert_eq!(cfg.glyphs, GlyphSet::Unicode);

        let mut cfg = Config::default();
        cfg.set("markers", "ascii").unwrap();
        assert_eq!(cfg.get("glyphs").as_deref(), Some("ascii"));
    }

    #[test]
    fn a_missing_conf_file_is_simply_the_defaults() {
        let td = tempfile::tempdir().unwrap();
        let (cfg, warning) = Config::load_from(Some(&td.path().join("nothing-here.conf")));
        assert_eq!(cfg.tab_width, Config::default().tab_width);
        assert!(warning.is_none());
    }
}
