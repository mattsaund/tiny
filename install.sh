#!/bin/sh
# Install tiny on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
#
# It downloads the ready-made binary from the latest GitHub release, for macOS
# or Linux, Intel or ARM. Only when there is none for this machine, or it will
# not run here, or this is run from inside a checkout, does it build from
# source, fetching the source itself when piped from the web.
#
# It puts tiny in the applications menu, with its icon, among the programs a
# file manager offers for text files. Then it offers what tiny reaches for
# while it runs (git for the git window, ffmpeg for video and audio details,
# xdg-utils to open files on Linux) with the system's own package manager, in
# one question. tiny runs without them.
#
# Override anything with the environment:
#   TINY_REPO=...         git URL to clone
#   TINY_REF=...          branch or tag (default: main); a version tag like
#                         v0.4.5 installs that release
#   TINY_PREFIX=...       where the binary lands (default: ~/.local/bin)
#   TINY_FROM_SOURCE=1    build even where a download is available

set -eu

REPO="${TINY_REPO:-https://github.com/mattsaund/tiny.git}"
REF="${TINY_REF:-main}"
PREFIX="${TINY_PREFIX:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'install: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# A yes-or-no question, where Enter means yes. It reads from the terminal
# rather than stdin, because piped from the web stdin is this script, and a
# `read` from there would take its next line as the answer. With no terminal to
# ask, the answer is no.
ask() {
    printf '  [Y/n] '
    if [ -t 0 ]; then reply_src=/dev/stdin; else reply_src=/dev/tty; fi
    { read -r reply < "$reply_src"; } 2>/dev/null || { reply=n; printf '\n'; }
    case "${reply:-y}" in [Nn]*) return 1 ;; esac
}

# A temporary directory, and a temporary file.
#
# `mktemp -d` with no template is a GNU extension. BSD's (which is macOS's)
# wants either a template or `-t`, and exits with a usage message otherwise, so
# a script that only knows the GNU spelling dies on a Mac before it has done
# anything. Try the short form, fall back to the form both understand.
temp_dir() { mktemp -d 2>/dev/null || mktemp -d -t tiny; }
temp_file() { mktemp 2>/dev/null || mktemp -t tiny; }

# Everything to clean up on the way out, however we leave. One EXIT trap,
# because a second would silently replace the first. Ctrl+C and a kill exit
# (which runs it): a handler that only cleaned up would return, and the script
# would carry on from where it was, using the files it had just deleted.
SRC=""
SRC_IS_TEMP=""
LOG=""
ROOT=""
cleanup() {
    [ -n "$SRC_IS_TEMP" ] && [ -n "$SRC" ] && rm -rf "$SRC"
    [ -n "$LOG" ] && rm -f "$LOG"
    [ -n "$ROOT" ] && rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Whether it is worth drawing anything that moves. A log file, a CI job or a
# pipe gets plain lines instead: a progress bar redrawn with carriage returns
# into a file is a single unreadable line thousands of characters long.
tty_out() { [ -t 1 ] && [ "${TERM:-dumb}" != dumb ]; }

# Put the binary at $1 in $PREFIX, over whatever tiny is already there.
#
# Copied in beside the old one and renamed over it. A rename replaces a program
# even while it runs (which it is, when tiny is updating itself) without
# touching the copy it is running from; writing over it in place would not,
# and a copy from a temporary directory on another disk is a write.
put_in_place() {
    mkdir -p "$PREFIX" \
        && cp "$1" "$PREFIX/.tiny.new" \
        && chmod +x "$PREFIX/.tiny.new" \
        && mv -f "$PREFIX/.tiny.new" "$PREFIX/tiny" \
        || { rm -f "$PREFIX/.tiny.new"; return 1; }
}

# Put the manual page at $1 where `man tiny` finds it: share/man/man1 beside
# the folder the binary is in, so ~/.local/share/man for ~/.local/bin. man
# looks there by itself, for every bin on the PATH. A page that cannot be put
# in place costs only `man tiny`, so it is never a reason to stop.
put_man() {
    [ -f "$1" ] || return 0
    man_dir="$(dirname "$PREFIX")/share/man/man1"
    { mkdir -p "$man_dir" && cp "$1" "$man_dir/tiny.1"; } 2>/dev/null || true
}

# --- the applications menu ---------------------------------------------------
#
# tiny where the system keeps its programs, with its icon, and among the
# programs a file manager offers to open a text file with: a menu entry on
# Linux, and an app in ~/Applications on a Mac. From the folder `$1`, which is
# the release's `desktop` folder or the source's `packaging/desktop`. Like the
# manual page, none of it is a reason to stop: tiny runs from the terminal
# without it. `tiny --uninstall` takes it away again.

# Set once tiny is in the menu, for `finish` to say so.
MENU=""

put_desktop() {
    [ -d "$1" ] || return 0
    case "$(uname -s)" in
        Darwin) put_app "$1" ;;
        *) put_menu_entry "$1" ;;
    esac
}

