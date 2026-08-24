# tiny

A personal knowledge manager that lives entirely in the terminal. Files on the
left, whatever you are hovering over on the right — notes and prose render,
code opens in a micro-like editor, pictures draw. Press `w` and the project
opens in a browser as a graph of what links to what, notes and code alike.

Everything it manages is a plain file in a plain folder. Nothing is locked in
a database.

```
 / config   up/down pick · Enter jump · Esc close
┌ 2 MATCHES ─────────────────────────────┐┌ utils.py  VIEW ───────────────────────┐
│src/utils.py:6 def load_config(path: Pat││ 5                                     │
│src/utils.py:7     """Read a JSON config││ 6 def load_config(path: Path) -> dict:│
│                                        ││ 7     """Read a JSON config."""       │
│                                        ││ 8     if not path.exists():           │
│                                        ││ 9         return {}                   │
└────────────────────────────────────────┘└───────────────────────────────────────┘
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
```

Or from a checkout:

```sh
git clone https://github.com/mattsaund/tiny.git && cd tiny && sh install.sh
```

Either way it offers to install Rust if you do not have it, builds, and drops
`tiny` in `~/.local/bin`.

Anything can be overridden with the environment:

| variable      | does                                  |
|---------------|---------------------------------------|
| `TINY_REPO`   | git URL to clone (a local path works) |
| `TINY_REF`    | branch or tag (default `main`)        |
| `TINY_PREFIX` | where the binary lands                |

That is also how to rehearse a change to the installer without pushing it —
serve the script locally and point it at your own checkout:

```sh
python3 -m http.server 8771 &
curl -fsSL http://127.0.0.1:8771/install.sh | TINY_REPO="$PWD" sh
```

## Use

```sh
tiny                      # this folder, or the project it sits inside
tiny ~/Desktop/project1   # that folder — created, with a project in it, if missing
tiny ~/code/main.py       # that file's folder, with the file already in the editor
```

Any folder you open without a project gets one: a `.tiny/` directory holding
an optional per-project `tiny.conf`, and nothing else. It never writes a README
or a welcome note into a folder that already has work in it. Turn the whole
thing off with `auto_init = false`.

## Keys

Press `?` inside for the same list.

**Tree**

| key         | does                                  |
|-------------|---------------------------------------|
| `↑` `↓`     | move the cursor (`k` `j` also work)   |
| `→` `Enter` | open a folder, or edit a file         |
| `←`         | close a folder, or jump to its parent |
| `g` `G`     | first / last entry                    |
| `n` `N`     | new file / new folder                 |
| `r`         | rename                                |
| `d`         | delete (asks first)                   |
| `.`         | show or hide dotfiles                 |
| `R` `F5`    | re-read the project from disk         |
| `q`         | quit                                  |

A new file's name may contain slashes — `journal/2026/aug.md` creates the
folders on the way down.

**Search and commands**

| key        | does                                       |
|------------|--------------------------------------------|
| `w`        | open the web view in a browser             |
| `/`        | search names and contents across the project |
| `:`        | commands                                   |
| `,` `F2`   | the settings area                          |
| `Ctrl+F`   | search, from anywhere including the editor |
| `Ctrl+P`   | commands, from anywhere                    |

Search runs as you type. Results replace the tree, the preview follows the
highlighted one, and `Enter` jumps to it with the cursor on the match.

Commands, with `Tab` to complete:

```
:set tab_width 2              change a setting; :set <key> alone reports it
:set theme.heading cyan bold  repaint without a restart
:replace old new              find-replace across the project, after confirming
:replace "old thing" "new"    quote anything with spaces in it
:graph  :web                  open the web view
:config                       open the settings area
:w  :q  :wq                   save, quit
:help  :reload  :init
```

**Editor**

| key                 | does                    |
|---------------------|-------------------------|
| `Ctrl+S`            | save                    |
| `Ctrl+Z` `Ctrl+Y`   | undo / redo             |
| `Ctrl+K`            | delete the current line |
| `Ctrl+←` `Ctrl+→`   | move by word            |
| `Esc`               | back to the tree        |

Notes open rendered; `e` switches to raw source. Code skips the rendered view.
Unsaved buffers survive navigating away and back, are marked `*` in both panes,
and quitting with unsaved work asks first.

## The web view

Press `w`, or run `:graph`. tiny serves a page on loopback that draws the
project as a graph you can pan, zoom and drag. **Click a file there and the
terminal jumps to it**, which is the point — the graph is a way of moving
around the project, not a picture of it.

Four kinds of connection, drawn together:

| edge          | comes from                                        |
|---------------|---------------------------------------------------|
| **wikilink**  | `[[another-note]]`, in markdown *and* plain text  |
| **md link**   | `[text](../notes/spec.md)` — relative, not URLs   |
| **import**    | `import utils`, `mod helpers;`, `from './lib.js'` |
| **call**      | one file calling a function another defines       |

