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
    # Clean up the checkout however this exits.
    trap 'rm -rf "$SRC"' EXIT INT TERM
    say "fetching $REPO ($REF)"
    git clone --depth 1 --branch "$REF" "$REPO" "$SRC" >/dev/null 2>&1 \
        || die "could not clone $REPO"
fi

# --- build ------------------------------------------------------------------

say "building — this takes a minute the first time"
mkdir -p "$PREFIX"
# --root puts the binary in $PREFIX/bin, so hand it the parent.
cargo install --path "$SRC" --bin tiny --root "$PREFIX/.." --force --quiet \
    || die "build failed"

BIN="$PREFIX/tiny"
[ -x "$BIN" ] || die "expected a binary at $BIN"

# --- PATH -------------------------------------------------------------------

say ""
say "installed $BIN"
case ":$PATH:" in
    *":$PREFIX:"*)
        say "run it with:  tiny ~/notes"
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
        ;;
esac
