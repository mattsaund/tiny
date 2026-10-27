//! The panes beside the browser: as many as fit, side by side and stacked,
//! in any mix.
//!
//! `Ctrl+W` moves the keyboard to the next pane, and with only one, splits
//! it in two. `*split` puts a new pane beside the one you are in, `*split
//! down` one below it, and `*close` closes it. `*close others` is back to
//! one.
//!
//! # One live pane, and the rest set aside
//!
//! Everything tiny knows about showing a file describes one pane: the preview
//! that follows the browser's cursor, the buffer and its cursor, a rendered
//! note and its scroll. Rather than teach all of that about many, a split
//! keeps exactly one pane live: the one with the keyboard, which is also the
//! one the browser opens files into. Each of the others is a [`Pane`], what
//! that pane showed and where, set aside. It is drawn by lending it the live
//! state for one draw (see `ui::draw_pinned`), and moving to another pane
//! swaps the two.
//!
//! So every pane behaves exactly as the single one always did, because there
//! is only ever one of each thing doing the behaving. It is also what lets one
//! file sit in several panes, each with its own place in it: a buffer's
//! cursor, scroll and selection are a [`View`], and each pane keeps its own.
//!
//! # The layout
//!
//! A tree, [`Layout`]: each pane a leaf, and each split a row of panes side
//! by side or a column of them stacked, each with its share of the space.
//! The shares start equal, and dragging the line between two panes moves it.
//! Splitting a pane the same way as the row or column it is in adds one to
//! that row or column, so three panes side by side are thirds rather than a
//! half and two quarters; once a line has been dragged, the new pane halves
//! the one it came from and the rest stay as they were. Splitting it the
//! other way makes it a row or column of its own.
//!
//! # Opening and closing
//!
//! A new pane shows the file you are in, twice: the new one takes the
//! keyboard and the old one keeps your place, so the first thing you do in
//! the new pane, whether that is scrolling or opening another file from the
//! browser, leaves the old view where it was. Closing a pane hands the
//! keyboard to the one before it.

use ratatui::layout::Rect;

use crate::config::keys::Action;
use crate::text::editor::View;
use crate::text::highlight::Resume;

use super::App;
use super::mode::{Focus, Window};
use super::parts::display_name;
use super::preview::Preview;

/// The narrowest a pane may be split into. Below this a pane is too narrow
/// to read a line of code in, so a split is refused rather than drawn.
pub const MIN_PANE_WIDTH: u16 = 24;

/// The shortest a pane may be split into: a border, and a few lines of text.
pub const MIN_PANE_HEIGHT: u16 = 6;

/// The main window, split.
pub struct Split {
    /// How the panes are arranged.
    pub layout: Layout,
    /// Which pane is live: the one with the keyboard.
    pub live: usize,
    /// Every other pane, as it was left, by its number in the layout.
    pub pinned: Vec<(usize, Pane)>,
    /// The number the next new pane gets.
    next: usize,
}

/// How the panes are arranged, see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// One pane, by number.
    Pane(usize),
    /// Panes side by side, or with `stacked`, one above another: `parts[i]`
    /// takes `shares[i]` out of all the shares of the space.
    Split {
        stacked: bool,
        parts: Vec<Layout>,
        shares: Vec<u32>,
    },
}

/// The line between two neighbors in a row or column of panes, as
/// [`Layout::areas`] laid them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLine {
    /// Which row or column: the part to go into at each level from the top.
    pub path: Vec<usize>,
    /// The first of the two, the line being after it.
    pub first: usize,
    /// Where the row or column is.
    pub area: Rect,
    /// Whether it is a column, so the line runs across and moves up and
    /// down.
    pub stacked: bool,
    /// Where the second of the two begins.
    pub at: u16,
    /// The first and the last place it may begin, leaving each of the two
    /// room for its panes.
    pub range: (u16, u16),
}

/// `area` cut into a row of parts, or with `stacked` a column of them, each
/// as long as its share says. Where each cut falls is rounded, rather than
/// each length on its own, so the parts always fill `area` exactly.
fn cut(area: Rect, stacked: bool, shares: &[u32]) -> Vec<Rect> {
    let total = shares.iter().map(|&s| u64::from(s)).sum::<u64>().max(1);
    let len = u64::from(if stacked { area.height } else { area.width });
    let mut sum = 0;
    let mut from = 0;
    shares
        .iter()
        .map(|&s| {
            sum += u64::from(s);
            let to = ((len * sum + total / 2) / total) as u16;
            let part = if stacked {
                Rect::new(area.x, area.y + from, area.width, to - from)
            } else {
                Rect::new(area.x + from, area.y, to - from, area.height)
            };
            from = to;
            part
        })
        .collect()
}

