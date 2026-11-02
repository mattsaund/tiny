#!/bin/sh
# Push both AUR packages, as update.sh left them, to their repositories on
# the AUR. The first push of each makes it.
#
# usage: packaging/aur/publish.sh
#
# Needs an AUR account with an SSH key, see README.md. The AUR's own clones
# are kept in $AUR_DIR, ~/aur unless said otherwise.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
aur="${AUR_DIR:-$HOME/aur}"
mkdir -p "$aur"

for pkg in tiny-ide-bin tiny-ide; do
    if [ ! -d "$aur/$pkg/.git" ]; then
        git clone "ssh://aur@aur.archlinux.org/$pkg.git" "$aur/$pkg"
    fi
    cp "$here/$pkg/PKGBUILD" "$here/$pkg/.SRCINFO" "$aur/$pkg/"
    version="$(sed -n 's/^pkgver=//p' "$here/$pkg/PKGBUILD")"
    rel="$(sed -n 's/^pkgrel=//p' "$here/$pkg/PKGBUILD")"
    (
        cd "$aur/$pkg"
        git add PKGBUILD .SRCINFO
        if git diff --cached --quiet; then
            echo "$pkg is already at $version-$rel"
        else
            git commit -q -m "tiny $version-$rel"
            # The AUR takes one branch, master, whatever git here calls a
            # new one.
            git push -q origin HEAD:master
            echo "$pkg published at $version-$rel"
        fi
    )
done
