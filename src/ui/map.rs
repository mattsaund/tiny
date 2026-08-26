//! The project map pane: boxes for files, lines for connections.
//!
//! Composed onto an [`super::ink`] grid rather than emitted as spans directly,
//! which is what lets a line and a box overlap sensibly: lines go down first,
//! boxes on top, so a connection running past a file never writes over its
//! name. The detail strip underneath describes whatever the cursor is on.
//!
//! Only the selected file's connections are drawn. A project of any size is a
//! dense graph — nineteen files with seventy-five connections between them is
//! not a picture, it is a smear — and the question the map answers is "what
//! does *this* file touch", which is exactly one file's worth of lines.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::ink::{Glyphs, Ink, InkStyle, route};

use crate::app::App;
use crate::map::graph::EdgeKind;
use crate::map::layout;
use crate::map::view::ProjectMap;

/// The project map: every file as a box, every connection as a line, and a
/// detail strip underneath.
///
/// Drawn by composing an [`Ink`] grid rather than by emitting spans directly.
/// Lines go down first and boxes on top, so a connection running past a file
/// never writes over its name, and two lines crossing resolve into a junction
/// without either of them having to know the other was there.
///
/// What is drawn is decided in three steps: `place` works out which boxes fit
/// and where, `route` draws the line between each pair of them, and then the
/// boxes are stamped over the top.
pub(super) fn draw_map(f: &mut Frame, app: &mut App, area: Rect) {
    if app.project_map.is_none() {
        return;
    }
    let pal = app.palette;
    let markers = app.config.markers;

    // The picture, then a strip describing whatever the cursor is on.
    let detail_h = if area.height > 14 { 6 } else { 0 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(detail_h)])
        .split(area);

    // The border is fixed, so the pane inside it can be measured before the
    // block that will carry the titles exists — and the titles need the
    // layout, which needs the size.
    let bordered = || Block::default().borders(Borders::ALL);
    let inner = bordered().inner(rows[0]);
    if inner.width < 8 || inner.height < 3 {
        f.render_widget(bordered().border_style(pal.border_focus), rows[0]);
        return;
    }

    // Two borrows, in order: laying the picture out writes `pos` back, then
    // the frame around it is read-only. The one-way rule bends exactly as far
    // as `last_tree_cols` already bends it.
    let placement = app
        .project_map
        .as_mut()
        .expect("checked above")
        .place(inner.width, inner.height);
    let view = app.project_map.as_ref().expect("checked above");
    let selected = view.selected;
    let near = view.neighbours(selected);

    let mut block = bordered()
        .border_style(pal.border_focus)
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("PROJECT MAP", pal.text.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", view.summary()), pal.dim),
            Span::raw(" "),
        ]));
    if view.filtering || !view.filter.is_empty() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(" / ", pal.text.add_modifier(Modifier::REVERSED)),
            Span::styled(view.filter.clone(), pal.text),
            Span::styled(if view.filtering { "_ " } else { " " }, pal.dim),
        ]));
    }
    if placement.offscreen > 0 {
        // A map cut off at the edge must not read as a finished picture. The
        // view follows the cursor, so moving it is what reaches the rest.
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {} more — arrow to reach them ", placement.offscreen),
                pal.dim,
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, rows[0]);

    // Index into the placed boxes by node, so link routing can find both ends
    // fast. A node with no entry is scrolled off the view.
    let mut slot_of: HashMap<usize, usize> = HashMap::new();
    for (n, p) in placement.boxes.iter().enumerate() {
        slot_of.insert(p.node, n);
    }

    let glyphs = Glyphs::for_markers(markers);
    let mut ink = Ink::new(inner.width as usize, inner.height as usize);

    // Lines first, so a box always sits on top of whatever runs past it.
    //
    // Only the file under the cursor's, and that is the single biggest thing
    // that makes this readable. A real project is a dense graph — this one is
    // nineteen files with seventy-five connections between them, which is not
    // a picture, it is a hatch pattern. Drawing every line at once meant no
    // individual line could be followed, which is what "the arrows look
    // detached" is: not that they were broken, but that no eye could trace one
    // to its end.
    //
    // So the map answers one question at a time — *what does this file
    // touch?* — and the arrow keys are how you ask it about another. Every
    // file is still drawn, the strip below still names every connection, and
    // the summary still counts them all.
    for l in view.links() {
        // Orient every line out of the selected file, so they can share one
        // trunk. `to_them` and `to_us` then say which ends get an arrowhead.
        let (them, to_them, to_us) = match (l.a == selected, l.b == selected) {
            (true, _) => (l.b, l.a_to_b, l.b_to_a),
            (_, true) => (l.a, l.b_to_a, l.a_to_b),
            _ => continue,
        };
        let (Some(&us), Some(&them)) = (slot_of.get(&selected), slot_of.get(&them)) else {
            continue;
        };
        route(
            &mut ink,
            &placement.boxes[us],
            &placement.boxes[them],
            &glyphs,
            placement.channels,
            to_them,
            to_us,
        );
    }

    for p in &placement.boxes {
        let style = if p.node == selected {
            InkStyle::Selected
        } else if near.contains(&p.node) {
            InkStyle::Near
        } else {
            InkStyle::Far
        };
        ink.draw_box(
            p,
            &layout::label_of(&view.graph.nodes[p.node].name),
            style,
            &glyphs,
        );
    }

    for g in &placement.folders {
        let files = if g.files == 1 { "file" } else { "files" };
        ink.write(
            0,
            g.row as i32,
            &format!("{}  {} {files}", g.label, g.files),
            InkStyle::Folder,
        );
    }
    f.render_widget(Paragraph::new(ink.render(&glyphs, &pal)), inner);

    if detail_h > 0 {
        draw_map_detail(f, app, view, rows[1]);
    }
}