/// The same shares in their smallest terms, so they cannot grow without end.
fn tidy(shares: &mut [u32]) {
    let gcd = |mut a: u32, mut b: u32| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let d = shares.iter().fold(0, |d, &s| gcd(d, s)).max(1);
    shares.iter_mut().for_each(|s| *s /= d);
}

impl Layout {
    /// Every pane, in reading order: across, then down.
    pub fn panes(&self) -> Vec<usize> {
        match self {
            Layout::Pane(id) => vec![*id],
            Layout::Split { parts, .. } => parts.iter().flat_map(Layout::panes).collect(),
        }
    }

    /// Put pane `new` after pane `id`, beside it or below it.
    fn split(&mut self, id: usize, new: usize, stacked: bool) {
        match self {
            Layout::Pane(here) if *here == id => {
                *self = Layout::Split {
                    stacked,
                    parts: vec![Layout::Pane(id), Layout::Pane(new)],
                    shares: vec![1, 1],
                };
            }
            Layout::Pane(_) => {}
            Layout::Split {
                stacked: way,
                parts,
                shares,
            } => {
                let at = parts.iter().position(|p| *p == Layout::Pane(id));
                match at {
                    Some(i) if *way == stacked => {
                        // Equal shares stay equal. Otherwise the new pane
                        // halves the one it came from, and the rest keep
                        // what they were dragged to.
                        if shares.iter().any(|&s| s != shares[0]) {
                            shares.iter_mut().for_each(|s| *s *= 2);
                            shares[i] /= 2;
                        }
                        parts.insert(i + 1, Layout::Pane(new));
                        shares.insert(i + 1, shares[i]);
                        tidy(shares);
                    }
                    _ => parts.iter_mut().for_each(|p| p.split(id, new, stacked)),
                }
            }
        }
    }

    /// Take pane `id` out, and any row or column it leaves with one pane in
    /// it, or inside another going the same way. What it gave up goes to the
    /// rest of its row or column, each keeping its share of what is left.
    fn remove(&mut self, id: usize) {
        let Layout::Split {
            stacked,
            parts,
            shares,
        } = self
        else {
            return;
        };
        if let Some(i) = parts.iter().position(|p| *p == Layout::Pane(id)) {
            parts.remove(i);
            shares.remove(i);
        }
        for part in parts.iter_mut() {
            part.remove(id);
        }
        // A row inside a row is one row, and each of its panes keeps the
        // part of the width it had.
        let way = *stacked;
        let total: f64 = shares.iter().map(|&s| f64::from(s)).sum();
        let mut flat = Vec::new();
        for (part, share) in std::mem::take(parts).into_iter().zip(shares.iter()) {
            let of = f64::from(*share) / total;
            match part {
                Layout::Split {
                    stacked,
                    parts: inner,
                    shares: inside,
                } if stacked == way => {
                    let sum: f64 = inside.iter().map(|&s| f64::from(s)).sum();
                    for (p, s) in inner.into_iter().zip(inside) {
                        flat.push((p, of * f64::from(s) / sum));
                    }
                }
                other => flat.push((other, of)),
            }
        }
        let (flat, mut new): (Vec<Layout>, Vec<u32>) = flat
            .into_iter()
            .map(|(p, of)| (p, ((of * 10_000.0).round() as u32).max(1)))
            .unzip();
        tidy(&mut new);
        *parts = flat;
        *shares = new;
        if parts.len() == 1 {
            *self = parts.remove(0);
        }
    }

    /// Where each pane goes in `area`, each row or column shared out as its
    /// shares say.
    pub fn areas(&self, area: Rect) -> Vec<(usize, Rect)> {
        match self {
            Layout::Pane(id) => vec![(*id, area)],
            Layout::Split {
                stacked,
                parts,
                shares,
            } => parts
                .iter()
                .zip(cut(area, *stacked, shares))
                .flat_map(|(part, cell)| part.areas(cell))
                .collect(),
        }
    }

