//! The project map, drawn in the terminal.
//!
//! Nodes are laid out once with a force simulation and then stay put — a
//! settled picture you can read, rather than an animation that never stops
//! moving. Each file is drawn as a box with its name in it, and the
//! connections between them as lines, the way anyone would draw this on paper.
//!
//! Navigation is spatial: the arrow keys move to the nearest file in that
//! direction, so the graph is something you walk around rather than a list
//! that happens to be drawn as dots.
//!
//! # Division of labour
//!
//! `graph` decides *what connects to what*; this module decides *where it sits
//! and what you can do with it*; `ui::draw_map` turns that into pixels. The
//! `Graph` itself is never mutated after construction — filtering happens by
//! asking [`ProjectMap::node_visible`] and [`ProjectMap::edge_visible`] at draw
//! time, so toggling an edge kind is instant and reversible.
//!
//! # Why the layout is not animated
//!
//! Force-directed graphs usually animate, and the animation is usually the
//! worst part of them: things drift while you are trying to read a label.
//! [`ProjectMap::layout`] runs the simulation to completion in one go and then
//! stops. The result is a still picture. `r` re-runs it, and since the seed is
//! fixed the same project always settles the same way.
//!
//! This also fits the event loop, which blocks on input and has no tick — an
//! animation would need a timer thread and a redraw signal that do not exist.
//!
//! # Coordinates
//!
//! Positions are in an abstract space centred on the origin, with **y running
//! upwards** like a graph and unlike a terminal. It is why `Up` calls
//! `move_towards(0.0, 1.0)` and looks backwards at first glance.
//!
//! [`ProjectMap::place`] is what turns that continuous space into the whole
//! character cells a box can actually occupy. It is kept here, next to the
//! layout it depends on, rather than in `ui`: it is geometry, it is worth
//! testing on its own, and `ui` only needs the answer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::graph::{self, Edge, EdgeKind, Graph, Node};

/// What a keypress asked the app to do.
///
/// The view handles its own navigation but cannot open files or close itself —
/// both need `App`. Returning an intent keeps this module free of any
/// dependency on application state.
pub enum Action {
    /// Handled internally; nothing for `App` to do.
    None,
    /// Leave the map and go back to the tree.
    Close,
    /// Open this file in the editor, closing the map.
    Open(PathBuf),
}

/// The link graph plus everything about how it is currently being looked at.
pub struct ProjectMap {
    /// Built once and never modified. All filtering is applied at read time.
    pub graph: Graph,
    /// Laid-out position of each node, parallel to `graph.nodes`.
    pub pos: Vec<(f64, f64)>,
    /// Index of the node under the cursor. May point at something currently
    /// filtered out, which is why [`ProjectMap::selected_node`] checks
    /// visibility and [`ProjectMap::ensure_selection`] repairs it.
    pub selected: usize,
    /// Which edge kinds to draw: wikilink, link, import, call. Indexed by
    /// [`kind_index`], and bound to the `1`-`4` keys.
    pub kinds: [bool; 4],
    /// Show files with no visible connections. Off by default, because a big
    /// project's unconnected files crowd out the structure you came to see.
    pub show_orphans: bool,
    /// Substring matched against each node's relative path, case-insensitively.
    /// Empty means no filtering.
    pub filter: String,
    /// True while the filter box has the keyboard.
    pub filtering: bool,
}

/// Position of an edge kind in [`ProjectMap::kinds`], and in the `1`-`4` key
/// bindings. `pub` because `ui` needs it to draw the toggle row.
pub fn kind_index(kind: EdgeKind) -> usize {
    match kind {
        EdgeKind::Wikilink => 0,
        EdgeKind::Link => 1,
        EdgeKind::Import => 2,
        EdgeKind::Call => 3,
    }
}

/// Where one file's box goes on the character grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    /// Index into `Graph::nodes`.
    pub node: usize,
    /// Column of the box's left edge, and row of its top edge.
    pub col: u16,
    pub row: u16,
    /// Including both borders, so the box spans `col .. col + width`.
    pub width: u16,
}

impl Placed {
    /// Boxes are always three rows: a top border, the name, a bottom border.
    pub const HEIGHT: u16 = 3;

    /// The row the name sits on — where a line into this box should arrive.
    pub fn middle(&self) -> u16 {
        self.row + 1
    }

    /// The column the box is centred on, for lines leaving top or bottom.
    pub fn centre(&self) -> u16 {
        self.col + self.width / 2
    }

    pub fn right(&self) -> u16 {
        self.col + self.width
    }

