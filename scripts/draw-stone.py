#!/usr/bin/env python3
"""Draw the procedural rock building layers: cobblestone and smooth stone.

Writes 16x16 layer images (8-level grayscale, fully opaque, as
src/block_texture/layer.rs checks) into OUT_DIR:

- cobblestone.png: rounded stones of different sizes packed together,
  with dark gaps between them. The stones are the cells of a jittered
  Voronoi diagram that wraps at the edges, each lit from the top left
  (a light rim at its upper left, a shaded one at its lower right) and a
  little lighter or darker than its neighbours. The gaps widen where
  three stones meet, rounding off their corners.
- smooth_stone.png: a calm polished face, flat but for a few faint flecks, with a one-pixel
  bevel: lit along the top and left edges, shaded along the bottom and
  right.

Both tile with themselves: the cobbles wrap across every edge, and the
smooth stone's bevel frames each block. Deterministic: the same seed
always draws the same images.

Usage:
    python3 scripts/draw-stone.py OUT_DIR [--preview PREVIEW.png]

--preview writes each image tiled 3x3, upscaled 8x, side by side, to
check the seams. Standard library only.
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


def px(level):
    g = GRAY[level]
    return (g, g, g, 255)


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


def wrap(d):
    """The shortest signed distance along one axis on the wrapping tile."""
    return (d + SIZE / 2) % SIZE - SIZE / 2


def scatter(rng, count, tries=40):
    """Points spread evenly but irregularly over the wrapping tile:
    each the best of `tries` random candidates, farthest from the rest."""
    points = [(rng.uniform(0, SIZE), rng.uniform(0, SIZE))]
    while len(points) < count:
        candidates = [(rng.uniform(0, SIZE), rng.uniform(0, SIZE)) for _ in range(tries)]
        points.append(
            max(candidates, key=lambda c: min(math.hypot(wrap(c[0] - p[0]), wrap(c[1] - p[1])) for p in points))
        )
    return points


def cobblestone(rng):
    centres = scatter(rng, 9)
    # Each stone's size (a bigger one pushes its border out) and how much
    # lighter or darker it is than the rest.
    sizes = [rng.uniform(-0.8, 0.8) for _ in centres]
    tone = [rng.choice((-1, 0, 0, 1)) for _ in centres]

    rows = []
    for y in range(SIZE):
        row = []
        for x in range(SIZE):
            p = (x + 0.5, y + 0.5)
            near = sorted(
                (math.hypot(dx, dy) - sizes[i], i, dx, dy)
                for i, c in enumerate(centres)
                for dx, dy in [(wrap(p[0] - c[0]), wrap(p[1] - c[1]))]
            )
            (d1, i, dx, dy), (d2, *_), (d3, *_) = near[:3]
            # 0 on the border between two stones; where three meet, the
            # corners are rounded off into a wider gap.
            edge = min(d2 - d1, (d3 - d1) * 0.75)
            if edge < 0.9:
                row.append(px(1))
                continue
            # Lit from the top left: positive toward the upper left of the
            # stone, strongest at its rim, flat across its middle.
            facing = -(dx + dy) / (math.sqrt(2) * max(math.hypot(dx, dy), 0.01))
            rim = max(0.0, 1.0 - (edge - 0.9) / 2.5)
            light = facing * (0.3 + 0.7 * rim)
            level = 4 + tone[i] + round(light * 1.6)
            # A dark lip where the stone turns down into the gap at its
            # lower right.
            if rim > 0.8 and facing < -0.4:
                level -= 1
            row.append(px(min(6, max(2, level))))
        rows.append(row)
    return rows


def smooth_stone(rng):
    rows = []
    for y in range(SIZE):
        row = []
        for x in range(SIZE):
            # A polished face: one level, with a rare faint fleck.
            level = 5 if rng.random() < 0.03 else 4
            # The bevel: lit along the top and left, shaded along the
            # bottom and right; the two corners where they meet in between.
            top, left = y == 0, x == 0
            bottom, right = y == SIZE - 1, x == SIZE - 1
            if (top and right) or (bottom and left):
                level = 4
            elif top or left:
                level = 6
            elif bottom or right:
                level = 2
            row.append(px(level))
        rows.append(row)
    return rows


def preview(path, images):
    scale, gap, tiles = 8, 8, 3
    side = SIZE * scale * tiles
    width = len(images) * (side + gap) + gap
    height = side + 2 * gap
    rows = [[(128, 128, 128, 255)] * width for _ in range(height)]
    for n, image in enumerate(images):
        ox = gap + n * (side + gap)
        for y in range(side):
            for x in range(side):
                rows[gap + y][ox + x] = image[(y // scale) % SIZE][(x // scale) % SIZE]
    write_png(path, rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out_dir")
    parser.add_argument("--preview")
    args = parser.parse_args()
    rng = random.Random(0xC0BB1E)
    images = {
        "cobblestone": cobblestone(rng),
        "smooth_stone": smooth_stone(rng),
    }
    os.makedirs(args.out_dir, exist_ok=True)
    for name, rows in images.items():
        write_png(os.path.join(args.out_dir, f"{name}.png"), rows)
    if args.preview:
        preview(args.preview, list(images.values()))


if __name__ == "__main__":
    main()
