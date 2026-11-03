# Changelog

Each version is a `v` tag, with ready-made binaries on its
[release page](https://github.com/mattsaund/tiny/releases).

## 0.6.8 - 2026-11-02

- **Changed lines are marked beside their numbers.** In a file in a git
  repository, a line new since the last commit is marked green, a changed
  one blue, and a red mark at the foot of a line says lines after it were
  deleted, the way other editors mark them. The marks follow what you type,
  saved or not, and go once it is committed, whatever made the commit.
  `*set line_marks false` turns them off, and `theme.line_added`,
  `theme.line_modified` and `theme.line_deleted` color them.

## 0.6.7 - 2026-11-02

- **Branches in the git window.** Under the changes, the branches: the ones
  here, with a `*` on the one checked out, and the ones on a remote that no
  branch here follows yet. Enter switches to one, making it here if it is
  on a remote, and the row at the top makes a new one. `n` makes a branch,
  `d` deletes one (asking first, and again if its commits are on no other
  branch), `R` renames one and `m` merges one into the branch checked out.
  The same from the bar: `*branch`, `*switch`, `*delete branch`,
  `*rename branch` and `*merge`, and Tab finishes a branch's name.
  Switching and merging wait until every file is saved. The history is
  beside the branches.
- **Ready for the AUR**, as `tiny-ide-bin`, the prebuilt release, and
  `tiny-ide`, built from source; see `packaging/aur`. A tiny that a package
  manager installed leaves updating and removing it to that manager.

## 0.6.6 - 2026-11-01

- **The arrows stay in the part you are in.** With the browser gone into,
  right on a file no longer jumps into the file, and in the git window right
  and left no longer cross between the list and the diff. Moving between
  parts is Esc and the arrows; Enter still opens a file.
- **The browser opens files into the pane you were last in.** With two
  panes, picking the other one on the way to the browser no longer takes it
  over: the browser's cursor shows its files in the pane last gone into,
  green, until another pane is gone into. Going into the terminal does not
  change which pane that is.

## 0.6.5 - 2026-11-01

- **Pick a part, then go in.** Esc steps out of the browser, a file or the
  terminal, and the arrows pick another part, outlined in white; Enter goes
  into it, and the outline turns green. Esc no longer quits; `Ctrl+Q` does.
  A click goes straight into the part it lands on, and the wheel scrolls
  whatever it is over. While picking, `/` searches and the new `*` starts a
  command, whatever is picked. The git window's list and diff work the same
  way, and both outline colors are settings: `theme.border_focus` and
  `theme.border_entered`.
- **Git in the browser.** In a repository, a file changed since the last
  commit is yellow, a new one green, one `.gitignore` leaves out gray, and a
  conflict red. A folder takes the color of what is inside it. These are
  settings too.
- **Initialize, publish, clone.** A project with no repository gets three
  buttons in the git window. Initialize makes it one. Publish makes it one if
  it is not, commits everything as the first commit, and pushes it: to a new
  private GitHub repository, made with `gh` (`*publish public` for a public
  one), or to any address typed after it. Clone brings a repository into the
  project, or into a folder of its own if the project has files. A branch
  with nowhere to push to yet gets a publish button too. `*init`,
  `*publish` and `*clone` do the same from the bar.
- In the terminal, Esc at the shell's prompt steps out, like everywhere
  else; a program that takes the whole screen still gets it.

## 0.6.4 - 2026-10-31

- **The map reads any language.** It used to know the calls in Rust, Python
  and JavaScript and nothing else. Now it finds them in C, C++, Go, Java,
  C#, Ruby, PHP, Kotlin, Swift, Lua, shell and most other languages tiny
  colors, and draws a fourth kind of link: a file that names another by its
  path, as an include, an import, a stylesheet or a script does. `4` shows
  or hides those. What `.gitignore` names is left out, and the map opens a
  second time at once.
- **It is called the map**, everywhere, rather than the project map.
- **Links are clickable.** A click on a link in a note follows it: a web
  address opens in your browser, and a link to a file, or a wikilink, opens
  that file in tiny, at the heading it names. In text being edited, where a
  click places the cursor, Ctrl and a click follows it.
- **Outside a git repository**, the git window simply says "no git
  repository".

## 0.6.3 - 2026-10-27

- **Typing is many times quicker.** Each key used to color every line on
  screen over again, and up to fifty lines above them. Now it colors the
  line it changed, and the lines after it only as far as the change reaches.
  In a file of Rust a key now reaches the screen in about a quarter of a
  millisecond, where it took 2 to 4, and sooner than in micro.

## 0.6.2 - 2026-10-27

- **Colors in files of any length.** A file past 20,000 lines used to show
  no colors at all, and the first jump to the end of a long one could hold
  tiny up for a second or more. Now every file has its colors, however
  long, and a jump anywhere in it shows them at once.
- **Resize with the mouse.** Drag the border between any two parts of the
  window: the browser's edge, the border between two panes or above the
  terminal, and in the git window the list's edge and the border between the
  halves of a diff. The parts always fill the window, and neither gets
  smaller than leaves room for what it holds.
- **Smaller and quicker.** Between 0.5 and 0.8 MB smaller, whatever the
  system, and a key reaches the screen a little sooner, now that the program
  is optimized as one piece. A long file's colors keep one copy of each state the parser
  comes back to, rather than one every fifty lines.

## 0.6.1 - 2026-10-23

- **The mouse.** A click puts the cursor anywhere and a drag selects. A
  double click takes a word, a triple click or the line numbers a line, and
  Alt with a click adds a cursor. In the browser, the map and the git window
  the first click picks and the second opens; a search result opens with
  one. A click on a pane or the terminal gives it the keyboard, and a
  program in the terminal that asks for the mouse gets it.
- **Tab finishes words.** At the end of a word, Tab finishes it from the
  words in the file, the nearest first, and again gives the next. Anywhere
  else it indents, as it did.
- **Tab in the bar** knows `split`, `close` and the values of a setting, and
  on a word with several endings goes round them.

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
