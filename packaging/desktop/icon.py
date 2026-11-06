#!/usr/bin/env python3
"""Draw tiny's icon, and write it in every form a system asks for.

The icon is the mark on https://msaunders.dev/tiny/ in color: a green disk,
the site's forest green, with JetBrains Mono's lowercase t on it in white.
The mark there is a disk of glyphs with the t knocked out of it; this keeps
its proportions, which were measured off that mark: the t, at weight 800, is
0.621 of the disk's width tall, and sits in the middle of it.

Everything here is made from the outline below, so only this file needs
keeping; run it again after changing it:

    python3 packaging/desktop/icon.py

It needs rsvg-convert, from librsvg, to turn the drawing into pictures, and
writes beside itself:

    tiny.svg       the drawing, for Linux at any size
    icons/hicolor  that drawing in the sizes a Linux desktop looks for
    tiny.icns      macOS, with the margin Apple's icons keep
    tiny.ico       Windows, built into tiny.exe (see build.rs)
"""

import os
import re
import struct
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))

GREEN = "#3a7a4c"
WHITE = "#ffffff"

# JetBrains Mono ExtraBold's lowercase t (release 2.304, under the SIL Open
# Font License): its outline in the font's own units, 1000 to the em with y
# going up, and the box its ink fills.
T = (
    "M355 0Q271 0 223 48Q175 96 175 180V415H27V550H175V705H325V550H540V415H325"
    "V185Q325 163 336.5 149Q348 135 370 135H535V0Z"
)
T_LEFT, T_BOTTOM, T_RIGHT, T_TOP = 27, 0, 540, 705

# How tall the t stands against the disk's width.
T_HEIGHT = 0.621


def drawing(radius, size=512):
    """The icon as SVG, on a `size` square, with a disk of `radius`."""
    c = size / 2
    s = T_HEIGHT * 2 * radius / (T_TOP - T_BOTTOM)
    # The t's ink box, centered on the disk's center.
    x0 = c - s * (T_LEFT + T_RIGHT) / 2
    y0 = c + s * (T_TOP + T_BOTTOM) / 2

    def num(v):
        return f"{v:.2f}".rstrip("0").rstrip(".")

    out = []
    for cmd, args in re.findall(r"([MQHVZ])([^MQHVZ]*)", T):
        vals = [float(v) for v in args.replace(",", " ").split()]
        if cmd == "H":
            vals = [x0 + s * v for v in vals]
        elif cmd == "V":
            vals = [y0 - s * v for v in vals]
        else:
            vals = [
                x0 + s * v if i % 2 == 0 else y0 - s * v for i, v in enumerate(vals)
            ]
        out.append(cmd + " ".join(num(v) for v in vals))
    path = "".join(out)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}"'
        f' viewBox="0 0 {size} {size}">\n'
        f'<circle cx="{num(c)}" cy="{num(c)}" r="{num(radius)}" fill="{GREEN}"/>\n'
        f'<path d="{path}" fill="{WHITE}"/>\n'
        f"</svg>\n"
    )


def picture(svg, px):
    """`svg` as a PNG `px` pixels square."""
    return subprocess.run(
        ["rsvg-convert", "-w", str(px), "-h", str(px)],
        input=svg.encode(),
        capture_output=True,
        check=True,
    ).stdout


def ico(images):
    """A Windows icon of PNG pictures, by size: what every Windows since
    Vista reads, and far smaller than the bitmaps the format began with."""
    head = struct.pack("<HHH", 0, 1, len(images))
    at = 6 + 16 * len(images)
    entries, data = b"", b""
    for px, png in images:
        side = 0 if px >= 256 else px
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(png), at)
        at += len(png)
        data += png
    return head + entries + data


def icns(images):
    """A macOS icon: each picture under the four letters that name its size."""
    body = b"".join(
        kind.encode() + struct.pack(">I", 8 + len(png)) + png for kind, png in images
    )
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def write(name, data):
    path = os.path.join(HERE, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def main():
    # Linux and Windows draw an icon to the edge of its square, the way the
    # icons beside it in a menu are drawn; the disk leaves a thirty-second.
    flat = drawing(radius=240)
    write("tiny.svg", flat.encode())
    write("icons/hicolor/scalable/apps/tiny.svg", flat.encode())
    pngs = {}
    for px in (16, 20, 22, 24, 32, 40, 48, 64, 128, 256, 512):
        pngs[px] = picture(flat, px)
    for px in (16, 22, 24, 32, 48, 64, 128, 256, 512):
        write(f"icons/hicolor/{px}x{px}/apps/tiny.png", pngs[px])
    write("tiny.ico", ico([(px, pngs[px]) for px in (16, 20, 24, 32, 40, 48, 64, 256)]))

    # macOS keeps a margin round every icon: the body of one is 824 of its
    # 1024, which for a disk is its width.
    mac = drawing(radius=206)
    at = {px: picture(mac, px) for px in (16, 32, 64, 128, 256, 512, 1024)}
    write(
        "tiny.icns",
        icns(
            [
                ("icp4", at[16]),
                ("ic11", at[32]),
                ("icp5", at[32]),
                ("ic12", at[64]),
                ("icp6", at[64]),
                ("ic07", at[128]),
                ("ic13", at[256]),
                ("ic08", at[256]),
                ("ic14", at[512]),
                ("ic09", at[512]),
                ("ic10", at[1024]),
            ]
        ),
    )
    print("wrote", ", ".join(["tiny.svg", "icons/hicolor", "tiny.ico", "tiny.icns"]))


if __name__ == "__main__":
    main()
