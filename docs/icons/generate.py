#!/usr/bin/env python3
"""Render every icon in src/config.rs to docs/icons/<NAME>.svg.

    usage:
    python3 docs/icons/generate.py [path/to/NerdFont.ttf]

Needs fontTools. The default font is whatever fc-match reports first.
"""

import os
import re
import subprocess
import sys

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG_RS = os.path.join(os.path.dirname(os.path.dirname(HERE)), "src", "config.rs")

SIZE = 64
PAD = 4
FILL = "#8b949e" # readable on both GitHub light and dark themes
CANDIDATES = ["MesloLGSNerdFont-Regular", "MesloLGMNerdFont-Regular", "JetBrainsMonoNerdFont-Regular"]


def find_font(argv):
    if len(argv) > 1:
        return argv[1]
    out = subprocess.run(["fc-list", ":", "file"], capture_output=True, text=True).stdout
    found = []
    for line in out.splitlines():
        path = line.split(":")[0]
        if path.endswith((".ttf", ".otf")) and "nerdfont" in path.replace(" ", "").lower():
            found.append(path)
    for want in CANDIDATES:
        for path in found:
            if os.path.basename(path) == want:
                return path
    for path in found:
        if "Mono" not in os.path.basename(path):
            return path
    if found:
        return found[0]
    sys.exit("no Nerd Font found; pass one as the first argument")


def icons():
    block = open(CONFIG_RS).read().split("pub mod ico {", 1)[1].split("\n}", 1)[0]
    return re.findall(r'pub const (\w+): &str = "\\u\{([0-9a-f]+)\}";', block)


def render(font, name, cp):
    cmaps, glyphs = font.getBestCmap(), font.getGlyphSet()
    gname = cmaps.get(int(cp, 16))
    if gname is None:
        sys.exit(f"{name}: U+{cp.upper()} is not in this font")

    pen = BoundsPen(glyphs)
    glyphs[gname].draw(pen)
    if pen.bounds is None:
        sys.exit(f"{name}: glyph {gname} has no outline")
    x0, y0, x1, y1 = pen.bounds
    w, h = x1 - x0, y1 - y0
    if w <= 0 or h <= 0:
        sys.exit(f"{name}: glyph {gname} is empty")

    scale = min((SIZE - 2 * PAD) / w, (SIZE - 2 * PAD) / h)
    tx = SIZE / 2 - scale * (x0 + x1) / 2
    ty = SIZE / 2 + scale * (y0 + y1) / 2

    path = SVGPathPen(glyphs)
    glyphs[gname].draw(TransformPen(path, (scale, 0, 0, -scale, tx, ty)))

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" '
        f'viewBox="0 0 {SIZE} {SIZE}" role="img" aria-label="{name}">'
        f'<path fill="{FILL}" d="{path.getCommands()}"/></svg>\n'
    )


def main():
    path = find_font(sys.argv)
    font = TTFont(path)
    print(f"font: {path}")
    for name, cp in icons():
        open(os.path.join(HERE, name + ".svg"), "w").write(render(font, name, cp))
        print(f"  {name:9} U+{cp.upper()}")
    print(f"{len(icons())} icons written to {HERE}")


if __name__ == "__main__":
    main()