**Imports and links are exact.** Calls are matched by name, using tree-sitter's
tags queries — currently **Python, Rust and JavaScript**. Other languages still
appear as files, they just have no outgoing call edges.

Name matching is a heuristic, and it behaves like one:

- Test code is left out — `test_*.py`, `*.spec.js`, anything under `tests/`, and
  Rust's trailing `#[cfg(test)]` module. Test helpers are named `render`,
  `fixture` and `plain`, and they collide with real names constantly.
- A name defined in more files than `graph_max_ambiguity` (default 3) says
  nothing about which was meant, so it draws no edge at all.
- Declarations are not calls: `mod parser;` never resolves a `parser()`.

What remains are common method names. Calling `.count()` on an iterator looks
exactly like calling a `count()` your project defines, and without full type
resolution nothing can tell them apart. If that noise gets in the way, untick
**calls** in the page and you are left with imports and links, which are exact.

The page is self-contained — no CDN, no network — and the server binds to
`127.0.0.1` only. It refuses any path that points outside the project.

## Prose and plain text

Markdown is not special-cased. Anything in `prose_extensions` — `.txt`, `.rst`,
`.org`, `.log` and friends, plus `LICENSE`-style files with no extension — opens
**wrapped and readable** rather than in a line-numbered editor, with
`[[wikilinks]]` and bare URLs picked out. Press `e` for the raw source, exactly
as with a note.

Everything else that is text is code: straight into the editor, with line
numbers and syntax highlighting. Move the line between them with
`:set prose_extensions md txt csv`.

## Design

Silent by default. The shipped theme names no colour at all — tiny renders in
whatever palette your terminal already uses, and meaning is carried by weight,
underline and reverse video. No logos, no icons, no emoji. Syntax highlighting
is the one place colour earns its keep, and its theme is configurable.

Everything is configurable, including where the panes and bars sit:

```toml
tree_side       = "left"    # or "right"
tree_width      = 0.30
search_position = "top"     # or "bottom"
status_position = "bottom"  # or "top"
borders         = true      # false for a plainer screen
markers         = "arrows"  # or "ascii"
```

## Config

`~/.config/tiny/tiny.conf`, written on first run. A project may override it
with its own `.tiny/tiny.conf`. Any field may be left out; the defaults fill in.
`tiny --config` prints the path.

Theme entries are style specs, so a line can carry weight as well as colour:
`"bold"`, `"underline"`, `"white on black"`, `"#7dcfff bold"`, `"reverse"`.

```toml
auto_init          = true
show_hidden        = false
tab_width          = 4
line_numbers       = true
syntax_theme       = "base16-ocean.dark"
max_search_results = 500
search_ignore      = [".git", "target", "node_modules", ".venv", "__pycache__"]
media_preview      = true
media_height       = 24
prose_extensions   = ["md", "txt", "rst", "org", "log"]
web_port           = 0     # 0 lets the OS pick a free port
graph_max_ambiguity = 3    # definitions before a name stops linking

[theme]
text         = "default"
dim          = "darkgray"
border       = "darkgray"
border_focus = "white"
selection    = "reverse"
directory    = "bold"
heading      = "bold"
link         = "underline"
code         = "dim"
marker       = "bold"
```

## Pictures and video

Images are drawn as coloured half-blocks — each cell holds a `▀` whose
foreground is the upper pixel and background the lower one. That works in any
terminal with 24-bit colour, with no graphics protocol to detect.

Video shows a poster frame, pulled with `ffmpeg` when it is installed. Without
it the pane says so instead of pretending the feature is missing.

## Source layout

| file           | holds                                               |
|----------------|-----------------------------------------------------|
| `main.rs`      | CLI, terminal setup, event loop                     |
| `project.rs`   | what `tiny <thing>` meant, and project creation     |
| `app.rs`       | state, keys, commands, file operations              |
| `ui.rs`        | all drawing                                         |
| `tree.rs`      | the lazily-loaded directory model                   |
| `editor.rs`    | the text buffer, cursor, and undo                   |
| `search.rs`    | project-wide search and find-replace                |
| `markdown.rs`  | markdown → styled terminal lines, wikilink scanning |
| `highlight.rs` | syntax highlighting via syntect                     |
| `media.rs`     | pictures and video frames as half-blocks            |
| `graph.rs`     | the link graph: wikilinks, imports, calls           |
| `web.rs`       | the loopback server behind the web view            |
| `config.rs`    | `tiny.conf`, style specs, the settings index        |

## Tests

```sh
cargo test
```

The suite renders real frames through ratatui's test backend, so navigation,
editing, saving, search, commands and the settings area are covered without a
terminal. To look at a project as tiny draws it:

```sh
TINY_SHOT=path/to/project cargo test screenshot -- --ignored --nocapture
```

## Next

TypeScript, Go and C in the call graph — each needs its grammar crate and a
tags query, and the rest of the machinery already works for any language that
has one.

After that, a local model as a completion sidekick — local only, and silent
unless asked.