# `$1` as one argument on a desktop entry's Exec line: in double quotes, with
# the four characters that mean something inside them escaped; then every
# backslash doubled, which the file's own rules for strings take back out;
# and `%`, which starts a field code, doubled.
desktop_quote() {
    printf '"%s"' "$(printf '%s' "$1" \
        | sed -e 's/[\\"`$]/\\&/g' -e 's/\\/\\\\/g' -e 's/%/%%/g')"
}

# A menu entry, in share/applications beside the folder the binary is in
# (so ~/.local/share for ~/.local/bin, where the desktop looks), and the icon
# in every size, in the hicolor theme every desktop falls back on. The entry
# names the binary by its full path, since what a desktop finds on its PATH
# is not always what a terminal does, and keeps any arguments the release's
# own entry gives it (0.7.0's gave --hide-browser). Terminal=true, so the
# desktop opens it in the terminal it is set to use.
#
# Started from the menu with no file, tiny opens the folder it starts in,
# and that is the home folder, with Desktop, Documents and the rest in it:
# Path says so, rather than leaving it to whatever started the terminal.
# Not for an install as root, which is everyone's and has no one home.
put_menu_entry() {
    [ -f "$1/tiny.desktop" ] || return 0
    share="$(dirname "$PREFIX")/share"
    apps="$share/applications"
    exe="$(desktop_quote "$PREFIX/tiny")"
    home=""
    if [ "$(id -u)" -ne 0 ] && [ -n "${HOME:-}" ]; then
        home="$(printf '%s' "$HOME" | sed 's/\\/\\\\/g')"
    fi
    if ! { mkdir -p "$apps" \
        && EXE="$exe" HOMEDIR="$home" awk '
            /^Path=/ && ENVIRON["HOMEDIR"] != "" { next }
            /^Exec=tiny / {
                print "Exec=" ENVIRON["EXE"] substr($0, 10)
                if (ENVIRON["HOMEDIR"] != "") print "Path=" ENVIRON["HOMEDIR"]
                next
            }
            { print }' \
            "$1/tiny.desktop" > "$apps/.tiny.desktop.new" \
        && mv -f "$apps/.tiny.desktop.new" "$apps/tiny.desktop"; } 2>/dev/null; then
        rm -f "$apps/.tiny.desktop.new"
        return 0
    fi
    if [ -d "$1/icons" ]; then
        { mkdir -p "$share/icons" && cp -R "$1/icons/." "$share/icons/"; } 2>/dev/null || true
    fi
    # Desktops that keep a cache of these, rather than looking each time,
    # are told: GNOME's of which programs open which files, GTK's of icons
    # (only where there already is one: one made here would hide every icon
    # it did not list), and KDE's of everything.
    if have update-desktop-database; then
        update-desktop-database -q "$apps" >/dev/null 2>&1 || true
    fi
    if [ -f "$share/icons/hicolor/icon-theme.cache" ] && have gtk-update-icon-cache; then
        gtk-update-icon-cache -q -t -f "$share/icons/hicolor" >/dev/null 2>&1 || true
    fi
    for sycoca in kbuildsycoca6 kbuildsycoca5; do
        if have "$sycoca"; then
            "$sycoca" >/dev/null 2>&1 || true
            break
        fi
    done
    MENU=1
}

# `$1` in single quotes, for a shell to read back as it is.
sh_quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# The terminal a Mac opens tiny in from Finder and Launchpad: the one this is
# being installed from, when it is one that can be told to run a program, and
# otherwise Terminal, which every Mac has.
mac_terminal() {
    case "${TERM_PROGRAM:-}" in
        iTerm.app) echo iTerm; return ;;
        WezTerm) echo WezTerm; return ;;
        ghostty) echo Ghostty; return ;;
    esac
    if [ -n "${KITTY_WINDOW_ID:-}" ]; then
        echo kitty
    elif [ -n "${ALACRITTY_WINDOW_ID:-}${ALACRITTY_SOCKET:-}" ]; then
        echo Alacritty
    else
        echo Terminal
    fi
}

# What tiny.app runs: tiny on the file it was handed, or on none, in a new
# window of the terminal from `mac_terminal`. A program Finder starts gets
# none of a shell's settings, so this one carries the PATH tiny was
# installed with (git and ffmpeg are often in Homebrew's folder) and the
# language, for tiny to know it may draw beyond ASCII.
mac_launcher() {
    term=$1
    q_tiny=$(sh_quote "$PREFIX/tiny")
    q_path=$(sh_quote "PATH=$PATH")
    q_lang=""
    if [ -n "${LANG:-}" ]; then q_lang=" $(sh_quote "LANG=$LANG")"; fi
    cat <<EOF
#!/bin/sh
# Opens tiny on the file named, in a new window of $term. Written by tiny's
# install.sh; tiny.app runs it.
set -eu
run="cd && exec /usr/bin/env $q_path$q_lang $q_tiny"
if [ -n "\${1:-}" ]; then
    run="\$run '\$(printf '%s' "\$1" | sed "s/'/'\\\\\\\\''/g")'"
fi
EOF
    case "$term" in
        Terminal|iTerm)
            # A .command file is a script both of these run in a window of
            # their own, and opening one asks no permission of anybody: the
            # way Ctrl+N opens its window (see terminal.rs). It deletes
            # itself as it starts.
            cat <<'EOF'
script="$(mktemp "${TMPDIR:-/tmp}/tiny.XXXXXX")"
mv "$script" "$script.command"
script="$script.command"
printf '#!/bin/sh\nrm -f "$0"\n%s\n' "$run" > "$script"
chmod +x "$script"
EOF
            printf 'open -a %s "$script"\n' "$term"
            ;;
        *)
            case "$term" in
                WezTerm) args="start -- /bin/sh -c" ;;
                kitty) args="/bin/sh -c" ;;
                *) args="-e /bin/sh -c" ;;
            esac
            printf 'open -na %s --args %s "$run"\n' "$term" "$args"
            ;;
    esac
}

# tiny.app, in ~/Applications, where Launchpad and Spotlight find it and a
# Mac's "Open With" can offer it for text. An AppleScript applet, because
# Finder hands an app the files to open as events rather than as arguments
# and AppleScript is what every Mac can build one from; all it does is run
# the launcher above, kept in the app beside it.
put_app() {
    have osacompile || return 0
    apps="$HOME/Applications"
    if [ "$(id -u)" -eq 0 ]; then apps=/Applications; fi
    work="$(temp_dir)" || return 0
    app="$work/tiny.app"
    cat > "$work/tiny.applescript" <<'EOF'
-- tiny, from Finder, Launchpad or the Dock: open-tiny runs it in a window of
-- a terminal, on each file it was given, or on none.
on run
	openTiny("")
end run

on open theItems
	repeat with anItem in theItems
		openTiny(POSIX path of anItem)
	end repeat
end open

on openTiny(thePath)
	set launcher to POSIX path of (path to resource "open-tiny")
	do shell script quoted form of launcher & " " & quoted form of thePath
end openTiny
EOF
    if ! osacompile -o "$app" "$work/tiny.applescript" >/dev/null 2>&1; then
        rm -rf "$work"
        return 0
    fi
    res="$app/Contents/Resources"
    plist="$app/Contents/Info.plist"
    mac_launcher "$(mac_terminal)" > "$res/open-tiny"
    chmod +x "$res/open-tiny"
    if [ -f "$1/tiny.icns" ]; then
        cp "$1/tiny.icns" "$res/applet.icns"
        # A newer osacompile puts the icon in an asset catalog as well,
        # which a Mac would show instead.
        rm -f "$res/Assets.car"
        plutil -remove CFBundleIconName "$plist" >/dev/null 2>&1 || true
    fi
    version="$("$PREFIX/tiny" --version 2>/dev/null | sed 's/^tiny //')"
    # What it is called, what it opens (text of every kind, offered rather
    # than taken: it is never the default for anything until you say), and
    # which tiny it starts, for `tiny --uninstall` to know it.
    plutil -replace CFBundleIdentifier -string dev.msaunders.tiny "$plist" \
        && plutil -replace CFBundleName -string tiny "$plist" \
        && plutil -replace CFBundleDisplayName -string tiny "$plist" \
        && plutil -replace CFBundleShortVersionString -string "${version:-0}" "$plist" \
        && plutil -replace TinyBinary -string "$PREFIX/tiny" "$plist" \
        && plutil -replace CFBundleDocumentTypes -json '[{
            "CFBundleTypeName": "Text",
            "CFBundleTypeRole": "Editor",
            "LSHandlerRank": "Alternate",
            "LSItemContentTypes": ["public.text", "public.plain-text",
                "public.source-code", "public.script", "public.shell-script",
                "public.json", "public.xml", "public.yaml",
                "net.daringfireball.markdown",
                "public.comma-separated-values-text",
                "public.tab-separated-values-text", "public.log"]
        }]' "$plist" >/dev/null 2>&1 \
        || { rm -rf "$work"; return 0; }
    # Changed after it was signed, so signed again, ad hoc: a Mac with Apple
    # silicon runs nothing unsigned.
    codesign --force --sign - "$app" >/dev/null 2>&1 || true
    if ! { mkdir -p "$apps" && rm -rf "$apps/tiny.app" && mv "$app" "$apps/tiny.app"; } 2>/dev/null; then
        rm -rf "$work"
        return 0
    fi
    rm -rf "$work"
    lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    if [ -x "$lsregister" ]; then
        "$lsregister" -f "$apps/tiny.app" >/dev/null 2>&1 || true
    fi
    MENU=1
}

# --- the system's package manager ---------------------------------------------

# The package manager this system has, by the name of its command: the
# distribution's own first, Homebrew last. Nothing when there is none.
package_manager() {
    for pm in apt-get dnf pacman zypper apk brew; do
        if have "$pm"; then
            echo "$pm"
            return 0
        fi
    done
}

# Run a command as root: directly when we are, through sudo when we are not.
# sudo asks for its password on the terminal, which works piped from the web.
as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif have sudo; then
        sudo "$@"
    else
        say "installing needs root, and there is no sudo"
        return 1
    fi
}

# What `$2` is called by the package manager `$1`, where it is not the same
# word everywhere. `cc` stands for "a C compiler", whatever provides one.
package_name() {
    case "$1:$2" in
        dnf:ffmpeg) echo ffmpeg-free ;;
        apt-get:cc) echo build-essential ;;
        pacman:cc) echo base-devel ;;
        apk:cc) echo build-base ;;
        *:cc) echo gcc ;;
        *) echo "$2" ;;
    esac
}

# Install packages, by tiny's names for them, with the package manager `$1`.
install_packages() {
    pm=$1
    shift
    names=""
    for p in "$@"; do names="$names $(package_name "$pm" "$p")"; done
    # `$names` is split into words on purpose: one package per word.
    case "$pm" in
        apt-get) as_root apt-get update -qq && as_root apt-get install -y $names ;;
        dnf) as_root dnf install -y $names ;;
        pacman) as_root pacman -S --needed --noconfirm $names ;;
        zypper) as_root zypper --non-interactive install $names ;;
        apk) as_root apk add $names ;;
        # Homebrew refuses to run as root, and does not need to.
        brew) brew install $names ;;
        *) return 1 ;;
    esac
}

# --- what tiny reaches for while it runs ----------------------------------------
#
# tiny runs on its own. Three programs make parts of it work, and those that
# are missing are offered together, in one question:
#
#   git        the git window
#   ffprobe    the size and length of a video or a song (part of ffmpeg)
#   xdg-open   handing a picture or a PDF to the desktop (Linux)
#
# A no, or no way to install them, leaves tiny working without them. Not asked
# again when tiny is updating itself: that was answered when it was installed.
offer_extras() {
    [ -z "${TINY_UPDATE:-}" ] || return 0
    nl='
'
    missing=""
    why=""
    if [ "$(uname -s)" = Darwin ]; then
        # The git a fresh Mac has is a stand-in that asks for Apple's command
        # line tools, so their presence is the question to ask.
        if ! xcode-select -p >/dev/null 2>&1 && ! [ -x /opt/homebrew/bin/git ] \
            && ! [ -x /usr/local/bin/git ]; then
            say ""
            say "The git window needs git, which comes with Apple's command line"
            say "tools. Install them now?"
            if ask; then
                xcode-select --install >/dev/null 2>&1 || true
                say "finish in the window that opened; tiny works meanwhile"
            fi
        fi
    else
        if ! have git; then
            missing="$missing git"
            why="$why${nl}  git        the git window"
        fi
        if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] && ! have xdg-open; then
            missing="$missing xdg-utils"
            why="$why${nl}  xdg-utils  opening pictures and PDFs"
        fi
    fi
    if ! have ffprobe; then
        missing="$missing ffmpeg"
        why="$why${nl}  ffmpeg     the size and length of videos and songs"
    fi
    [ -n "$missing" ] || return 0

    say ""
    say "tiny can also use these, which are not installed:$why"
    pm=$(package_manager)
    if [ -z "$pm" ]; then
        say "install them to turn those on; tiny works without them"
        return 0
    fi
    say "Install them with $pm?"
    if ! ask; then
        say "skipped; tiny works without them"
        return 0
    fi
    # `$missing` is split into words on purpose: one package per word.
    install_packages "$pm" $missing \
        || say "not all of them installed; tiny works without them"
}

# What to say once there is a binary, from either route to one.
finish() {
    offer_extras
    say ""
    say "installed $PREFIX/tiny"
    # tiny updating itself already knows where it is and how to start; it
    # says so itself, straight after this.
    [ -z "${TINY_UPDATE:-}" ] || return 0
    # It is offered for text files, never made the program for them: that
    # is the person's to choose, once, and here is how.
    if [ -n "$MENU" ]; then
        if [ "$(uname -s)" = Darwin ]; then
            say "tiny is in Launchpad. To open text files with it, Get Info on one,"
            say "choose tiny under Open with, then Change All."
        else
            say "tiny is in your applications menu. To open text files with it:"
            say "  xdg-mime default tiny.desktop text/plain text/markdown"
        fi
    fi
    case ":$PATH:" in
        *":$PREFIX:"*)
            say "run it with:      tiny ~/notes"
            say "remove it with:   tiny --uninstall"
            ;;
        *)
            say "$PREFIX is not on your PATH. Add it:"
            say ""
            case "${SHELL##*/}" in
                fish) say "  fish_add_path $PREFIX" ;;
                zsh)  say "  echo 'export PATH=\"$PREFIX:\$PATH\"' >> ~/.zshrc" ;;
                *)    say "  echo 'export PATH=\"$PREFIX:\$PATH\"' >> ~/.bashrc" ;;
            esac
            say ""
            say "then run:  tiny ~/notes"
            say ""
            say "to remove tiny later:  tiny --uninstall"
            ;;
    esac
}

# --- a ready-made binary -----------------------------------------------------
#
# Building needs Rust, and on a Mac the Xcode command line tools as well (a
# large download to try a small program), so this takes the binary the release
# workflow built, and only builds when there is none to take.
#
# A Linux binary is linked against the C library of the system that built it,
# and will not start on an older one, or on a different one, like Alpine's musl.
# So it is asked its version before it replaces anything, and a binary that
# cannot answer is a reason to build instead.
#
# Not when asked to build, and not for a branch: a branch today is not what any
# release was. A version tag names its release exactly, and `main` means the
# newest one.
prebuilt() {
    [ -z "${TINY_FROM_SOURCE:-}" ] || return 1
    case "$REF" in
        main) release=latest/download ;;
        v[0-9]*) release="download/$REF" ;;
        *) return 1 ;;
    esac
    have curl || return 1
    case "$(uname -s)" in
        Darwin) os=apple-darwin ;;
        Linux) os=unknown-linux-gnu ;;
        *) return 1 ;;
    esac
    case "$(uname -m)" in
        arm64|aarch64) arch=aarch64 ;;
        x86_64|amd64) arch=x86_64 ;;
        *) return 1 ;;
    esac
    slug=${REPO#https://github.com/}
    slug=${slug%.git}
    case "$slug" in *://*|*/*/*) return 1 ;; */*) ;; *) return 1 ;; esac
    url="https://github.com/$slug/releases/$release/tiny-$arch-$os.tar.gz"
    dir="$(temp_dir)" || return 1
    printf 'downloading tiny for %s ... ' "$arch-$os"
    if ! curl -fsSL "$url" -o "$dir/tiny.tar.gz" 2>/dev/null; then
        say "none published for it, so building from source instead"
        rm -rf "$dir"
        return 1
    fi
    if ! tar -xzf "$dir/tiny.tar.gz" -C "$dir" tiny 2>/dev/null; then
        say "could not unpack it, so building instead"
        rm -rf "$dir"
        return 1
    fi
    if ! "$dir/tiny" --version >/dev/null 2>&1; then
        say "it does not run on this system, so building from source instead"
        rm -rf "$dir"
        return 1
    fi
    if ! put_in_place "$dir/tiny"; then
        rm -rf "$dir"
        die "could not put tiny in $PREFIX"
    fi
    # Releases before 0.6.0 carry no manual page, and before 0.6.9 no menu
    # entry or icon.
    if tar -xzf "$dir/tiny.tar.gz" -C "$dir" tiny.1 2>/dev/null; then
        put_man "$dir/tiny.1"
    fi
    if tar -xzf "$dir/tiny.tar.gz" -C "$dir" desktop 2>/dev/null; then
        put_desktop "$dir/desktop"
    fi
    rm -rf "$dir"
    say "done"
}

# Running from inside a checkout means "build this", so a checkout never
# downloads.
if ! { [ -f "./Cargo.toml" ] && grep -q 'name = "tiny"' ./Cargo.toml 2>/dev/null; } \
    && prebuilt; then
    finish
    exit 0
fi

# --- a compiler -------------------------------------------------------------
#
# Rust compiles tiny, but it links with the system's C toolchain, which on a
# Mac is Apple's command line tools. macOS answers to `cc` either way: without
# the tools it is a stub, and the build fails at the very end, after every
# crate has compiled, with "invalid active developer path". So ask before
# building, and install them the way macOS itself offers to.
if [ "$(uname -s)" = Darwin ] \
    && ! { dev=$(xcode-select -p 2>/dev/null) && [ -d "$dev" ]; }; then
    say "Building tiny needs Apple's command line tools: the compiler and linker"
    say "macOS installs on request. They are a free download from Apple and"
    say "install in a window of their own. Install them now?"
    ask || die "the command line tools are needed to build: xcode-select --install"
    xcode-select --install >/dev/null 2>&1 || true
    die "finish installing the command line tools in the window that opened, then run this again"
fi

# Linux links with its C compiler too, and tiny compiles a little C of its
# own: the map's parsers, and Lua for plugins. A minimal system often has
# none, and the build would fail at the first crate that needs it.
if [ "$(uname -s)" != Darwin ] && ! { have cc || have gcc || have clang; }; then
    say "Building tiny needs a C compiler, and this system has none."
    pm=$(package_manager)
    [ -n "$pm" ] || die "install gcc (or clang) with your package manager, then run this again"
    say "Install one with $pm now?"
    ask || die "a C compiler is needed to build tiny: install gcc, then run this again"
    install_packages "$pm" cc || die "could not install a C compiler"
fi

# --- rust -------------------------------------------------------------------

if ! have cargo; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        PATH="$HOME/.cargo/bin:$PATH"
    else
        say "tiny is written in Rust, and cargo is not installed."
        say "Install the Rust toolchain now? It goes in ~/.rustup and ~/.cargo,"
        # Single quotes inside: backquotes in a double-quoted string are a
        # command substitution, so this line used to *run* the uninstaller.
        say "and 'rustup self uninstall' removes it again."
        ask || die "cargo is required: see https://rustup.rs"
        have curl || die "curl is required to fetch rustup"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable
        PATH="$HOME/.cargo/bin:$PATH"
    fi
fi
have cargo || die "cargo still not on PATH"

# --- source -----------------------------------------------------------------

if [ -f "./Cargo.toml" ] && grep -q 'name = "tiny"' ./Cargo.toml 2>/dev/null; then
    SRC="$(pwd)"
    say "building from $SRC"
else
    have git || die "git is required to fetch the source"
    SRC="$(temp_dir)" || die "could not make a temporary directory"
    SRC_IS_TEMP=1
    printf 'fetching %s (%s) ... ' "$REPO" "$REF"
    git clone --depth 1 --branch "$REF" "$REPO" "$SRC" >/dev/null 2>&1 \
        || { printf '\n'; die "could not clone $REPO"; }
    say "done"
fi

# --- build ------------------------------------------------------------------

mkdir -p "$PREFIX"

# Turn cargo's running commentary into one line that moves.
#
# The bar is drawn with `\r` and no newline, so the whole build occupies a
# single line however many crates go past. The field holding the crate name is
# padded, because a shorter name has to wipe the longer one it replaced and no
# clear-to-end-of-line escape is worth assuming.
#
# With no total (an old cargo, or a `cargo tree` that failed) it counts
# instead of filling. A bar that cannot say how far along it is should say so
# rather than invent a denominator.
#
# This was an awk script, and the bar arrived in one burst at the end of the
# build. The output was already flushed every frame, so it was not awk's
# writing that was buffered; it was awk's *reading*. mawk, which is `awk` on
# Debian and Ubuntu, fills a several-kilobyte input buffer before it runs a
# single rule, so nothing reached the screen until cargo had printed enough to
# fill it, which for a build is somewhere near the end. A `while read` loop has
# no such buffer: the shell takes one line at a time, which is what a progress
# bar needs and is cheap at the couple of hundred lines cargo prints.
BAR_FULL='############################'
BAR_EMPTY='............................'
BAR_WIDTH=28

# A one-off message in the same place the bar lives. Silent when there is
# nothing to draw on, so the log-file case stays readable.
draw() { if tty_out; then printf '\r  %-52.52s' "$1"; fi; }

# `$1` crates done of `$TOTAL`, and what is happening right now.
bar() {
    if [ "$TOTAL" -le 0 ]; then
        if [ "$1" -eq 1 ]; then
            printf '\r  %d crate   %-30.30s' "$1" "$2"
        else
            printf '\r  %d crates  %-30.30s' "$1" "$2"
        fi
        return 0
    fi
    p=$(( $1 * 100 / TOTAL ))
    if [ "$p" -gt 100 ]; then p=100; fi
    f=$(( p * BAR_WIDTH / 100 ))
    printf '\r  [%.*s%.*s] %3d%%  %-30.30s' \
        "$f" "$BAR_FULL" "$(( BAR_WIDTH - f ))" "$BAR_EMPTY" "$p" "$2"
}

watch_build() {
    if tty_out; then
        n=0
        d=0
        # `|| [ -n "$line" ]` catches a last line with no newline on the end,
        # which `read` reports as failure even though it read something.
        while IFS= read -r line || [ -n "$line" ]; do
            # Cargo indents its verbs by a variable amount to line them up.
            rest=${line#"${line%%[! ]*}"}
            case "$rest" in
                # The sentinel is bookkeeping, not something to show anyone.
                tiny-install-failed*) ;;
                Updating\ *) draw "updating the crate index" ;;
                Downloaded\ *)
                    d=$(( d + 1 ))
                    if [ "$d" -eq 1 ]; then
                        draw "fetched 1 crate"
                    else
                        draw "fetched $d crates"
                    fi
                    ;;
                Compiling\ *)
                    n=$(( n + 1 ))
                    name=${rest#Compiling }
                    name=${name%% *}
                    # tiny is always the last crate: everything it depends on
                    # has to exist before it can be built. So its arrival means
                    # the only work left is the link, however few `Compiling`
                    # lines came before it, which is the ordinary case when
                    # the build directory is warm and the dependencies are all
                    # still there from last time.
                    if [ "$name" = tiny ] && [ "$TOTAL" -gt 0 ]; then
                        bar "$(( TOTAL - 1 ))" "linking tiny"
                    else
                        bar "$n" "compiling $name"
                    fi
                    ;;
                # The end of the build, and the only honest marker of it.
                # `cargo install` prints `Installing tiny v0.3.1` when it
                # *starts* as well as `Installing <path>` when it finishes, so
                # matching on that drew a full bar before the first crate had
                # compiled.
                Finished\ *)
                    # A full bar when there is a total to fill; with no total
                    # the line is a count, and the count is what it is.
                    if [ "$TOTAL" -gt 0 ]; then
                        bar "$TOTAL" "installing"
                    else
                        bar "$n" "installing"
                    fi
                    ;;
            esac
            line=''
        done
        if [ "$n" -gt 0 ] || [ "$d" -gt 0 ]; then printf '\n'; fi
    else
        # Not a terminal: a bar redrawn with carriage returns into a log file
        # is one unreadable line thousands of characters long. Drain it and
        # let the log speak.
        cat >/dev/null
    fi
    # The log is what actually says whether it worked: the loop's exit status
    # is its own, and the shell has no way to reach back for cargo's through a
    # pipe.
    ! grep -q 'tiny-install-failed' "$LOG"
}

# How many crates the bar is counting against.
#
# `cargo tree` lists the packages actually reachable at build time (normal and
# build dependencies, no dev-dependencies) and cargo prints one `Compiling`
# line for each. Asking cargo rather than writing a number down means the bar
# stays right when the dependency list changes, and an old cargo that does not
# know the command just leaves this empty, which turns the bar into a count.
say "building (this takes a minute the first time)"
draw "reading the dependency list"
TOTAL=$(cargo tree --manifest-path "$SRC/Cargo.toml" \
            -e normal,build --prefix none --no-dedupe 2>/dev/null \
        | awk 'NF' | sort -u | wc -l | tr -d ' ')
case "$TOTAL" in ''|*[!0-9]*|0) TOTAL=0 ;; esac

LOG="$(temp_file)" || die "could not make a temporary file"

# Cargo says what it is doing on stderr, a line per crate. Reading those is
# what turns "wait for a minute with nothing on screen" into something you can
# watch, and the log keeps the whole of it for the failure case, where what
# went wrong matters more than how far it got.
#
# The sentinel is how a POSIX shell gets the exit status of the first command
# in a pipeline: `$?` is the last one's, and there is no `PIPESTATUS` here.
#
# Cargo installs into `<root>/bin` and keeps its own records in `<root>`, so it
# is given a root of its own, thrown away afterwards, and the binary is moved to
# where it was asked for. Handing it the prefix's parent instead only put the
# binary in the prefix when the prefix happened to be called `bin`.
build() {
    { cargo install --path "$SRC" --bin tiny --root "$ROOT" 2>&1 \
        || echo "tiny-install-failed"; } | tee "$LOG" | watch_build
}

ROOT="$(temp_dir)" || die "could not make a temporary directory"
if ! build; then
    # The sentinel is ours; showing it to someone whose build just failed would
    # only be one more confusing line among the ones that matter.
    say ""
    grep -v 'tiny-install-failed' "$LOG" | tail -n 30 >&2
    die "build failed: the output above says why"
fi

[ -x "$ROOT/bin/tiny" ] || die "the build finished without leaving a binary"
put_in_place "$ROOT/bin/tiny" || die "could not put tiny in $PREFIX"
put_man "$SRC/docs/tiny.1"
put_desktop "$SRC/packaging/desktop"
finish
