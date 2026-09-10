# tiny

Tiny is a personal knowledge management system (PKMS), IDE, and text editor that works entirely in the terminal.

Everything tiny manages and edits is non proprietary and not obfuscated.

## Install:
Full Installation and binary size: 7.54 MB

**One Liner** (Linux, macOS):
```sh
curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
```

**One Liner** (Windows):
```powershell
irm https://raw.githubusercontent.com/mattsaund/tiny/main/install.ps1 | iex
```

**Checkout**:
```sh
git clone https://github.com/mattsaund/tiny.git && cd tiny && sh install.sh
```

**Uninstall:**
```sh
tiny --uninstall
```

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

**Moving**

| key                                | does                                     |
|------------------------------------|------------------------------------------|
| `↑` `↓` `←` `→` — or `i` `k` `j` `l` | move                                   |
| `Ctrl+↑` `Ctrl+↓` | five at a time — and in a file, `Ctrl+←` `Ctrl+→` are a word |
| `Home` `End` — or `I` `K`          | to the ends of a list, or of a line      |
| `Ctrl+Home` `Ctrl+End`             | the first and last line of a file        |
| `Enter`                            | open or close a folder, or edit a file   |
| `Tab`                              | hand the keyboard to the file            |
| `Esc`                              | back — and from the browser, quit        |

**Files**

| key       | does                                        |
|-----------|---------------------------------------------|
| `Ctrl+S`  | save — on a folder, everything unsaved in it|
| `Ctrl+C` `Ctrl+V` | copy, and paste into the folder you are in |
| `.`       | show or hide dotfiles (in the browser)      |

**Windows**

| key         | does                                        |
|-------------|---------------------------------------------|
| `Ctrl+/`    | the bar — searches names and contents (`/` in the browser) |
| `*`         | typed into the bar first, it is a command   |
| `Ctrl+P`    | the bar, with the `*` already typed         |
| `F1`        | keys and commands                           |
| `Ctrl+Q`    | quit — offers to save anything unsaved      |

**Windows**

| key      | window                                      |
|----------|---------------------------------------------|
| `Ctrl+1` | the browser and the file                    |
| `Ctrl+2` | git: what has changed, and what to do about it |
| `Ctrl+3` | the project map                             |

**The browser pane**

| key                    | does                                |
|------------------------|-------------------------------------|
| `Ctrl+←` `Ctrl+→`      | narrower / wider                    |
| `Ctrl+Space`           | fold it away, and bring it back     |


```toml
[keys]
tree.narrower = "ctrl+["
tree.wider    = "ctrl+]"
```

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
*rename old.md to new.md      rename, or move between folders
*rename new.md                just a name renames what the cursor is on
*replace "old thing" "new"    quote anything with spaces in it
*commit [message]             commit what is staged
*reload                       re-read the project from disk now
*config                       open the settings area
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
| `Home` `End`      | start / end of the line |
| `Ctrl+Home` `Ctrl+End` | first / last line       |
| wheel             | one line per notch      |
| `*line 42`        | jump to a line          |


**Saving**
`Ctrl+S` saves whatever file you are working in or hovering over. If you `Ctrl+S` on
a directory it will save all files under that directory.

## Source control

`Ctrl+2` is git. The left column lists what has changed, the two columns beside it are the file before and after, and the panel underneath is the branch map — `git log --graph`, as git draws it.

