# tiny

Tiny is a personal knowledge management system (PKMS) and IDE that works entirely in the terminal. This program exists to serve as a full, terminal based project manager that is as lightweight as programs like micro. Tiny has a built in text/code editor, picture/video previewer, file manager, and project map for projects.

Everything tiny manages and edits is non proprietary and not obfuscated.
## Install:

**One Liner:**
```sh
curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
```

**Checkout**:
```sh
git clone https://github.com/mattsaund/tiny.git && cd tiny && sh install.sh
```

the installer will offer to install rust and its dependencies if you do not have them. builds and drops `tiny` in `~/.local/bin`.
## Use (in terminal):

```sh
tiny                      # this folder, or the project it sits inside
tiny ~/Desktop/project1   # that folder — created, with a project in it, if missing
tiny ~/code/main.py       # that one file, in the editor, with its project beside it
tiny notes.txt            # same, and the file is written if it isn't there yet
```

To turn off the starting README.md in tiny projects change this in config

`starter_readme = false`.
## Controls and Commands:

Press `?` in the program: it lists every key and every command at once, side by side if the window is wide enough.

**Tree**

| key         | does                                       |
|-------------|--------------------------------------------|
| `↑` `i` `↓` `k`    | move the cursor                     |
| `Enter`     | open or close a folder, or edit a file     |
| `→` `l`     | open a folder, or step inside an open one  |
| `←` `j`     | close a folder, or jump to its parent      |
| `⇧↑` `⇧I` `⇧↓` `⇧K`   | first / last entry               |
| `^↑` `^↓`   | five entries at a time                     |
| `n`         | new — a dot in the name makes it a file    |
| `r`         | rename                                     |
| `^C` `^V`   | copy, and paste into the folder you are in |
| `^S`        | save — on a folder, everything unsaved in it|
| `d`         | delete (asks first)                        |
| `.`         | show or hide dotfiles                      |
| `F5`        | re-read from disk (or `*reload`)           |
| `^B`        | fold the tree away, and bring it back      |
| `q`         | quit                                       |


**Search and commands**

| key        | does                                        |
|------------|---------------------------------------------|
| `m`        | the project map                             |
| `/`        | the bar — searches names and contents       |
| `*`        | typed first, it is a command instead        |
| `,`        | the settings area                           |
| `Ctrl+F`   | the bar, from anywhere including the editor |
| `Ctrl+P`   | the same, with the `*` already typed        |


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

**Editor**

| key               | does                    |
| ----------------- | ----------------------- |
| `→` `l`           | into the file           |
| `Esc`             | back to the tree        |
| `Ctrl+S`          | save                    |
| `Ctrl+Z` `Ctrl+Y` | undo / redo             |
| `Ctrl+K`          | delete the current line |
| `Ctrl+←` `Ctrl+→` | move by word            |
| `Ctrl+↑` `Ctrl+↓` | five lines at a time    |
| `⇧←` `⇧→`         | start / end of the line |
| `⇧↑` `⇧↓`         | first / last line       |
| wheel             | one line per notch      |
| `*line 42`        | jump to a line          |


**Saving**
`Ctrl+s` saves whatever file you are working in or hovering over. If you `Ctrl+s` on
a directory it will save all files under that directory.

**The Project Map**

The project map `m` shows the entire project and linking between files in the project.
any links between files or functions that call other functions will form a link in the map.

```
tiny/  2 files
╭─────────╮                        ╭────────╮
│README.md│───────────────────────▸│LICENSE │
╰─────────╯                        ╰────────╯

src/  3 files
╭──────╮         ╭───────╮        ╭─────────╮
│cli.py│────────▸│main.py│──────┬▸│utils.py │
╰──────╯         ╰───────╯      │ ╰─────────╯
                                │
                     ───────────┘
```

| key       | does                                        |
|-----------|---------------------------------------------|
| arrows    | move to the nearest file that way (`i j k l`)|
| `Tab`     | step through every file in turn             |
| `Enter`   | open the file the cursor is on              |
| `1`-`3`   | wikilinks / md links / calls                |
| `o`       | show files connected to nothing             |
| `/`       | filter to matching paths                    |
| `r`       | build the map again                         |


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

Press `,` (or run `*config`) for the settings area. Two buttons sit at the top
of it:

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
editor.undo  = "ctrl+u"
map.orphans  = ""           # or none at all
```

Names are what the keybinds window shows in its left column. A key is written
the way it reads: `ctrl+s`, `shift+up`, `f5`, `enter`, `esc`, `pageup`, `.`, or
a single character. A capital letter *is* the shifted one — `I` is Shift+i.

Theme entries are style specs, so a line can carry weight as well as color:

`"bold"`, `"underline"`, `"white on black"`, `"#7dcfff bold"`, `"reverse"`.

```toml
starter_readme     = true
show_hidden        = false
tab_width          = 4
line_numbers       = true
syntax_theme       = "base16-ocean.dark"
max_search_results = 500
search_ignore      = [".git", "target", "node_modules", ".venv", "__pycache__"]
media_preview      = true
media_height       = 24
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

## Pictures and video

terminal needs to support 24-bit color to work with no graphics protocol.

Video shows a poster frame, pulled with `ffmpeg` when it is installed. 

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
| `graph.rs`     | the link graph: wikilinks, md links, calls          |
| `projectmap.rs`| the map you look at: its layout, boxes and keys     |
| `keys.rs`      | what every key does, and how to rebind it           |
| `config.rs`    | `tiny.conf`, style specs, the settings index        |

## Features:

**Current**
- Markdown text editor, easy editing like obsidian, perfectly viewable
- plaintext editor (.txt), no formatting
- pictures and videos viewer
- project map showing the connections and links between files
- full project searchbar and command caller

**Future**
- PDF Viewer/editor
- code editor with syntax highlighting for all languages.
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
