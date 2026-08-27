# tiny

Tiny is a personal knowledge management system (PKMS), IDE, and text editor that works entirely in the terminal.

Everything tiny manages and edits is non proprietary and not obfuscated.

## Install:
Full Installation Size: 7.12 MB

**One Liner:**
```sh
curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
```

**Checkout**:
```sh
git clone https://github.com/mattsaund/tiny.git && cd tiny && sh install.sh
```

the installer will offer to install rust and its dependencies if you do not have them, then shows a progress bar while it builds and drops `tiny` in `~/.local/bin`. Running it again over an existing install is how you update.

**Uninstall:**
```sh
tiny --uninstall
```

shows what it is about to delete — the binary, `tiny.conf`, and it asks before doing it. your notes and projects are not touched.

## Use (in terminal):

```sh
tiny                      # this folder, or the project it sits inside
tiny ~/Desktop/project1   # that folder — created empty if missing
tiny ~/code/main.py       # that one file, in the editor, with its project beside it
tiny notes.txt            # same, and the file is written if it isn't there yet

tiny --config             # where tiny.conf lives
tiny --licenses           # terms of the bundled syntax definitions
tiny --uninstall          # remove tiny, leaving your notes alone
```

## Controls and Commands:

Press `F1` — or `?` in the browser — and it lists every key and every command at once, side by side if the window is wide enough.

Every control that does something to a project is a **chord**, so it works while you are typing into a file as well as from the browser. Bare letters would not: in the editor `n` is the letter n, and always will be.

**Moving**

| key                                | does                                     |
|------------------------------------|------------------------------------------|
| `↑` `↓` `←` `→` — or `i` `k` `j` `l` | move                                   |
| `Ctrl+↑` `Ctrl+↓` `Ctrl+←` `Ctrl+→` | five at a time, or a word              |
| `Alt+↑` `Alt+↓` `Alt+←` `Alt+→` — or `I` `K` | to the ends                    |
| `Enter`                            | open or close a folder, or edit a file   |
| `Tab`                              | hand the keyboard to the file            |
| `Esc`                              | back — and from the browser, quit        |

`i` `j` `k` `l` are the arrows, in the inverted-T anyone who has held a keyboard sideways already knows. They work wherever a pane is being navigated rather than typed into: the browser, the map, a picture. Not in the editor, where they are letters.

**Files**

| key       | does                                        |
|-----------|---------------------------------------------|
| `Ctrl+N`  | new — a dot in the name makes it a file     |
| `Ctrl+R`  | rename                                      |
| `Ctrl+D`  | delete (asks first)                         |
| `Ctrl+S`  | save — on a folder, everything unsaved in it|
| `Ctrl+C` `Ctrl+V` | copy, and paste into the folder you are in |
| `Ctrl+.`  | show or hide dotfiles                       |
| `F5`      | re-read from disk (or `*reload`)            |

**Windows**

| key         | does                                        |
|-------------|---------------------------------------------|
| `Ctrl+/`    | the bar — searches names and contents       |
| `*`         | typed into the bar first, it is a command   |
| `Ctrl+P`    | the bar, with the `*` already typed         |
| `Ctrl+M`    | the project map                             |
| `Ctrl+,`    | the settings area                           |
| `F1`        | keys and commands                           |
| `Ctrl+Q`    | quit                                        |

**The browser pane**

| key       | does                                        |
|-----------|---------------------------------------------|
| `Alt+-` `Alt+=` | narrower / wider                          |
| `Ctrl+Space` | fold it away, and bring it back          |


```toml
[keys]
pane_narrower = "alt+["
pane_wider    = "alt+]"
```

The browser also keeps the short forms for the keys above: `n`, `r`, `d`, `.`, `m`, `/`, `?` and `,` all work there, where nothing is being typed.

**Search**

Results drop down from the top of the panes, so the browser stays beside you and the file behind keeps showing the hit. Arrow up and down through them and the preview follows, with **every occurrence of the word marked in the file itself** — so you can see where a hit sits before committing to it. Enter opens the file with the cursor on the match.

