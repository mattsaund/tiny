//! The finished frame, in what the terminal can show.
//!
//! Everything that draws does so in whatever color and style it likes: the
//! theme, the colors a grammar picked for code, the map's two directions,
//! faint text for what matters less. This is the one place that knows what
//! the terminal can show (see [`crate::config::host`]) and it brings every
//! cell of the finished frame down to that, so nothing that draws has to
//! know.
//!
//! Down, never up: a color the terminal has is left as it is, a 24-bit one
//! becomes the nearest of xterm's 256 or of the sixteen, and with no colors
//! at all every cell takes the terminal's own. Weight, underline and reverse
//! video are not colors and stay, which is what keeps a frame with no color
//! readable: tiny's own look says everything with them.
//!
//! # Said another way, not dropped
//!
//! Faint and dark gray are tiny's two ways of saying "this matters less",
//! and a terminal that lacks one of them has the other. Windows' own console
//! draws faint text at full strength, so there it becomes dark gray; asked
//! for no color, dark gray has nothing to be, so it becomes faint.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use crate::config::Colors;

/// Bring every cell in `buf` down to `colors`, on a terminal that does or
/// does not draw `faint` text.
pub(super) fn fit(buf: &mut Buffer, colors: Colors, faint: bool) {
    if matches!(colors, Colors::Truecolor | Colors::Auto) && faint {
        return;
    }
    for cell in &mut buf.content {
        if colors == Colors::None {
            if faint && cell.fg == Color::DarkGray {
                cell.modifier.insert(Modifier::DIM);
            }
        } else if !faint && cell.modifier.contains(Modifier::DIM) {
            cell.modifier.remove(Modifier::DIM);
            if cell.fg == Color::Reset {
                cell.fg = Color::DarkGray;
            }
        }
        cell.fg = fit_color(cell.fg, colors);
        cell.bg = fit_color(cell.bg, colors);
        cell.underline_color = fit_color(cell.underline_color, colors);
    }
}

/// One color, brought down to `colors`.
fn fit_color(color: Color, colors: Colors) -> Color {
    match (colors, color) {
        (Colors::None, _) => Color::Reset,
        (Colors::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256([r, g, b])),
        (Colors::Ansi16, Color::Rgb(r, g, b)) => nearest_16([r, g, b]),
        (Colors::Ansi16, Color::Indexed(n)) => match SIXTEEN.get(n as usize) {
            Some((named, _)) => *named,
            None => nearest_16(rgb_of_256(n)),
        },
        (_, other) => other,
    }
}

/// The sixteen, as xterm draws them. Every terminal draws them a little
/// differently, which is why they are only ever a last resort for a color
/// that has to come down this far.
const SIXTEEN: [(Color, [u8; 3]); 16] = [
    (Color::Black, [0, 0, 0]),
    (Color::Red, [205, 0, 0]),
    (Color::Green, [0, 205, 0]),
    (Color::Yellow, [205, 205, 0]),
    (Color::Blue, [0, 0, 238]),
    (Color::Magenta, [205, 0, 205]),
    (Color::Cyan, [0, 205, 205]),
    (Color::Gray, [229, 229, 229]),
    (Color::DarkGray, [127, 127, 127]),
    (Color::LightRed, [255, 0, 0]),
    (Color::LightGreen, [0, 255, 0]),
    (Color::LightYellow, [255, 255, 0]),
    (Color::LightBlue, [92, 92, 255]),
    (Color::LightMagenta, [255, 0, 255]),
    (Color::LightCyan, [0, 255, 255]),
    (Color::White, [255, 255, 255]),
];

/// The levels of each channel in xterm's 6 by 6 by 6 color cube.
const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// How far apart two colors look, roughly: green counts most and blue more
/// than red, the way eyes weigh them.
fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    let d = |i: usize| (i32::from(a[i]) - i32::from(b[i])).unsigned_abs();
    2 * d(0).pow(2) + 4 * d(1).pow(2) + 3 * d(2).pow(2)
}

