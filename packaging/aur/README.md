# AUR packages

Two packages, each installing the `tiny` command, its manual page and its
license:

- `tiny-ide-bin`: the release's own prebuilt binary, for x86_64 and aarch64.
  Installs in seconds.
- `tiny-ide`: built from the tag's source with cargo, tests and all.

Both conflict with Arch's own `tiny`, an IRC client that is also a program
called `tiny`.

## Each release

Once the release is published on GitHub:

```sh
packaging/aur/update.sh 0.6.7   # versions, checksums, .SRCINFO
packaging/aur/publish.sh        # push both to the AUR
```

To try one first: `cd packaging/aur/tiny-ide-bin && makepkg -si`.

## Once, before the first publish

An account at <https://aur.archlinux.org>, with an SSH public key added under
My Account, and in `~/.ssh/config`:

```
Host aur.archlinux.org
  IdentityFile ~/.ssh/aur
  User aur
```

The first `publish.sh` makes both packages. Then anyone on Arch can:

```sh
yay -S tiny-ide-bin   # or tiny-ide, to build it
```
