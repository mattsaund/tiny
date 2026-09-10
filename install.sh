#!/bin/sh
# Install tiny — a terminal knowledge manager.
#
#   curl -fsSL https://raw.githubusercontent.com/mattsaund/tiny/main/install.sh | sh
#
# Works two ways: piped from the web, where it fetches the source itself, or
# run from inside a checkout, where it builds what is already there.
#
# Override anything with the environment:
#   TINY_REPO=...    git URL to clone
#   TINY_REF=...     branch or tag (default: main)
#   TINY_PREFIX=...  where the binary lands (default: ~/.local/bin)

set -eu

REPO="${TINY_REPO:-https://github.com/mattsaund/tiny.git}"
REF="${TINY_REF:-main}"
PREFIX="${TINY_PREFIX:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'install: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# A temporary directory, and a temporary file.
#
# `mktemp -d` with no template is a GNU extension. BSD's — which is macOS's —
# wants either a template or `-t`, and exits with a usage message otherwise, so
# a script that only knows the GNU spelling dies on a Mac before it has done
# anything. Try the short form, fall back to the form both understand.
temp_dir() { mktemp -d 2>/dev/null || mktemp -d -t tiny; }
temp_file() { mktemp 2>/dev/null || mktemp -t tiny; }

# Everything to clean up on the way out, however we leave. One trap, because a
# second `trap ... EXIT` would silently replace the first.
SRC=""
SRC_IS_TEMP=""
LOG=""
cleanup() {
    [ -n "$SRC_IS_TEMP" ] && [ -n "$SRC" ] && rm -rf "$SRC"
    [ -n "$LOG" ] && rm -f "$LOG"
    return 0
}
trap cleanup EXIT INT TERM

# Whether it is worth drawing anything that moves. A log file, a CI job or a
# pipe gets plain lines instead — a progress bar redrawn with carriage returns
# into a file is a single unreadable line thousands of characters long.
tty_out() { [ -t 1 ] && [ "${TERM:-dumb}" != dumb ]; }


# --- rust -------------------------------------------------------------------

if ! have cargo; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        PATH="$HOME/.cargo/bin:$PATH"
    else
        say "tiny is written in Rust, and cargo is not installed."
        say "Install the Rust toolchain now? It goes in ~/.rustup and ~/.cargo,"
        say "and `rustup self uninstall` removes it again."
        # Reads from the terminal, not stdin, so this still works when piped.
        if [ -t 0 ]; then reply_src=/dev/stdin; else reply_src=/dev/tty; fi
        printf '  [Y/n] '
        if [ -r "$reply_src" ]; then read -r reply < "$reply_src"; else reply=n; fi
        case "${reply:-y}" in
            [Nn]*) die "cargo is required — see https://rustup.rs" ;;
        esac
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
# With no total — an old cargo, or a `cargo tree` that failed — it counts
# instead of filling. A bar that cannot say how far along it is should say so
# rather than invent a denominator.
#
# This was an awk script, and the bar arrived in one burst at the end of the
# build. The output was already flushed every frame, so it was not awk's
# writing that was buffered — it was awk's *reading*. mawk, which is `awk` on
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
                    # lines came before it — which is the ordinary case when
                    # the build directory is warm and the dependencies are all
                    # still there from last time.
                    if [ "$name" = tiny ] && [ "$TOTAL" -gt 0 ]; then
                        bar "$(( TOTAL - 1 ))" "linking tiny"
                    else
                        bar "$n" "compiling $name"
                    fi
                    ;;
                # The end of the build, and the only honest marker of it.
                # `cargo install` prints `Installing tiny v0.2.0` when it
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
    # The log is what actually says whether it worked — the loop's exit status
    # is its own, and the shell has no way to reach back for cargo's through a
    # pipe.
    ! grep -q 'tiny-install-failed' "$LOG"
}

# How many crates the bar is counting against.
#
# `cargo tree` lists the packages actually reachable at build time — normal and
# build dependencies, no dev-dependencies — and cargo prints one `Compiling`
# line for each. Asking cargo rather than writing a number down means the bar
# stays right when the dependency list changes, and an old cargo that does not
# know the command just leaves this empty, which turns the bar into a count.
say "building — this takes a minute the first time"
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
build() {
    # --root puts the binary in $PREFIX/bin, so hand it the parent.
    { cargo install --path "$SRC" --bin tiny --root "$PREFIX/.." --force 2>&1 \
        || echo "tiny-install-failed"; } | tee "$LOG" | watch_build
}


if ! build; then
    # The sentinel is ours; showing it to someone whose build just failed would
    # only be one more confusing line among the ones that matter.
    say ""
    grep -v 'tiny-install-failed' "$LOG" | tail -n 30 >&2
    die "build failed — the output above says why"
fi

BIN="$PREFIX/tiny"
[ -x "$BIN" ] || die "expected a binary at $BIN"

# --- PATH -------------------------------------------------------------------

say ""
say "installed $BIN"
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