/// The nearest of xterm's 256: of its color cube and its gray ramp, never
/// the sixteen at the start, which every terminal draws differently.
fn nearest_256(c: [u8; 3]) -> u8 {
    let step = |v: u8| {
        (0..STEPS.len())
            .min_by_key(|&i| (i32::from(STEPS[i]) - i32::from(v)).unsigned_abs())
            .unwrap_or(0)
    };
    let (r, g, b) = (step(c[0]), step(c[1]), step(c[2]));
    let cube = (16 + 36 * r + 6 * g + b) as u8;
    let cube_rgb = [STEPS[r], STEPS[g], STEPS[b]];
    // The ramp's 24 grays run from 8 to 238, ten apart.
    let mean = (u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2])) / 3;
    let i = (mean.saturating_sub(3) / 10).min(23) as u8;
    let level = 8 + 10 * i;
    if distance(c, [level; 3]) < distance(c, cube_rgb) {
        232 + i
    } else {
        cube
    }
}

/// The color one of xterm's 256 stands for.
fn rgb_of_256(n: u8) -> [u8; 3] {
    match n {
        0..=15 => SIXTEEN[n as usize].1,
        16..=231 => {
            let n = (n - 16) as usize;
            [STEPS[n / 36], STEPS[(n / 6) % 6], STEPS[n % 6]]
        }
        _ => [8 + 10 * (n - 232); 3],
    }
}

/// The nearest of the sixteen.
fn nearest_16(c: [u8; 3]) -> Color {
    SIXTEEN
        .iter()
        .min_by_key(|(_, rgb)| distance(c, *rgb))
        .map_or(Color::Reset, |(named, _)| *named)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn a_24_bit_color_comes_down_to_the_nearest_of_256() {
        assert_eq!(nearest_256([255, 0, 0]), 196, "the cube's red");
        assert_eq!(nearest_256([0, 0, 0]), 16, "the cube's black");
        assert_eq!(nearest_256([128, 128, 128]), 244, "a gray from the ramp");
        assert_eq!(nearest_256([95, 135, 175]), 67, "exactly in the cube");
        for n in 16..=255u8 {
            assert_eq!(nearest_256(rgb_of_256(n)), n, "each of the 256 is itself");
        }
    }

    #[test]
    fn and_to_the_nearest_of_sixteen() {
        assert_eq!(nearest_16([250, 10, 10]), Color::LightRed);
        assert_eq!(nearest_16([190, 20, 10]), Color::Red);
        // base16-ocean's text and its comments.
        assert_eq!(nearest_16([192, 197, 206]), Color::Gray);
        assert_eq!(nearest_16([101, 115, 126]), Color::DarkGray);
        assert_eq!(
            fit_color(Color::Indexed(9), Colors::Ansi16),
            Color::LightRed
        );
        assert_eq!(
            fit_color(Color::Indexed(196), Colors::Ansi16),
            Color::LightRed
        );
    }

    #[test]
    fn colors_the_terminal_has_are_left_alone() {
        for colors in [Colors::Ansi256, Colors::Ansi16] {
            for c in [Color::Reset, Color::Green, Color::DarkGray] {
                assert_eq!(fit_color(c, colors), c);
            }
        }
        assert_eq!(
            fit_color(Color::Indexed(67), Colors::Ansi256),
            Color::Indexed(67)
        );
        assert_eq!(
            fit_color(Color::Rgb(1, 2, 3), Colors::Truecolor),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn faint_is_dark_gray_where_there_is_no_faint_and_the_other_way_round() {
        use ratatui::style::Style;
        let faint = Style::new().add_modifier(Modifier::DIM);
        let quiet = Style::new().fg(Color::DarkGray);
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.set_string(0, 0, "a", faint);
        buf.set_string(1, 0, "b", quiet);
        // Windows' own console: every color, no faint.
        fit(&mut buf, Colors::Truecolor, false);
        let a = &buf.content[0];
        assert_eq!(a.fg, Color::DarkGray);
        assert!(!a.modifier.contains(Modifier::DIM));

        // No color asked for, on a terminal that has faint.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.set_string(0, 0, "a", faint);
        buf.set_string(1, 0, "b", quiet);
        fit(&mut buf, Colors::None, true);
        let b = &buf.content[1];
        assert_eq!(b.fg, Color::Reset);
        assert!(b.modifier.contains(Modifier::DIM), "dark gray became faint");
        assert!(buf.content[0].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn with_none_every_cell_takes_the_terminals_own_and_keeps_its_weight() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        let style = Style::new()
            .fg(Color::Rgb(10, 200, 30))
            .bg(Color::Blue)
            .add_modifier(ratatui::style::Modifier::BOLD);
        buf.set_string(0, 0, "ab", style);
        fit(&mut buf, Colors::None, true);
        let cell = &buf.content[0];
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset));
        assert!(cell.modifier.contains(ratatui::style::Modifier::BOLD));
    }
}
