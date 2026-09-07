//! The source-control window: `Ctrl+2`.
//!
//! Three parts, laid out the way the questions are asked. On the left, what has
//! changed. On the right, the file the cursor is on — before on one side, after
//! on the other. Underneath, the branch map, because "what have I done" and
//! "where am I" are the two halves of the same look.
//!
//! # Reading across, not down
//!
//! The two halves of the diff are drawn from [`crate::git::Diff`], whose sides are the
//! same length by construction: where one has a line the other does not, the
//! other has a blank. So line 12 on the left is the same place in the file as
//! line 12 on the right, and the eye can read across without counting.
//!
//! # Colour is the code
//!
//! The letter and the colour say the same thing twice on purpose — the letters
//! are git's own, and on a terminal with no colour the window still works.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::parts::highlight_row;

use crate::app::{App, GIT_BUTTONS, GitFocus, GitRow};
use crate::config::Palette;
use crate::git::{Code, DiffLine, LineKind};
use crate::text::highlight::Piece;

/// How many rows the branch map gets, when there is room for it at all.
const GRAPH_HEIGHT: u16 = 8;

/// The colour of a status letter.
///
/// Git's own scheme is two colours — green for staged, red for not — which
/// says where a change is but not what it is. These say what it is, which is
/// the question the letter answers too.
fn code_style(code: Code, pal: &Palette) -> Style {
    let colour = match code {
        Code::Added => Color::Green,
        Code::Modified => Color::Yellow,
        Code::Deleted => Color::Red,
        Code::Renamed => Color::Cyan,
        Code::Copied => Color::Magenta,
        Code::Untracked => Color::Blue,
        Code::Conflict => Color::LightRed,
    };
    pal.text.fg(colour).add_modifier(Modifier::BOLD)
}

pub(super) fn draw_source(f: &mut Frame, app: &mut App, area: Rect) {
    let pal = app.palette;

    // The map goes at the bottom and gives up its space first: on a short
    // window, what has changed matters more than what happened before.
    let graph_h = if area.height >= 20 && !app.git.graph.is_empty() {
        GRAPH_HEIGHT
    } else {
        0
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(graph_h)])
        .split(area);

    let width = app.config.tree_width.clamp(0.15, 0.5);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((width * 100.0) as u16),
            Constraint::Min(20),
        ])
        .split(rows[0]);

    draw_changes(f, app, columns[0], &pal);
    // The message box stands in for both diff columns rather than squeezing in
    // beside them: writing is the only thing happening while it is open, and a
    // paragraph wants the width.
    if app.git.message.is_some() {
        draw_message(f, app, columns[1], &pal);
    } else {
        draw_diff(f, app, columns[1], &pal);
    }
    if graph_h > 0 {
        draw_graph(f, app, rows[1], &pal);
    }
}