    /// The least room this needs across, or with `stacked` up and down, to
    /// give every pane in it at least `least` of it.
    pub fn least(&self, stacked: bool, least: u16) -> u16 {
        match self {
            Layout::Pane(_) => least,
            Layout::Split {
                stacked: way,
                parts,
                ..
            } => {
                let each = parts.iter().map(|p| p.least(stacked, least));
                if *way == stacked {
                    each.sum()
                } else {
                    each.max().unwrap_or(least)
                }
            }
        }
    }

    /// Every line between two neighbors, as [`Layout::areas`] lays them out
    /// in `area`, each kept from leaving a pane narrower than `width` or
    /// shorter than `height`.
    pub fn lines(&self, area: Rect, width: u16, height: u16) -> Vec<PaneLine> {
        let mut out = Vec::new();
        self.lines_into(area, (width, height), &mut Vec::new(), &mut out);
        out
    }

    fn lines_into(
        &self,
        area: Rect,
        least: (u16, u16),
        path: &mut Vec<usize>,
        out: &mut Vec<PaneLine>,
    ) {
        let Layout::Split {
            stacked,
            parts,
            shares,
        } = self
        else {
            return;
        };
        let cells = cut(area, *stacked, shares);
        let min = if *stacked { least.1 } else { least.0 };
        // Where a part begins and ends, along the way the line moves.
        let span = |r: &Rect| {
            if *stacked {
                (r.y, r.bottom())
            } else {
                (r.x, r.right())
            }
        };
        for k in 0..parts.len() - 1 {
            let (begins, _) = span(&cells[k]);
            let (at, ends) = span(&cells[k + 1]);
            let lo = begins + parts[k].least(*stacked, min);
            let hi = ends.saturating_sub(parts[k + 1].least(*stacked, min));
            out.push(PaneLine {
                path: path.clone(),
                first: k,
                area,
                stacked: *stacked,
                at,
                // A window too small for the panes it has leaves them where
                // they are rather than making them smaller still.
                range: (lo.min(at), hi.max(at)),
            });
        }
        for (k, (part, cell)) in parts.iter().zip(cells).enumerate() {
            path.push(k);
            part.lines_into(cell, least, path, out);
            path.pop();
        }
    }

    /// Move a line [`Layout::lines`] gave: the second of its two panes now
    /// begins at `to`, and the shares are the lengths that leaves.
    pub fn move_line(&mut self, line: &PaneLine, to: u16) {
        let mut node = self;
        for &k in &line.path {
            let Layout::Split { parts, .. } = node else {
                return;
            };
            let Some(part) = parts.get_mut(k) else {
                return;
            };
            node = part;
        }
        let Layout::Split {
            stacked, shares, ..
        } = node
        else {
            return;
        };
        if line.first + 1 >= shares.len() {
            return;
        }
        let cells = cut(line.area, *stacked, shares);
        let (start, end) = if *stacked {
            (cells[line.first].y, cells[line.first + 1].bottom())
        } else {
            (cells[line.first].x, cells[line.first + 1].right())
        };
        let to = to.clamp(start, end);
        let mut lengths: Vec<u32> = cells
            .iter()
            .map(|r| u32::from(if *stacked { r.height } else { r.width }))
            .collect();
        lengths[line.first] = u32::from(to - start);
        lengths[line.first + 1] = u32::from(end - to);
        // A share of nothing would be a pane that could never be given room
        // again by sharing; the ranges keep it from coming to that.
        lengths.iter_mut().for_each(|l| *l = (*l).max(1));
        tidy(&mut lengths);
        *shares = lengths;
    }
}

/// A pane without the keyboard: what it showed, and where.
pub struct Pane {
    pub preview: Preview,
    /// How far down a rendered note was scrolled.
    pub preview_scroll: usize,
    /// Where in the file the pane was, when it shows a file with a buffer.
    pub view: Option<View>,
    /// Its own saved highlighting, so files drawn every frame do not throw
    /// each other's away and each start again from the top.
    pub highlight: Resume,
}

