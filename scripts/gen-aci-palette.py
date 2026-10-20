#!/usr/bin/env python3
"""Generate `ACI_RGB` for src/bits/color.rs: the AutoCAD Color Index palette.

Usage:
    curl -sSfLO https://raw.githubusercontent.com/mozman/ezdxf/v1.3.0/src/ezdxf/colors.py
    python3 -I scripts/gen-aci-palette.py colors.py > palette.rs

The source is ezdxf's `DXF_DEFAULT_COLORS` (MIT, Copyright (c) 2020 Manfred Moitzi).
Each entry is checked against the palette's structure:
- indices 1-9 are fixed colors;
- 10-249 are 24 hues 15 degrees apart, each with 10 shades. Even shades are the full hue, odd
  shades the hue halfway to white, both scaled by 1, 0.65, 0.5, 0.3 or 0.15 and floored;
- 250-255 are a gray ramp from 51 to 255.
ezdxf agrees with this structure on all but 26 entries. Those entries are digit slips (88 or 126
for 38 or 76 in the darkest odd shades of hues 9-23) and an older gray ramp. The generator replaces
them with the structural value and lists them on stderr.
"""

import ast
import math
import sys


def structural():
    p = [None] * 256
    fixed = [(0, 0, 0), (255, 0, 0), (255, 255, 0), (0, 255, 0), (0, 255, 255),
             (0, 0, 255), (255, 0, 255), (255, 255, 255), (128, 128, 128), (192, 192, 192)]
    for i, c in enumerate(fixed):
        p[i] = c
    for i in range(10, 250):
        k, s = divmod(i - 10, 10)
        h = k * 15.0

        def comp(n):
            x = (n + h / 60.0) % 6
            return 255.0 * (1.0 - max(0.0, min(1.0, min(x, 4 - x))))

        c = [comp(5), comp(3), comp(1)]
        if s % 2:
            c = [(x + 255.0) / 2 for x in c]
        f = [1.0, 0.65, 0.5, 0.3, 0.15][s // 2]
        p[i] = tuple(int(math.floor(x * f + 1e-9)) for x in c)
    for j in range(5):
        v = int(math.floor(51 + 204 * j / 5 + 1e-9))
        p[250 + j] = (v, v, v)
    p[255] = (255, 255, 255)
    return p


def ezdxf_palette(path):
    tree = ast.parse(open(path, encoding="utf-8").read())
    for node in tree.body:
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            target = node.targets[0] if isinstance(node, ast.Assign) else node.target
            if getattr(target, "id", None) == "DXF_DEFAULT_COLORS":
                values = ast.literal_eval(node.value)
                assert len(values) == 256, len(values)
                return [((v >> 16) & 255, (v >> 8) & 255, v & 255) for v in values]
    sys.exit("DXF_DEFAULT_COLORS not found")


def main():
    source = ezdxf_palette(sys.argv[1])
    rule = structural()
    table = list(source)
    for i in range(1, 256):
        if source[i] != rule[i]:
            print(f"corrected {i}: ezdxf {source[i]} -> {rule[i]}", file=sys.stderr)
            table[i] = rule[i]
    table[0] = (0, 0, 0)
    lines, cur = [], "   "
    for c in table:
        item = " [%d, %d, %d]," % c
        if len(cur) + len(item) > 99:
            lines.append(cur)
            cur = "   "
        cur += item
    lines.append(cur)
    print("pub const ACI_RGB: [[u8; 3]; 256] = [")
    print("\n".join(lines))
    print("];")


if __name__ == "__main__":
    main()
