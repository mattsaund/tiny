//! The project map, drawn in the terminal.
//!
//! Every file in the project is a box with its name in it, and the connections
//! between them are lines, the way anyone would draw this on paper. The boxes
//! are grouped by the folder they live in, under a heading naming it.
//!
//! Folders are not boxes. They are not selectable, nothing connects to one,
//! and opening one does nothing — a folder is *where files are*, and the
//! heading is how the picture says so.
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
//! # Why the layout is not force-directed
//!
//! It used to be. A spring-electrical simulation gathers connected files into
//! clusters, which sounds right and reads badly: a file's position comes from
//! wherever the forces converged, so it moves when an unrelated edge changes,
//! and a project with a densely-called module settles into a hairball with the
//! interesting structure buried in the middle of it.
//!
//! Grouping by folder gives up the clustering and gets back something worth
//! more — a position you can predict. `src/app.rs` is under the `src/`
//! heading, in alphabetical order, on every machine and after every edit.
//!
//! # Coordinates
//!
//! [`ProjectMap::place`] does the whole layout and writes `pos` as it goes, in
//! **grid** units — one per box, not one per character — with **y running
//! upwards**, which is why `Up` calls `move_towards(0.0, 1.0)`. Navigation
//! then walks the picture exactly as it is drawn.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::graph::{self, Edge, EdgeKind, Graph, Node};
use crate::keys::Action;

/// What a keypress asked the app to do. Named for the request rather than
/// the key, since `keys::Action` is now what a key resolves to.
///
/// The view handles its own navigation but cannot open files or close itself —
/// both need `App`. Returning an intent keeps this module free of any
/// dependency on application state.
pub enum Intent {
    /// Handled internally; nothing for `App` to do.
    None,
    /// Leave the map and go back to the tree.
    Close,
    /// Open this file in the editor, closing the map.
    Open(PathBuf),
    /// Read the project again and draw a fresh map of it. The layout is
    /// deterministic, so redrawing the same files is a no-op — what this is
    /// for is picking up files and links that changed on disk.
    Rebuild,
}

/// The link graph plus everything about how it is currently being looked at.
pub struct ProjectMap {
    /// Built once and never modified. All filtering is applied at read time.
    pub graph: Graph,
    /// The project folder the map is of. Nothing above it is ever walked, so
    /// this is the whole extent of the picture.
    root: PathBuf,
    /// Laid-out position of each node, parallel to `graph.nodes`.
    pub pos: Vec<(f64, f64)>,
    /// Index of the node under the cursor. May point at something currently
    /// filtered out, which is why [`ProjectMap::selected_node`] checks
    /// visibility and [`ProjectMap::ensure_selection`] repairs it.
    pub selected: usize,
    /// Which edge kinds to draw: wikilink, link, call. Indexed by
    /// [`kind_index`], and bound to the `1`-`3` keys.
    pub kinds: [bool; 3],
    /// Substring matched against each node's relative path, case-insensitively.
    /// Empty means no filtering.
    pub filter: String,
    /// True while the filter box has the keyboard.
    pub filtering: bool,
}

/// Position of an edge kind in [`ProjectMap::kinds`], and in the `1`-`3` key
/// bindings. `pub` because `ui` needs it to draw the toggle row.
pub fn kind_index(kind: EdgeKind) -> usize {
    match kind {
        EdgeKind::Wikilink => 0,
        EdgeKind::Link => 1,
        EdgeKind::Call => 2,
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

    pub fn right(&self) -> u16 {
        self.col + self.width
    }

    pub fn bottom(&self) -> u16 {
        self.row + Self::HEIGHT
    }
}

/// Which heading a file is filed under.
///
/// The order of the variants is the order the headings appear in, and that is
/// deliberate rather than incidental: `Root` first because a project's front
/// door is its top-level files, then folders by path so `src` is always in the
/// same place, and `Unconnected` last because it is the pile of things the
/// picture has nothing to say about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Root,
    Folder(String),
    /// Files nothing drawn reaches. Kept on screen — a map that omits files is
    /// a map you have to remember the gaps in — but out of the way of the part
    /// that has structure in it.
    Unconnected,
}

/// One folder's heading: the strip of dim text naming the folder whose files
/// sit under it. Folders are not nodes — you cannot select one, and nothing
/// connects to one. They are how the files are arranged, and the heading is
/// what says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Written with a trailing slash, so it cannot be mistaken for a file.
    pub label: String,
    pub row: u16,
    /// How many files sit under it, for the count beside the name.
    pub files: usize,
}

/// One drawn connection between two files, in whichever directions it runs.
/// `a` is always the lower node index, so a pair has exactly one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub a_to_b: bool,
    pub b_to_a: bool,
}

