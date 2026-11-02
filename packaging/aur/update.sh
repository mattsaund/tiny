#!/bin/sh
# Point both AUR packages at a release of tiny: the version, the checksums of
# its two Linux archives (from the release's own SHA256SUMS) and of the
# source GitHub makes for the tag, and the .SRCINFO the AUR reads.
#
# usage: packaging/aur/update.sh 0.6.7
#
# Run it on Arch, once the release is published: it needs curl, sha256sum
# and makepkg.
set -eu

version="${1:?usage: update.sh <version>, like 0.6.7}"
version="${version#v}"
here="$(cd "$(dirname "$0")" && pwd)"
url="https://github.com/mattsaund/tiny"

sums="$(curl -fsSL "$url/releases/download/v$version/SHA256SUMS")"
sum_of() {
    printf '%s\n' "$sums" | awk -v f="$1" '$2 == f { print $1 }'
}
x86_64="$(sum_of tiny-x86_64-unknown-linux-gnu.tar.gz)"
aarch64="$(sum_of tiny-aarch64-unknown-linux-gnu.tar.gz)"
source="$(curl -fsSL "$url/archive/refs/tags/v$version.tar.gz" | sha256sum | cut -d' ' -f1)"
for s in "$x86_64" "$aarch64" "$source"; do
    case "$s" in
        [0-9a-f]*) [ "${#s}" -eq 64 ] || { echo "a checksum is missing: $s" >&2; exit 1; } ;;
        *) echo "a checksum is missing for $version" >&2; exit 1 ;;
    esac
done

bump() {
    sed -i -e "s/^pkgver=.*/pkgver=$version/" -e "s/^pkgrel=.*/pkgrel=1/" "$1"
}

bin="$here/tiny-ide-bin/PKGBUILD"
bump "$bin"
sed -i -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$x86_64')/" \
    -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('$aarch64')/" "$bin"

src="$here/tiny-ide/PKGBUILD"
bump "$src"
sed -i -e "s/^sha256sums=.*/sha256sums=('$source')/" "$src"

for pkg in tiny-ide-bin tiny-ide; do
    (cd "$here/$pkg" && makepkg --printsrcinfo > .SRCINFO)
done
echo "tiny-ide-bin and tiny-ide are at $version"
