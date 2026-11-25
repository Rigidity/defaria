#!/usr/bin/env python3
"""Draw the ten crack stages shown on a block being mined.

Writes OUT_DIR/stage_0.png .. stage_9.png: 16x16 layer images (8-level
grayscale with alpha, as src/block_texture/layer.rs checks), transparent
where there's no crack. The game multiplies the block under them by the
gray, so dark pixels are deep cracks and lighter ones shallow chips.

The cracks are one fixed network of jagged, branching lines that spread
from a point a little off the middle. Each line has a time it reaches each
of its pixels; stage N shows every pixel reached by stage N's time, so each
stage has everything the one before had, and more. A pixel's level darkens
as the cracks open up: new pixels start shallow (level 3) and deepen to
level 1 two stages later. From stage 5 on, a few chips (level 5) flake off
next to the oldest lines.

Deterministic: the same seed always draws the same stages.

Usage:
    python3 scripts/crack-stages.py OUT_DIR [--preview PREVIEW.png]

--preview writes all ten stages side by side over a mid gray, upscaled 8x.
Standard library only.
"""

import argparse
import math
import os
import random
import struct
import zlib

SIZE = 16
STAGES = 10
LEVELS = 8
GRAY = [round(i * 255 / (LEVELS - 1)) for i in range(LEVELS)]
SEED = 27


def write_png(path, rows):
    height, width = len(rows), len(rows[0])
    raw = b"".join(b"\0" + bytes(c for px in row for c in px) for row in rows)

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    open(path, "wb").write(png)


def walk(rng, start, angle, length, born, times, lines):
    """A jagged line from `start`, heading `angle` (radians), wandering a
    little at each step. Records the time each new pixel is reached."""
    x, y = start
    fx, fy = float(x), float(y)
    t = born
    for _ in range(length):
        angle += rng.uniform(-0.45, 0.45)
        fx += math.cos(angle)
        fy += math.sin(angle)
        nx, ny = round(fx), round(fy)
        if not (0 <= nx < SIZE and 0 <= ny < SIZE):
            break
        if (nx, ny) == (x, y):
            continue
        x, y = nx, ny
        t += 1
        if (x, y) not in times or times[(x, y)] > t:
            times[(x, y)] = t
        lines.append(((x, y), t, angle))
    return lines


def network():
    """Every crack pixel and the time the cracks reach it."""
    rng = random.Random(SEED)
    times = {}
    origin = (7, 8)
    times[origin] = 0
    # Main cracks out from the origin, then branches off them.
    trunk = []
    for angle, length in [(-2.6, 12), (-1.4, 9), (-0.3, 12), (1.0, 11), (2.3, 10)]:
        walk(rng, origin, angle, length, 0, times, trunk)
    branches = []
    # Branches from the outer part of each crack, where it has room.
    outer = [line for line in trunk if line[1] >= 3]
    for (point, t, angle) in rng.sample(outer, 9):
        side = rng.choice([-1, 1])
        walk(rng, point, angle + side * rng.uniform(0.8, 1.3), rng.randint(3, 6), t + 2, times, branches)
    for (point, t, angle) in rng.sample(branches, 4):
        side = rng.choice([-1, 1])
        walk(rng, point, angle + side * rng.uniform(0.7, 1.2), rng.randint(2, 3), t + 1, times, [])
    return times


def stages(times):
    """The ten stages: each a map from pixel to gray level."""
    last = max(times.values())
    # Stage N reaches this far: a short crack at first, all of it at 9.
    reach = [round(1 + (last - 1) * (n / (STAGES - 1)) ** 0.9) for n in range(STAGES)]
    rng = random.Random(SEED + 1)
    chips = {}
    out = []
    for n in range(STAGES):
        pixels = {}
        for (p, t) in times.items():
            if t > reach[n]:
                continue
            # Stages since this pixel appeared.
            age = n - next(i for i in range(STAGES) if t <= reach[i])
            pixels[p] = 3 if age == 0 else 2 if age == 1 else 1
        if n >= 5:
            # A few chips flake off beside the oldest lines.
            old = [p for p, level in sorted(pixels.items()) if level == 1]
            for _ in range(2):
                x, y = rng.choice(old)
                dx, dy = rng.choice([(1, 0), (-1, 0), (0, 1), (0, -1)])
                q = (x + dx, y + dy)
                if 0 <= q[0] < SIZE and 0 <= q[1] < SIZE and q not in pixels:
                    chips[q] = 5
        for q, level in chips.items():
            pixels.setdefault(q, level)
        out.append(pixels)
    return out


def rows_of(pixels):
    return [
        [
            (GRAY[pixels[(x, y)]],) * 3 + (255,) if (x, y) in pixels else (0, 0, 0, 0)
            for x in range(SIZE)
        ]
        for y in range(SIZE)
    ]


def preview(path, all_stages, scale=8, gap=2):
    bg = 150
    width = STAGES * SIZE + (STAGES + 1) * gap
    height = SIZE + 2 * gap
    canvas = [[(bg, bg, bg, 255) for _ in range(width)] for _ in range(height)]
    for n, pixels in enumerate(all_stages):
        ox = gap + n * (SIZE + gap)
        for (x, y), level in pixels.items():
            v = bg * GRAY[level] // 255
            canvas[gap + y][ox + x] = (v, v, v, 255)
    big = [[px for px in row for _ in range(scale)] for row in canvas for _ in range(scale)]
    write_png(path, big)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("out_dir")
    parser.add_argument("--preview")
    args = parser.parse_args()
    all_stages = stages(network())
    os.makedirs(args.out_dir, exist_ok=True)
    for n, pixels in enumerate(all_stages):
        write_png(os.path.join(args.out_dir, f"stage_{n}.png"), rows_of(pixels))
        print(f"stage_{n}.png: {len(pixels)} pixels")
    if args.preview:
        preview(args.preview, all_stages)


if __name__ == "__main__":
    main()