/// Everything `ui` needs to draw one frame of the map.
/// Where a line may travel without ever crossing a box.
///
/// The layout is a grid of slots, and a box sits in the top-left corner of its
/// own. That leaves two kinds of channel no box in any row or column reaches
/// into:
///
/// - the last two **rows** of every slot, clear across the full width;
/// - the last few **columns** of every slot, clear down the full height,
///   because [`Channels::gutter`] is measured from the widest name on screen
///   and not from whichever box happens to be in that slot.
///
/// Routing that stays inside those stays visible. Routing that does not goes
/// behind a box and comes out the far side, which is what makes one line read
/// as two unrelated stubs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Channels {
    pub slot_w: u16,
    pub slot_h: u16,
    /// Column offset within a slot that no box in it reaches.
    pub gutter: u16,
}

impl Channels {
    /// The clear column belonging to whichever slot `col` sits in.
    pub fn column_by(&self, col: u16) -> u16 {
        let w = self.slot_w.max(1);
        col - col % w + self.gutter
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    pub boxes: Vec<Placed>,
    pub folders: Vec<Folder>,
    /// How many visible files did not fit on the pane. Non-zero means the
    /// picture is cut off and the view has to say so rather than looking
    /// complete — moving the cursor is what reaches them.
    pub offscreen: usize,
    /// The clear rows and columns, for routing.
    pub channels: Channels,
}

/// The folder part of a project-relative path, `""` for a file in the root.
/// Always forward-slashed, because `Node::rel` is.
fn folder_of(rel: &str) -> &str {
    match rel.rfind('/') {
        Some(i) => &rel[..i],
        None => "",
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
    /// Build the graph and put the cursor somewhere sensible. Laying it out
    /// is `place`'s job, and happens per frame against the pane it has.
    ///
    /// Everything expensive happens here, synchronously — this is what `w`
    /// costs.
    pub fn build(root: &Path, opts: &graph::Options) -> Self {
        let graph = graph::build(root, opts);
        let mut view = Self {
            pos: vec![(0.0, 0.0); graph.nodes.len()],
            graph,
            root: root.to_path_buf(),
            selected: 0,
            kinds: [true; 3],
            filter: String::new(),
            filtering: false,
        };
        view.select_busiest();
        // `pos` is written by `place`, which only runs when a frame is drawn.
        // Seed it against a nominal pane so the arrow keys mean something if a
        // key ever arrives before the first draw; the first frame overwrites
        // this with the real geometry.
        view.place(100, 30);
        view
    }

    // ---- what is on screen ------------------------------------------------

    /// Whether a node is currently drawn.
    ///
    /// Two independent tests: it must match the filter, and — unless orphans
    /// are shown — it must touch at least one edge of a kind still switched
    /// Whether a file is drawn at all.
    ///
    /// Only the `/` filter can hide one. A file that links to nothing is still
    /// part of the project — it is filed under its own heading rather than
    /// left out, because a map that quietly omits things is a map you cannot
    /// trust to tell you what is there. See [`ProjectMap::connected`].
    pub fn node_visible(&self, i: usize) -> bool {
        let Some(n) = self.graph.nodes.get(i) else {
            return false;
        };
        self.filter.is_empty() || n.rel.to_lowercase().contains(&self.filter.to_lowercase())
    }

    /// Whether any connection reaches this file, in either direction.
    ///
    /// Depends on which edge kinds are switched on, which is the point: turn
    /// `call` off in a code project and most files move to the unconnected
    /// heading, because with calls hidden nothing does connect them.
    ///
    /// Deliberately blind to the `/` filter. A filter narrows what you are
    /// looking at; it does not change what a file is joined to, and filing
    /// every file as unconnected the moment you type into the box would be
    /// telling you something untrue about your project.
    ///
    /// Scans every edge, and is called per node, so this is quadratic. Fine at
    /// the scale a terminal can draw; the place to start if a huge project
    /// ever feels sluggish.
    fn connected(&self, i: usize) -> bool {
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

    /// Lay the visible files out on the pane and say where every box goes.
    ///
    /// Files are grouped by the folder they live in, and the folders run down
    /// the pane in path order — root first, then alphabetically. That is the
    /// whole layout: no simulation, no settling, no seed. A project map is
    /// something you consult, and you consult it by name, so a file's position
    /// should be predictable from the tree you already know rather than from
    /// wherever a force converged.
    ///
    /// Within a folder the order is the walk order, which is alphabetical. It
    /// is not the crossing-minimal order, and that is deliberate: a layout you
    /// can predict beats one with fewer line crossings that moves a file every
    /// time an edge changes.
    ///
    /// `pos` is refreshed here rather than at build time, because it *is* the
    /// picture — the arrow keys move between boxes exactly as they are drawn,
    /// which is why this takes `&mut self`.
    pub fn place(&mut self, width: u16, height: u16) -> Placement {
        let empty = Placement::default();
        let visible = self.visible_indices();
        if visible.is_empty() || width < 6 || height < Placed::HEIGHT {
            return empty;
        }

        // One slot size for everything, so the grid is a grid. The widest name
        // on screen sets it, plus two borders and a two-column gutter for the
        // lines to run down. Two columns and two rows of gutter around every
        // box: one is not enough, because a line travelling past a box would
        // run flush against its border and the eye reads that as the box
        // having grown a tail.
        let widest = visible
            .iter()
            .map(|i| label_of(&self.graph.nodes[*i].name).chars().count())
            .max()
            .unwrap_or(1);
        let slot_w = (widest + 6).max(8) as u16;
        let slot_h = Placed::HEIGHT + 2;
        let cols = (width / slot_w).max(1);
        // Three in from the right of the slot: past `widest + 2`, which is as
        // wide as a box in it can be, and still inside the slot. Measured from
        // the widest name rather than from each box, so the column is clear in
        // every row and a line may run the whole height of it.
        let channels = Channels {
            slot_w,
            slot_h,
            gutter: slot_w - 3,
        };

        // Walk the whole layout first, unbounded by the pane, so scrolling has
        // something to scroll through.
        let mut boxes: Vec<Placed> = Vec::new();
        let mut folders: Vec<Folder> = Vec::new();
        // Which heading each box belongs to, so a heading whose files all fall
        // off the bottom goes with them instead of standing over nothing.
        let mut under: Vec<usize> = Vec::new();
        let groups = self.folder_groups(&visible);
        let mut row: u16 = 0;
        let mut slot_row: usize = 0;

        for (group, members) in &groups {
            // Always headed, even when there is only one: the top heading names
            // the project folder, which is where the map starts and how far it
            // goes. A picture with no heading would not say either.
            folders.push(Folder {
                label: self.group_label(group),
                row,
                files: members.len(),
            });
            // Two rows, not one. The blank is what makes the row above every
            // box a row no heading occupies, which is where lines arriving
            // from above have to land — see the routing in `ui`.
            row += 2;
            for (n, &i) in members.iter().enumerate() {
                let (c, r) = ((n % cols as usize) as u16, (n / cols as usize) as u16);
                let name = label_of(&self.graph.nodes[i].name);
                boxes.push(Placed {
                    node: i,
                    col: c * slot_w,
                    row: row + r * slot_h,
                    width: name.chars().count() as u16 + 2,
                });
                under.push(folders.len() - 1);
                // Grid coordinates, not character ones: navigation should step
                // box to box, and a box is far wider than it is tall.
                self.pos[i] = (c as f64, -((slot_row + r as usize) as f64));
            }
            let used = members.len().div_ceil(cols as usize) as u16;
            // No extra gap: the slot already carries two rows of gutter under
            // its bottom row, which is the space between one folder and the
            // next heading.
            row += used * slot_h;
            slot_row += used as usize + 1;
        }

        // Follow the cursor rather than offering a scroll key: the arrow keys
        // already walk the map, so the view goes where the cursor goes.
        let total = row;
        let wanted = boxes.len();
        let scroll = self.scroll_to(&boxes, height, total);
        let mut kept = vec![false; folders.len()];
        let mut n = 0;
        boxes.retain_mut(|p| {
            let group = under[n];
            n += 1;
            match p.row.checked_sub(scroll) {
                Some(r) if r + Placed::HEIGHT <= height => {
                    p.row = r;
                    kept[group] = true;
                    true
                }
                _ => false,
            }
        });
        let mut group = 0;
        folders.retain_mut(|g| {
            let showing = kept[group];
            group += 1;
            match g.row.checked_sub(scroll) {
                Some(r) if r < height && showing => {
                    g.row = r;
                    true
                }
                _ => false,
            }
        });
        let offscreen = wanted - boxes.len();
        Placement {
            boxes,
            folders,
            offscreen,
            channels,
        }
    }

    /// How far down to shift the picture so the selected box is on screen.
    fn scroll_to(&self, boxes: &[Placed], height: u16, total: u16) -> u16 {
        if total <= height {
            return 0;
        }
        let Some(p) = boxes.iter().find(|p| p.node == self.selected) else {
            return 0;
        };
        // Keep the folder heading above the cursor's row visible where it can
        // be: a box with no folder over it is a box you cannot place.
        p.bottom()
            .saturating_sub(height)
            .min(total.saturating_sub(height))
            .min(p.row.saturating_sub(2))
    }

    /// The visible files, split by heading. Never empty when `visible` is not.
    ///
    /// A file goes under its folder if anything drawn reaches it, and under
    /// [`Group::Unconnected`] otherwise — which means the headings shift as
    /// the `1`-`3` toggles change what counts as a connection. That is the
    /// honest arrangement: the picture should say which files it is actually
    /// telling you something about.
    fn folder_groups(&self, visible: &[usize]) -> Vec<(Group, Vec<usize>)> {
        let mut groups: Vec<(Group, Vec<usize>)> = Vec::new();
        for &i in visible {
            let key = if !self.connected(i) {
                Group::Unconnected
            } else {
                match folder_of(&self.graph.nodes[i].rel) {
                    "" => Group::Root,
                    dir => Group::Folder(dir.to_string()),
                }
            };
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push(i),
                None => groups.push((key, vec![i])),
            }
        }
        // Ordering is the enum's own — see [`Group`].
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        groups
    }

    /// The heading text for a group, which is the only place its name appears.
    fn group_label(&self, group: &Group) -> String {
        match group {
            Group::Root => format!("{}/", self.root_label()),
            Group::Folder(dir) => format!("{dir}/"),
            Group::Unconnected => "unconnected".to_string(),
        }
    }

    /// What to call the project folder itself in a heading.
    fn root_label(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string())
    }

    /// Every drawn connection, with all the edges between a pair of files
    /// merged into one.
    ///
    /// Two files that share a dozen function names are one connection, not a
    /// dozen parallel lines. The lines were the tangle: the graph is keyed per
    /// symbol so that the detail strip can name them, but the picture only has
    /// to say *that* they are connected and which way it runs.
    pub fn links(&self) -> Vec<Link> {
        let mut out: Vec<Link> = Vec::new();
        for e in &self.graph.edges {
            if !self.edge_visible(e) {
                continue;
            }
            let (a, b) = (e.from.min(e.to), e.from.max(e.to));
            let forward = e.from == a;
            match out.iter_mut().find(|l| l.a == a && l.b == b) {
                Some(l) => {
                    l.a_to_b |= forward;
                    l.b_to_a |= !forward;
                }
                None => out.push(Link {
                    a,
                    b,
                    a_to_b: forward,
                    b_to_a: !forward,
                }),
            }
        }
        out
    }

    // ---- moving around ----------------------------------------------------

    /// Park the cursor on the first drawn node, used at build time and
    /// whenever a filter change strands it.
    fn select_first_visible(&mut self) {
        if let Some(i) = self.visible_indices().first() {
            self.selected = *i;
        }
    }

    /// Put the cursor on the file with the most connections.
    ///
    /// Only used when the map is first opened, and only because the map draws
    /// the cursor's connections rather than everyone's: landing on whichever
    /// file happens to sort first would often mean opening on a picture with
    /// no lines in it. The busiest file is the one that says the most about
    /// how a project fits together, so it is the right thing to be looking at
    /// before you have asked for anything in particular.
    fn select_busiest(&mut self) {
        let degree = |i: usize| {
            self.graph
                .edges
                .iter()
                .filter(|e| self.kind_on(e) && (e.from == i || e.to == i))
                .count()
        };
        if let Some(i) = self
            .visible_indices()
            .into_iter()
            .max_by_key(|i| degree(*i))
        {
            self.selected = i;
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
    pub fn on_key(&mut self, key: KeyEvent, action: Option<Action>) -> Intent {
        // While the filter box has the keyboard every key is a character, so
        // whatever the key would otherwise have meant is beside the point.
        if self.filtering {
            return self.on_filter_key(key);
        }
        let Some(action) = action else {
            return Intent::None;
        };
        match action {
            Action::MapClose => return Intent::Close,
            Action::MapOpen => {
                if let Some(n) = self.selected_node() {
                    return Intent::Open(n.path.clone());
                }
            }
            Action::MapLeft => self.move_towards(-1.0, 0.0),
            Action::MapRight => self.move_towards(1.0, 0.0),
            // Screen coordinates run downwards; the layout runs upwards.
            Action::MapUp => self.move_towards(0.0, 1.0),
            Action::MapDown => self.move_towards(0.0, -1.0),
            Action::MapNext => self.cycle(true),
            Action::MapPrevious => self.cycle(false),
            Action::MapFilter => {
                self.filtering = true;
                self.filter.clear();
            }
            Action::MapWikilinks => self.toggle_kind(0),
            Action::MapLinks => self.toggle_kind(1),
            Action::MapCalls => self.toggle_kind(2),
            Action::MapReload => return Intent::Rebuild,
            _ => {}
        }
        Intent::None
    }

    /// Turn one kind of connection on or off, and make sure the cursor is
    /// still on something that is drawn.
    fn toggle_kind(&mut self, i: usize) {
        self.kinds[i] = !self.kinds[i];
        self.ensure_selection();
    }

    /// Keys for the `/` filter box. Esc clears and closes; Enter keeps the
    /// filter but hands the keyboard back to navigation.
    fn on_filter_key(&mut self, key: KeyEvent) -> Intent {
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
        Intent::None
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
        // Connections, not edges: the picture draws one line per pair of
        // files, so counting per-symbol edges here would promise lines that
        // are not there.
        let links = self.links().len();
        let hidden = self.graph.nodes.len() - shown;
        let mut out = format!("{shown}/{} files | {links} links", self.graph.nodes.len());
        if hidden > 0 {
            out.push_str(&format!(" | {hidden} hidden"));
        }
        out
    }

    /// Languages whose calls can be followed, for the line beside the toggles.
    pub fn languages(&self) -> &[String] {
        &self.graph.languages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyModifiers};
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

    /// What the shipped keyboard makes of a key. These tests are about the
    /// view, not the bindings.
    fn act(key: KeyEvent) -> Option<Action> {
        crate::keys::Keymap::default().find(crate::keys::Context::Map, &key)
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

    // ---- laying the files out ---------------------------------------------

    /// Which folder heading each placed box sits under, by relative path.
    fn folder_of_box(place: &Placement, p: &Placed) -> String {
        place
            .folders
            .iter()
            .filter(|g| g.row < p.row)
            .max_by_key(|g| g.row)
            .map(|g| g.label.clone())
            .unwrap_or_default()
    }

    #[test]
    fn placed_boxes_never_overlap_and_stay_on_screen() {
        let (_td, mut view) = fixture();
        let (w, h) = (80u16, 30u16);
        let place = view.place(w, h);
        assert!(!place.boxes.is_empty(), "something should fit in 80x30");

        for p in &place.boxes {
            assert!(p.right() <= w, "{p:?} runs off the right of {w}");
            assert!(p.bottom() <= h, "{p:?} runs off the bottom of {h}");
        }
        for (i, a) in place.boxes.iter().enumerate() {
            for b in &place.boxes[i + 1..] {
                let apart = a.right() <= b.col
                    || b.right() <= a.col
                    || a.bottom() <= b.row
                    || b.bottom() <= a.row;
                assert!(apart, "{a:?} and {b:?} are on top of each other");
            }
        }
    }

    #[test]
    fn a_channel_column_is_clear_of_every_box() {
        // The invariant the whole of `ui::route` rests on: a line running down
        // a slot's channel column cannot pass behind a box, at any row, in any
        // slot. Break this and lines start disappearing mid-run.
        let (_td, mut view) = fixture();
        let place = view.place(90, 60);
        let ch = place.channels;
        assert!(ch.slot_w > 0, "the geometry came back");

        for p in &place.boxes {
            let column = ch.column_by(p.col);
            for other in &place.boxes {
                assert!(
                    column < other.col || column >= other.right(),
                    "the channel at {column} runs through {other:?}"
                );
            }
        }
    }

    #[test]
    fn a_channel_column_stays_inside_its_own_slot() {
        // If it strayed into the next slot, a route would cross the boundary
        // and could end up on the far side of the box it was aiming at.
        let (_td, mut view) = fixture();
        let place = view.place(90, 60);
        let ch = place.channels;
        for p in &place.boxes {
            let column = ch.column_by(p.col);
            assert!(column >= p.right(), "the channel is past the box it serves");
            assert!(
                column < p.col + ch.slot_w,
                "the channel belongs to the next slot along"
            );
        }
    }

    #[test]
    fn the_row_above_a_box_is_never_a_heading() {
        // Lines arriving from above land on it, and a heading there would be
        // overwritten by them — or would cut the line in half.
        let (_td, mut view) = fixture();
        let place = view.place(90, 60);
        for p in &place.boxes {
            assert!(
                !place.folders.iter().any(|g| g.row + 1 == p.row),
                "{p:?} sits directly under a heading"
            );
        }
    }

    #[test]
    fn the_map_opens_on_the_busiest_file() {
        let (_td, view) = fixture();
        // design.md and architecture.md point at each other, and main.py calls
        // utils.py; alone.md is joined to nothing. Whichever is picked, it must
        // not be the lonely one — the map would open showing no connections.
        assert!(
            view.connected(view.selected),
            "opening on a file with no lines makes the map look broken"
        );
    }

    #[test]
    fn a_box_is_wide_enough_for_the_name_it_holds() {
        let (_td, mut view) = fixture();
        for p in view.place(80, 30).boxes {
            let name = label_of(&view.graph.nodes[p.node].name);
            assert_eq!(
                p.width as usize,
                name.chars().count() + 2,
                "two borders and the name"
            );
        }
    }

    #[test]
    fn files_sit_under_the_heading_for_the_folder_they_live_in() {
        let (_td, mut view) = fixture();
        let place = view.place(90, 40);
        assert_eq!(place.offscreen, 0, "the fixture fits in 90x40");

        for p in &place.boxes {
            let rel = view.graph.nodes[p.node].rel.clone();
            if !view.connected(p.node) {
                continue; // filed by connectedness instead — see below
            }
            let want = format!("{}/", folder_of(&rel));
            assert_eq!(
                folder_of_box(&place, p),
                want,
                "{rel} is filed under the wrong heading"
            );
        }
        let labels: Vec<&str> = place.folders.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(
            labels,
            ["notes/", "src/", "unconnected"],
            "folders run in path order"
        );
        assert_eq!(place.folders[0].files, 2, "design and architecture");
    }

    #[test]
    fn a_project_with_no_folders_still_names_its_root() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "design.md", "see [[architecture]]\n");
        write(td.path(), "architecture.md", "back to [[design]]\n");
        let mut view = ProjectMap::build(td.path(), &graph::Options::default());
        let place = view.place(90, 40);
        let root = td.path().file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(
            place
                .folders
                .iter()
                .map(|g| g.label.clone())
                .collect::<Vec<_>>(),
            [format!("{root}/")],
            "the project folder is the one heading"
        );
    }

    #[test]
    fn root_files_come_before_the_folders() {
        let (td, _) = fixture();
        write(td.path(), "README.md", "see [[design]]\n");
        let mut view = ProjectMap::build(td.path(), &graph::Options::default());
        let place = view.place(90, 40);
        let labels: Vec<&str> = place.folders.iter().map(|g| g.label.as_str()).collect();
        assert!(
            !labels[0].starts_with("notes/") && !labels[0].starts_with("src/"),
            "the project's own files head the map, not a subfolder: {labels:?}"
        );
        assert_eq!(&labels[1..], ["notes/", "src/", "unconnected"]);
    }

    #[test]
    fn nothing_on_the_map_comes_from_outside_the_project() {
        let outer = tempfile::tempdir().unwrap();
        write(outer.path(), "secret.md", "not part of it\n");
        let root = outer.path().join("project");
        write(
            &root,
            "notes.md",
            "escape to [[secret]] and [../secret.md](x)\n",
        );
        write(&root, "inside.md", "see [[notes]]\n");

        let mut view = ProjectMap::build(&root, &graph::Options::default());
        for n in &view.graph.nodes {
            assert!(
                root.join(&n.rel).starts_with(&root),
                "{} is not in the project",
                n.rel
            );
        }
        assert_eq!(view.graph.nodes.len(), 2, "only the two files under it");
        assert!(
            !view.graph.nodes.iter().any(|n| n.rel.contains("secret")),
            "a link pointing above the project folder reaches nothing"
        );
        assert!(
            view.place(90, 40).folders.len() == 1,
            "one folder: the root"
        );
    }

    #[test]
    fn a_pane_too_small_for_any_box_places_nothing() {
        let (_td, mut view) = fixture();
        assert!(view.place(4, 2).boxes.is_empty());
    }

    #[test]
    fn a_layout_taller_than_the_pane_says_what_it_left_out() {
        let (_td, mut view) = fixture();
        let tall = view.place(90, 40);
        let short = view.place(90, 8);
        assert_eq!(tall.offscreen, 0);
        assert!(short.offscreen > 0, "five files do not fit in eight rows");
        assert_eq!(
            short.boxes.len() + short.offscreen,
            tall.boxes.len(),
            "every file is either drawn or counted"
        );
    }

    #[test]
    fn a_heading_never_stands_over_files_that_were_left_out() {
        let (_td, mut view) = fixture();
        let place = view.place(90, 8);
        for g in &place.folders {
            assert!(
                place.boxes.iter().any(|p| p.row > g.row),
                "{} has no files under it",
                g.label
            );
        }
    }

    #[test]
    fn the_view_follows_the_cursor_past_the_bottom() {
        let (_td, mut view) = fixture();
        // The last file in walk order is the furthest down the layout.
        let last = *view.visible_indices().last().unwrap();
        view.selected = last;
        let place = view.place(90, 8);
        assert!(
            place.boxes.iter().any(|p| p.node == last),
            "the selected file is scrolled into view"
        );
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
        let (_td, mut view) = fixture();
        view.place(90, 40);
        assert_eq!(view.pos.len(), view.graph.nodes.len());
        assert!(view.pos.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        for i in 0..view.pos.len() {
            for j in (i + 1)..view.pos.len() {
                let d = (view.pos[i].0 - view.pos[j].0).hypot(view.pos[i].1 - view.pos[j].1);
                // Grid units: adjacent boxes are exactly one apart.
                assert!(
                    d >= 1.0,
                    "{} and {} landed on top of each other",
                    rel_of(&view, i),
                    rel_of(&view, j)
                );
            }
        }
    }

    #[test]
    fn the_layout_is_the_same_every_time() {
        let (td, mut a) = fixture();
        let mut b = ProjectMap::build(td.path(), &graph::Options::default());
        let (pa, pb) = (a.place(90, 40), b.place(90, 40));
        assert_eq!(pa, pb, "the same project draws the same picture");
        assert_eq!(a.pos, b.pos);
    }

    #[test]
    fn a_second_pass_over_the_same_pane_does_not_move_anything() {
        let (_td, mut view) = fixture();
        let first = view.place(90, 40);
        let again = view.place(90, 40);
        assert_eq!(
            first, again,
            "placing is a function of the pane, not a step"
        );
    }

    #[test]
    fn files_in_one_folder_end_up_nearer_than_files_in_another() {
        let (_td, mut view) = fixture();
        view.place(90, 40);
        let find = |rel: &str| view.graph.nodes.iter().position(|n| n.rel == rel).unwrap();
        let dist = |a: usize, b: usize| {
            (view.pos[a].0 - view.pos[b].0).hypot(view.pos[a].1 - view.pos[b].1)
        };
        let design = find("notes/design.md");
        let arch = find("notes/architecture.md");
        let utils = find("src/utils.py");
        assert!(
            dist(design, arch) < dist(design, utils),
            "two notes in one folder are neighbours; a file in src is not"
        );
    }

    #[test]
    fn a_single_file_project_lays_out_without_trouble() {
        let td = tempfile::tempdir().unwrap();
        write(td.path(), "only.md", "alone\n");
        let mut view = ProjectMap::build(td.path(), &graph::Options::default());
        assert_eq!(view.pos.len(), 1);
        let placed = view.place(60, 20).boxes;
        assert_eq!(placed.len(), 1);
        assert!(placed[0].right() <= 60 && placed[0].bottom() <= 20);
    }

    #[test]
    fn an_empty_project_does_not_panic() {
        let td = tempfile::tempdir().unwrap();
        let mut view = ProjectMap::build(td.path(), &graph::Options::default());
        assert!(view.graph.nodes.is_empty());
        assert!(view.visible_indices().is_empty());
        assert_eq!(view.place(60, 20), Placement::default());
        let _ = view.summary();
    }

    // ---- what is shown ----------------------------------------------------

    #[test]
    fn turning_off_an_edge_kind_never_takes_a_file_off_the_map() {
        let (_td, mut view) = fixture();
        let all: Vec<String> = visible_names(&view).iter().map(|s| s.to_string()).collect();
        assert!(all.iter().any(|r| r == "notes/design.md"));

        // The notes are held on only by wikilinks, so switching those off
        // leaves nothing reaching them — but they are still files in the
        // project, and the map still has to show them.
        view.kinds[kind_index(EdgeKind::Wikilink)] = false;
        assert_eq!(
            visible_names(&view),
            all,
            "what a connection kind changes is the arrangement, not the census"
        );
        assert!(
            !view.connected(
                view.graph
                    .nodes
                    .iter()
                    .position(|n| n.rel == "notes/design.md")
                    .unwrap()
            )
        );
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
        let (_td, view) = fixture();
        assert!(view.summary().contains("/5 files"), "{}", view.summary());
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
        assert!(!out.is_empty(), "main.py calls utils.load");
        assert!(incoming.is_empty(), "nothing points at main.py");
        assert!(out.iter().all(|e| e.from == main));
    }

    // ---- moving around ----------------------------------------------------

    #[test]
    fn arrows_move_to_the_nearest_file_in_that_direction() {
        let (_td, mut view) = fixture();
        // Put two nodes in known places and check the cursor goes the right way.
        view.selected = 0;
        view.pos[0] = (0.0, 0.0);
        view.pos[1] = (100.0, 0.0);
        view.pos[2] = (-100.0, 0.0);
        view.pos[3] = (0.0, 100.0);
        view.pos[4] = (0.0, -100.0);

        view.on_key(k(KeyCode::Right), act(k(KeyCode::Right)));
        assert_eq!(view.selected, 1);
        view.selected = 0;
        view.on_key(k(KeyCode::Left), act(k(KeyCode::Left)));
        assert_eq!(view.selected, 2);
        view.selected = 0;
        view.on_key(k(KeyCode::Up), act(k(KeyCode::Up)));
        assert_eq!(view.selected, 3, "up is up on screen");
        view.selected = 0;
        view.on_key(k(KeyCode::Down), act(k(KeyCode::Down)));
        assert_eq!(view.selected, 4);
    }

    #[test]
    fn moving_towards_nothing_leaves_the_cursor_alone() {
        let (_td, mut view) = fixture();
        view.selected = 0;
        view.pos = vec![
            (0.0, 0.0),
            (-50.0, 0.0),
            (-60.0, 0.0),
            (-70.0, 0.0),
            (-80.0, 0.0),
        ];
        view.on_key(k(KeyCode::Right), act(k(KeyCode::Right)));
        assert_eq!(view.selected, 0, "everything is to the left");
    }

    #[test]
    fn tab_cycles_through_every_visible_file_and_wraps() {
        let (_td, mut view) = fixture();
        let visible = view.visible_indices();
        view.selected = visible[0];
        for _ in 0..visible.len() {
            view.on_key(k(KeyCode::Tab), act(k(KeyCode::Tab)));
        }
        assert_eq!(view.selected, visible[0], "a full lap comes back round");
    }

    #[test]
    fn the_cursor_never_sits_on_something_hidden() {
        let (_td, mut view) = fixture();
        let alone = view
            .graph
            .nodes
            .iter()
            .position(|n| n.rel == "notes/alone.md")
            .unwrap();
        view.selected = alone;

        // The filter is the only thing that can take a file off the map now.
        view.filtering = true;
        for c in "utils".chars() {
            view.on_key(ch(c), None);
        }
        assert!(
            view.node_visible(view.selected),
            "the cursor should have moved to something still drawn"
        );
    }

    #[test]
    fn a_file_nothing_reaches_is_still_on_the_map() {
        let (_td, view) = fixture();
        let alone = view
            .graph
            .nodes
            .iter()
            .position(|n| n.rel == "notes/alone.md")
            .unwrap();
        assert!(
            view.node_visible(alone),
            "a file with no links is part of the project too"
        );
        assert!(!view.connected(alone), "it just has nothing pointing at it");
    }

    #[test]
    fn unconnected_files_are_filed_under_their_own_heading_last() {
        let (_td, mut view) = fixture();
        let place = view.place(120, 60);
        let headings: Vec<&str> = place.folders.iter().map(|f| f.label.as_str()).collect();

        assert_eq!(
            headings.last(),
            Some(&"unconnected"),
            "and it comes after the folders, not among them: {headings:?}"
        );
        let lonely = place
            .folders
            .last()
            .expect("there is an unconnected heading");
        assert_eq!(lonely.files, 1, "just alone.md");
        assert!(
            headings.contains(&"notes/") && headings.contains(&"src/"),
            "the folders are still there: {headings:?}"
        );
    }

    #[test]
    fn turning_a_connection_kind_off_moves_its_files_to_unconnected() {
        let (_td, mut view) = fixture();
        let before = view.place(120, 60);
        let notes = before
            .folders
            .iter()
            .find(|f| f.label == "notes/")
            .expect("design and architecture link to each other")
            .files;
        assert_eq!(notes, 2, "alone.md is already elsewhere");

        // With wikilinks hidden, nothing reaches the two notes either.
        view.toggle_kind(0);
        let after = view.place(120, 60);
        let lonely = after
            .folders
            .iter()
            .find(|f| f.label == "unconnected")
            .expect("still a heading for them");
        assert_eq!(
            lonely.files, 3,
            "the map should say which files it has nothing to tell you about"
        );
    }

    // ---- keys -------------------------------------------------------------

    #[test]
    fn escape_and_w_both_close_the_graph() {
        for key in [k(KeyCode::Esc), ch('m'), ch('q')] {
            let (_td, mut view) = fixture();
            assert!(matches!(view.on_key(key, act(key)), Intent::Close));
        }
    }

    #[test]
    fn enter_asks_for_the_selected_file_to_be_opened() {
        let (td, mut view) = fixture();
        view.selected = view.visible_indices()[0];
        let want = view.graph.nodes[view.selected].path.clone();
        match view.on_key(k(KeyCode::Enter), act(k(KeyCode::Enter))) {
            Intent::Open(p) => {
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
        view.on_key(ch('3'), act(ch('3')));
        assert!(!view.kinds[kind_index(EdgeKind::Call)]);
        view.on_key(ch('3'), act(ch('3')));
        assert!(view.kinds[kind_index(EdgeKind::Call)]);
    }

    #[test]
    fn slash_opens_a_filter_that_takes_typing_and_escape_clears_it() {
        let (_td, mut view) = fixture();
        view.on_key(ch('/'), act(ch('/')));
        assert!(view.filtering);
        for c in "src".chars() {
            view.on_key(ch(c), act(ch(c)));
        }
        assert_eq!(view.filter, "src");
        assert!(visible_names(&view).iter().all(|r| r.contains("src")));

        view.on_key(k(KeyCode::Backspace), act(k(KeyCode::Backspace)));
        assert_eq!(view.filter, "sr");
        view.on_key(k(KeyCode::Esc), act(k(KeyCode::Esc)));
        assert!(!view.filtering);
        assert!(view.filter.is_empty(), "escape clears the filter");
    }

    #[test]
    fn keys_that_mean_something_elsewhere_do_not_close_the_graph() {
        let (_td, mut view) = fixture();
        // `q` closes, but only when it is not being typed into the filter.
        view.on_key(ch('/'), act(ch('/')));
        assert!(matches!(view.on_key(ch('q'), act(ch('q'))), Intent::None));
        assert_eq!(view.filter, "q");
    }

    #[test]
    fn control_chords_are_ignored_rather_than_acted_on() {
        let (_td, mut view) = fixture();
        let ctrl_c = KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..ch('c')
        };
        assert!(matches!(view.on_key(ctrl_c, act(ctrl_c)), Intent::None));
    }

    #[test]
    fn r_lays_the_graph_out_again_to_the_same_place() {
        let (_td, mut view) = fixture();
        let before = view.pos.clone();
        view.on_key(ch('r'), act(ch('r')));
        for (a, b) in before.iter().zip(view.pos.iter()) {
            assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        }
    }
}
