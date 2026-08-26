//! Where every box goes.
//!
//! Given a [`ProjectMap`] and a width, this decides the whole picture: which
//! heading each file sits under, which slot in the grid it occupies, and which
//! rows and columns are left clear for lines to travel in.
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
//! more — a position you can predict. `src/app/mod.rs` is under the `src/app/`
//! heading, in alphabetical order, on every machine and after every edit.
//!
//! # Channels
//!
//! [`Channels`] is the contract with the drawing code: a row under every box
//! and a column beside every slot that the layout guarantees nothing is
//! written into. Lines are only allowed to turn there, which is what keeps a
//! connection between two distant files from disappearing behind a third one.
//! Change the slot arithmetic here and `ui::ink` starts drawing
//! through boxes.
//!
//! # Unconnected files are still files
//!
//! A file nothing reaches gets a heading of its own at the bottom rather than
//! being left off. Being unlinked is a fact about a project worth being able
//! to see — a note you meant to link and did not is exactly what you would
//! open the map to find.

use super::view::ProjectMap;

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
const MAX_LABEL: usize = 18;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::graph;
    use crate::map::testing::*;

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
}
