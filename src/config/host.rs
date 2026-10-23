//! What tiny is running in, and what that can show.
//!
//! A terminal does not say what it can draw, but it does say what it is, in
//! the variables it sets for the programs it starts, and that is enough to
//! know the rest: a Linux console draws from one font with a few hundred
//! characters in it and has sixteen colors, Windows' own console has every
//! color but no faint text, a `vt100` has no color at all, and kitty and
//! Windows Terminal have everything. [`Host::detect`] reads those variables
//! once, as tiny starts, and the `glyphs` and `colors` settings follow what
//! it found while they are left at `auto`. Either can be set by hand where
//! the guess is wrong.
//!
//! # The best each one can do
//!
//! The aim is not the least every terminal has in common but the most each
//! one can show: every color and every character where they are there, box
//! lines from the old PC character set on a console, ASCII only where a
//! terminal is not reading UTF-8 at all. And what a terminal cannot draw is
//! said another way rather than dropped: faint text is dark gray on a console
//! with no faint, and dark gray is faint on a terminal asked for no color.
//!
//! # Guessing low
//!
//! Where a rule has to guess, it errs toward less. A terminal nothing
//! recognizes gets 256 colors rather than all of them. Too few colors looks a
//! little plain; too many draws code in colors nobody chose, and a character
//! missing from the font turns the frame of the screen into boxes.
//!
//! # What cannot be seen from here
//!
//! Over `ssh` most of these variables stay behind on the machine the terminal
//! is on; `TERM` alone makes the trip. That is the case the rules are built
//! around: `TERM` decides when nothing else has spoken, and says little more
//! than "256 colors" for any modern terminal, which is right.

use serde::{Deserialize, Serialize};

use super::glyphs::GlyphSet;

/// The `colors` setting: how many colors tiny draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colors {
    /// As many as the terminal turns out to have, see [`Host`].
    #[default]
    Auto,
    /// Any color at all, twenty-four bits of it.
    Truecolor,
    /// The 256 of xterm's palette.
    #[serde(rename = "256")]
    Ansi256,
    /// The sixteen every color terminal has.
    #[serde(rename = "16")]
    Ansi16,
    /// None at all. Weight, underline and reverse video still say what
    /// tiny's own colors say, which is how its default look was built.
    None,
}

impl Colors {
    /// As it is written in the config.
    pub fn name(self) -> &'static str {
        match self {
            Colors::Auto => "auto",
            Colors::Truecolor => "truecolor",
            Colors::Ansi256 => "256",
            Colors::Ansi16 => "16",
            Colors::None => "none",
        }
    }

    /// Read a value typed into `*set`, the settings area, or a plugin.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Colors::Auto,
            "truecolor" | "24bit" | "24-bit" => Colors::Truecolor,
            "256" => Colors::Ansi256,
            "16" => Colors::Ansi16,
            "none" | "off" | "no" => Colors::None,
            _ => return None,
        })
    }

    /// The setting as this terminal draws it: `auto` is what was found.
    pub fn resolve(self, host: &Host) -> Self {
        match self {
            Colors::Auto => host.colors,
            fixed => fixed,
        }
    }
}

/// What the terminal tiny is running in can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    /// The characters its fonts can be counted on for. Never `Auto`.
    pub characters: GlyphSet,
    /// How many colors it shows. Never `Auto`.
    pub colors: Colors,
    /// Whether it draws faint text fainter than the rest.
    pub faint: bool,
}

impl Default for Host {
    /// A modern terminal: what tiny assumes until it has looked, and what
    /// the tests draw in.
    fn default() -> Self {
        Self {
            characters: GlyphSet::Unicode,
            colors: Colors::Truecolor,
            faint: true,
        }
    }
}

/// `TERM` names with no color and no characters past ASCII: the hardware
/// terminals and their descendants, and the name for no terminal at all.
const PLAIN: &[&str] = &[
    "dumb",
    "vt52",
    "vt100",
    "vt102",
    "vt220",
    "vt320",
    "xterm-mono",
];

/// `TERM` names of sixteen-color terminals with no UTF-8 in them: the BSD
/// consoles and plain ANSI, which read one byte as one character.
const SIXTEEN_COLOR_BYTES: &[&str] = &["cons25", "cons25-m", "wsvt25", "pcvt25", "ansi", "pcansi"];