impl App {
    /// `Ctrl+W`: the keyboard to the next pane, or with one, a second.
    ///
    /// Panes belong to the main window, so from anywhere else the key brings
    /// you to them: to the ones that are there, or to two new ones.
    pub(super) fn next_pane(&mut self) {
        if self.window != Window::Main {
            self.show_window(Window::Main);
            if self.split.is_some() {
                self.status = format!("{} panes", self.pane_count());
                return;
            }
        }
        let Some(split) = &self.split else {
            return self.split_pane(false);
        };
        let order = split.layout.panes();
        let at = order.iter().position(|id| *id == split.live).unwrap_or(0);
        let next = order[(at + 1) % order.len()];
        self.focus_pane(next);
    }

    /// How many panes there are.
    pub fn pane_count(&self) -> usize {
        self.split.as_ref().map_or(1, |s| s.pinned.len() + 1)
    }

    /// `*split`, or `*split down`: a new pane beside the one you are in, or
    /// below it, showing the same thing. It takes the keyboard.
    pub(super) fn split_pane(&mut self, stacked: bool) {
        if self.window != Window::Main {
            self.show_window(Window::Main);
        }
        let room = self.last_live_area;
        let too_small = if stacked {
            room.height / 2 < MIN_PANE_HEIGHT
        } else {
            room.width / 2 < MIN_PANE_WIDTH
        };
        if too_small {
            let way = if stacked { "short" } else { "narrow" };
            self.status = format!("this pane is too {way} to split");
            return;
        }
        // Set aside a copy of where you are; the live pane carries on from
        // the same place in the new one.
        let copy = Pane {
            preview: self.preview.clone(),
            preview_scroll: self.preview_scroll,
            view: self.active_buffer().map(|ed| ed.view()),
            highlight: Resume::default(),
        };
        let split = self.split.get_or_insert_with(|| Split {
            layout: Layout::Pane(0),
            live: 0,
            pinned: Vec::new(),
            next: 1,
        });
        let new = split.next;
        split.next += 1;
        split.layout.split(split.live, new, stacked);
        split.pinned.push((split.live, copy));
        split.live = new;
        self.status = format!(
            "{} panes, {} moves between them",
            self.pane_count(),
            self.key_name(Action::NextPane),
        );
    }

    /// `*close`: the pane with the keyboard goes, and the one before it takes
    /// the keyboard.
    pub(super) fn close_pane(&mut self) {
        let Some(split) = &mut self.split else {
            self.status = "only one pane".into();
            return;
        };
        let order = split.layout.panes();
        let at = order.iter().position(|id| *id == split.live).unwrap_or(0);
        let heir = if at == 0 { order[1] } else { order[at - 1] };
        let closing = split.live;
        split.layout.remove(closing);
        let Some(i) = split.pinned.iter().position(|(id, _)| *id == heir) else {
            return;
        };
        let (_, pane) = split.pinned.remove(i);
        split.live = heir;
        if split.pinned.is_empty() {
            self.split = None;
        }
        self.take_up(pane);
        self.status = format!("closed a pane, {} left", self.pane_count());
    }

    /// `*close others`: back to one pane, the one you are in.
    pub(super) fn close_other_panes(&mut self) {
        if self.split.take().is_none() {
            self.status = "only one pane".into();
            return;
        }
        self.status = "one pane".into();
    }

    /// The keyboard to pane `id`.
    pub(super) fn focus_pane(&mut self, id: usize) {
        let Some(split) = &mut self.split else {
            return;
        };
        if split.live != id {
            let Some(i) = split.pinned.iter().position(|(p, _)| *p == id) else {
                return;
            };
            let (_, arriving) = split.pinned.remove(i);
            let leaving_id = split.live;
            split.live = id;
            // The view comes from the buffer the live pane was showing, so it
            // is the first field: fields are worked out in the order written,
            // and the next one takes the preview that says which buffer that
            // is.
            let leaving = Pane {
                view: self.active_buffer().map(|ed| ed.view()),
                preview: std::mem::take(&mut self.preview),
                preview_scroll: self.preview_scroll,
                highlight: std::mem::take(&mut self.highlight_cache),
            };
            if let Some(split) = &mut self.split {
                split.pinned.push((leaving_id, leaving));
            }
            self.take_up(arriving);
        }
        // Straight to the pane, not through `focus_editor`: on a picture that
        // would open it in another program, and moving between panes is not
        // asking to open anything.
        self.focus = Focus::Editor;
        self.status = match self.preview.path() {
            Some(path) => display_name(path),
            None => "an empty pane".into(),
        };
    }