    pub fn bottom(&self) -> u16 {
        self.row + Self::HEIGHT
    }
}

/// Longest file name drawn inside a box before it is shortened.
///
/// Every slot on the grid is the same size, so one very long name would
/// otherwise set the column width for the whole picture and fit half as many
/// files on screen.
pub const MAX_LABEL: usize = 18;

/// The name as it appears inside its box, shortened from the front if need be.
///
/// The front is what goes: `…ent/reducers.js` tells you more than
/// `components/red…`, because the end of a file name is the part that differs.
pub fn label_of(name: &str) -> String {
    let count = name.chars().count();
    if count <= MAX_LABEL {
        return name.to_string();
    }
    let tail: String = name.chars().skip(count - (MAX_LABEL - 1)).collect();
    format!("…{tail}")
}

impl ProjectMap {
    /// Build the graph, lay it out, and put the cursor somewhere sensible.
    ///
    /// Everything expensive happens here, synchronously — this is what `w`
    /// costs.
    pub fn build(root: &Path, opts: &graph::Options) -> Self {
        let graph = graph::build(root, opts);
        let mut view = Self {
            pos: vec![(0.0, 0.0); graph.nodes.len()],
            graph,
            selected: 0,
            kinds: [true; 4],
            show_orphans: false,
            filter: String::new(),
            filtering: false,
        };
        view.layout();
        view.select_first_visible();
        view
    }

    // ---- what is on screen ------------------------------------------------

    /// Whether a node is currently drawn.
    ///
    /// Two independent tests: it must match the filter, and — unless orphans
    /// are shown — it must touch at least one edge of a kind still switched
    /// on. That second condition is why turning off `call` can make whole
    /// files disappear, not just the lines between them.
    ///
    /// Scans every edge, and is called per node, so this is quadratic. Fine at
    /// the scale a terminal can draw; the place to start if a huge project
    /// ever feels sluggish.
    pub fn node_visible(&self, i: usize) -> bool {
        let Some(n) = self.graph.nodes.get(i) else {
            return false;
        };
        if !self.filter.is_empty() && !n.rel.to_lowercase().contains(&self.filter.to_lowercase()) {
            return false;
        }
        if self.show_orphans {
            return true;
        }
        // A file counts as connected only through an edge kind still shown.
        self.graph
            .edges
            .iter()
            .any(|e| self.kind_on(e) && (e.from == i || e.to == i))
    }

    /// Whether this edge's kind is switched on, ignoring everything else.
    fn kind_on(&self, e: &Edge) -> bool {
        self.kinds[kind_index(e.kind)]
    }

    /// An edge is drawn only when its kind is on *and* both endpoints are
    /// visible — otherwise a filter would leave lines dangling into nothing.
    pub fn edge_visible(&self, e: &Edge) -> bool {
        self.kind_on(e) && self.node_visible(e.from) && self.node_visible(e.to)
    }

    /// Every drawn node, in index order. Also the order `Tab` steps through.
    pub fn visible_indices(&self) -> Vec<usize> {
        (0..self.graph.nodes.len())
            .filter(|i| self.node_visible(*i))
            .collect()
    }

    /// The node under the cursor, or `None` when the cursor is on something
    /// currently filtered out.
    pub fn selected_node(&self) -> Option<&Node> {
        let i = self.selected;
        self.node_visible(i).then(|| self.graph.nodes.get(i))?
    }

    /// Everything connected to `i` by a visible edge, in either direction.
    /// Drives label priority and highlighting: neighbours are drawn brighter
    /// than the rest of the graph.
    pub fn neighbours(&self, i: usize) -> HashSet<usize> {
        let mut out = HashSet::new();
        for e in &self.graph.edges {
            if !self.edge_visible(e) {
                continue;
            }
            if e.from == i {
                out.insert(e.to);
            } else if e.to == i {
                out.insert(e.from);
            }
        }
        out
    }

    /// Edges touching a node, split into the ones it points at and the ones
    /// that point at it.
    pub fn connections(&self, i: usize) -> (Vec<&Edge>, Vec<&Edge>) {
        let mut out = Vec::new();
        let mut incoming = Vec::new();
        for e in &self.graph.edges {
            if !self.edge_visible(e) {
                continue;
            }
            if e.from == i {
                out.push(e);
            } else if e.to == i {
                incoming.push(e);
            }
        }
        (out, incoming)
    }

    // ---- layout -----------------------------------------------------------

