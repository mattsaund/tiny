# Changelog

Each version is a `v` tag, with ready-made binaries on its
[release page](https://github.com/mattsaund/tiny/releases).

## 0.5.0 — 2026-10-11

- **Commands are sentences.** `*copy temp.txt to notes`, `*goto function
  main`, `*goto word todo in notes.md`, `*replace hi with hello`. Many ways to
  say each (`go to`, `open`, `jump to`; `remove`, `rm`; `change … to`), and
  every older spelling still works.
- **The bar says what Enter will do** on the line under it, with counts —
  before anything happens.
- **Names from anywhere.** `notes` is the folder called notes wherever it is;
  `readme` finds README.md. Two with one name are listed, not guessed.
- **goto** a line, function, word, text, heading, file or folder — in one file
  with `in a.md`, otherwise across the project. Again goes to the next one.
- **replace** is whole words. With no `in` it does the whole project and asks
  first; `in a.md` changes that file, and `Ctrl+Z` undoes it.
- `*open map`, `*show git`, `*help` open those windows.
- Updating no longer prints the first-install advice.

## 0.4.6 — 2026-10-11

- **A much faster project map.** Opening it reads files on every core and
  skips the ones with nothing to find in them, like pictures. A 20,000-note
  vault opens in half a second instead of three, and a 25,000-file code
  folder in about five instead of thirty. Moving around it takes
  milliseconds, not two seconds a key.
- **Faster search.** Files are read on every core, a lowercase search is no
  slower than one with capitals, and fast typing searches once rather than
  once a letter. Up to 10× quicker on big folders.
- **Faster notes.** Code blocks in markdown draw quicker.
- **Fixes:** maps of very large projects lay out correctly; `tiny
  --licenses` lists the crates only the Windows build uses.

## 0.4.5 — 2026-10-10

- **Releases.** Ready-made tiny for macOS, Linux and Windows, Intel and ARM.
  The install line downloads it; no Rust needed. The installers also offer
  git and ffmpeg (and xdg-utils on Linux) if they are missing, and Windows
  needs no Visual C++ runtime.
- **Updates.** tiny says when a new version is out, and `y` installs it.
  Also `tiny --update` and `*update`. `update_check = false` stops it asking.
- **Plugins.** Lua files in the plugins folder add commands and react to
  opening and saving. Settings → `Plugins` opens the folder; `*reload` loads
  new ones.
- **Keys that work on every terminal**, macOS Terminal included: `?` help,
  `Ctrl+G` git, `Ctrl+L` map, `Ctrl+T` browser, `Ctrl+F` search, `Ctrl+A` /
  `Ctrl+E` line ends, `Ctrl+U` / `Ctrl+D` page, `⌥←` / `⌥→` words, `<` / `>`
  browser width. No function keys, Ctrl+digits or Ctrl+arrows needed.
- **`Ctrl+N`** opens another tiny on the same project, in a new window.
- **Windows:** file paste is `Ctrl+P`, since Windows Terminal keeps `Ctrl+V`.
- **PDFs and audio** open in your own viewer on Enter, like pictures and video.
- **Keybinds, settings and help:** fewer words, no duplicate keys. Key names
  from older configs still work.

## 0.3.1 and earlier

Before releases — see the git history.
