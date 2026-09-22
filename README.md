# tiny

Tiny is a personal knowledge management system (PKMS), IDE, and text editor
that works entirely in the terminal.

Everything tiny manages and edits is non proprietary and not obfuscated: plain
files in plain folders, on your own disk. Notes are markdown, code is code, and
nothing is stored anywhere you cannot open with something else. It reads your
project, edits it, searches it, draws the links between its files, and does
your git — in one window, from the keyboard, in about 7 MB.

## Install

**One liner** (Linux, macOS):

```sh
curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
```

**One liner** (Windows):

```powershell
irm https://raw.githubusercontent.com/mattsaund/tiny/main/install.ps1 | iex
```

**From a checkout**:

```sh
git clone https://github.com/mattsaund/tiny.git && cd tiny && sh install.sh
```

The installers take the ready-made binary from the latest release when there is
one for your machine, and otherwise build from source — offering Rust, and the
compiler your system needs, if they are missing.

**Uninstall**:

```sh
tiny --uninstall
```

It removes the binary and `tiny.conf`. Your notes and projects are not touched.

## Use

```sh
tiny                      # this folder, or the project it sits inside
tiny ~/Desktop/project1   # that folder — created empty if missing
tiny ~/code/main.py       # that one file, in the editor, with its project beside it

tiny --config             # where tiny.conf lives
tiny --licenses           # tiny's terms, and everything it is built from
tiny --uninstall          # remove tiny, leaving your notes alone
```

## Controls

`F1` lists every key, read from your own keymap, so it is right even after you
rebind something.

**Moving**

| key | does |
|---|---|
| `↑` `↓` `←` `→` — or `i` `k` `j` `l` | move |
| `Ctrl+↑` `Ctrl+↓` | five at a time — and in a file, `Ctrl+←` `Ctrl+→` are a word |
| `Home` `End` — or `I` `K` | to the ends of a list, or of a line |
| `Ctrl+Home` `Ctrl+End` | the first and last line of a file |
| `Enter` | open or close a folder, or edit a file |
| `Tab` | hand the keyboard to the file |
| `Esc` | back — and from the browser, quit |

**Files**

| key | does |
|---|---|
| `Ctrl+S` | save — on a folder, everything unsaved in it |
| `Ctrl+C` `Ctrl+V` | copy, and paste into the folder you are in |
| `.` | show or hide dotfiles |

**Windows**

| key | does |
|---|---|
| `Ctrl+1` | the browser and the file |
| `Ctrl+2` | git: what has changed, and what to do about it |
| `Ctrl+3` | the project map |
| `Ctrl+Space` | fold the browser away, and bring it back |
| `Ctrl+←` `Ctrl+→` | a narrower or wider browser |

**The bar**

One field does all the finding there is. `/` opens it from anywhere you are not
typing, `Ctrl+/` opens it from inside a file as well, and what it searches is
whatever you are looking at: the project from the main window, the map when the
map is up. A `*` as the first character turns the same field into the command
line.

| key | does |
|---|---|
| `/` or `Ctrl+/` | the bar — names and contents, or the map |
| `Ctrl+P` | the bar, with the `*` already typed |
| `↑` `↓` | through the results, with the preview following |
| `Enter` | open the hit — or, on the map, keep the filter |
| `Esc` | close it; on the map, clear the filter too |

Search is smart-cased: `widget` matches `Widget`, `Widget` matches only
`Widget`. Every occurrence is marked in the preview, so you can see where a hit
sits before opening it.

**Commands**

```
*new [file.txt]               make a file, whatever the name looks like
*mkdir [folder]               make a folder
*rename [old] to [new]        rename, or move between folders
*copy [file] to [folder]      copy; a folder brings everything under it
*delete [path]                delete, after confirming — or bare, what the cursor is on
*line [42]                    jump to a line; *42 does too
*replace [old] [new]          find-replace across the project, after confirming
*commit [message]             commit what is staged
*set [setting] to [value]     change a setting; *set [setting] alone reports it
*config                       the settings area
*reload                       re-read the project from disk now
```

**Editing**

| key | does |
|---|---|
| `Ctrl+Z` `Ctrl+Y` | undo / redo |
| `Ctrl+K` | delete the current line |
| wheel | one line per notch |

**Git** — `Ctrl+2`

The left column is what has changed, the two beside it are the file before and
after, and the panel underneath is the branch graph. `Enter` stages or unstages
a file — or a whole section from its header — `→` goes into the diff, `Tab`
opens the file, `r` asks git again. The `commit` button opens a message box
across both columns; `Ctrl+S` sends it. Letters and colors are git's own: `A`
added, `M` modified, `D` deleted, `R` renamed, `C` copied, `U` untracked, `!` a
conflict.

Everything goes through the `git` on your machine, so tiny agrees with the git
you already have — your aliases, your config, your credential helper.

**The project map** — `Ctrl+3`

Every file, grouped by folder, with lines to whatever the file under the cursor
is joined to: green for the files that reach it, red for the files it reaches.
`1` `2` `3` turn wikilinks, markdown links and calls on and off, `r` builds the
map again, and `/` narrows it.

| edge | comes from |
|---|---|
| **wikilink** | `[[another-note]]`, in markdown *and* plain text |
| **md link** | `[text](../notes/spec.md)` — relative, not URLs |
| **call** | one file calling a function another defines |

**Settings**

`*config` opens the settings area; every setting can also be changed with
`*set`, and both write to `tiny.conf`:

| platform | where |
|---|---|
| Linux / macOS | `~/.config/tiny/tiny.conf` |
| Windows | `%APPDATA%\tiny\tiny.conf` |

Keys live in a `[keys]` section by action name — the names the keybinds window
shows — and only what you change needs to be there:

```toml
[keys]
down        = "z"          # one key
up          = "up i w"     # or several, space separated
editor.undo = "ctrl+u"
refresh     = ""           # or none at all
```

A name with no prefix is one key doing one job wherever it applies: `down` is
down in the browser, in a note, on the map and in the change list, so rebinding
it once rebinds it everywhere. A prefix means the key belongs to that pane
alone — `editor.undo`, `map.links`, `tree.hidden`.

## Contributing

**AI policy**

I am open to AI and agentic coding, but the code written needs to follow
specific guidelines:

1. MUST be human readable, acceptable variable/function names.
2. Easily traceable, following a good program flow.
3. Contributors MUST look at and document code and code changes. You need to
   understand the code that is being written.

**Tests**

```sh
cargo test
```

The suite renders real frames through ratatui's test backend, so navigation,
editing, saving, search, commands, git and the settings area are covered
without a terminal. To look at a project as tiny draws it:

```sh
TINY_SHOT=path/to/project cargo test screenshot -- --ignored --nocapture
```

Issues and pull requests welcome. `cargo test` should pass and `cargo clippy`
should be quiet before you open one; CI checks both, along with `cargo fmt`.

## License and credits

Created by Matthew Saunders — https://msaunders.dev

tiny is MIT. See [LICENSE](LICENSE).

`tiny --licenses` prints the whole picture: tiny's own terms, every crate
compiled into the binary with the license it declares — all permissive, none
copyleft — and the syntax definitions, which come from the
[bat](https://github.com/sharkdp/bat) project by way of
[two-face](https://codeberg.org/CosmicHarper/two-face) and carry their own
notices.