/// The left column: buttons, then what has changed, in two sections.
///
/// Steps back when the keyboard is not in it, exactly as the browser does in
/// the main window: dim border, dim title, and the cursor's row shown quietly
/// rather than reversed. Two panes that both hold a list have to say "you are
/// not typing here" the same way, or the answer to "which one is live" becomes
/// a thing you work out instead of see.
fn draw_changes(f: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let git = &app.git;
    let focused = git.focus == GitFocus::List;
    let after = match (&git.error, &git.job) {
        (Some(_), _) => String::new(),
        (_, Some(job)) => format!(" {}… ", job.what),
        _ => format!(" {} ", git.status.branch.summary()),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            pal.border_focus
        } else {
            pal.border
        })
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "SOURCE CONTROL",
                if focused {
                    pal.text.add_modifier(Modifier::BOLD)
                } else {
                    pal.dim
                },
            ),
            Span::styled(after, pal.dim),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(error) = &git.error {
        let lines = vec![
            Line::from(Span::styled(format!("  {error}"), pal.marker)),
            Line::from(""),
            Line::from(Span::styled("  Ctrl+1 back to the browser", pal.dim)),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in git.rows.iter().enumerate() {
        let on = i == git.selected;
        match row {
            GitRow::Buttons => {
                lines.extend(button_lines(git.button, on && focused, inner.width, pal))
            }
            GitRow::Header { staged } => {
                let (word, n) = if *staged {
                    ("STAGED", git.status.staged.len())
                } else {
                    ("UNSTAGED", git.status.unstaged.len())
                };
                let mut spans = vec![Span::styled(format!(" {word} {n}"), pal.heading)];
                if on {
                    highlight_row(&mut spans, inner.width as usize, *pal, focused);
                }
                lines.push(Line::from(spans));
            }
            GitRow::File { staged, index } => {
                let list = if *staged {
                    &git.status.staged
                } else {
                    &git.status.unstaged
                };
                let Some(change) = list.get(*index) else {
                    continue;
                };
                let mut spans = vec![
                    Span::styled(
                        format!(" {} ", change.code.letter()),
                        code_style(change.code, pal),
                    ),
                    Span::styled(change.label(), pal.text),
                ];
                // The same full-width bar the browser draws, and the same
                // quiet version of it when the keyboard is elsewhere.
                if on {
                    highlight_row(&mut spans, inner.width as usize, *pal, focused);
                }
                lines.push(Line::from(spans));
            }
        }
    }
    if git.rows.len() <= 1 {
        lines.push(Line::from(Span::styled("  nothing to commit", pal.dim)));
    }
    // Written back the way every other pane's height is: the key handler
    // cannot know how tall this is until it has been drawn.
    app.git.last_height = inner.height as usize;
    f.render_widget(Paragraph::new(lines), inner);
}

/// The button row, wrapped to the pane's width.
///
/// Wrapping is a drawing decision and nothing more: the buttons are one row as
/// far as the cursor is concerned, and left and right walk them however many
/// lines they take to show.
fn button_lines(selected: usize, on_row: bool, width: u16, pal: &Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for (i, button) in GIT_BUTTONS.iter().enumerate() {
        let label = format!(" {} ", button.label());
        let w = label.chars().count() + 1;
        if used + w > width as usize && !spans.is_empty() {
            lines.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        let style = match (on_row, i == selected) {
            (true, true) => pal.text.add_modifier(Modifier::REVERSED),
            (true, false) => pal.text,
            _ => pal.dim,
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        used += w;
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// The right column: the file before, and the file after.
///
/// Both sides are syntax-highlighted, by the grammar the file's own name picks
/// — a diff of a `.rs` file reads as Rust on both sides. The two sides are
/// highlighted separately, because they *are* two different texts: a line that
/// opens a string on the right may not exist on the left at all.
///
/// Only the visible window is parsed for colour, the same way the editor does
/// it, so a diff of a very long file costs what is on screen rather than what
/// is in the file.
fn draw_diff(f: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let git = &app.git;
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let name = git
        .current()
        .map(|(c, staged)| {
            let where_ = if staged { "staged" } else { "not staged" };
            format!(" {}  {} ", c.label(), where_)
        })
        .unwrap_or_else(|| " nothing selected ".to_string());

    // The grammar comes from the file being looked at, so both halves agree.
    let syntax = git
        .current()
        .map(|(change, _)| {
            let path = std::path::Path::new(&change.path);
            let first = git
                .diff
                .after
                .iter()
                .chain(git.diff.before.iter())
                .find(|l| l.kind != LineKind::Filler)
                .map(|l| l.text.as_str())
                .unwrap_or("");
            app.highlighter.syntax_for_path(path, first).clone()
        })
        .unwrap_or_else(|| app.highlighter.syntax_for_token("").clone());

    for (i, half) in halves.iter().enumerate() {
        let side = if i == 0 { "before" } else { "after" };
        let title = if i == 0 {
            format!(" {side} ")
        } else {
            format!(" {side} —{name}")
        };
        // The border says which half the arrow keys are in, the same way the
        // two panes of the main window do.
        let border = if git.focus == GitFocus::Diff {
            pal.border_focus
        } else {
            pal.border
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title(Line::from(Span::styled(title, pal.dim)));
        let inner = block.inner(*half);
        f.render_widget(block, *half);

        if git.diff.is_empty() {
            let word = match git.current() {
                Some((c, _)) if c.code == Code::Deleted && i == 1 => "the file is gone",
                Some(_) => "no lines changed",
                None => "",
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(format!("  {word}"), pal.dim))),
                inner,
            );
            continue;
        }
        let lines = if i == 0 {
            &git.diff.before
        } else {
            &git.diff.after
        };
        // Syntect wants plain lines; a filler is a line that is not there, and
        // an empty string is the closest true thing to hand it.
        let plain: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
        let coloured = app.highlighter.highlight_window(
            &plain,
            &syntax,
            git.diff_scroll,
            inner.height as usize,
        );
        let rows: Vec<Line> = lines
            .iter()
            .enumerate()
            .skip(git.diff_scroll)
            .take(inner.height as usize)
            .map(|(n, l)| {
                let pieces = coloured.get(n - git.diff_scroll);
                diff_line(l, pieces, i == 0, gap_label(lines, n, i == 0), pal)
            })
            .collect();
        f.render_widget(Paragraph::new(rows), inner);
    }
    // Written back for the same reason every other height is: paging cannot
    // know how tall the columns are until they have been drawn.
    let height = halves[0].height.saturating_sub(2) as usize;
    app.git.diff_height = height;
}

/// What to write on the first line of a run of fillers, and nothing on the rest
/// of them.
///
/// A gap on one side is lines the other side has, so the two stay level. Marking
/// every line of it — which is what a `~` per row was — turns a twelve-line
/// addition into a twelve-row column of marks sitting where the file's own text
/// should be, and the eye reads it as content. One quiet line saying how many
/// lines are missing and why says the same thing and then gets out of the way.
///
/// The count is of the whole run, so it is the same number wherever the run is
/// cut off by the top of the pane.
fn gap_label(lines: &[DiffLine], n: usize, before: bool) -> Option<String> {
    if lines.get(n).map(|l| l.kind) != Some(LineKind::Filler) {
        return None;
    }
    // Mid-run: the line above already said it.
    if n > 0 && lines[n - 1].kind == LineKind::Filler {
        return Some(String::new());
    }
    let run = lines[n..]
        .iter()
        .take_while(|l| l.kind == LineKind::Filler)
        .count();
    let plural = if run == 1 { "" } else { "s" };
    // A gap on the left is what the right gained; a gap on the right is what
    // it lost.
    let what = if before { "added" } else { "removed" };
    Some(format!("· {run} line{plural} {what}"))
}

/// One line of one side: a two-character gutter, then the line itself in
/// whatever colours its grammar gives it.
///
/// The gutter is what carries the diff — `-` red on the left, `+` green on the
/// right — because the text is already spoken for by the syntax colours, and a
/// line cannot be two colours at once. Keeping them apart means a change is
/// still obvious at a glance *and* the code still reads as code.
///
/// A filler is the absence of a line rather than an empty one; the run it
/// belongs to says so once, on its first row — see [`gap_label`].
fn diff_line(
    line: &DiffLine,
    pieces: Option<&Vec<Piece>>,
    before: bool,
    gap: Option<String>,
    pal: &Palette,
) -> Line<'static> {
    if let Some(label) = gap {
        return Line::from(Span::styled(label, pal.dim));
    }
    let gutter = match (line.kind, before) {
        (LineKind::Changed, true) => {
            Span::styled("- ", pal.text.fg(Color::Red).add_modifier(Modifier::BOLD))
        }
        (LineKind::Changed, false) => {
            Span::styled("+ ", pal.text.fg(Color::Green).add_modifier(Modifier::BOLD))
        }
        _ => Span::styled("  ", pal.dim),
    };
    let mut spans = vec![gutter];
    match pieces {
        Some(pieces) if !pieces.is_empty() => spans.extend(
            pieces
                .iter()
                .map(|(style, text)| Span::styled(text.clone(), *style)),
        ),
        // No grammar matched, or the line is past what syntect will parse.
        _ => spans.push(Span::styled(line.text.clone(), pal.text)),
    }
    Line::from(spans)
}

/// The commit message, over the width of both diff columns.
///
/// Drawn from a real [`Editor`](crate::text::editor::Editor), so it has a
/// cursor you can see and a keyboard that works the way the editor pane's does.
/// The first line is shown apart from the rest because git treats it apart:
/// it is the summary every log and every UI shows on its own.
fn draw_message(f: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let Some(editor) = &app.git.message else {
        return;
    };
    let staged = app.git.status.staged.len();
    let files = if staged == 1 { "file" } else { "files" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border_focus)
        .title(Line::from(vec![
            Span::styled(" commit message ", pal.text.add_modifier(Modifier::BOLD)),
            Span::styled(format!("— {staged} staged {files} "), pal.dim),
        ]))
        .title_bottom(Line::from(Span::styled(
            " Ctrl+S commits · Esc puts it down ",
            pal.dim,
        )));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (n, text) in editor.lines().iter().enumerate() {
        if n >= inner.height as usize {
            break;
        }
        // The summary line reads as the title it is; the body is body text.
        let style = if n == 0 {
            pal.text.add_modifier(Modifier::BOLD)
        } else {
            pal.text
        };
        lines.push(Line::from(Span::styled(format!(" {text}"), style)));
    }
    // An empty box says what it is for rather than sitting there blank.
    if editor.lines().iter().all(String::is_empty) {
        lines = vec![Line::from(Span::styled(
            " a line saying what changed, then a blank line, then why",
            pal.dim,
        ))];
    }
    f.render_widget(Paragraph::new(lines), inner);

    // The real cursor, so typing looks like typing.
    let x = inner.x
        + 1
        + editor
            .cursor_col
            .min(inner.width.saturating_sub(2) as usize) as u16;
    let y = inner.y
        + editor
            .cursor_line
            .min(inner.height.saturating_sub(1) as usize) as u16;
    f.set_cursor_position((x, y));
}

/// The branch map, exactly as `git log --graph` drew it.
fn draw_graph(f: &mut Frame, app: &App, area: Rect, pal: &Palette) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border)
        .title(Line::from(Span::styled(" branches ", pal.dim)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines: Vec<Line> = app
        .git
        .graph
        .iter()
        .take(inner.height as usize)
        .map(|row| {
            // The commit the window is on is the one at the top of the graph;
            // git marks it itself with `HEAD`, so nothing here has to guess.
            let style = if row.contains("HEAD") {
                pal.text
            } else {
                pal.dim
            };
            Line::from(Span::styled(format!(" {row}"), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