    /// Make a pane set aside the live one again: the browser's cursor goes to
    /// its file, and the file comes back where that pane left it.
    fn take_up(&mut self, pane: Pane) {
        let found = match pane.preview.path() {
            Some(path) => {
                let path = path.to_path_buf();
                self.reveal(&path)
            }
            None => false,
        };
        if !found {
            // Gone since, or never a file: show it as it was rather than
            // something the browser happens to be resting on.
            self.preview = pane.preview;
        }
        self.preview_scroll = pane.preview_scroll;
        self.highlight_cache = pane.highlight;
        if let (Some(view), Some(ed)) = (pane.view, self.active_buffer_mut()) {
            ed.set_view(view);
            // The cursor has jumped to another pane's place, which ends a run
            // of typing like any other move: one file open in two panes must
            // not undo an edit in one along with an edit in the other.
            ed.break_undo_group();
        }
    }

    /// Keep the panes without the keyboard showing something real.
    ///
    /// A pane's file can change under it while it is set aside: a delete, or
    /// a file that stops being text, drops its buffer. The pane then reads
    /// the file again the way the live pane would, or says it is gone, rather
    /// than drawing an empty box under a name. Called from `sync_preview`,
    /// which everything that changes the tree goes through.
    pub(super) fn sync_pinned(&mut self) {
        let Some(split) = &self.split else {
            return;
        };
        // Which panes need their file read again, and which are showing
        // one that has gone, worked out before any reading is done.
        let mut read_again = Vec::new();
        let mut gone = Vec::new();
        for (id, pane) in &split.pinned {
            match &pane.preview {
                Preview::Buffer { path, .. } if self.buffers.contains_key(path) => {}
                Preview::Buffer { path, .. } if path.exists() => {
                    read_again.push((*id, path.clone()));
                }
                other => {
                    if let Some(path) = other.path().filter(|p| !p.exists()) {
                        gone.push((*id, format!("{} is gone", display_name(path))));
                    }
                }
            }
        }
        let fresh = read_again
            .into_iter()
            .map(|(id, path)| (id, self.load_file_preview(&path)))
            .collect::<Vec<_>>();
        let gone = gone
            .into_iter()
            .map(|(id, why)| (id, Preview::Unreadable(why)));
        for (id, preview) in fresh.into_iter().chain(gone) {
            if let Some(pane) = self.pinned_mut(id) {
                pane.preview = preview;
                pane.highlight = Resume::default();
            }
        }
    }

    /// The pane set aside as number `id`.
    pub(super) fn pinned_mut(&mut self, id: usize) -> Option<&mut Pane> {
        self.split
            .as_mut()?
            .pinned
            .iter_mut()
            .find(|(p, _)| *p == id)
            .map(|(_, pane)| pane)
    }