    /// Settle the graph with a force simulation: nodes push apart, edges pull
    /// together, and everything drifts gently towards the middle.
    ///
    /// A standard spring-electrical model, run to a stop rather than animated:
    ///
    /// - **Repulsion** — every pair of nodes pushes apart with an inverse-square
    ///   force, so nothing overlaps. This is the O(n²) part, capped by ignoring
    ///   pairs further apart than 800 units.
    /// - **Springs** — each edge pulls its endpoints towards `REST` distance,
    ///   which is what gathers connected files into clusters.
    /// - **Centring** — a weak pull towards the origin stops disconnected
    ///   components drifting off to infinity, since nothing else attracts them.
    /// - **Damping and cooling** — velocity is scaled by `DAMP` each step and
    ///   `heat` decays, so motion converges instead of oscillating.
    ///
    /// The constants are tuned for *reading*, not compactness: `REST` is large
    /// because each dot needs room for a filename beside it, not just for the
    /// dot.
    ///
    /// Two details that matter more than they look:
    ///
    /// - Nodes start on a **ring**, not at a point. From a single point,
    ///   repulsion alone takes an enormous number of iterations to untangle.
    /// - The jitter comes from a seeded [`Lcg`], so layout is **deterministic**.
    ///   The same project always draws the same way, which is the difference
    ///   between a picture you can learn and one you have to re-read.
    ///
    /// Iteration count falls as the graph grows: a rough layout of 500 files
    /// beats a slow one, and the cost per iteration is already quadratic.
    /// Snap the settled layout onto whole character cells, as boxes that do
    /// not overlap.
    ///
    /// The force simulation puts connected files near each other, but in
    /// continuous space, where boxes three rows tall would sit on top of one
    /// another. This lays a coarse grid of same-sized slots over the picture
    /// and moves each box to the nearest free one — which keeps the clustering
    /// the simulation found while guaranteeing that every box is readable and
    /// every gap between two of them is a real gap.
    ///
    /// `order` decides who gets their first choice; see the loop below.
    pub fn place(&self, width: u16, height: u16, order: &[usize]) -> Vec<Placed> {
        let visible = self.visible_indices();
        if visible.is_empty() || width < 6 || height < Placed::HEIGHT {
            return Vec::new();
        }

        // One slot size for everything, so the grid is a grid. The widest name
        // on screen sets it, plus two borders and a two-column gutter for the
        // lines to run down.
        let widest = visible
            .iter()
            .map(|i| label_of(&self.graph.nodes[*i].name).chars().count())
            .max()
            .unwrap_or(1);
        // Two columns and two rows of gutter around every box. One is not
        // enough: a line travelling past a box would run flush against its
        // border, and the eye reads that as the box having grown a tail.
        let slot_w = (widest + 6).max(8) as u16;
        let slot_h = Placed::HEIGHT + 2;
        let cols = (width / slot_w).max(1);
        let rows = (height / slot_h).max(1);

        // The raw extent of the settled layout, not `bounds` — the padding
        // that gives the old dot picture some air is dead space here, where a
        // wasted column is a whole box that did not fit.
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &i in &visible {
            let (x, y) = self.pos[i];
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        let (dx, dy) = ((x1 - x0).max(1e-6), (y1 - y0).max(1e-6));

        let mut taken = vec![false; (cols as usize) * (rows as usize)];
        let mut out = Vec::new();
        // `order` is most-wanted first, so the selected file and its
        // neighbours get their preferred slot and everything else fits around
        // them. A file with nowhere left to go is simply not drawn.
        for &i in order {
            let (px, py) = self.pos[i];
            let want_c = (((px - x0) / dx) * (cols - 1) as f64).round() as i64;
            let want_r = (((y1 - py) / dy) * (rows - 1) as f64).round() as i64;
            let Some((c, r)) = nearest_free(&taken, cols, rows, want_c, want_r) else {
                continue;
            };
            taken[r as usize * cols as usize + c as usize] = true;
            let name = label_of(&self.graph.nodes[i].name);
            out.push(Placed {
                node: i,
                col: c * slot_w,
                row: r * slot_h,
                width: name.chars().count() as u16 + 2,
            });
        }
        out
    }

    pub fn layout(&mut self) {
        let n = self.graph.nodes.len();
        if n == 0 {
            return;
        }
        // Tuned for reading rather than compactness: nodes need room for a
        // filename beside them, not just for the dot.
        const REPULSION: f64 = 16_000.0;
        const SPRING: f64 = 0.013;
        const REST: f64 = 170.0;
        const DAMP: f64 = 0.86;
        const CENTER: f64 = 0.0016;

        // Start on a ring rather than a point, which repulsion alone takes a
        // very long time to untangle. Deterministic, so the same project
        // always lays out the same way.
        let radius = 60.0 + (n as f64) * 0.7;
        let mut rng = Lcg::new(0x5eed_1234);
        for (i, p) in self.pos.iter_mut().enumerate() {
            let a = (i as f64 / n as f64) * std::f64::consts::TAU;
            *p = (
                a.cos() * radius + rng.jitter(),
                a.sin() * radius + rng.jitter(),
            );
        }
        if n == 1 {
            self.pos[0] = (0.0, 0.0);
            return;
        }

        // Big graphs get fewer passes: the cost is quadratic in nodes, and a
        // rough layout of 500 files beats a slow one.
        let iterations = (20_000 / n).clamp(80, 400);
        let mut vel = vec![(0.0f64, 0.0f64); n];
        let mut heat = 1.0f64;

        for _ in 0..iterations {
            for i in 0..n {
                for j in (i + 1)..n {
                    let (mut dx, mut dy) =
                        (self.pos[i].0 - self.pos[j].0, self.pos[i].1 - self.pos[j].1);
                    let mut d2 = dx * dx + dy * dy;
                    if d2 < 1.0 {
                        // Exactly coincident: nudge them apart deterministically.
                        d2 = 1.0;
                        dx = rng.jitter();
                        dy = rng.jitter();
                    }
                    if d2 > 640_000.0 {
                        continue; // far enough to ignore
                    }
                    let d = d2.sqrt();
                    let f = REPULSION / d2;
                    let (fx, fy) = (dx / d * f, dy / d * f);
                    vel[i].0 += fx;
                    vel[i].1 += fy;
                    vel[j].0 -= fx;
                    vel[j].1 -= fy;
                }
            }

            for e in &self.graph.edges {
                let (a, b) = (e.from, e.to);
                let (dx, dy) = (self.pos[b].0 - self.pos[a].0, self.pos[b].1 - self.pos[a].1);
                let d = dx.hypot(dy).max(0.001);
                let f = (d - REST) * SPRING;
                let (fx, fy) = (dx / d * f, dy / d * f);
                vel[a].0 += fx;
                vel[a].1 += fy;
                vel[b].0 -= fx;
                vel[b].1 -= fy;
            }

            for (i, v) in vel.iter_mut().enumerate() {
                v.0 += -self.pos[i].0 * CENTER;
                v.1 += -self.pos[i].1 * CENTER;
                v.0 *= DAMP;
                v.1 *= DAMP;
                self.pos[i].0 += v.0 * heat;
                self.pos[i].1 += v.1 * heat;
            }
            heat *= 0.995;
        }
    }

    // ---- moving around ----------------------------------------------------

    /// Park the cursor on the first drawn node, used at build time and
    /// whenever a filter change strands it.
    fn select_first_visible(&mut self) {
        if let Some(i) = self.visible_indices().first() {
            self.selected = *i;
        }
    }

    /// Move to the nearest visible node in a direction.
    ///
    /// `(dx, dy)` is a unit vector. Each candidate is decomposed into distance
    /// *along* that direction and distance *across* it; anything with a
    /// non-positive `along` is behind the cursor and skipped. The score
    /// `along + across * 2.0` weights sideways displacement double, so the
    /// cursor prefers something straight ahead over something closer but well
    /// off-axis — otherwise arrow keys feel like they teleport.
    fn move_towards(&mut self, dx: f64, dy: f64) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if !self.node_visible(self.selected) {
            self.selected = visible[0];
            return;
        }
        let (sx, sy) = self.pos[self.selected];
        let mut best: Option<(f64, usize)> = None;
        for i in visible {
            if i == self.selected {
                continue;
            }
            let (x, y) = self.pos[i];
            let (ox, oy) = (x - sx, y - sy);
            let along = ox * dx + oy * dy;
            if along <= 0.0 {
                continue; // behind us
            }
            let across = (ox * dy - oy * dx).abs();
            // Prefer straight ahead, then near. Weighting the sideways part
            // stops the cursor leaping across the graph to a distant node
            // that happens to be a degree off the axis.
            let score = along + across * 2.0;
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, i));
            }
        }
        if let Some((_, i)) = best {
            self.selected = i;
        }
    }

    /// `Tab` / `Shift+Tab`: step through every visible node in index order,
    /// wrapping. The exhaustive alternative to spatial movement, for when a
    /// node is not reachable by arrow keys.
    fn cycle(&mut self, forward: bool) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let at = visible.iter().position(|i| *i == self.selected);
        let next = match (at, forward) {
            (Some(p), true) => (p + 1) % visible.len(),
            (Some(p), false) => (p + visible.len() - 1) % visible.len(),
            (None, _) => 0,
        };
        self.selected = visible[next];
    }

    // ---- keys -------------------------------------------------------------

    /// Handle one keypress and say what, if anything, `App` should do.
    ///
    /// Ctrl chords are refused outright and passed back as `None`, so global
    /// bindings keep their meaning here rather than being eaten as graph keys.
    /// While the filter box is open every key goes to it instead.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::None;
        }
        if self.filtering {
            return self.on_filter_key(key);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') => return Action::Close,
            KeyCode::Enter => {
                if let Some(n) = self.selected_node() {
                    return Action::Open(n.path.clone());
                }
            }
            KeyCode::Left | KeyCode::Char('j') => self.move_towards(-1.0, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.move_towards(1.0, 0.0),
            // Screen coordinates run downwards; the layout runs upwards.
            KeyCode::Up | KeyCode::Char('i') => self.move_towards(0.0, 1.0),
            KeyCode::Down | KeyCode::Char('k') => self.move_towards(0.0, -1.0),
            KeyCode::Tab => self.cycle(true),
            KeyCode::BackTab => self.cycle(false),
            KeyCode::Char('/') => {
                self.filtering = true;
                self.filter.clear();
            }
            KeyCode::Char(c @ '1'..='4') => {
                let i = c as usize - '1' as usize;
                self.kinds[i] = !self.kinds[i];
                self.ensure_selection();
            }
            KeyCode::Char('o') => {
                self.show_orphans = !self.show_orphans;
                self.ensure_selection();
            }
            KeyCode::Char('r') => {
                self.layout();
            }
            _ => {}
        }
        Action::None
    }

    /// Keys for the `/` filter box. Esc clears and closes; Enter keeps the
    /// filter but hands the keyboard back to navigation.
    fn on_filter_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.filtering = false;
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => {}
        }
        self.ensure_selection();
        Action::None
    }

    /// Keep the cursor on something that is actually drawn.
    fn ensure_selection(&mut self) {
        if !self.node_visible(self.selected) {
            self.select_first_visible();
        }
    }

    /// One-line summary for the pane title.
    pub fn summary(&self) -> String {
        let shown = self.visible_indices().len();
        let links = self
            .graph
            .edges
            .iter()
            .filter(|e| self.edge_visible(e))
            .count();
        let hidden = self.graph.nodes.len() - shown;
        let mut out = format!("{shown}/{} files | {links} links", self.graph.nodes.len());
        if hidden > 0 {
            out.push_str(&format!(" | {hidden} hidden"));
        }
        out
    }

    /// Files that link to nothing and are linked from nothing, whether they
    /// are being shown or not. Tells you whether `o` has anything to reveal.
    pub fn orphan_count(&self) -> usize {
        self.graph.orphans
    }

    /// Languages whose calls can be followed, for the line beside the toggles.
    pub fn languages(&self) -> &[String] {
        &self.graph.languages
    }
}