A search started from inside a file lists that file's hits first. When you search while writing, the word is usually one you just wrote.

Search is smart-cased: `widget` matches `Widget`, `Widget` matches only `Widget`. The marking follows the same rule, so what is highlighted is exactly what was matched.


```
*set tab_width 2              change a setting; *set <key> alone reports it
*set theme.heading cyan bold  repaint without a restart
*new LICENSE                  make a file, whatever the name looks like
*mkdir notes                  make a folder, likewise
*copy README.md to notes      copy into a folder, or to a new name
*copy notes to archive        a folder brings everything under it
*delete                       delete what the cursor is on, after confirming
*delete notes/old.md          delete a path, counted from the project root
*line 42                      jump to a line; *42 on its own does too
*replace old new              find-replace across the project, after confirming
*replace "old thing" "new"    quote anything with spaces in it
*map                          open the project map
*config                       open the settings area
*w  *q  *wq                   save, quit
*reload                       re-read the project from disk
*help
```

**Editing**

Everything in the tables above works here too. These are the keys that only mean something with a cursor in a file:

| key               | does                    |
| ----------------- | ----------------------- |
| `Tab` `→` `l`     | into the file           |
| `Esc`             | back to the browser     |
| `Ctrl+Z` `Ctrl+Y` | undo / redo             |
| `Ctrl+K`          | delete the current line |
| `Ctrl+←` `Ctrl+→` | move by word            |
| `Ctrl+↑` `Ctrl+↓` | five lines at a time    |
| `Alt+←` `Alt+→`   | start / end of the line |
| `Alt+↑` `Alt+↓`   | first / last line       |
| wheel             | one line per notch      |
| `*line 42`        | jump to a line          |


**Saving**
`Ctrl+S` saves whatever file you are working in or hovering over. If you `Ctrl+S` on
a directory it will save all files under that directory.

**The Project Map**

The project map `Ctrl+M` shows every file in the project, grouped under the folder it lives in, and draws what the file under the cursor is joined to.

Files that nothing reaches are grouped under `unconnected` at the bottom. Underneath, `out:` and `in:` name every file this one reaches and every file that reaches it — which is where direction lives, because the lines themselves are plain.

```
┌ PROJECT MAP  6/6 files | 4 links ────────────────────────────────────────┐
│src/  4 files                                                             │
│                                                                          │
│╭──────╮       ╭───────╮      ╭─────────╮    ╭────────╮                   │
││cli.py│       │main.py│──────│parser.py│────│utils.py│                   │
│╰──────╯       ╰───────╯      ╰─────────╯    ╰────────╯                   │
│                                                                          │
│unconnected  2 files                                                      │
│                                                                          │
│╭────────╮     ╭────────╮                                                 │
││notes.md│     │store.py│                                                 │
│╰────────╯     ╰────────╯                                                 │
└──────────────────────────────────────────────────────────────────────────┘
  src/parser.py  code
  out: 1   utils.py:load
  in:  1   main.py:read
  defines read
  1:wikilink 2:link 3:call   calls traced in Python, Rust, JavaScript
```

| key       | does                                        |
|-----------|---------------------------------------------|
| arrows    | move to the nearest file that way (`i j k l`)|
| `Tab`     | step through every file in turn             |
| `Enter`   | open the file the cursor is on              |
| `1`-`3`   | wikilinks / md links / calls                |
| `/`       | filter to matching paths                    |
| `r`       | build the map again                         |
| `Esc`     | back to the browser (or `Ctrl+M` again)     |


A line means two files are actually joined — a link you could click, or a
function one of them calls in the other:

| edge          | comes from                                        |
|---------------|---------------------------------------------------|
| **wikilink**  | `[[another-note]]`, in markdown *and* plain text  |
| **md link**   | `[text](../notes/spec.md)` — relative, not URLs   |
| **call**      | one file calling a function another defines       |


## Config:

The default config generated comes preloaded with these values. tiny also uses your specific terminal theme and palette. 

