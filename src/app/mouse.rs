//! The mouse: the wheel, a click, a drag.
//!
//! A click puts things where it lands: the cursor in a file, the browser's
//! cursor on a row, the keyboard in a pane or in the terminal. A drag in a
//! file selects, by characters, or by whole words after a double click and
//! whole lines after a triple one, or from the line numbers. Shift and a
//! click selects to there; Alt and a click (or Ctrl, the way micro has it)
//! adds a cursor.
//!
//! In a list, the first click picks and the second does: a second click on
//! the browser's cursor opens what it is on, as Enter would, and the same
//! goes for the git window's rows and buttons and the map's boxes. A double
//! click is two clicks, so it opens.
//!
//! A drag on a line where two parts meet moves it: the browser's edge, the
//! line between two panes or above the terminal, and in the git window the
//! edge of the list and the line between a diff's halves. The parts on
//! either side always fill the space between them, and neither is made
//! smaller than leaves room for what it holds.
//!
//! # Knowing what is where
//!
//! Only `ui` knows where anything was drawn, so every frame it writes down
//! what each part of the screen held: the browser's rows, each pane's lines
//! (see [`TextHits`]), the search results, the map's boxes, the git window's
//! rows and the lines between parts (see [`Divider`]). A click is read
//! against the last frame, which is the one that was being looked at.
//!
//! # The terminal
//!
//! A program in the terminal pane that asks for the mouse (a full-screen
//! editor, `htop`) gets it there, spelled the way it asked, see
//! `shell::Shell::mouse`. One that does not ask gets the wheel as scrolling
//! back through what has gone by, and a click gives the terminal the
//! keyboard.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::text::editor::{Editor, Pos, Unit};
use crate::text::tabs;

use super::App;
use super::actions::{TREE_WIDTH_MAX, TREE_WIDTH_MIN};
use super::mode::{Focus, Mode, Window};
use super::parts::display_name;
use super::preview::Preview;
use super::source::GitFocus;
use super::split::PaneLine;

/// How soon a second press on the same cell counts as a double click.
const DOUBLE: Duration = Duration::from_millis(400);

/// Where one pane's lines were drawn in the last frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextHits {
    /// The text: inside the border, right of the line numbers.
    pub area: Rect,
    /// How far across the lines were scrolled, in columns.
    pub scroll_x: usize,
    /// The line each drawn row showed, and whether as it is written (`true`)
    /// or formatted. A formatted row has no columns in common with its line,
    /// so a click on one lands as near as can be told.
    pub rows: Vec<(usize, bool)>,
}

/// One pane as it was drawn in the last frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneHit {
    /// Which pane: `None` for the one with the keyboard, otherwise the
    /// number of one set aside.
    pub pinned: Option<usize>,
    /// All of it, border and title included.
    pub area: Rect,
    /// Where its lines were, when it showed a file's lines.
    pub text: Option<TextHits>,
}

/// One row of the git window's list as it was drawn in the last frame: the
/// screen row, which row of the list it is, and the columns each button
/// covered when it is the button row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitHit {
    pub y: u16,
    pub row: usize,
    pub buttons: Vec<(u16, u16, usize)>,
}

/// A line where two parts of the window meet, which a drag moves, as the
/// last frame drew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divider {
    /// What moving it resizes.
    pub moves: Moves,
    /// The two borders that meet there, either of which takes hold of it.
    pub hit: Rect,
    /// Whether the two parts are side by side, so it moves across, rather
    /// than one above the other.
    pub across: bool,
    /// Where the second part begins, and the first and the last place it
    /// may begin.
    pub at: u16,
    pub range: (u16, u16),
}

/// What a [`Divider`] resizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moves {
    /// The browser's share of `area`, the main window, with the browser on
    /// the `right` of it or the left.
    Browser { area: Rect, right: bool },
    /// The terminal's share of `area`: the panes, and it under them.
    Terminal { area: Rect },
    /// Two panes side by side, or one above the other.
    Panes(PaneLine),
    /// The git window's list, its share of `area`.
    Changes { area: Rect },
    /// A diff's first half, its share of `area`.
    Halves { area: Rect },
}

