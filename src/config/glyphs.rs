//! The characters tiny draws its structure with.
//!
//! A terminal program does not choose its font. The terminal draws every
//! character in its own, and tiny only decides which characters to send, so
//! tiny already uses whatever font your terminal uses. What can go wrong is
//! the characters: box lines, folder arrows, bullets and rules are in nearly
//! every monospaced font, but not in every one, and a missing glyph turns into
//! a box or a question mark exactly where the shape of the screen should be.
//!
//! So the font question is really a character question, and it has one
//! setting: `glyphs`, which is `auto` until it is set. [`Glyphs`] is
//! everything a set draws, and there are three, from the most a font is
//! likely to have to the least:
//!
//! - `unicode`, for a terminal that can borrow a character its font lacks
//!   from another font, which every terminal on a desktop does.
//! - `console`, for one that cannot: Windows' own console and the Linux
//!   console draw from one font, and a character missing from it is a box.
//!   Box lines, arrows and bullets still, but only ones every console font
//!   has, since every console font starts from the same old character set.
//! - `ascii`, the floor every font on every terminal has, for a terminal that
//!   is not reading UTF-8 at all.
//!
//! `auto` is whichever suits the terminal tiny is running in, see
//! [`super::host`].
//!
//! # Where the set reaches
//!
//! [`Palette`](super::Palette) carries the active set, so everything that
//! draws already has it: pane borders, the browser's folder markers, the
//! project map, markdown's bullets, rules, gutters and tables, and the arrows
//! and ellipses in the status line and the git window. A test renders every
//! screen with `ascii` and checks that nothing outside ASCII reaches it, which
//! is what keeps a new screen from quietly drawing a character the set does
//! not cover.

use ratatui::symbols::border;
use serde::{Deserialize, Serialize};

/// The `glyphs` setting: which characters tiny draws its structure with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlyphSet {
    /// Whichever suits the terminal tiny is running in, see
    /// [`Host`](super::host::Host).
    #[default]
    Auto,
    /// Box drawing, arrows and bullets.
    ///
    /// `arrows` is what tiny called this before it covered more than the
    /// browser's markers, in a setting called `markers`. Both old spellings
    /// are still read, so an old config keeps its look.
    #[serde(alias = "arrows")]
    Unicode,
    /// Box drawing, arrows and bullets from the characters every console
    /// font has, for a console that draws from one font.
    Console,
    /// Nothing outside ASCII, for a font or a terminal that has none of it.
    Ascii,
}

impl GlyphSet {
    /// As it is written in the config.
    pub fn name(self) -> &'static str {
        match self {
            GlyphSet::Auto => "auto",
            GlyphSet::Unicode => "unicode",
            GlyphSet::Console => "console",
            GlyphSet::Ascii => "ascii",
        }
    }

    /// Read a value typed into `*set`, the settings area, or a plugin.
    /// `arrows` is accepted for the reason the variant says.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(GlyphSet::Auto),
            "unicode" | "arrows" => Some(GlyphSet::Unicode),
            "console" => Some(GlyphSet::Console),
            "ascii" => Some(GlyphSet::Ascii),
            _ => None,
        }
    }

    /// The setting as this terminal draws it: `auto` is whichever set its
    /// fonts can be counted on for.
    pub fn resolve(self, host: &super::host::Host) -> Self {
        match self {
            GlyphSet::Auto => host.characters,
            fixed => fixed,
        }
    }

    /// The characters this set draws. `auto` not yet resolved draws as a
    /// modern terminal would, the assumption until tiny has looked.
    pub fn glyphs(self) -> &'static Glyphs {
        match self {
            GlyphSet::Auto | GlyphSet::Unicode => &UNICODE,
            GlyphSet::Console => &CONSOLE,
            GlyphSet::Ascii => &ASCII,
        }
    }
}