Everything is configurable, including where the panes and bars sit:

```toml
tree_side       = "left"    # or "right"
tree_width      = 0.30
search_position = "top"     # or "bottom"
status_position = "bottom"  # or "top"
borders         = true      # false for a plainer screen
markers         = "arrows"  # or "ascii"
```

### Settings and keys, from inside

Press `Ctrl+,` (or `,` in the browser, or run `*config`) for the settings area.
Two buttons sit at the top of it:

| button           | does                                                     |
|------------------|----------------------------------------------------------|
| `Keybinds`      | opens a window listing every action and the keys that reach it |
| `Reset settings` | puts every setting back to what tiny ships with, after asking |


### The config file

| platform      | where                                          |
|---------------|------------------------------------------------|
| Linux / macOS | `~/.config/tiny/tiny.conf`                     |
| Windows       | `%APPDATA%\tiny\tiny.conf`                     |

`$XDG_CONFIG_HOME` wins over both when it is set.

Keys live in a `[keys]` section, by action name, and only what you have changed
needs to be there — anything left out is whatever tiny ships with, including a
binding added in a later version:

```toml
[keys]
tree.down    = "z"          # one key
tree.up      = "up i w"     # or several, space separated
new          = "ctrl+e"     # the chords have plain names
editor.undo  = "ctrl+u"
map.reload   = ""           # or none at all
```

Names without a prefix are the chords that work everywhere: `save`, `quit`,
`bar`, `command`, `new`, `rename`, `delete`, `copy`, `paste`, `hidden`, `map`,
`reload`, `help`, `settings`, `fold_tree`, `pane_narrower`, `pane_wider`. A
prefix names the pane a key only works in — `tree.`, `view.`, `editor.`,
`map.`.

Names are what the keybinds window shows in its left column. A key is written
the way it reads: `ctrl+s`, `alt+up`, `ctrl+alt+left`, `f5`, `enter`, `esc`,
`pageup`, `.`, or a single character. A capital letter *is* the shifted one —
`I` is Shift+i.

Theme entries are style specs, so a line can carry weight as well as color:

`"bold"`, `"underline"`, `"white on black"`, `"#7dcfff bold"`, `"reverse"`.