```
┌ SOURCE CONTROL  main ─────────┐┌ before ──────────────┐┌ after — notes.md  not staged ─┐
│ fetch   pull   push   sync    ││  # Notes             ││  # Notes                      │
│ rebase   commit               ││                      ││                               │
│ STAGED 1                      ││- the first draft     ││+ the second draft             │
│ D src/utils.py                ││  second line         ││  second line                  │
│ UNSTAGED 4                    ││- third line          ││+ a new third line             │
│ M notes.md                    ││~                     ││+ and a fourth                 │
│ M src/main.py                 ││                      ││                               │
│ U scratch.txt                 ││                      ││                               │
└───────────────────────────────┘└──────────────────────┘└───────────────────────────────┘
┌ branches ────────────────────────────────────────────────────────────────────────────────┐
│ * 99c3a99  (feature) add extra                                                           │
│ * 367b380  (HEAD -> main) first commit                                                   │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

The two halves of the diff are always the same height: where one side has a line the other does not, the other is padded. So line 12 on the left is the same place in the file as line 12 on the right, and you read across rather than counting. A gap says what it is once — `· 11 lines added` — rather than marking every row of it.

The diff is **syntax highlighted** on both sides, by the grammar the file's own name picks. The `-` and `+` stay in the gutter rather than coloring the text: a line cannot be two colors at once, and this way a change is obvious at a glance while the code still reads as code.

| key       | does                                        |
|-----------|---------------------------------------------|
| `↑` `↓`   | move through the changes — or scroll the diff, once you are in it |
| `Ctrl+↑` `Ctrl+↓` | the same, five at a time — either half, and the message box |
| `Enter`   | stage or unstage — a file, or a whole section from its header |
| `→`       | into the diff; `←` comes back. On the button row, walk along it |
| `Tab`     | open this file in the editor                |
| `PgUp` `PgDn` | scroll the diff a screen at a time       |
| `Ctrl+←` `Ctrl+→` | narrower / wider change list        |
| `r`       | ask git again                               |
| `Esc`     | back to the browser                         |

Both halves scroll on one offset, so they cannot drift apart. The change list steps back while you are in the diff — dim border, dim title, the cursor's row marked quietly rather than highlighted — exactly as the browser does when the keyboard goes into a file.

**Committing** is two presses of the `commit` button. The first opens a message box over both diff columns; the second sends what is in it. `Ctrl+S` from inside does the same without moving, and `Esc` puts the box down *without losing what you wrote*.

```
┌ SOURCE CONTROL  main ──────┐┌ commit message — 1 staged file ────────────────────────────────────┐
│ fetch   pull   push        ││ double the loop                                                    │
│ sync   rebase   commit     ││                                                                    │
│ STAGED 1                   ││ and print the total when it is done                                │
│ M src/main.py              ││                                                                    │
└────────────────────────────┘└ Ctrl+S commits · Esc puts it down ─────────────────────────────────┘
```

It is a real editor, not a one-line prompt — the same keyboard as the file editor, undo and all. The first line is drawn apart from the rest because git treats it apart: it is the summary every log shows on its own. `*commit [message]` is still there for a one-liner from anywhere.

**The letters are git's own**, and so is what they mean: `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied, `U` untracked, `!` a conflict. Each has a color as well, so the list reads at a glance — and still reads on a terminal with no color, because the letter says it too.

Everything goes through the `git` on your machine rather than a library linked into tiny. That keeps the binary small and the build fast, and it means tiny agrees with the `git` you already have — your aliases, your config, your credential helper. Pushing and pulling run on a thread, so the window keeps drawing while they work.


**The Project Map**

The project map `Ctrl+3` shows every file in the project, grouped under the folder it lives in, and draws what the file under the cursor is joined to.


```
┌ PROJECT MAP  6/6 files | 3 links ────────────────────────────────────────┐
│src/  4 files                                                             │
│                                                                          │
│╭──────╮       ╭───────╮      ╭─────────╮    ╭────────╮                   │
││cli.py│       │main.py│──────│parser.py│────│utils.py│                   │
│╰──────╯       ╰───────╯      ╰─────────╯    ╰────────╯                   │
│                                                                          │
│                                                                          │
│unconnected  2 files                                                      │
│                                                                          │
│╭────────╮     ╭────────╮                                                 │
││notes.md│     │store.py│                                                 │
│╰────────╯     ╰────────╯                                                 │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
  src/parser.py  code
  out: 1   utils.py:load
  in:  1   main.py:read
  defines read
  1:wikilink 2:link 3:call


| key       | does                                        |
|-----------|---------------------------------------------|
| arrows    | move to the nearest file that way (`i j k l`)|
| `Tab`     | step through every file in turn             |
| `Enter`   | open the file the cursor is on              |
| `1`-`3`   | wikilinks / md links / calls                |
| `/`       | filter to matching paths                    |
| `r`       | build the map again                         |
| `Esc`     | back to the browser (or `Ctrl+1`)           |


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

Run `*config` for the settings area. Two buttons sit at the top of it:

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
`bar`, `command`, `copy`, `paste`, `help`, `fold_tree`, `window_main`,
`window_source`, `window_map`. A prefix names the pane a key only works in —
`tree.`, `view.`, `editor.`, `map.`.

Names are what the keybinds window shows in its left column. A key is written
the way it reads: `ctrl+s`, `alt+up`, `ctrl+home`, `f5`, `enter`, `esc`,
`pageup`, `.`, or a single character. A capital letter *is* the shifted one —
`I` is Shift+i.

Theme entries are style specs, so a line can carry weight as well as color:

`"bold"`, `"underline"`, `"white on black"`, `"#7dcfff bold"`, `"reverse"`.

```toml
show_hidden        = false
tab_width          = 4
auto_reload        = true   # pick up changes made by other programs
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
map_in       = "green"      # project map: files that reach the selected one
map_out      = "red"        # project map: files the selected one reaches
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

## License and Credits

Created by Matthew Saunders https://msaunders.dev
MIT. See [LICENSE](LICENSE).

Syntax definitions come from the [bat](https://github.com/sharkdp/bat) project by way of [two-face](https://codeberg.org/CosmicHarper/two-face). They are third-party files with their own terms, which `tiny --licenses` prints.
