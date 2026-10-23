# Changelog

Each version is a `v` tag, with ready-made binaries on its
[release page](https://github.com/mattsaund/tiny/releases).

## 0.6.0 - 2026-10-23

- **Select, copy and paste.** Shift with any key that moves selects, and
  `Ctrl+A` selects everything. `Ctrl+C`, `Ctrl+X` and `Ctrl+V` copy, cut and
  paste through your system's clipboard (over SSH, the terminal's), or the
  whole line with nothing selected. A paste from the terminal goes in as it
  was copied, indents and all.
- **More cursors.** `Alt+N` selects the word and adds a cursor at each next
  place it is, and `Alt+Shift+Up` and `Down` add one above or below.
  Typing, deleting, pasting and moving happen at every cursor; `Esc` lets
  go.
- **Any number of panes**, side by side and stacked in any mix: `*split`,
  `*split down` and `*close`. `Ctrl+W` goes to the next one, and with one,
  splits it.
- **A terminal.** `Ctrl+O` runs your shell in the project, under the panes,
  and `Ctrl+O` comes back.
- **Patterns.** A search between slashes, `/fn \w+/`, is a regular
  expression, and so is a `*replace`, whose replacement can use what it
  captured: `*replace /(\w+)@a/ with $1@b`.
- **Macros.** `Ctrl+R` records keys and `Ctrl+P` plays them back. `*play 20`
  plays them twenty times.
- **The cursor's shape**, with `cursor_shape` (`block`, `bar`, `underline`)
  and `cursor_blink`.
- **A manual page.** `man tiny` has every key, and the installer puts it
  beside tiny.
- **Keys that moved:** `Ctrl+A` selects all, so `Home` is the start of a
  line. `Shift+Left` and `Shift+Right` select, so `Ctrl+W` moves between
  panes. `Ctrl+C` and `Ctrl+V` copy text in a file and files in the browser.
- Undo puts back what was selected, and each pane keeps its own selection.

## 0.5.6 - 2026-10-23

- **The best each terminal can do.** Windows' own console (around Command
  Prompt or PowerShell) and the Linux console draw box lines, arrows and
  bullets from the characters every console font has, where 0.5.5 drew
  plain ASCII. ASCII is left for terminals not reading UTF-8.
- **Faint, said another way.** Windows' own console draws faint text at full
  strength, so there it is dark gray, and headings still fade. With
  `NO_COLOR`, dark gray text is faint instead of plain.
- `glyphs` takes `console` too.

## 0.5.5 - 2026-10-23

- **Any font, any terminal.** tiny looks at the terminal it is running in
  and draws to suit it: line drawing where the font has it and plain ASCII
  where it may not (the Linux console, a vt100, Windows' own console), and
  as many colors as the terminal shows, down to none for `NO_COLOR`. The
  `glyphs` and `colors` settings override it, and plugins can set them with
  the new `tiny.set`.
- **Two panes.** `Ctrl+W` splits the space beside the browser in two, on
  every OS. `Shift+Left` and `Shift+Right` move between them.
- **Windows on Arm** has its own build, and the installer picks it.
- **Sizes** read in KiB, MiB and GiB, to two decimal places.
- **Git:** the change list and the message box scroll, git never stops to
  ask for a password, and Sync pushes after it pulls.
- **Notes:** headings fade one step per level. Lists, quotes and code in
  lists render the way they are written, and wikilinks, footnotes, HTML,
  front matter and table alignment draw properly. Editing a long note keeps
  your place.
- **Map:** sees all of a Rust file, and links land where they point
  (`[[note#heading]]`, `My%20note.md`, part of a path).
- **Files:** renaming a folder keeps the unsaved edits inside it, a replace
  changes open files where you can undo it, and mixed line endings are
  saved as they were read.
- **Commands:** a commit message keeps every word, Tab works after every
  spelling of a verb, and hidden files open by name.
- **Lighter:** files you only look at are not kept in memory, minified files
  no longer stall the editor, undo has a memory limit, and idle tiny draws
  nothing.
- **Fixes:** Backspace works on terminals that send it as Ctrl+H, Shift+Tab
  works in the map, tabs and wide characters keep the cursor on its text,
  Ctrl+C stops the installer, and running the tests no longer resets your
  `tiny.conf`.

## 0.5.1 - 2026-10-21

- **Every heading level renders.** `###` and deeper used to show their
  hashes. They fade by depth now, bold to dim, with no markup showing through
  and no extra rules on the page.
- **Tabs are drawn.** A tab reached the screen as a cell holding nothing, so
  a tab indented file lost its shape and a tab between two words joined them.
  Tabs now expand to their stops everywhere: the editor, the preview, markdown
  and the git diff. Makefiles and Go keep their indentation.

## 0.5.0 - 2026-10-11

- **Commands are sentences.** `*copy temp.txt to notes`, `*goto function
  main`, `*goto word todo in notes.md`, `*replace hi with hello`. Many ways to
  say each (`go to`, `open`, `jump to`; `remove`, `rm`; `change … to`), and
  every older spelling still works.
- **The bar says what Enter will do** on the line under it, with counts,
  before anything happens.
- **Names from anywhere.** `notes` is the folder called notes wherever it is;
  `readme` finds README.md. Two with one name are listed, not guessed.
- **goto** a line, function, word, text, heading, file or folder, in one file
  with `in a.md`, otherwise across the project. Again goes to the next one.
- **replace** is whole words. With no `in` it does the whole project and asks
  first; `in a.md` changes that file, and `Ctrl+Z` undoes it.
- `*open map`, `*show git`, `*help` open those windows.
- Updating no longer prints the first-install advice.
- **Third-party notices.** `tiny --licenses`, and every download, carry the
  copyright and license of everything compiled in: each crate, Lua, and the
  color themes.

## 0.4.6 - 2026-10-11

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

## 0.4.5 - 2026-10-10

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

Before releases, see the git history.
