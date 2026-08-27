//! A grid of box-drawing characters, and the routing that fills it.
//!
//! Terminal line art has one hard problem: two lines that cross have to become
//! a junction glyph, not one line written over the other. [`Ink`] solves it by
//! storing a *direction bitmask* per cell — up, down, left, right — instead of
//! a character. Drawing a line ORs bits in, and the glyph is only chosen at
//! the end, from whatever bits a cell ended up with. A cell that acquired
//! `LEFT | RIGHT | UP` becomes `┴` without anything having to notice that two
//! separate lines met there.
//!
//! # Why lines travel in channels
//!
//! A route that turns at the midpoint between two boxes lands *inside* a third
//! one whenever the two are not neighbours, and since boxes are drawn last the
//! line then runs behind it and reads as broken. So routes are not allowed to
//! turn wherever they like: they turn in [`crate::map::layout::Channels`], the
//! rows and columns that the layout guarantees are clear of every box. Every
//! line out of one file also shares a trunk, so a file with eight connections
//! produces one bundle leaving it rather than eight separate departures.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::config::{Markers, Palette};
use crate::map::layout::{Channels, Placed};

/// How brightly one cell of the graph is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InkStyle {
    /// A folder heading. Not part of the picture — it names the group of
    /// boxes under it — so it is written into the grid rather than over the
    /// finished row, and a line running past it keeps the cells it had.
    Folder,
    /// Everything not connected to the cursor.
    Far,
    /// The file under the cursor, and the lines reaching it.
    Near,
    /// The file under the cursor itself.
    Selected,
}

/// The characters the graph is drawn out of.
///
/// Two sets, chosen by the `markers` setting, so a terminal with no box-drawing
/// characters gets a picture made of `+`, `-` and `|` rather than a screen of
/// replacement glyphs. Boxes use rounded corners where they exist, which keeps
/// them visually separate from the sharp corners a line turns with.
pub(super) struct Glyphs {
    /// Indexed by the direction bits a line leaves a cell by.
    lines: [char; 16],
    corners: [char; 4],
    horizontal: char,
    vertical: char,
}

impl Glyphs {
    pub(super) fn for_markers(markers: Markers) -> Self {
        match markers {
            Markers::Arrows => Self {
                lines: [
                    ' ', '│', '│', '│', '─', '┘', '┐', '┤', '─', '└', '┌', '├', '─', '┴', '┬', '┼',
                ],
                corners: ['╭', '╮', '╰', '╯'],
                horizontal: '─',
                vertical: '│',
            },
            Markers::Ascii => Self {
                lines: [
                    ' ', '|', '|', '|', '-', '+', '+', '+', '-', '+', '+', '+', '-', '+', '+', '+',
                ],
                corners: ['+', '+', '+', '+'],
                horizontal: '-',
                vertical: '|',
            },
        }
    }
}

// Which way a line leaves a cell. Kept as bits so two lines crossing produce
// the right junction on their own, without anyone having to notice they met.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// The character grid the graph is composed on before it becomes a widget.
///
/// Two layers: lines, held as direction bits so crossings merge themselves,
/// and text, which is the boxes and always wins. Composing into a grid rather
/// than emitting spans as we go is what lets an edge be drawn without knowing
/// what else has already been drawn where it lands.
pub(super) struct Ink {
    w: usize,
    h: usize,
    bits: Vec<u8>,
    text: Vec<Option<(char, InkStyle)>>,
}