/// A tiny linear congruential generator, so layouts are reproducible and
/// there is no random-number dependency.
///
/// Deliberately not a good PRNG — it only has to break ties between coincident
/// nodes and scatter the starting ring slightly. Determinism is the feature;
/// statistical quality is irrelevant here.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0
    }
    /// A small offset in roughly [-1, 1].
    fn jitter(&mut self) -> f64 {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// The free slot closest to `(want_c, want_r)`, searched in rings outwards.
///
/// Ring by ring rather than by true distance: it is close enough to look right,
/// and it stops at the first hit instead of scoring every slot on the grid.
/// `None` when the whole grid is full.
fn nearest_free(
    taken: &[bool],
    cols: u16,
    rows: u16,
    want_c: i64,
    want_r: i64,
) -> Option<(u16, u16)> {
    let free = |c: i64, r: i64| -> bool {
        c >= 0
            && r >= 0
            && c < cols as i64
            && r < rows as i64
            && !taken[r as usize * cols as usize + c as usize]
    };
    if free(want_c, want_r) {
        return Some((want_c as u16, want_r as u16));
    }
    let reach = (cols.max(rows)) as i64;
    for ring in 1..=reach {
        for dr in -ring..=ring {
            for dc in -ring..=ring {
                // Only the edge of the ring; the inside was covered already.
                if dr.abs() != ring && dc.abs() != ring {
                    continue;
                }
                if free(want_c + dc, want_r + dr) {
                    return Some(((want_c + dc) as u16, (want_r + dr) as u16));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;
    use std::fs;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// Two notes that link to each other, two code files that call across,
    /// and one file connected to nothing.
    fn fixture() -> (tempfile::TempDir, ProjectMap) {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "notes/design.md", "see [[architecture]]\n");
        write(td.path(), "notes/architecture.md", "back to [[design]]\n");
        write(td.path(), "src/utils.py", "def load():\n    return 1\n");
        write(td.path(), "src/main.py", "import utils\nutils.load()\n");
        write(td.path(), "notes/alone.md", "nothing points here\n");
        let view = ProjectMap::build(td.path(), &graph::Options::default());
        (td, view)
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

    fn rel_of(view: &ProjectMap, i: usize) -> &str {
        &view.graph.nodes[i].rel
    }

    fn visible_names(view: &ProjectMap) -> Vec<&str> {
        view.visible_indices()
            .into_iter()
            .map(|i| rel_of(view, i))
            .collect()
    }

    // ---- layout -----------------------------------------------------------

    // ---- placing boxes ----------------------------------------------------

    /// Everything visible, most-wanted first, the way `ui` asks for it.
    fn order_of(view: &ProjectMap) -> Vec<usize> {
        view.visible_indices()
    }

    #[test]
    fn placed_boxes_never_overlap_and_stay_on_screen() {
        let (_td, mut view) = fixture();
        view.show_orphans = true;
        let (w, h) = (80u16, 30u16);
        let placed = view.place(w, h, &order_of(&view));
        assert!(!placed.is_empty(), "something should fit in 80x30");

        for p in &placed {
            assert!(p.right() <= w, "{p:?} runs off the right of {w}");
            assert!(p.bottom() <= h, "{p:?} runs off the bottom of {h}");
        }
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                let apart = a.right() <= b.col
                    || b.right() <= a.col
                    || a.bottom() <= b.row
                    || b.bottom() <= a.row;
                assert!(apart, "{a:?} and {b:?} are on top of each other");
            }
        }
    }

    #[test]
    fn a_box_is_wide_enough_for_the_name_it_holds() {
        let (_td, view) = fixture();
        for p in view.place(80, 30, &order_of(&view)) {
            let name = label_of(&view.graph.nodes[p.node].name);
            assert_eq!(
                p.width as usize,
                name.chars().count() + 2,
                "two borders and the name"
            );
        }
    }

    #[test]
    fn the_file_asked_for_first_is_the_one_that_fits() {
        let (_td, mut view) = fixture();
        view.show_orphans = true;
        // A pane with room for a single box: whoever leads the order gets it.
        let wanted = view.visible_indices()[2];
        let mut order = vec![wanted];
        order.extend(view.visible_indices().into_iter().filter(|i| *i != wanted));
        let placed = view.place(20, 5, &order);
        assert_eq!(placed.len(), 1, "only one slot");
        assert_eq!(placed[0].node, wanted);
    }

    #[test]
    fn a_pane_too_small_for_any_box_places_nothing() {
        let (_td, view) = fixture();
        assert!(view.place(4, 2, &order_of(&view)).is_empty());
    }

    #[test]
    fn a_long_name_is_shortened_from_the_front() {
        assert_eq!(label_of("short.md"), "short.md");
        let long = "a-very-long-file-name-indeed.md";
        let cut = label_of(long);
        assert_eq!(cut.chars().count(), MAX_LABEL);
        assert!(cut.starts_with('…'), "{cut}");
        assert!(
            cut.ends_with("indeed.md"),
            "the end is the part that differs"
        );
    }

    #[test]
    fn every_node_gets_a_position_and_none_are_stacked() {
        let (_td, view) = fixture();
        assert_eq!(view.pos.len(), view.graph.nodes.len());
        assert!(view.pos.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        for i in 0..view.pos.len() {
            for j in (i + 1)..view.pos.len() {
                let d = (view.pos[i].0 - view.pos[j].0).hypot(view.pos[i].1 - view.pos[j].1);
                assert!(
                    d > 1.0,
                    "{} and {} landed on top of each other",
                    rel_of(&view, i),
                    rel_of(&view, j)
                );
            }
        }
    }

    #[test]
    fn the_layout_is_the_same_every_time() {
        let (_td, a) = fixture();
        // Laying the same graph out a second time must not move anything.
        let mut b = ProjectMap {
            pos: vec![(0.0, 0.0); a.graph.nodes.len()],
            graph: a.graph.clone(),
            selected: 0,
            kinds: [true; 4],
            show_orphans: false,
            filter: String::new(),
            filtering: false,
        };
        b.layout();
        for (i, (p, q)) in a.pos.iter().zip(b.pos.iter()).enumerate() {
            assert!(
                (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9,
                "node {i} moved between runs"
            );
        }
    }

    #[test]
    fn linked_files_end_up_nearer_than_unlinked_ones() {
        let (_td, mut view) = fixture();
        view.show_orphans = true;
        let find = |rel: &str| view.graph.nodes.iter().position(|n| n.rel == rel).unwrap();
        let dist = |a: usize, b: usize| {
            (view.pos[a].0 - view.pos[b].0).hypot(view.pos[a].1 - view.pos[b].1)
        };
        let design = find("notes/design.md");
        let arch = find("notes/architecture.md");
        let alone = find("notes/alone.md");
        assert!(
            dist(design, arch) < dist(design, alone),
            "the spring between linked notes should pull them together"
        );
    }

    #[test]
    fn a_single_file_project_lays_out_without_trouble() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "only.md", "alone\n");
        let mut view = ProjectMap::build(td.path(), &graph::Options::default());
        assert_eq!(view.pos.len(), 1);
        view.show_orphans = true;
        // One node has no extent to scale against; it still gets a box.
        let placed = view.place(60, 20, &view.visible_indices());
        assert_eq!(placed.len(), 1);
        assert!(placed[0].right() <= 60 && placed[0].bottom() <= 20);
    }

    #[test]
    fn an_empty_project_does_not_panic() {
        let td = tempfile::tempdir().unwrap();
        let view = ProjectMap::build(td.path(), &graph::Options::default());
        assert!(view.graph.nodes.is_empty());
        assert!(view.visible_indices().is_empty());
        assert!(view.place(60, 20, &[]).is_empty());
        let _ = view.summary();
    }

    // ---- what is shown ----------------------------------------------------

    #[test]
    fn unconnected_files_are_hidden_until_asked_for() {
        let (_td, mut view) = fixture();
        assert!(!visible_names(&view).contains(&"notes/alone.md"));
        view.show_orphans = true;
        assert!(visible_names(&view).contains(&"notes/alone.md"));
    }

    #[test]
    fn turning_off_an_edge_kind_hides_what_only_it_connected() {
        let (_td, mut view) = fixture();
        assert!(visible_names(&view).contains(&"notes/design.md"));

        view.kinds[kind_index(EdgeKind::Wikilink)] = false;
        let shown = visible_names(&view);
        assert!(
            !shown.contains(&"notes/design.md"),
            "the notes were only held on by wikilinks: {shown:?}"
        );
        assert!(shown.contains(&"src/main.py"), "the code is still linked");
    }

    #[test]
    fn the_filter_narrows_to_matching_paths() {
        let (_td, mut view) = fixture();
        view.filter = "src/".into();
        let shown = visible_names(&view);
        assert!(shown.iter().all(|r| r.starts_with("src/")), "{shown:?}");
        assert!(!shown.is_empty());
    }

    #[test]
    fn the_summary_counts_what_is_actually_drawn() {
        let (_td, mut view) = fixture();
        assert!(view.summary().contains("/5 files"), "{}", view.summary());
        view.show_orphans = true;
        assert!(view.summary().starts_with("5/5"), "{}", view.summary());
    }

    #[test]
    fn connections_are_split_into_outgoing_and_incoming() {
        let (_td, view) = fixture();
        let main = view
            .graph
            .nodes
            .iter()
            .position(|n| n.rel == "src/main.py")
            .unwrap();
        let (out, incoming) = view.connections(main);
        assert!(!out.is_empty(), "main.py imports and calls utils");
        assert!(incoming.is_empty(), "nothing points at main.py");
        assert!(out.iter().all(|e| e.from == main));
    }

    // ---- moving around ----------------------------------------------------

    #[test]
    fn arrows_move_to_the_nearest_file_in_that_direction() {
        let (_td, mut view) = fixture();
        view.show_orphans = true;
        // Put two nodes in known places and check the cursor goes the right way.
        view.selected = 0;
        view.pos[0] = (0.0, 0.0);
        view.pos[1] = (100.0, 0.0);
        view.pos[2] = (-100.0, 0.0);
        view.pos[3] = (0.0, 100.0);
        view.pos[4] = (0.0, -100.0);

        view.on_key(k(KeyCode::Right));
        assert_eq!(view.selected, 1);
        view.selected = 0;
        view.on_key(k(KeyCode::Left));
        assert_eq!(view.selected, 2);
        view.selected = 0;
        view.on_key(k(KeyCode::Up));
        assert_eq!(view.selected, 3, "up is up on screen");
        view.selected = 0;
        view.on_key(k(KeyCode::Down));
        assert_eq!(view.selected, 4);
    }

    #[test]
    fn moving_towards_nothing_leaves_the_cursor_alone() {
        let (_td, mut view) = fixture();
        view.show_orphans = true;
        view.selected = 0;
        view.pos = vec![
            (0.0, 0.0),
            (-50.0, 0.0),
            (-60.0, 0.0),
            (-70.0, 0.0),
            (-80.0, 0.0),
        ];
        view.on_key(k(KeyCode::Right));
        assert_eq!(view.selected, 0, "everything is to the left");
    }

    #[test]
    fn tab_cycles_through_every_visible_file_and_wraps() {
        let (_td, mut view) = fixture();
        let visible = view.visible_indices();
        view.selected = visible[0];
        for _ in 0..visible.len() {
            view.on_key(k(KeyCode::Tab));
        }
        assert_eq!(view.selected, visible[0], "a full lap comes back round");
    }

    #[test]
    fn the_cursor_never_sits_on_something_hidden() {
        let (_td, mut view) = fixture();
        view.show_orphans = true;
        let alone = view
            .graph
            .nodes
            .iter()
            .position(|n| n.rel == "notes/alone.md")
            .unwrap();
        view.selected = alone;

        view.on_key(ch('o')); // hide unconnected files again
        assert!(!view.show_orphans);
        assert!(
            view.node_visible(view.selected),
            "the cursor should have moved to something still drawn"
        );
    }

    // ---- keys -------------------------------------------------------------

    #[test]
    fn escape_and_w_both_close_the_graph() {
        for key in [k(KeyCode::Esc), ch('m'), ch('q')] {
            let (_td, mut view) = fixture();
            assert!(matches!(view.on_key(key), Action::Close));
        }
    }

    #[test]
    fn enter_asks_for_the_selected_file_to_be_opened() {
        let (td, mut view) = fixture();
        view.selected = view.visible_indices()[0];
        let want = view.graph.nodes[view.selected].path.clone();
        match view.on_key(k(KeyCode::Enter)) {
            Action::Open(p) => {
                assert_eq!(p, want);
                assert!(p.starts_with(td.path()));
            }
            _ => panic!("Enter should open the file"),
        }
    }

    #[test]
    fn the_number_keys_toggle_edge_kinds() {
        let (_td, mut view) = fixture();
        assert!(view.kinds.iter().all(|k| *k));
        view.on_key(ch('3'));
        assert!(!view.kinds[kind_index(EdgeKind::Import)]);
        view.on_key(ch('3'));
        assert!(view.kinds[kind_index(EdgeKind::Import)]);
    }

    #[test]
    fn slash_opens_a_filter_that_takes_typing_and_escape_clears_it() {
        let (_td, mut view) = fixture();
        view.on_key(ch('/'));
        assert!(view.filtering);
        for c in "src".chars() {
            view.on_key(ch(c));
        }
        assert_eq!(view.filter, "src");
        assert!(visible_names(&view).iter().all(|r| r.contains("src")));

        view.on_key(k(KeyCode::Backspace));
        assert_eq!(view.filter, "sr");
        view.on_key(k(KeyCode::Esc));
        assert!(!view.filtering);
        assert!(view.filter.is_empty(), "escape clears the filter");
    }

    #[test]
    fn keys_that_mean_something_elsewhere_do_not_close_the_graph() {
        let (_td, mut view) = fixture();
        // `q` closes, but only when it is not being typed into the filter.
        view.on_key(ch('/'));
        assert!(matches!(view.on_key(ch('q')), Action::None));
        assert_eq!(view.filter, "q");
    }

    #[test]
    fn control_chords_are_ignored_rather_than_acted_on() {
        let (_td, mut view) = fixture();
        let ctrl_c = KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..ch('c')
        };
        assert!(matches!(view.on_key(ctrl_c), Action::None));
    }

    #[test]
    fn r_lays_the_graph_out_again_to_the_same_place() {
        let (_td, mut view) = fixture();
        let before = view.pos.clone();
        view.on_key(ch('r'));
        for (a, b) in before.iter().zip(view.pos.iter()) {
            assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        }
    }
}
