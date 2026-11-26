#!/usr/bin/env python3
"""Draw the network and machine layer images: wires, pipes, tubes, casings.

Writes 16x16 layer images (8-level grayscale with alpha, as
src/block_texture/layer.rs checks) into OUT_DIR. Every machine face is the
same casing (a two-pixel riveted frame, lit from the top left, around a
recessed flat panel) with its feature drawn into the panel, so the set reads
as one family. Arms (wire, pipe_side, tube_side) run left to right in a band
across the middle and tile horizontally; partial boxes sample the tile at
their own position, so the band sits where an 8-pixel (pipe, tube) or
4-pixel (wire) arm box samples it. Transparent around them.

The wrench item is split from a Retro Diffusion sprite (WRENCH_RAW, in
color) by hue: the red grip becomes wrench_handle, the gray metal
wrench_head, each rounded to the gray levels and remapped like the other
tools.

Usage:
    python3 draw-machines.py OUT_DIR WRENCH_RAW.png

Standard library only. Deterministic.
"""

import math
import os
import struct
import sys
import zlib

GRAY = [0, 36, 73, 109, 146, 182, 219, 255]
SIZE = 16


def read_png(path):
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    pos, idat, plte, trns = 8, b"", None, None
    while pos < len(data):
        (n,) = struct.unpack(">I", data[pos : pos + 4])
        kind, body = data[pos + 4 : pos + 8], data[pos + 8 : pos + 8 + n]
        pos += 12 + n
        if kind == b"IHDR":
            w, h, depth, ctype, _, _, inter = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        elif kind == b"PLTE":
            plte = body
        elif kind == b"tRNS":
            trns = body
    assert depth == 8 and not inter, (path, depth, inter)
    ch = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    raw, stride = zlib.decompress(idat), w * ch
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(h):
        f, line = raw[i], bytearray(raw[i + 1 : i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - ch] if x >= ch else 0
            b = prev[x]
            c = prev[x - ch] if x >= ch else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    out = []
    for line in rows:
        r = []
        for x in range(w):
            p = line[x * ch : (x + 1) * ch]
            if ctype == 0:
                r.append((p[0], p[0], p[0], 255))
            elif ctype == 2:
                r.append((p[0], p[1], p[2], 255))
            elif ctype == 3:
                k = p[0]
                al = trns[k] if trns and k < len(trns) else 255
                r.append((plte[3 * k], plte[3 * k + 1], plte[3 * k + 2], al))
            elif ctype == 4:
                r.append((p[0], p[0], p[0], p[1]))
            else:
                r.append(tuple(p))
        out.append(r)
    return out


def write_png(path, rows):
    h, w = len(rows), len(rows[0])
    raw = b"".join(b"\0" + bytes(c for p in row for c in p) for row in rows)

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    open(path, "wb").write(png)


def save_levels(path, grid):
    rows = []
    for r in grid:
        out = []
        for c in r:
            if c is None:
                out.append((0, 0, 0, 0))
            else:
                lv, al = c if isinstance(c, tuple) else (c, 255)
                out.append((GRAY[lv], GRAY[lv], GRAY[lv], al))
        rows.append(out)
    write_png(path, rows)


N = SIZE


def blank():
    return [[None] * N for _ in range(N)]


def solid(level):
    return [[level] * N for _ in range(N)]


def stamp(grid, x0, y0, text):
    """Draw ASCII rows ('.' leaves the pixel) with the top left at x0, y0."""
    for dy, row in enumerate(text.strip("\n").split("\n")):
        for dx, ch in enumerate(row.strip()):
            if ch != ".":
                grid[y0 + dy][x0 + dx] = int(ch)
    return grid


# --- the casing --------------------------------------------------------


def casing():
    """A riveted frame around a recessed panel; fully opaque."""
    g = solid(3)
    for i in range(N):
        g[0][i] = 5  # outer rim: lit top and left, shaded bottom and right
        g[i][0] = 5
        g[N - 1][i] = 2
        g[i][N - 1] = 2
    g[0][0], g[0][N - 1], g[N - 1][0], g[N - 1][N - 1] = 6, 4, 4, 1
    for i in range(1, N - 1):  # inner ring of the frame
        g[1][i] = g[N - 2][i] = g[i][1] = g[i][N - 2] = 4
    for x, y in ((1, 1), (14, 1), (1, 14), (14, 14)):  # rivets
        g[y][x] = 6
    for i in range(2, N - 2):  # the panel's lip, shading its top and left
        g[2][i] = 2
        g[i][2] = 2
    for x0, y0, n in ((4, 5, 3), (9, 9, 2), (5, 11, 2), (10, 4, 2)):  # brushed streaks
        for x in range(x0, x0 + n):
            g[y0][x] = 4
    return g


def clear_panel(g, x0, y0, x1, y1, level=3):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            g[y][x] = level
    return g


def machine_side():
    return casing()


def machine_top():
    g = clear_panel(casing(), 3, 3, 13, 13)
    for x in range(4, 12):
        g[4][x] = 2  # the grille's recess
    for y in (5, 7, 9, 11):
        for x in range(4, 12):
            g[y][x] = 1 if 4 < x < 11 else 2  # slots
    for y in (6, 8, 10, 12):
        for x in range(4, 12):
            g[y][x] = 4 if 4 < x < 11 else 3  # slats, lit on top
    return g


def furnace_front(lit):
    g = clear_panel(casing(), 3, 3, 13, 13)
    # A vent slot above the door: dark, glowing faintly when lit.
    stamp(g, 5, 4, "555555" if lit else "111111")
    stamp(g, 5, 5, "444444")
    # The raised frame around the firebox and its dark opening.
    stamp(
        g,
        3,
        7,
        """
        5555555554
        5111111112
        5100000012
        5100000012
        5100000012
        5144444442
        4222222221
        """,
    )
    if lit:
        stamp(
            g,
            5,
            8,
            """
            050050
            565765
            676776
            727272
            """,
        )
    else:
        stamp(g, 5, 11, "202020")  # grate bars
    return g


def furnace_fire():
    """Just the fire of furnace_front_lit, for a palette of its own."""
    lit, out = furnace_front(True), blank()
    for y in range(8, 12):
        for x in range(5, 11):
            if lit[y][x] >= 5:
                out[y][x] = lit[y][x]
    for x in range(5, 11):
        out[4][x] = 5
    return out


def generator_front():
    """A flywheel behind a round opening: a rim, four spokes, a hub."""
    g = clear_panel(casing(), 3, 3, 13, 13)
    cx = cy = 8
    for y in range(3, 14):
        for x in range(3, 14):
            dx, dy = x - cx, y - cy
            d = math.hypot(dx, dy)
            side = dx + dy  # negative toward the light (top left)
            if d > 5.5:
                continue
            if d > 4.5:  # the opening's edge in the casing
                g[y][x] = 1 if side < 0 else 4
            elif d > 3.2:  # the wheel's rim
                g[y][x] = 6 if side < -2 else 5 if side <= 2 else 3
            elif d < 1.5:  # the hub
                g[y][x] = 6 if side < 0 else 5 if side == 0 else 3
            elif dx == 0 or dy == 0:  # spokes
                g[y][x] = 4
            else:  # dark gaps between the spokes
                g[y][x] = 0 if d < 2.5 else 1
    return g


def battery_side():
    g = clear_panel(casing(), 3, 3, 13, 13)
    stamp(
        g,
        5,
        3,
        """
        111111
        166664
        155554
        111114
        166664
        155554
        111114
        166664
        155554
        144444
        """,
    )
    return g


def pump_front():
    g = clear_panel(casing(), 3, 3, 13, 13)
    stamp(
        g,
        3,
        3,
        """
        5555555554
        5111111112
        5101401012
        5144544412
        5101401012
        5101401012
        5144544412
        5101401012
        5111111112
        4222222221
        """,
    )
    return g


def outlet_front():
    g = clear_panel(casing(), 3, 3, 13, 13)
    stamp(
        g,
        4,
        3,
        """
        55555554
        56444462
        54444442
        42544322
        ..5443..
        ..5443..
        ..5443..
        ..5443..
        .554432.
        .410012.
        ..2222..
        """,
    )
    return g


def tank_side():
    g = clear_panel(casing(), 3, 3, 13, 13)
    stamp(
        g,
        6,
        3,
        """
        1111
        1654
        1654
        1654
        1544
        1544
        1544
        1544
        1544
        1444
        """,
    )
    for y in (5, 8, 11):  # gauge marks beside the glass
        stamp(g, 11, y, "11")
    return g


def tank_fill(level):
    """The fluid in the tank's sight glass, `level` quarters full: an
    overlay for `tank_side`, colored by the fluid. The glass inside is
    x 7-9, y 4-12; a lit surface row on top, a lighter left edge."""
    g = blank()
    rows = -(-9 * level // 4)
    for y in range(13 - rows, 13):
        for x in range(7, 10):
            g[y][x] = 5 if x == 7 else 4
    for x in range(7, 10):
        g[13 - rows][x] = 6
    return g


PORT = """
655554
511112
510032
510032
513332
422221
"""


def port():
    return stamp(blank(), 5, 5, PORT)


def solar_cell():
    """A solar panel's face, an overlay over the casing's panel (2-14): a
    three by three grid of cells in dark seams, each lit at its top left
    with a glint, its bottom right in shade. Transparent around it."""
    g = blank()
    for y in range(2, 15):
        for x in range(2, 15):
            g[y][x] = 1  # the seams
    for cy in (3, 7, 11):
        for cx in (3, 7, 11):
            for dy in range(3):
                for dx in range(3):
                    g[cy + dy][cx + dx] = 3
            g[cy][cx] = 5  # glint
            g[cy][cx + 1] = 4
            g[cy + 1][cx] = 4
            g[cy + 2][cx + 2] = 2  # shade
    return g


def crate_side():
    """Crossed braces over a dark panel, the port where they meet."""
    g = clear_panel(casing(), 3, 3, 13, 13, level=2)
    for i in range(3, 14):
        g[2][i] = g[i][2] = 1
    for y in range(3, 14):
        for x in range(3, 14):
            a, b = x - y, x + y - 15  # the two diagonals, through the middle
            if abs(a) <= 0.5 or abs(b) <= 0.5:
                g[y][x] = 4
            elif a == -1 or b == -1:
                g[y][x] = 5  # the braces' lit upper edges
            elif a == 1 or b == 1:
                g[y][x] = 3
    stamp(g, 3, 3, "44\n4")  # corner brackets
    stamp(g, 12, 3, "44\n.3")
    stamp(g, 3, 12, "4\n33")
    stamp(g, 12, 12, ".3\n33")
    return stamp(g, 6, 6, "5554\n5002\n5002\n4221")  # the port


# --- wires, pipes, tubes ----------------------------------------------


def wire():
    g = blank()
    rows = {6: 4, 7: 6, 8: 4, 9: 2}
    for y, base in rows.items():
        for x in range(N):
            level = base
            if (x - y) % 4 == 0:  # the strands' twist
                level = max(1, base - (2 if y == 7 else 1))
            g[y][x] = level
    return g


def wire_core():
    g = blank()
    cx = cy = 7.5
    for y in range(N):
        for x in range(N):
            dx, dy = x - cx, y - cy
            if math.hypot(dx, dy) > 3.3:
                continue
            light = -(dx + dy) / 2  # higher toward the top left
            level = 4 + round(light / 1.5)
            if abs(dx - dy) < 1:  # a wrap of the cable across the knot
                level -= 2
            g[y][x] = max(1, min(6, level))
    return g


def pipe_side():
    g = blank()
    rows = [3, 5, 6, 5, 4, 4, 3, 1]
    for i, base in enumerate(rows):
        for x in range(N):
            g[4 + i][x] = max(1, base - 1) if x in (0, N - 1) else base
    return g


def pipe_end():
    g = blank()
    cx = cy = 7.5
    for y in range(N):
        for x in range(N):
            dx, dy = x - cx, y - cy
            d = math.hypot(dx, dy)
            side = dx + dy
            if d > 4.3:
                continue
            if d > 2.3:  # the flange and wall, lit from the top left
                level = 5 if side < -1 else 2 if side > 1.5 else 4
                if d > 3.6 and level > 2:
                    level -= 1
                g[y][x] = level
            else:  # the hollow; its far wall catches a little light
                g[y][x] = 1 if side > 1 else 0
    return g


def pipe_core():
    g = blank()
    stamp(
        g,
        3,
        3,
        """
        5555555554
        5644444462
        5444444442
        5444554442
        5445334442
        5445314442
        5443114442
        5444444442
        5644444462
        4222222221
        """,
    )
    return g


def tube_side():
    g = blank()
    # (level, alpha) per row: solid rims, a clear middle with a glint.
    rows = [(5, 255), (7, 150), (6, 80), (5, 40), (5, 40), (5, 40), (4, 70), (3, 255)]
    for i, (level, alpha) in enumerate(rows):
        for x in range(N):
            if x in (0, N - 1):  # a collar where it meets the next tube
                g[4 + i][x] = (4 if 0 < i < 7 else level, 255)
            elif i == 1 and x % 8 in (5, 6):
                g[4 + i][x] = (5, 60)  # breaks in the glint
            else:
                g[4 + i][x] = (level, alpha)
    return g


def tube_end():
    g = blank()
    cx = cy = 7.5
    for y in range(N):
        for x in range(N):
            dx, dy = x - cx, y - cy
            d = math.hypot(dx, dy)
            side = dx + dy
            if d > 4.3:
                continue
            if d > 3.0:  # the glass rim
                g[y][x] = (6 if side < -1 else 3 if side > 1.5 else 5, 255)
            elif d > 2.2 and side < -1:
                g[y][x] = (7, 140)  # a glint inside the rim
            else:
                g[y][x] = (5, 40)
    return g


# --- the wrench --------------------------------------------------------


def lum(r, g, b):
    return 0.299 * r + 0.587 * g + 0.114 * b


def wrench(raw_path):
    rows = read_png(raw_path)
    head, handle = blank(), blank()
    for y, row in enumerate(rows):
        for x, (r, g, b, a) in enumerate(row):
            if a == 0:
                continue
            level = min(7, round(lum(r, g, b) / 255 * 7))
            if r - max(g, b) > 30:  # the red grip
                handle[y][x] = {1: 1, 2: 2, 3: 3, 4: 4, 5: 5}.get(level, level)
            else:  # the metal: remapped like the other tool heads
                head[y][x] = {1: 1, 2: 2, 3: 3, 4: 5, 5: 6}.get(level, level)
    # By hand: the grip's top pixel is the head's collar.
    handle[7][6] = None
    head[7][6] = 4
    return head, handle


def main():
    out, raw = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    layers = {
        "wire": wire(),
        "wire_core": wire_core(),
        "pipe_side": pipe_side(),
        "pipe_end": pipe_end(),
        "pipe_core": pipe_core(),
        "tube_side": tube_side(),
        "tube_end": tube_end(),
        "machine_side": machine_side(),
        "machine_top": machine_top(),
        "furnace_front": furnace_front(False),
        "furnace_front_lit": furnace_front(True),
        "furnace_fire": furnace_fire(),
        "generator_front": generator_front(),
        "battery_side": battery_side(),
        "pump_front": pump_front(),
        "outlet_front": outlet_front(),
        "tank_side": tank_side(),
        "tank_fill_1": tank_fill(1),
        "tank_fill_2": tank_fill(2),
        "tank_fill_3": tank_fill(3),
        "tank_fill_4": tank_fill(4),
        "crate_side": crate_side(),
        "port": port(),
        "solar_cell": solar_cell(),
    }
    head, handle = wrench(raw)
    layers["items/wrench_head"] = head
    layers["items/wrench_handle"] = handle
    for name, grid in layers.items():
        path = os.path.join(out, name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        save_levels(path, grid)
    print(f"wrote {len(layers)} layers to {out}")


if __name__ == "__main__":
    main()
