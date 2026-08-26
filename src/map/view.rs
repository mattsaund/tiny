//! The project map: what is on it, and what the keyboard does to it.
//!
//! Every file in the project is a box with its name in it, and the connections
//! between them are lines, the way anyone would draw this on paper. This half
//! owns the *state* — which file is selected, which connection kinds are
//! switched on, what the filter is hiding. Where each box physically sits is
//! [`super::layout`]'s job, and painting it is `ui::map`'s.
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
//! [`super::graph`] decides *what connects to what*; this module decides
//! *what you can see and do*; [`super::layout`] decides *where it sits*. The
//! `Graph` itself is never mutated after construction — filtering happens by
//! asking [`ProjectMap::node_visible`] and [`ProjectMap::edge_visible`] at
//! draw time, so toggling an edge kind is instant and reversible.
//!
//! # Coordinates
//!
//! `pos` is written by [`super::layout`] in **grid** units — one per box, not
//! one per character — with **y running upwards**, which is why `Up` calls
//! `move_towards(0.0, 1.0)`. Navigation then walks the picture exactly as it
//! is drawn.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::keys::Action;
use crate::map::graph::{self, Edge, EdgeKind, Graph, Node};

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
    pub(super) root: PathBuf,
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
    pub(super) fn connected(&self, i: usize) -> bool {
        self.graph
            .edges
            .iter()
            .any(|e| self.kind_on(e) && (e.from == i || e.to == i))
    }

    /// Whether this edge's kind is switched on, ignoring everything else.
    pub(super) fn kind_on(&self, e: &Edge) -> bool {
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
    use crate::map::testing::*;
    use crossterm::event::KeyModifiers;

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