/// Every character tiny draws for structure rather than content.
///
/// Anything a file or a name contains is drawn as it is: these are only the
/// lines, markers and marks tiny adds around it.
#[derive(Debug)]
pub struct Glyphs {
    /// The border around every pane and overlay.
    pub border: border::Set<'static>,
    /// An open folder and a closed one in the browser. Two columns each, so
    /// names line up whichever marker is in front of them.
    pub folder_open: &'static str,
    pub folder_closed: &'static str,
    /// List bullets by depth, repeating after the last.
    pub bullets: [&'static str; 3],
    /// The rule under a first level heading, and under a second level one.
    /// The lighter one is also a horizontal rule and the line under a table's
    /// header.
    pub rule_heavy: &'static str,
    pub rule: &'static str,
    /// The bar down the left of a quote or a code block, with its space.
    pub gutter: &'static str,
    /// Between two table columns, and where that meets the header rule.
    pub column: &'static str,
    pub cross: &'static str,
    /// Where text was cut short to fit.
    pub ellipsis: &'static str,
    /// From one thing to another: a rename, a branch and its upstream.
    pub arrow: &'static str,
    /// In front of a search hit's line, under the file it is in.
    pub continued: &'static str,
    /// Between a picture's width and height.
    pub times: &'static str,
    /// The project map's lines, indexed by the direction bits a line leaves a
    /// cell by: up 1, down 2, left 4, right 8.
    pub map_lines: [char; 16],
    /// A map box's corners, clockwise from the top left. Rounded where the
    /// font has them, which keeps a box apart from a line turning a corner.
    pub map_corners: [char; 4],
    pub map_horizontal: char,
    pub map_vertical: char,
}

pub static UNICODE: Glyphs = Glyphs {
    border: border::PLAIN,
    folder_open: "▾ ",
    folder_closed: "▸ ",
    bullets: ["• ", "◦ ", "▪ "],
    rule_heavy: "━",
    rule: "─",
    gutter: "│ ",
    column: " │ ",
    cross: "─┼─",
    ellipsis: "…",
    arrow: "→",
    continued: "↳",
    times: "×",
    map_lines: [
        ' ', '│', '│', '│', '─', '┘', '┐', '┤', '─', '└', '┌', '├', '─', '┴', '┬', '┼',
    ],
    map_corners: ['╭', '╮', '╰', '╯'],
    map_horizontal: '─',
    map_vertical: '│',
};

/// What a console draws, from one font: only characters in both code page
/// 437, the IBM PC's, which every console font still starts from, and WGL4,
/// the set every Windows font is made to cover. So square corners rather
/// than rounded ones, ► rather than ▸, and a double rule where the Unicode
/// set has a heavy one; a map box has double edges, which keeps it apart
/// from a single line turning a corner the way rounded corners do. `...`
/// and `x`, because neither `…` nor `×` is in code page 437.
pub static CONSOLE: Glyphs = Glyphs {
    border: border::PLAIN,
    folder_open: "▼ ",
    folder_closed: "► ",
    bullets: ["• ", "○ ", "∙ "],
    rule_heavy: "═",
    rule: "─",
    gutter: "│ ",
    column: " │ ",
    cross: "─┼─",
    ellipsis: "...",
    arrow: "→",
    continued: "└",
    times: "x",
    map_lines: [
        ' ', '│', '│', '│', '─', '┘', '┐', '┤', '─', '└', '┌', '├', '─', '┴', '┬', '┼',
    ],
    map_corners: ['╔', '╗', '╚', '╝'],
    map_horizontal: '═',
    map_vertical: '║',
};

pub static ASCII: Glyphs = Glyphs {
    border: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    },
    folder_open: "v ",
    folder_closed: "> ",
    bullets: ["* ", "- ", "+ "],
    rule_heavy: "=",
    rule: "-",
    gutter: "| ",
    column: " | ",
    cross: "-+-",
    ellipsis: "...",
    arrow: "->",
    continued: ">",
    times: "x",
    map_lines: [
        ' ', '|', '|', '|', '-', '+', '+', '+', '-', '+', '+', '+', '-', '+', '+', '+',
    ],
    map_corners: ['+', '+', '+', '+'],
    map_horizontal: '-',
    map_vertical: '|',
};

/// The characters outside ASCII that are in both code page 437 and WGL4,
/// of the ones the console set might want: what every console font has.
#[cfg(test)]
pub(crate) const CONSOLE_CHARACTERS: &str = "─│┌┐└┘├┤┬┴┼═║╔╗╚╝►▼•○∙→";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every string and character in a set, for checking all of them at once.
    fn everything(g: &Glyphs) -> String {
        let b = g.border;
        let mut out: String = [
            b.top_left,
            b.top_right,
            b.bottom_left,
            b.bottom_right,
            b.vertical_left,
            b.vertical_right,
            b.horizontal_top,
            b.horizontal_bottom,
            g.folder_open,
            g.folder_closed,
            g.rule_heavy,
            g.rule,
            g.gutter,
            g.column,
            g.cross,
            g.ellipsis,
            g.arrow,
            g.continued,
            g.times,
        ]
        .concat();
        out.extend(g.bullets.iter().copied());
        out.extend(g.map_lines.iter());
        out.extend(g.map_corners.iter());
        out.push(g.map_horizontal);
        out.push(g.map_vertical);
        out
    }

    #[test]
    fn the_ascii_set_is_all_ascii() {
        let all = everything(&ASCII);
        assert!(all.is_ascii(), "something outside ASCII: {all:?}");
    }

    #[test]
    fn the_console_set_is_what_every_console_font_has() {
        for c in everything(&CONSOLE).chars().filter(|c| !c.is_ascii()) {
            assert!(
                super::CONSOLE_CHARACTERS.contains(c),
                "{c:?} is not in every console font"
            );
        }
    }

    #[test]
    fn the_sets_line_up_column_for_column() {
        // A marker that is two columns in one set and three in another
        // would shift every name in the browser when the setting changed.
        use unicode_width::UnicodeWidthStr;
        for other in [&CONSOLE, &ASCII] {
            for (u, a) in [
                (UNICODE.folder_open, other.folder_open),
                (UNICODE.folder_closed, other.folder_closed),
                (UNICODE.gutter, other.gutter),
                (UNICODE.column, other.column),
                (UNICODE.cross, other.cross),
                (UNICODE.rule, other.rule),
                (UNICODE.rule_heavy, other.rule_heavy),
            ] {
                assert_eq!(u.width(), a.width(), "{u:?} and {a:?}");
            }
            for (u, a) in UNICODE.bullets.iter().zip(other.bullets) {
                assert_eq!(u.width(), a.width(), "{u:?} and {a:?}");
            }
        }
    }

    #[test]
    fn an_old_config_keeps_its_look() {
        assert_eq!(GlyphSet::parse("arrows"), Some(GlyphSet::Unicode));
        assert_eq!(GlyphSet::parse("ascii"), Some(GlyphSet::Ascii));
        assert_eq!(GlyphSet::parse("console"), Some(GlyphSet::Console));
        assert_eq!(GlyphSet::parse(" Unicode "), Some(GlyphSet::Unicode));
        assert_eq!(GlyphSet::parse("emoji"), None);
    }
}
