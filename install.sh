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
    SRC="$(mktemp -d)"
    SRC_IS_TEMP=1
    printf 'fetching %s (%s) ... ' "$REPO" "$REF"
    git clone --depth 1 --branch "$REF" "$REPO" "$SRC" >/dev/null 2>&1 \
        || { printf '\n'; die "could not clone $REPO"; }
    say "done"
fi

# --- build ------------------------------------------------------------------

mkdir -p "$PREFIX"

# How many crates the bar is counting against.
#
# `cargo tree` lists the packages actually reachable at build time — normal and
# build dependencies, no dev-dependencies — and cargo prints one `Compiling`
# line for each. Asking cargo rather than writing a number down means the bar
# stays right when the dependency list changes, and an old cargo that does not
# know the command just leaves this empty, which turns the bar into a count.
TOTAL=$(cargo tree --manifest-path "$SRC/Cargo.toml" \
            -e normal,build --prefix none --no-dedupe 2>/dev/null \
        | awk 'NF' | sort -u | wc -l | tr -d ' ')
case "$TOTAL" in ''|*[!0-9]*|0) TOTAL=0 ;; esac

say "building — this takes a minute the first time"
LOG="$(mktemp)"

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

# Turn cargo's running commentary into one line that moves.
#
# The bar is drawn with `\r` and no newline, so the whole build occupies a
# single line however many crates go past. Two details are not optional:
# the field holding the crate name is padded, because a shorter name has to
# wipe the longer one it replaced and no clear-to-end-of-line escape is worth
# assuming; and every frame is flushed, because awk buffers and a bar that
# arrives in one burst at the end is not a bar.
#
# With no total — an old cargo, or a `cargo tree` that failed — it counts
# instead of filling. A bar that cannot say how far along it is should say so
# rather than invent a denominator.
watch_build() {
    if tty_out; then
        awk -v total="$TOTAL" '
            function bar(n, what,   p, f, s, i) {
                if (total <= 0) {
                    printf "\r  %d crates  %-30.30s", n, what
                    fflush()
                    return
                }
                p = int(n * 100 / total); if (p > 100) p = 100
                f = int(p * 28 / 100); s = ""
                for (i = 0; i < 28; i++) s = s (i < f ? "#" : ".")
                printf "\r  [%s] %3d%%  %-30.30s", s, p, what
                fflush()
            }
            /^ *Updating /   { printf "\r  %-52.52s", "updating the crate index"; fflush(); next }
            /^ *Downloaded / { d++; printf "\r  %-52.52s", "fetched " d " crate" (d == 1 ? "" : "s"); fflush(); next }
            /^ *Compiling /  { n++; bar(n, "compiling " $2); next }
            /^ *Installing / { bar(total, "installing"); next }
            # The sentinel is bookkeeping, not something to show anyone.
            /tiny-install-failed/ { next }
            END { if (n > 0 || d > 0) printf "\n" }
        '
    else
        # Not a terminal: a bar redrawn with carriage returns into a log file
        # is one unreadable line thousands of characters long. Drain it and
        # let the log speak.
        cat >/dev/null
    fi
    # The log is what actually says whether it worked — awk's exit status is
    # awk's, and the shell has no way to reach back for cargo's through a pipe.
    ! grep -q 'tiny-install-failed' "$LOG"
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
