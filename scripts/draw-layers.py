#!/usr/bin/env python3
"""Draw the procedural layer images: glass, water, lava, a grass tuft, an
ingot, moss and a bucket.

Writes 16x16 layer images (8-level grayscale with alpha, as
src/block_texture/layer.rs checks) into OUT_DIR:

- glass.png: a pane. A solid one-pixel frame, faint highlight streaks
  running diagonally, and a nearly clear middle. Partly transparent, so
  the glass blends (`render: "translucent"`).
- water.png: soft ripples, wider than tall, fully solid; the water
  material's palette carries the transparency.
- lava.png: dark crust plates with bright molten seams between them.
- tuft.png: blades of grass rising from the bottom edge, transparent
  around them, for the crossed-quad plant model.
- items/ingot.png: a metal bar seen from above and in front, its top
  lit, outlined, transparent around it: the ingot item's icon.
- moss.png: soft clumps of moss, transparent between them, to lay over
  stone. Wraps at the edges.
- items/bucket.png: a metal pail seen from the front and a little above:
  a wire handle arching over an open rim, its inside dark, the body
  tapering to the bottom with a hoop around it, lit from the left:
  the bucket item's icon.

Each tiles with itself where it needs to (water and lava wrap at the
edges). Deterministic: the same seed always draws the same images.

Usage:
    python3 scripts/draw-layers.py OUT_DIR [--preview PREVIEW.png]

--preview writes the images side by side over a mid gray, upscaled
8x. Standard library only.
"""

import argparse
import math
import os
import random
import struct
import zlib

SIZE = 16
LEVELS = 8
GRAY = [round(i * 255 / (LEVELS - 1)) for i in range(LEVELS)]
CLEAR = (0, 0, 0, 0)


def px(level, alpha=255):
    g = GRAY[level]
    return (g, g, g, alpha)


def write_png(path, rows):
    height, width = len(rows), len(rows[0])
    raw = b"".join(b"\0" + bytes(c for p in row for c in p) for row in rows)

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def wrapping_noise(rng, cells):
    """Smooth value noise on a SIZE grid that wraps at the edges, 0 to 1."""
    grid = [[rng.random() for _ in range(cells)] for _ in range(cells)]

    def smooth(t):
        return t * t * (3 - 2 * t)

    def at(x, y):
        fx, fy = x * cells / SIZE, y * cells / SIZE
        x0, y0 = int(fx) % cells, int(fy) % cells
        x1, y1 = (x0 + 1) % cells, (y0 + 1) % cells
        tx, ty = smooth(fx - int(fx)), smooth(fy - int(fy))
        top = grid[y0][x0] * (1 - tx) + grid[y0][x1] * tx
        bottom = grid[y1][x0] * (1 - tx) + grid[y1][x1] * tx
        return top * (1 - ty) + bottom * ty

    return at


def glass():
    rows = [[px(5, 40) for _ in range(SIZE)] for _ in range(SIZE)]
    # Two short parallel streaks, from bottom left to top right, in the
    # upper left of the pane: (where x + y is, which x they cover).
    for diagonal, xs in ((9, range(3, 8)), (12, range(5, 9))):
        for x in xs:
            rows[diagonal - x][x] = px(7, 120)
    for i in range(SIZE):
        for x, y in ((i, 0), (i, SIZE - 1), (0, i), (SIZE - 1, i)):
            rows[y][x] = px(6, 255)
    # The frame's lower and right edges a shade darker, lit from the top left.
    for i in range(1, SIZE):
        rows[SIZE - 1][i] = px(4, 255)
        rows[i][SIZE - 1] = px(4, 255)
    return rows


def water(rng):
    noise = wrapping_noise(rng, 4)
    rows = []
    for y in range(SIZE):
        row = []
        for x in range(SIZE):
            # Ripples: horizontal bands warped by the noise.
            wave = math.sin((y + noise(x, y) * 5) * 2 * math.pi / 8)
            value = 0.5 + 0.35 * wave + 0.25 * (noise((x + 7) % SIZE, (y + 3) % SIZE) - 0.5)
            row.append(px(min(5, max(2, 2 + int(value * 4)))))
        rows.append(row)
    return rows


def lava(rng):
    # Crust plates around random centres (wrapping); the seams between two
    # plates are molten and bright.
    centres = [(rng.uniform(0, SIZE), rng.uniform(0, SIZE)) for _ in range(6)]

    def dist(x, y, c):
        dx = min(abs(x - c[0]), SIZE - abs(x - c[0]))
        dy = min(abs(y - c[1]), SIZE - abs(y - c[1]))
        return math.hypot(dx, dy)

    noise = wrapping_noise(rng, 4)
    rows = []
    for y in range(SIZE):
        row = []
        for x in range(SIZE):
            d = sorted(dist(x + 0.5, y + 0.5, c) for c in centres)
            seam = d[1] - d[0]  # 0 on the border between two plates
            heat = 0.85 * max(0.0, 1.0 - seam / 1.6) + 0.3 * noise(x, y)
            row.append(px(min(7, max(1, 1 + int(heat * 6)))))
        rows.append(row)
    return rows