impl Divider {
    /// The line between two parts of `along`, side by side when `across`,
    /// the second beginning at `at` and free to begin anywhere in `range`.
    pub fn new(moves: Moves, along: Rect, across: bool, at: u16, range: (u16, u16)) -> Self {
        let hit = if across {
            Rect::new(at.saturating_sub(1), along.y, 2, along.height)
        } else {
            Rect::new(along.x, at.saturating_sub(1), along.width, 2)
        };
        Divider {
            moves,
            hit,
            across,
            at,
            // Where it is now is always somewhere it may be, even in a
            // window too small for both parts' least.
            range: (range.0.min(at), range.1.max(at)),
        }
    }
}

/// A drag going on in a file: what the press selected, and by what.
#[derive(Debug, Clone, Copy)]
pub(super) struct Drag {
    origin: (Pos, Pos),
    unit: Unit,
}

/// The last press, for telling a double or a triple click.
#[derive(Debug, Clone, Copy)]
pub(super) struct Press {
    when: Instant,
    at: Position,
    count: u8,
}

/// Where in `ed` the cell `at` falls, by what was drawn in `hits`. Above the
/// text is the line before the first one shown, and below it the line after
/// the last, so a drag past an edge goes on into the file and the view
/// follows.
fn pos_at(ed: &Editor, hits: &TextHits, at: Position, tab_width: usize) -> Pos {
    let last = ed.line_count().saturating_sub(1);
    let first_shown = hits.rows.first().map_or(0, |r| r.0);
    let last_shown = hits.rows.last().map_or(0, |r| r.0);
    let (line, raw) = if at.y < hits.area.y {
        (first_shown.saturating_sub(1), true)
    } else {
        match hits.rows.get(usize::from(at.y - hits.area.y)) {
            Some(&(line, raw)) => (line, raw),
            None => ((last_shown + 1).min(last), true),
        }
    };
    let across = usize::from(at.x.saturating_sub(hits.area.x));
    let column = if raw { across + hits.scroll_x } else { across };
    let text = ed.lines().get(line).map_or("", String::as_str);
    Pos::new(line, tabs::index_at(text, column, tab_width))
}

impl App {
    /// One event from the mouse. Returns whether there is anything new to
    /// draw.
    pub fn on_mouse(&mut self, m: MouseEvent) -> bool {
        let at = Position::new(m.column, m.row);
        if self.mouse_to_terminal(&m, at) {
            return true;
        }
        match m.kind {
            MouseEventKind::ScrollDown => self.on_scroll(true, m.column, m.row),
            MouseEventKind::ScrollUp => self.on_scroll(false, m.column, m.row),
            MouseEventKind::Down(MouseButton::Left) => self.press(at, m.modifiers),
            MouseEventKind::Drag(MouseButton::Left) => return self.drag_to(at),
            MouseEventKind::Up(MouseButton::Left) => self.release(),
            _ => return false,
        }
        true
    }

    /// Whether the event is for the program in the terminal, and if so,
    /// hand it over.
    fn mouse_to_terminal(&mut self, m: &MouseEvent, at: Position) -> bool {
        let (Some(area), Some(shell)) = (self.last_term_inner, self.term.as_mut()) else {
            return false;
        };
        // A drag that began in a file, or on a divider, stays its own,
        // wherever it goes.
        if !area.contains(at)
            || self.drag.is_some()
            || self.resizing.is_some()
            || self.window != Window::Main
        {
            return false;
        }
        if !matches!(self.mode, Mode::Normal) {
            return false;
        }
        let taken = shell.mouse(m.kind, at.x - area.x, at.y - area.y, m.modifiers);
        // A click hands it the keyboard as well, whoever has the mouse.
        if taken && matches!(m.kind, MouseEventKind::Down(_)) && self.focus != Focus::Terminal {
            self.open_terminal();
        }
        taken
    }

    /// How many presses in a row this one makes on the same cell: one, two
    /// or three.
    fn count_press(&mut self, at: Position) -> u8 {
        let count = match self.press {
            Some(p) if p.at == at && p.when.elapsed() < DOUBLE => (p.count % 3) + 1,
            _ => 1,
        };
        self.press = Some(Press {
            when: Instant::now(),
            at,
            count,
        });
        count
    }

