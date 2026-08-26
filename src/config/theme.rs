//! Colors, or rather the deliberate absence of them.
//!
//! Styles are written as *specs* rather than as raw colors — `"bold"`,
//! `"underline"`, `"white on black"`, `"#7dcfff bold"` — so a line can carry
//! weight as well as hue, and so the shipped defaults can name no color at
//! all. That is the point: tiny renders in whatever the terminal is already
//! using, and meaning is carried by bold, dim, underline and reverse.
//!
//! [`Theme`] is the strings as the user wrote them; [`Palette`] is those
//! strings parsed into ratatui styles, rebuilt whenever the theme changes.
//! Keeping both means a spec survives a round trip through the config file
//! exactly as typed, including the parts tiny did not understand.
//!
//! # A typo costs an underline, never the program
//!
//! [`parse_style`] ignores words it does not recognise and returns whatever it
//! did understand. A misspelled modifier therefore loses you that modifier and
//! nothing else — there is no error to report and nothing to fail.

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

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
        // Monochrome by design. Nothing here names a color, so tiny inherits
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

/// Parse a style spec: color names, `#rrggbb`, palette indices, `on <color>`
/// for a background, and the modifiers bold / dim / italic / underline /
/// reverse / strike. Unknown words are ignored rather than failing — a typo in
/// a config file should cost you an underline, not the program.
pub(super) fn parse_style(spec: &str) -> Style {
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

/// Parse one color word: a name, `#rgb`, `#rrggbb`, or a 0-255 palette index.
///
/// Named colors and `default` map to ratatui's 16-color constants, which the
/// terminal renders with its own palette — that is what lets tiny match the
/// theme a user already has. `#rrggbb` forces a true-color value instead, and
/// needs a 24-bit terminal.
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    let named = match s.to_ascii_lowercase().as_str() {
        // `default` keeps the terminal's own color, which is how tiny stays
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

/// `#abc` or `#aabbcc` to an RGB color. Three-digit form expands each nibble
/// by multiplying by 17, so `#abc` and `#aabbcc` are the same color.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_monochrome() {
        let p = Palette::default();
        // Nothing in the chrome names a color: the terminal's own palette
        // shows through, and meaning is carried by weight and reverse video.
        assert_eq!(p.text.fg, Some(Color::Reset));
        assert_eq!(p.heading.fg, None);
        assert!(p.heading.add_modifier.contains(Modifier::BOLD));
        assert!(p.link.add_modifier.contains(Modifier::UNDERLINED));
        assert!(p.selection.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn style_specs_combine_color_and_modifiers() {
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
    fn colors_accept_names_hex_and_palette_indices() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("default"), Some(Color::Reset));
        assert_eq!(parse_color("#f0a"), Some(Color::Rgb(255, 0, 170)));
        assert_eq!(parse_color("ffffff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_color("42"), Some(Color::Indexed(42)));
        assert_eq!(parse_color("banana"), None);
    }
}