impl Ink {
    pub(super) fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![0; w * h],
            text: vec![None; w * h],
        }
    }

    /// Bounds-checked index. Everything else goes through this, so a route
    /// that runs off the edge is clipped instead of panicking.
    fn at(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return None;
        }
        Some(y as usize * self.w + x as usize)
    }

    pub(super) fn mark(&mut self, x: i32, y: i32, dirs: u8) {
        if let Some(i) = self.at(x, y) {
            self.bits[i] |= dirs;
        }
    }

    fn put(&mut self, x: i32, y: i32, ch: char, style: InkStyle) {
        if let Some(i) = self.at(x, y) {
            self.text[i] = Some((ch, style));
        }
    }

    /// A run of horizontal line. Endpoints only get the bit pointing along the
    /// run, so a corner formed with a vertical run turns properly.
    pub(super) fn hline(&mut self, y: i32, x1: i32, x2: i32) {
        let (a, b) = (x1.min(x2), x1.max(x2));
        for x in a..=b {
            let mut dirs = 0;
            if x > a {
                dirs |= LEFT;
            }
            if x < b {
                dirs |= RIGHT;
            }
            self.mark(x, y, dirs);
        }
    }

    pub(super) fn vline(&mut self, x: i32, y1: i32, y2: i32) {
        let (a, b) = (y1.min(y2), y1.max(y2));
        for y in a..=b {
            let mut dirs = 0;
            if y > a {
                dirs |= UP;
            }
            if y < b {
                dirs |= DOWN;
            }
            self.mark(x, y, dirs);
        }
    }

    pub(super) fn draw_box(&mut self, p: &Placed, name: &str, style: InkStyle, g: &Glyphs) {
        let (x0, y0) = (p.col as i32, p.row as i32);
        let x1 = x0 + p.width as i32 - 1;
        let y1 = y0 + Placed::HEIGHT as i32 - 1;
        self.put(x0, y0, g.corners[0], style);
        self.put(x1, y0, g.corners[1], style);
        self.put(x0, y1, g.corners[2], style);
        self.put(x1, y1, g.corners[3], style);
        for x in x0 + 1..x1 {
            self.put(x, y0, g.horizontal, style);
            self.put(x, y1, g.horizontal, style);
        }
        self.put(x0, y0 + 1, g.vertical, style);
        self.put(x1, y0 + 1, g.vertical, style);
        for (i, c) in name.chars().enumerate() {
            self.put(x0 + 1 + i as i32, y0 + 1, c, style);
        }
    }

    /// Write a run of text starting at a cell, clipped at the right edge.
    pub(super) fn write(&mut self, x: i32, y: i32, text: &str, style: InkStyle) {
        for (i, c) in text.chars().enumerate() {
            self.put(x + i as i32, y, c, style);
        }
    }

    /// Turn the grid into one `Line` per row, merging neighbouring cells that
    /// share a style so a row is a handful of spans rather than one per column.
    pub(super) fn render(&self, g: &Glyphs, pal: &Palette) -> Vec<Line<'static>> {
        let style_of = |s: InkStyle| match s {
            InkStyle::Selected => pal.text.add_modifier(Modifier::REVERSED),
            InkStyle::Near => pal.text.add_modifier(Modifier::BOLD),
            // Everything the cursor does not touch steps back, so the handful
            // of lines that are drawn have the picture to themselves.
            InkStyle::Far => pal.dim,
            InkStyle::Folder => pal.heading,
        };
        let mut out = Vec::with_capacity(self.h);
        for y in 0..self.h {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_style: Option<Style> = None;
            for x in 0..self.w {
                let i = y * self.w + x;
                let (ch, style) = match self.text[i] {
                    Some((ch, s)) => (ch, style_of(s)),
                    None if self.bits[i] != 0 => (g.lines[self.bits[i] as usize], pal.text),
                    None => (' ', pal.dim),
                };
                if run_style != Some(style) {
                    if !run.is_empty() {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            run_style.unwrap_or(pal.dim),
                        ));
                    }
                    run_style = Some(style);
                }
                run.push(ch);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, run_style.unwrap_or(pal.dim)));
            }
            out.push(Line::from(spans));
        }
        out
    }
}