def tuft(rng):
    rows = [[CLEAR for _ in range(SIZE)] for _ in range(SIZE)]
    # (base x, height, how far the tip bends sideways): blades fanning out
    # from the middle, bending more toward their tips.
    blades = [(3, 7, -3), (5, 11, -4), (6, 14, -2), (7, 9, 1), (8, 15, 0),
              (9, 12, 2), (10, 14, 4), (11, 8, 3), (12, 11, 3)]
    for base, height, bend in blades:
        height += rng.choice((-1, 0, 0, 1))
        height = min(height, SIZE)
        for i in range(height):
            y = SIZE - 1 - i
            x = int(round(base + bend * (i / height) ** 2))
            if not 0 <= x < SIZE:
                continue
            t = i / max(1, height - 1)
            level = 2 if t < 0.2 else 6 if t > 0.85 else 4 if (x + i) % 3 else 5
            rows[y][x] = px(level)
            # Thicker near the ground.
            if t < 0.35 and 0 <= x + 1 < SIZE and rows[y][x + 1] == CLEAR:
                rows[y][x + 1] = px(max(2, level - 1))
    return rows


INGOT = [
    "................",
    "................",
    "................",
    "................",
    "....11111111....",
    "...1677777761...",
    "..166666666661..",
    ".15555555555551.",
    ".14444444444441.",
    ".14444444444441.",
    ".13333333333331.",
    "..111111111111..",
    "................",
    "................",
    "................",
    "................",
]


def ingot():
    rows = [[CLEAR for _ in range(SIZE)] for _ in range(SIZE)]
    for y, line in enumerate(INGOT):
        for x, c in enumerate(line):
            if c != ".":
                rows[y][x] = px(int(c))
    # A glint on the front face.
    rows[8][3] = px(6)
    rows[8][4] = px(5)
    return rows


BUCKET = [
    "................",
    "....11111111....",
    "...12......21...",
    "..12........21..",
    "..1..........1..",
    ".16666666666661.",
    ".61111111111116.",
    ".15777766665551.",
    "..156554444321..",
    "..156554444321..",
    "..167665555431..",
    "...1655444431...",
    "...1655444431...",
    "...1655444431...",
    "...1111111111...",
    "................",
]


def bucket():
    return [[CLEAR if c == "." else px(int(c)) for c in line] for line in BUCKET]


def moss(rng):
    noise = wrapping_noise(rng, 4)
    fine = wrapping_noise(rng, 16)
    rows = [[CLEAR for _ in range(SIZE)] for _ in range(SIZE)]
    for y in range(SIZE):
        for x in range(SIZE):
            v = noise(x, y) * 0.6 + fine(x, y) * 0.4
            if v < 0.47:
                continue
            # Thicker toward the middle of a clump, darker at its ragged
            # edge, with single lit tufts here and there.
            t = min(1.0, (v - 0.47) / 0.3)
            level = 1 + round(t * 4)
            if fine(x, y) > 0.72 and t > 0.3:
                level = 6
            rows[y][x] = px(level)
    return rows


def preview(path, images):
    scale, gap = 8, 8
    width = len(images) * (SIZE * scale + gap) + gap
    height = SIZE * scale + 2 * gap
    mid = 128
    rows = [[(mid, mid, mid, 255)] * width for _ in range(height)]
    for n, image in enumerate(images):
        ox = gap + n * (SIZE * scale + gap)
        for y in range(SIZE * scale):
            for x in range(SIZE * scale):
                r, g, b, a = image[y // scale][x // scale]
                t = a / 255
                c = round(r * t + mid * (1 - t))
                rows[gap + y][ox + x] = (c, c, c, 255)
    write_png(path, rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out_dir")
    parser.add_argument("--preview")
    args = parser.parse_args()
    rng = random.Random(0xDEFA41A)
    images = {
        "glass": glass(),
        "water": water(rng),
        "lava": lava(rng),
        "tuft": tuft(rng),
        "items/ingot": ingot(),
        "moss": moss(rng),
        "items/bucket": bucket(),
    }
    for name, rows in images.items():
        path = os.path.join(args.out_dir, f"{name}.png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        write_png(path, rows)
    if args.preview:
        preview(args.preview, list(images.values()))


if __name__ == "__main__":
    main()