    /// The left button going down at `at`.
    fn press(&mut self, at: Position, mods: KeyModifiers) {
        let count = self.count_press(at);
        self.drag = None;
        self.resizing = None;
        // What is over the window gets the click first.
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => {}
            // A click puts the help away, as any key does.
            Mode::Help(_) => return,
            Mode::Bar(b) => {
                if self.last_bar_area.is_some_and(|r| r.contains(at)) {
                    self.mode = Mode::Bar(b);
                    return;
                }
                if let Some((area, scroll)) = self.last_results
                    && area.contains(at)
                    && !b.is_command()
                {
                    let i = scroll + usize::from(at.y - area.y);
                    if let Some(hit) = b.results.get(i).cloned() {
                        self.jump_to(&hit);
                        self.status = display_name(&hit.path);
                    } else {
                        self.mode = Mode::Bar(b);
                    }
                    return;
                }
                // Anywhere else puts the bar away, as Esc does, and goes on
                // to whatever was clicked.
                if self.window == Window::Map {
                    self.filter_map("");
                }
            }
            // A question, the settings, the keybinds: answered with keys.
            other => {
                self.mode = other;
                return;
            }
        }
        // A line between two parts, before either part: its borders are
        // where it is taken hold of.
        if let Some(d) = self.last_dividers.iter().find(|d| d.hit.contains(at)) {
            let from = if d.across { at.x } else { at.y };
            let grab = i32::from(d.at) - i32::from(from);
            self.resizing = Some((d.clone(), grab));
            return;
        }
        match self.window {
            Window::Map => self.press_map(at),
            Window::Source => self.press_git(at),
            Window::Main => self.press_main(at, mods, count),
        }
    }

    fn press_main(&mut self, at: Position, mods: KeyModifiers, count: u8) {
        if self.last_term_area.is_some_and(|r| r.contains(at)) {
            if self.focus != Focus::Terminal {
                self.open_terminal();
            }
            return;
        }
        if let Some(inner) = self.last_tree_inner
            && inner.contains(at)
        {
            return self.press_tree(at, inner);
        }
        if self.over_tree(at.x) {
            self.focus_tree();
            return;
        }
        if let Some(i) = self.last_panes.iter().position(|p| p.area.contains(at)) {
            self.press_pane(i, at, mods, count);
        }
    }

    /// A row of the browser: the cursor to it, and a second time, open it.
    fn press_tree(&mut self, at: Position, inner: Rect) {
        let was_here = self.focus == Focus::Tree;
        self.focus_tree();
        let row = self.tree_scroll + usize::from(at.y - inner.y);
        if row >= self.rows.len() {
            return;
        }
        if row == self.selected && was_here {
            self.toggle_or_open();
        } else {
            self.select_index(row);
        }
    }

    /// A pane: the keyboard to it, and the cursor to where it was clicked.
    fn press_pane(&mut self, i: usize, at: Position, mods: KeyModifiers, count: u8) {
        let hit = self.last_panes[i].clone();
        if let Some(id) = hit.pinned {
            self.focus_pane(id);
        }
        if !matches!(self.preview, Preview::Buffer { .. }) {
            return;
        }
        self.focus = Focus::Editor;
        // A note drawn formatted, with the browser in charge: the keyboard is
        // all a click can give it, there being no lines on screen to aim at.
        let Some(text) = hit.text else {
            return;
        };
        let tab_width = self.config.tab_width;
        let Some(ed) = self.active_buffer_mut() else {
            return;
        };
        let p = pos_at(ed, &text, at, tab_width);
        if mods.contains(KeyModifiers::SHIFT) {
            let from = ed.anchor.unwrap_or(ed.pos());
            ed.click_extend(p);
            self.drag = Some(Drag {
                origin: (from, from),
                unit: Unit::Char,
            });
            return;
        }
        if mods.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) {
            ed.click_another(p);
            let n = ed.others.len() + 1;
            self.status = format!("{n} cursors");
            return;
        }
        // On the line numbers, a line at a time.
        let unit = match count {
            _ if at.x < text.area.x => Unit::Line,
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let origin = ed.unit_at(p, unit);
        if unit == Unit::Char {
            ed.click(p);
        } else {
            ed.drag(origin, p, unit);
        }
        self.drag = Some(Drag { origin, unit });
    }

    /// The left button moving while down: the divider it took hold of, or
    /// the selection in the pane the press was in, follows it.
    fn drag_to(&mut self, at: Position) -> bool {
        if let Some((d, grab)) = self.resizing.take() {
            let from = i32::from(if d.across { at.x } else { at.y });
            let to = (from + grab).clamp(i32::from(d.range.0), i32::from(d.range.1));
            self.move_divider(&d, to as u16);
            self.resizing = Some((d, grab));
            return true;
        }
        let Some(drag) = self.drag else {
            return false;
        };
        let Some(text) = self
            .last_panes
            .iter()
            .find(|p| p.pinned.is_none())
            .and_then(|p| p.text.clone())
        else {
            return false;
        };
        let tab_width = self.config.tab_width;
        let Some(ed) = self.active_buffer_mut() else {
            return false;
        };
        let p = pos_at(ed, &text, at, tab_width);
        ed.drag(drag.origin, p, drag.unit);
        true
    }

    /// Move `d` so the second of its two parts begins at `to`.
    ///
    /// Shares, not lengths, are what is kept, so a window made wider or
    /// narrower afterwards keeps the proportions and still has no part left
    /// over.
    fn move_divider(&mut self, d: &Divider, to: u16) {
        let share = |part: u16, of: u16| f32::from(part) / f32::from(of.max(1));
        match &d.moves {
            Moves::Browser { area, right } => {
                let width = if *right {
                    area.right() - to
                } else {
                    to - area.x
                };
                let width = share(width, area.width);
                self.config.tree_width = width.clamp(TREE_WIDTH_MIN, TREE_WIDTH_MAX);
                self.status = format!(
                    "browser {}% of the window",
                    (self.config.tree_width * 100.0).round()
                );
            }
            Moves::Terminal { area } => self.term_share = share(area.bottom() - to, area.height),
            Moves::Panes(line) => {
                if let Some(split) = &mut self.split {
                    split.layout.move_line(line, to);
                }
            }
            Moves::Changes { area } => {
                self.config.tree_width = share(to - area.x, area.width);
                self.status = format!(
                    "changes {}% of the window",
                    (self.config.tree_width * 100.0).round()
                );
            }
            Moves::Halves { area } => self.diff_share = share(to - area.x, area.width),
        }
    }

    /// The left button coming up: the end of a drag, and a click that moved
    /// nowhere selects nothing.
    fn release(&mut self) {
        self.resizing = None;
        if self.drag.take().is_some()
            && let Some(ed) = self.active_buffer_mut()
            && ed.anchor == Some(ed.pos())
        {
            ed.anchor = None;
        }
    }

    /// A box on the map: the cursor to it, and a second time, open it.
    fn press_map(&mut self, at: Position) {
        let Some(&(_, node)) = self.last_map_boxes.iter().find(|(r, _)| r.contains(at)) else {
            return;
        };
        let Some(view) = self.project_map.as_mut() else {
            return;
        };
        if view.selected != node {
            view.selected = node;
            self.status = format!("project map: {}", view.summary());
            return;
        }
        if let Some(path) = view.selected_node().map(|n| n.path.clone()) {
            self.window = Window::Main;
            self.open_path(&path);
        }
    }

    /// The git window: a row or a button picked, and a second time, done; a
    /// click on what is drawn on the right gives that the keyboard.
    fn press_git(&mut self, at: Position) {
        if self.git.diff_area.is_some_and(|r| r.contains(at)) {
            if self.git.message.is_some() {
                self.git.focus = GitFocus::Message;
            } else if !self.git.diff.is_empty() {
                self.git.focus = GitFocus::Diff;
            }
            return;
        }
        let Some(hit) = self
            .git
            .list_hits
            .iter()
            .find(|h| {
                h.y == at.y
                    && self
                        .git
                        .list_cols
                        .is_some_and(|(x0, x1)| at.x >= x0 && at.x < x1)
            })
            .cloned()
        else {
            return;
        };
        let was_list = self.git.focus == GitFocus::List;
        self.git.focus = GitFocus::List;
        let again = was_list && self.git.selected == hit.row;
        let button = hit
            .buttons
            .iter()
            .find(|(x0, x1, _)| at.x >= *x0 && at.x < *x1)
            .map(|b| b.2);
        if let Some(button) = button {
            let again = again && self.git.button == button;
            self.move_git_selection(hit.row as isize - self.git.selected as isize);
            self.git.button = button;
            if again {
                self.activate_git();
            }
            return;
        }
        if !hit.buttons.is_empty() {
            return;
        }
        if again {
            self.activate_git();
        } else {
            self.move_git_selection(hit.row as isize - self.git.selected as isize);
        }
    }
}