    /// One notch of the wheel over pane `id`, which does not have the
    /// keyboard.
    ///
    /// Moves that pane and nothing else. A file with a buffer moves by its
    /// cursor, the way the live editor does, so the view cannot be left
    /// somewhere the cursor would snap it back from; anything else scrolls.
    pub(super) fn scroll_pinned(&mut self, id: usize, down: bool) {
        let Some(pane) = self.pinned_mut(id) else {
            return;
        };
        // A note set aside is drawn rendered, which scrolls by its own count
        // and has no cursor to move. Drawing clamps both.
        let rendered = matches!(&pane.preview, Preview::Buffer { kind, .. } if kind.reads_first());
        match (&mut pane.view, rendered) {
            (Some(view), false) => {
                view.cursor_line = if down {
                    view.cursor_line + 1
                } else {
                    view.cursor_line.saturating_sub(1)
                };
            }
            _ => {
                pane.preview_scroll = if down {
                    pane.preview_scroll + 1
                } else {
                    pane.preview_scroll.saturating_sub(1)
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(parts: Vec<Layout>) -> Layout {
        let shares = vec![1; parts.len()];
        Layout::Split {
            stacked: false,
            parts,
            shares,
        }
    }

    fn column(parts: Vec<Layout>) -> Layout {
        let shares = vec![1; parts.len()];
        Layout::Split {
            stacked: true,
            parts,
            shares,
        }
    }

    fn widths(layout: &Layout, area: Rect) -> Vec<u16> {
        layout.areas(area).iter().map(|(_, r)| r.width).collect()
    }

    #[test]
    fn splitting_the_same_way_adds_to_the_row() {
        let mut layout = Layout::Pane(0);
        layout.split(0, 1, false);
        layout.split(1, 2, false);
        assert_eq!(
            layout,
            row(vec![Layout::Pane(0), Layout::Pane(1), Layout::Pane(2)])
        );
        let widths: Vec<u16> = layout
            .areas(Rect::new(0, 0, 90, 10))
            .iter()
            .map(|(_, r)| r.width)
            .collect();
        assert_eq!(widths, [30, 30, 30], "thirds");
    }

    #[test]
    fn splitting_the_other_way_makes_a_column_in_the_row() {
        let mut layout = Layout::Pane(0);
        layout.split(0, 1, false);
        layout.split(1, 2, true);
        assert_eq!(
            layout,
            row(vec![
                Layout::Pane(0),
                column(vec![Layout::Pane(1), Layout::Pane(2)])
            ])
        );
        assert_eq!(layout.panes(), [0, 1, 2], "reading order");
        let areas = layout.areas(Rect::new(0, 0, 80, 20));
        assert_eq!(areas[0].1, Rect::new(0, 0, 40, 20));
        assert_eq!(areas[1].1, Rect::new(40, 0, 40, 10));
        assert_eq!(areas[2].1, Rect::new(40, 10, 40, 10));
    }

    #[test]
    fn closing_a_pane_tidies_what_it_leaves() {
        let mut layout = row(vec![
            Layout::Pane(0),
            column(vec![
                Layout::Pane(1),
                row(vec![Layout::Pane(2), Layout::Pane(3)]),
            ]),
        ]);
        layout.remove(1);
        assert_eq!(
            layout.panes(),
            [0, 2, 3],
            "a column of one is its pane, and a row in a row is the row"
        );
        assert_eq!(
            widths(&layout, Rect::new(0, 0, 80, 20)),
            [40, 20, 20],
            "and nobody's width changed"
        );
        layout.remove(2);
        layout.remove(3);
        assert_eq!(layout, Layout::Pane(0));
    }

    #[test]
    fn panes_always_fill_the_space_exactly() {
        let layout = row(vec![Layout::Pane(0), Layout::Pane(1), Layout::Pane(2)]);
        for w in 30..200 {
            let areas = layout.areas(Rect::new(3, 0, w, 10));
            assert_eq!(areas[0].1.x, 3);
            for pair in areas.windows(2) {
                assert_eq!(pair[0].1.right(), pair[1].1.x, "no gap at width {w}");
            }
            assert_eq!(areas[2].1.right(), 3 + w, "nothing left over at width {w}");
        }
    }

    #[test]
    fn a_line_moves_where_it_is_dragged_and_no_further_than_leaves_room() {
        let mut layout = Layout::Pane(0);
        layout.split(0, 1, false);
        let area = Rect::new(0, 0, 100, 20);
        let line = layout.lines(area, 24, 6).remove(0);
        assert_eq!((line.at, line.range), (50, (24, 76)));

        layout.move_line(&line, 70);
        assert_eq!(widths(&layout, area), [70, 30]);
        // The proportion is what is kept, so a wider window keeps it.
        assert_eq!(widths(&layout, Rect::new(0, 0, 200, 20)), [140, 60]);

        // A third pane split from the narrow one halves it; the wide one
        // keeps what it was dragged to.
        layout.split(1, 2, false);
        assert_eq!(widths(&layout, area), [70, 15, 15]);
    }

    #[test]
    fn a_line_leaves_room_for_every_pane_beside_it() {
        // A row of two, the second a column with a row of two at its foot:
        // the line between the first and the rest cannot squeeze those two.
        let mut layout = Layout::Pane(0);
        layout.split(0, 1, false);
        layout.split(1, 2, true);
        layout.split(2, 3, false);
        let lines = layout.lines(Rect::new(0, 0, 120, 30), 24, 6);
        let outer = lines.iter().find(|l| l.path.is_empty()).unwrap();
        assert_eq!(outer.range, (24, 120 - 48));
        let column = lines.iter().find(|l| l.stacked).unwrap();
        assert_eq!(column.range, (6, 24));
    }
}