/// Variables that terminals with every color, and fonts that between them
/// have every character, set to say so: Windows Terminal, kitty, WezTerm,
/// Konsole, Alacritty, Ghostty and the JetBrains IDEs.
const MODERN_VARIABLES: &[&str] = &[
    "WT_SESSION",
    "KITTY_WINDOW_ID",
    "WEZTERM_PANE",
    "KONSOLE_VERSION",
    "ALACRITTY_WINDOW_ID",
    "GHOSTTY_RESOURCES_DIR",
    "TERMINAL_EMULATOR",
];

/// The same, said through `TERM_PROGRAM`.
const MODERN_PROGRAMS: &[&str] = &[
    "iTerm.app",
    "vscode",
    "WezTerm",
    "ghostty",
    "mintty",
    "Hyper",
    "Tabby",
    "rio",
    "WarpTerminal",
    "contour",
];

impl Host {
    /// Look at the terminal tiny was started in.
    pub fn detect() -> Self {
        Self::from_env(
            &|var| std::env::var(var).ok().filter(|v| !v.is_empty()),
            cfg!(windows),
        )
    }

    /// The rules, with the environment passed in, so every terminal can be
    /// checked from any machine.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>, windows: bool) -> Self {
        let term = env("TERM").unwrap_or_default().to_ascii_lowercase();
        let program = env("TERM_PROGRAM").unwrap_or_default();
        // GNOME Terminal, Tilix, Terminator and the rest of the VTE family
        // have had every color since this version, 0.36.
        let vte = env("VTE_VERSION")
            .and_then(|v| v.parse::<u32>().ok())
            .is_some_and(|v| v >= 3600);
        let modern = vte
            || MODERN_VARIABLES.iter().any(|v| env(v).is_some())
            || MODERN_PROGRAMS.contains(&program.as_str());
        let says_truecolor = env("COLORTERM")
            .is_some_and(|c| matches!(c.to_ascii_lowercase().as_str(), "truecolor" | "24bit"))
            || term.ends_with("-direct");
        // The console Windows opens on its own, around Command Prompt or
        // PowerShell when nothing else is: anything else says what it is
        // (Windows Terminal, VS Code, mintty, ConEmu).
        let windows_console = windows && term.is_empty() && !modern && env("ConEmuANSI").is_none();
        let linux_console = term == "linux";
        let plain = PLAIN.contains(&term.as_str());
        let bytes = SIXTEEN_COLOR_BYTES.contains(&term.as_str());

        let colors = if env("NO_COLOR").is_some() || plain {
            Colors::None
        } else if says_truecolor || modern {
            Colors::Truecolor
        } else if linux_console || bytes {
            Colors::Ansi16
        } else if term.contains("256color") || program == "Apple_Terminal" {
            Colors::Ansi256
        } else if windows && term.is_empty() {
            // Windows' console has drawn every color since the Windows 10
            // that Rust needs to run at all.
            Colors::Truecolor
        } else if matches!(
            term.as_str(),
            "xterm" | "screen" | "tmux" | "rxvt" | "putty"
        ) {
            // What these names promise, read the way terminfo reads them.
            Colors::Ansi16
        } else {
            Colors::Ansi256
        };