```toml
show_hidden        = false
tab_width          = 4
line_numbers       = true
syntax_theme       = "base16-ocean.dark"
max_search_results = 500
search_ignore      = [".git", "target", "node_modules", ".venv", "__pycache__"]
prose_extensions   = ["md", "txt", "rst", "org", "log"]
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


## Source layout

`src/` is one file for startup and six folders, one per layer. Every folder's
`mod.rs` opens with docs explaining what the folder is for and how its files
divide the work between them — that is the place to start.

| folder / file | holds |
|---------------|-------|
| `main.rs` | CLI, terminal setup, the event loop, the uninstaller |
| **`app/`** | **state: one `App`, and every keypress** |
| `app/mod.rs` | the `App` struct and the questions anyone can ask it |
| `app/mode.rs` | the overlays: the bar, prompts, confirmations, settings |
| `app/preview.rs` | what the cursor is on, and what the right pane becomes |
| `app/input.rs` | every keypress, dispatched; and the mouse wheel |
| `app/bar.rs` | the one field that is both a search and a command line |
| `app/command.rs` | what each `*command` does |
| `app/fileops.rs` | new, rename, delete, copy, paste, save |
| `app/actions.rs` | the plain navigation keys and the view toggles |
| `app/settings.rs` | the settings area and the keybinds window |
| `app/prompt.rs` | answering a prompt or a confirmation |
| `app/parts.rs` | small helpers more than one of those needs |
| `app/tests/` | the fixtures, and most of the suite — one file per area |
| **`ui/`** | **all drawing, one file per pane** |
| `ui/mod.rs` | the layout, and which module fills each rectangle |
| `ui/parts.rs` | the border, the selected row, the marking of a hit |
| `ui/tree.rs` | the browser pane, and the results that drop down in front |
| `ui/preview.rs` | the right pane, and what it decides to be |
| `ui/editor.rs` | the file, with the real cursor in it |
| `ui/map.rs` | the project map pane |
| `ui/ink.rs` | the box-drawing grid, and the routing that fills it |
| `ui/bar.rs` | the bar and the status line |
| `ui/help.rs` | every key and command, on one screen |
| `ui/settings.rs` | the settings and keybinds overlays |
| **`text/`** | **text, and what can be done to it** |
| `text/editor/mod.rs` | the buffer, the cursor, and reading and writing the file |
| `text/editor/edit.rs` | everything that changes the text |
| `text/editor/motion.rs` | everything that moves the cursor |
| `text/editor/undo.rs` | the history, and the grouping that makes it usable |
| `text/markdown/mod.rs` | the entry points, and the block splitter |
| `text/markdown/render.rs` | the event stream, turned into styled rows |
| `text/markdown/wrap.rs` | fitting those rows into the pane, styles intact |
| `text/highlight.rs` | syntax highlighting, and the parser-state cache |
| `text/search.rs` | project-wide search and find-replace |
| **`files/`** | **things on disk** |
| `files/tree.rs` | the lazily-loaded directory model |
| `files/project.rs` | what `tiny <thing>` meant, and project creation |
| `files/media/mod.rs` | what a picture or video is, and how to open it |
| `files/media/size.rs` | pixel dimensions, read straight out of a file header |
| **`map/`** | **what links to what, and the screen that shows it** |
| `map/graph.rs` | the link graph: wikilinks, md links, calls |
| `map/scan.rs` | reading one file and saying what is in it |
| `map/view.rs` | what is on the map, and what the keyboard does to it |
| `map/layout.rs` | where every box goes |
| **`config/`** | **settings and key bindings** |
| `config/mod.rs` | `tiny.conf` and the settings index |
| `config/theme.rs` | style specs, and the palette they parse into |
| `config/keys.rs` | what every key does, and how to rebind it |
| `config/keyspec.rs` | `"ctrl+space"`, and the event it matches |

Tests live beside the code they cover, except `app`'s, which are large enough
to have a folder of their own. `map/testing.rs` and `text/markdown/testing.rs`
hold fixtures two files there share.

## Features:

**Current**
- Markdown text editor, easy editing like obsidian, perfectly viewable
- plaintext editor (.txt), no formatting
- code editor with syntax highlighting for 213 languages
- pictures and videos described in the pane, opened in your own viewer
- project map showing the connections and links between files
- full project searchbar and command caller, matches marked in the preview
- every control on a chord, so the whole program works from inside a file
- a resizable browser pane, from most of the window to none of it

**Future**
- PDF Viewer/editor
- source control and github integration
- local AI implementation
- HTML Viewer with local web server
- Window tiling manager allowing user to open up multiple windows in the tui. shift+arrow keys to move around to different windows. 
- apt/brew install support
## Contributer Rules and Procedures:

**AI Policy**

I am open to AI and agentic coding, but the code written needs to follow specific guidelines:
1. MUST be human readable, acceptable variable/function names.
2. easily tracable, following a good program flow
3. Contributer MUST look at/document code and code changes. you need to understand the code that is being written.

**Tests**

```sh
cargo test
```

The suite renders real frames through ratatui's test backend, so navigation, editing, saving, search, commands and the settings area are covered without a terminal. To look at a project as tiny draws it:

```sh
TINY_SHOT=path/to/project cargo test screenshot -- --ignored --nocapture
```

Issues and pull requests welcome. `cargo test` should pass and `cargo clippy` should be quiet before you open one; CI checks both, along with `cargo fmt`.

## Licence and Credits

Created by Matthew Saunders https://msaunders.dev
MIT. See [LICENSE](LICENSE).

Syntax definitions come from the [bat](https://github.com/sharkdp/bat) project by way of [two-face](https://codeberg.org/CosmicHarper/two-face). They are third-party files with their own terms, which `tiny --licenses` prints.