/// Draw the connection between the file under the cursor and one of its
/// neighbours.
///
/// `a` is always the selected file, whichever way the connection runs. A plain
/// line, with nothing on either end: a mutual connection is the same line as a
/// one-way one, and which way it runs is a question the strip underneath
/// answers in words.
///
/// # Why no arrowheads
///
/// A head is one cell, at the far end of a route that has already turned two
/// corners, and at that size the difference between `▸` and `◂` is a smudge —
/// it reads as debris on the end of the line rather than as a direction.
/// Worse, it is debris in a place where two lines from the same trunk arrive a
/// row apart, so the eye reads a column of them as texture. What the direction
/// is actually *for* — which file calls which — is a thing you look up rather
/// than glance at, and the `in:` and `out:` lists below say it in words that
/// cannot be mistaken for a corner glyph.
///
/// Orthogonal on purpose. A character grid has no diagonals worth the name —
/// a diagonal has to be faked out of dots and reads as a smear rather than as
/// a diagram. Right angles are what anyone drawing this on paper would use,
/// and they land on cell boundaries exactly.
///
/// # One trunk, many branches
///
/// Two rules keep this readable, and both were learned by breaking them.
///
/// **A line only travels in a channel.** The grid leaves two clear rows under
/// every box and a clear column beside every slot ([`Channels`]), and a route
/// made of those is visible along its whole length. Aiming straight at the
/// other box and turning at the halfway point — the obvious approach — puts
/// that corner inside some third box, and since boxes are drawn last and win,
/// what you see is a stub leaving one file and an unrelated stub arriving at
/// another with nothing joining them.
///
/// **Every line out of one file shares one trunk.** The vertical always runs
/// down [`Channels::column_by`] of the *selected* file's slot, never the other
/// one's. So a file with ten neighbours draws one spine with ten branches off
/// it, instead of ten separate routes that cross each other on the way. It is
/// the difference between a diagram and a hatch pattern, and it costs nothing:
/// the trunk is a column no box occupies either way.
///
/// The one shape that skips all of this is two neighbours side by side in the
/// same row, which connect straight across the gap between them. Nothing is in
/// the way, and it is the plainest line on the map.
pub(super) fn route(ink: &mut Ink, a: &Placed, b: &Placed, ch: Channels) {
    if a.col == b.col && a.row == b.row {
        return; // the same slot: nothing to draw between them
    }

    // Side to side, for neighbours in the same row.
    if a.row == b.row && a.col.abs_diff(b.col) == ch.slot_w {
        let y = a.middle() as i32;
        let right = b.col > a.col;
        let (near, far) = if right {
            (a.right() as i32, b.col as i32 - 1)
        } else {
            (a.col as i32 - 1, b.right() as i32)
        };
        ink.hline(y, near, far);
        return;
    }

    // Out of the selected file's right edge, down its own trunk, then along a
    // row channel into the other box's top or bottom.
    let ay = a.middle() as i32;
    let cx = ch.column_by(a.col) as i32;
    let below = b.row > a.row;
    // Above, or level with other boxes in between: either way the line arrives
    // underneath.
    let by = if below {
        b.row as i32 - 1
    } else {
        b.bottom() as i32
    };
    let bx = b.col as i32 + 1;

    ink.hline(ay, a.right() as i32, cx);
    ink.vline(cx, ay, by);
    ink.hline(by, cx, bx);
    // One more bit at the far end, turning it from a run that stops a cell
    // short of the border into a junction that visibly touches the box.
    ink.mark(bx, by, if below { DOWN } else { UP });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- map routing ------------------------------------------------------

    fn placed(node: usize, col: u16, row: u16, width: u16) -> Placed {
        Placed {
            node,
            col,
            row,
            width,
        }
    }

    /// The geometry `place` would produce for boxes this wide.
    fn channels(slot_w: u16) -> Channels {
        Channels {
            slot_w,
            slot_h: Placed::HEIGHT + 2,
            gutter: slot_w - 3,
        }
    }

    /// Whether a cell is inside a box's borders.
    fn in_box(p: &Placed, x: i32, y: i32) -> bool {
        (p.col as i32..p.col as i32 + p.width as i32).contains(&x)
            && (p.row as i32..p.row as i32 + Placed::HEIGHT as i32).contains(&y)
    }

    /// Assert the drawn route has no dangling end: wherever a line points, it
    /// must reach a line pointing back, an arrowhead, or a box.
    ///
    /// This is the property the whole router exists to satisfy. A route that
    /// stops in empty space is one that went behind a box and came out the far
    /// side, and it is what makes the map look like disconnected stubs.
    fn assert_no_loose_ends(ink: &Ink, boxes: &[&Placed]) {
        let dirs = [
            (UP, 0, -1, DOWN),
            (DOWN, 0, 1, UP),
            (LEFT, -1, 0, RIGHT),
            (RIGHT, 1, 0, LEFT),
        ];
        for y in 0..ink.h as i32 {
            for x in 0..ink.w as i32 {
                let Some(i) = ink.at(x, y) else { continue };
                if ink.bits[i] == 0 {
                    continue;
                }
                for (bit, dx, dy, back) in dirs {
                    if ink.bits[i] & bit == 0 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    let Some(n) = ink.at(nx, ny) else { continue };
                    let joined = ink.bits[n] & back != 0
                        || ink.text[n].is_some()
                        || boxes.iter().any(|p| in_box(p, nx, ny));
                    assert!(
                        joined,
                        "the line at ({x},{y}) points at nothing:\n{}",
                        sketch(ink)
                    );
                }
            }
        }
    }

    /// The grid as text, for a failure message.
    fn sketch(ink: &Ink) -> String {
        let g = Glyphs::for_markers(Markers::Arrows);
        (0..ink.h)
            .map(|y| {
                (0..ink.w)
                    .map(|x| match ink.text[y * ink.w + x] {
                        Some((c, _)) => c,
                        None => g.lines[ink.bits[y * ink.w + x] as usize],
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every arrangement two boxes can be in, in slot coordinates.
    fn arrangements() -> Vec<(Placed, Placed, &'static str)> {
        let ch = channels(14);
        let slot = |c: u16, r: u16, w: u16| placed(0, c * ch.slot_w, 1 + r * ch.slot_h, w);
        vec![
            (slot(0, 0, 8), slot(1, 0, 9), "neighbours in a row"),
            (slot(0, 0, 8), slot(3, 0, 9), "same row, far apart"),
            (slot(0, 0, 8), slot(0, 1, 11), "directly below"),
            (slot(0, 2, 8), slot(0, 0, 11), "directly above"),
            (slot(1, 0, 8), slot(0, 2, 11), "down and to the left"),
            (slot(0, 0, 8), slot(3, 2, 9), "down and to the right"),
            (slot(3, 2, 9), slot(0, 0, 8), "up and to the left"),
        ]
    }

    #[test]
    fn a_route_never_leaves_a_loose_end() {
        let ch = channels(14);
        for (a, b, what) in arrangements() {
            let mut ink = Ink::new(90, 40);
            route(&mut ink, &a, &b, ch);
            assert_no_loose_ends(&ink, &[&a, &b]);
            assert!(
                ink.bits.iter().any(|b| *b != 0),
                "{what} should have drawn something"
            );
        }
    }

    /// Whether the drawn route visibly meets this box.
    ///
    /// Orientation matters, and it is the whole point of the check. A `─`
    /// sitting immediately right of a border runs *into* it and reads as
    /// attached; the same `─` on the row above a box runs *past* it, parallel,
    /// and reads as a line that stopped nearby for no reason. So a horizontal
    /// end counts on the left and right, and a vertical end above and below.
    fn touches(ink: &Ink, p: &Placed) -> bool {
        let (x0, y0) = (p.col as i32, p.row as i32);
        let (x1, y1) = (x0 + p.width as i32 - 1, y0 + Placed::HEIGHT as i32 - 1);
        let met = |x: i32, y: i32, along: u8| match ink.at(x, y) {
            Some(i) => ink.text[i].is_some() || ink.bits[i] & along != 0,
            None => false,
        };
        let sides = (y0..=y1).any(|y| met(x0 - 1, y, LEFT | RIGHT) || met(x1 + 1, y, LEFT | RIGHT));
        let ends = (x0..=x1).any(|x| met(x, y0 - 1, UP | DOWN) || met(x, y1 + 1, UP | DOWN));
        sides || ends
    }

    #[test]
    fn a_route_meets_both_of_the_boxes_it_joins() {
        // The complement of `a_route_never_leaves_a_loose_end`: that one says
        // the line does not point at nothing, this one says it actually
        // arrives. A run that stops one cell short of a border satisfies the
        // first and fails the second, and with nothing drawn on the end of a
        // line any more, that gap would be all there was to see.
        let ch = channels(14);
        for (a, b, what) in arrangements() {
            let mut ink = Ink::new(90, 40);
            route(&mut ink, &a, &b, ch);
            assert!(
                touches(&ink, &a),
                "{what}: the line never reaches the first box:\n{}",
                sketch(&ink)
            );
            assert!(
                touches(&ink, &b),
                "{what}: the line never reaches the second box:\n{}",
                sketch(&ink)
            );
        }
    }

    #[test]
    fn a_route_never_passes_through_a_box() {
        let ch = channels(14);
        // A third box in the middle of the grid, of the widest kind, standing
        // in for whatever the layout put between the two being joined.
        for (a, b, what) in arrangements() {
            let mut ink = Ink::new(90, 40);
            route(&mut ink, &a, &b, ch);
            for slot_c in 0..5u16 {
                for slot_r in 0..3u16 {
                    let other =
                        placed(9, slot_c * ch.slot_w, 1 + slot_r * ch.slot_h, ch.slot_w - 4);
                    if other.col == a.col && other.row == a.row {
                        continue;
                    }
                    if other.col == b.col && other.row == b.row {
                        continue;
                    }
                    for y in other.row as i32..other.row as i32 + Placed::HEIGHT as i32 {
                        for x in other.col as i32..other.col as i32 + other.width as i32 {
                            let i = ink.at(x, y).expect("inside the grid");
                            assert_eq!(
                                ink.bits[i],
                                0,
                                "{what}: a line crosses the box at slot ({slot_c},{slot_r}) \
                                 — it would vanish behind it:\n{}",
                                sketch(&ink)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_connection_is_a_plain_line_with_nothing_on_either_end() {
        let ch = channels(14);
        for (a, b, what) in arrangements() {
            let mut ink = Ink::new(90, 40);
            route(&mut ink, &a, &b, ch);
            // `route` puts nothing in the text layer at all — every cell it
            // touches is a line bit, and line bits can only ever render as one
            // of the box-drawing runs or junctions.
            assert!(
                ink.text.iter().all(|t| t.is_none()),
                "{what}: a connection should be line and nothing else:\n{}",
                sketch(&ink)
            );
        }
    }
}