        // A locale that is not UTF-8 means each byte of a box line arrives as
        // a character of something else. One that is not set says nothing,
        // which is the usual case on Windows and over ssh.
        let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|var| env(var))
            .is_none_or(|locale| {
                let l = locale.to_ascii_lowercase();
                l.contains("utf-8") || l.contains("utf8")
            });
        let characters = if !utf8 || plain || bytes {
            GlyphSet::Ascii
        } else if windows_console || linux_console {
            // One font, and no other to borrow from: what every console font
            // has, which is still box lines, arrows and bullets.
            GlyphSet::Console
        } else {
            GlyphSet::Unicode
        };

        Self {
            characters,
            colors,
            // Windows' console draws faint text at full strength.
            faint: !windows_console,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(vars: &[(&str, &str)], windows: bool) -> Host {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Host::from_env(
            &|var| vars.iter().find(|(k, _)| k == var).map(|(_, v)| v.clone()),
            windows,
        )
    }

    #[test]
    fn the_terminals_people_use_get_everything() {
        for vars in [
            &[("TERM", "xterm-kitty"), ("KITTY_WINDOW_ID", "1")][..],
            &[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")],
            &[("TERM", "xterm-256color"), ("KONSOLE_VERSION", "260802")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "7800")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "iTerm.app")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
            &[("TERM", "wezterm"), ("WEZTERM_PANE", "0")],
        ] {
            assert_eq!(host(vars, false), Host::default(), "{vars:?}");
        }
        // Windows Terminal, around Command Prompt or anything else.
        assert_eq!(host(&[("WT_SESSION", "1d3c")], true), Host::default());
    }

    #[test]
    fn macos_terminal_and_tmux_and_ssh_get_256() {
        let mac = host(
            &[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal"),
            ],
            false,
        );
        assert_eq!(mac.colors, Colors::Ansi256);
        assert_eq!(mac.characters, GlyphSet::Unicode);
        // Inside tmux, or at the far end of ssh, `TERM` is all there is.
        for term in ["tmux-256color", "screen-256color", "xterm-256color"] {
            assert_eq!(
                host(&[("TERM", term)], false).colors,
                Colors::Ansi256,
                "{term}"
            );
        }
        // A name nothing here knows: the middle of the road.
        assert_eq!(host(&[("TERM", "st-meta")], false).colors, Colors::Ansi256);
    }

    #[test]
    fn the_linux_console_gets_box_lines_and_sixteen_colors() {
        assert_eq!(
            host(&[("TERM", "linux"), ("LANG", "en_US.UTF-8")], false),
            Host {
                characters: GlyphSet::Console,
                colors: Colors::Ansi16,
                faint: true,
            }
        );
        assert_eq!(host(&[("TERM", "xterm")], false).colors, Colors::Ansi16);
    }

    #[test]
    fn windows_own_console_gets_box_lines_every_color_and_no_faint() {
        // Command Prompt or PowerShell in the window Windows opens for them.
        assert_eq!(
            host(&[], true),
            Host {
                characters: GlyphSet::Console,
                colors: Colors::Truecolor,
                faint: false,
            }
        );
        // Anything that says what it is draws everything.
        for vars in [
            &[("WT_SESSION", "1d3c")][..],
            &[("TERM_PROGRAM", "vscode")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "mintty")],
            &[("ConEmuANSI", "ON")],
        ] {
            let h = host(vars, true);
            assert_eq!(h.characters, GlyphSet::Unicode, "{vars:?}");
            assert!(h.faint, "{vars:?}");
        }
    }

    #[test]
    fn a_terminal_without_color_gets_none_and_no_color_is_honored() {
        for term in ["vt100", "vt220", "dumb"] {
            let h = host(&[("TERM", term)], false);
            assert_eq!(h.colors, Colors::None, "{term}");
            assert_eq!(h.characters, GlyphSet::Ascii, "{term}");
        }
        let asked = host(
            &[
                ("TERM", "xterm-kitty"),
                ("KITTY_WINDOW_ID", "1"),
                ("NO_COLOR", "1"),
            ],
            false,
        );
        assert_eq!(asked.colors, Colors::None);
        assert_eq!(
            asked.characters,
            GlyphSet::Unicode,
            "only the colors were asked away"
        );
    }

    #[test]
    fn a_terminal_reading_bytes_rather_than_utf8_gets_ascii() {
        for (var, value) in [
            ("LANG", "C"),
            ("LC_ALL", "POSIX"),
            ("LC_CTYPE", "en_US.ISO-8859-1"),
        ] {
            let h = host(&[("TERM", "xterm-256color"), (var, value)], false);
            assert_eq!(h.characters, GlyphSet::Ascii, "{var}={value}");
        }
        // LC_ALL wins over LANG, the way the C library reads them.
        let h = host(
            &[
                ("TERM", "xterm-256color"),
                ("LC_ALL", "en_US.UTF-8"),
                ("LANG", "C"),
            ],
            false,
        );
        assert_eq!(h.characters, GlyphSet::Unicode);
        // The BSD consoles and plain ANSI read one byte as one character.
        for term in ["cons25", "ansi"] {
            let h = host(&[("TERM", term), ("LANG", "en_US.UTF-8")], false);
            assert_eq!(
                (h.characters, h.colors),
                (GlyphSet::Ascii, Colors::Ansi16),
                "{term}"
            );
        }
    }

    #[test]
    fn the_setting_wins_over_what_was_found() {
        let linux = host(&[("TERM", "linux")], false);
        assert_eq!(Colors::Auto.resolve(&linux), Colors::Ansi16);
        assert_eq!(Colors::Truecolor.resolve(&linux), Colors::Truecolor);
        assert_eq!(GlyphSet::Auto.resolve(&linux), GlyphSet::Console);
        assert_eq!(GlyphSet::Unicode.resolve(&linux), GlyphSet::Unicode);
        for value in ["auto", "truecolor", "256", "16", "none"] {
            let c = Colors::parse(value).expect(value);
            assert_eq!(c.name(), value, "round trips");
        }
        assert_eq!(Colors::parse("24bit"), Some(Colors::Truecolor));
        assert_eq!(Colors::parse("lots"), None);
    }
}