/// Human-readable name for a node kind, for the detail strip.
fn node_word(kind: crate::map::graph::NodeKind) -> &'static str {
    use crate::map::graph::NodeKind;
    match kind {
        NodeKind::Note => "note",
        NodeKind::Prose => "prose",
        NodeKind::Code => "code",
        NodeKind::Other => "file",
    }
}

/// Human-readable name for an edge kind. Also labels the `1`-`3` toggles, so
/// the words on screen match the words in the help and the README.
fn kind_word(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Wikilink => "wikilink",
        EdgeKind::Link => "link",
        EdgeKind::Call => "call",
    }
}

/// The strip under the map: what the cursor is on, what it connects to, and
/// which edge kinds are switched on.
fn draw_map_detail(f: &mut Frame, app: &App, view: &ProjectMap, area: Rect) {
    let pal = app.palette;
    let Some(node) = view.selected_node() else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("  nothing selected", pal.dim))),
            area,
        );
        return;
    };
    let (out, incoming) = view.connections(view.selected);

    // Which edge kinds are switched on, as the 1-3 keys see them.
    let toggles: Vec<Span> = [EdgeKind::Wikilink, EdgeKind::Link, EdgeKind::Call]
        .iter()
        .enumerate()
        .flat_map(|(i, k)| {
            let on = view.kinds[crate::map::view::kind_index(*k)];
            [
                Span::styled(
                    format!("{}:{} ", i + 1, kind_word(*k)),
                    if on { pal.text } else { pal.dim },
                ),
                Span::raw(""),
            ]
        })
        .collect();

    let summarise = |edges: &[&crate::map::graph::Edge], outgoing: bool| {
        if edges.is_empty() {
            return vec![Span::styled("none", pal.dim)];
        }
        let mut spans = Vec::new();
        for e in edges.iter().take(4) {
            let other = if outgoing { e.to } else { e.from };
            spans.push(Span::styled(view.graph.nodes[other].name.clone(), pal.text));
            // The symbol is the point of a call edge. For an import or a link
            // the label just repeats the file name, so it is left out.
            if e.kind == EdgeKind::Call {
                let times = if e.count > 1 {
                    format!(":{} x{}", e.label, e.count)
                } else {
                    format!(":{}", e.label)
                };
                spans.push(Span::styled(times, pal.dim));
            }
            spans.push(Span::raw(" "));
        }
        if edges.len() > 4 {
            spans.push(Span::styled(format!("+{} more", edges.len() - 4), pal.dim));
        }
        spans
    };

    let mut lines = vec![
        Line::from(vec![
            Span::raw("  "),
            Span::styled(node.rel.clone(), pal.text.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", node_word(node.kind)), pal.dim),
        ]),
        Line::from(
            std::iter::once(Span::styled(format!("  out {:<3} ", out.len()), pal.dim))
                .chain(summarise(&out, true))
                .collect::<Vec<_>>(),
        ),
        Line::from(
            std::iter::once(Span::styled(
                format!("  in  {:<3} ", incoming.len()),
                pal.dim,
            ))
            .chain(summarise(&incoming, false))
            .collect::<Vec<_>>(),
        ),
    ];
    if !node.defines.is_empty() {
        // A file can define dozens of things; the first few say enough.
        let shown = node.defines.len().min(6);
        let mut text = node.defines[..shown].join(", ");
        if node.defines.len() > shown {
            text.push_str(&format!(", +{}", node.defines.len() - shown));
        }
        lines.push(Line::from(vec![
            Span::styled("  defines ", pal.dim),
            Span::styled(text, pal.dim),
        ]));
    }
    // The toggles, with a note on which languages calls can be followed in —
    // right beside the switch it explains.
    let mut footer: Vec<Span> = std::iter::once(Span::raw("  ")).chain(toggles).collect();
    footer.push(Span::styled(
        format!("  calls traced in {}", view.languages().join(", ")),
        pal.dim,
    ));
    lines.push(Line::from(footer));

    f.render_widget(Paragraph::new(lines), area);
}

// ---- the bar --------------------------------------------------------------
