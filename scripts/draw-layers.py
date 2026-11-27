#!/usr/bin/env python3
"""Draw the procedural layer images: glass, stained glass, water, lava, a
grass tuft, an ingot, moss, a bucket and a dead bush.

Writes 16x16 layer images (8-level grayscale with alpha, as
src/block_texture/layer.rs checks) into OUT_DIR:

- glass.png: a pane. A solid one-pixel frame, faint highlight streaks
  running diagonally, and a nearly clear middle. Partly transparent, so
  the glass blends (`render: "translucent"`).
- stained_glass.png: the same pane with a fuller middle (alpha 150), so
  its material's color shows, and the streaks brighter.
- water.png: soft ripples, wider than tall, fully solid; the water
  material's palette carries the transparency. A strip of WATER_FRAMES
  frames: two sets of ripples crossing, drifting opposite ways, so the
  surface shimmers without going anywhere, and loops.
- lava.png: dark crust plates with bright molten seams between them. A
  strip of LAVA_FRAMES frames: the plates sway on small circles and the
  seams' heat swells and ebbs, looping; the texture blends between them.
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
- dead_bush.png: bare twigs, one pixel wide, forking up and out from one
  point on the bottom edge, transparent around them, for the
  crossed-quad plant model.

Each tiles with itself where it needs to (water and lava wrap at the
edges, in every frame). Deterministic: the same seed always draws the same
images.

Usage:
    python3 scripts/draw-layers.py OUT_DIR [--preview PREVIEW.png]

--preview writes the images side by side over a mid gray, upscaled 8x,
animated ones as a column of their frames. Standard library only.
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


def glass(middle=px(5, 40), streak=px(7, 120)):
    rows = [[middle for _ in range(SIZE)] for _ in range(SIZE)]
    # Two short parallel streaks, from bottom left to top right, in the
    # upper left of the pane: (where x + y is, which x they cover).
    for diagonal, xs in ((9, range(3, 8)), (12, range(5, 9))):
        for x in xs:
            rows[diagonal - x][x] = streak
    for i in range(SIZE):
        for x, y in ((i, 0), (i, SIZE - 1), (0, i), (SIZE - 1, i)):
            rows[y][x] = px(6, 255)
    # The frame's lower and right edges a shade darker, lit from the top left.
    for i in range(1, SIZE):
        rows[SIZE - 1][i] = px(4, 255)
        rows[i][SIZE - 1] = px(4, 255)
    return rows


WATER_FRAMES = 16
LAVA_FRAMES = 16


def looping_noise(rng, cells, frames, frame_cells):
    """Smooth value noise over SIZE x SIZE x frames that wraps in all
    three, 0 to 1: at(x, y, frame)."""
    grid = [[[rng.random() for _ in range(cells)] for _ in range(cells)] for _ in range(frame_cells)]

    def smooth(t):
        return t * t * (3 - 2 * t)

    def at(x, y, frame):
        fx, fy, ft = x * cells / SIZE, y * cells / SIZE, frame * frame_cells / frames
        x0, y0, t0 = int(fx) % cells, int(fy) % cells, int(ft) % frame_cells
        x1, y1, t1 = (x0 + 1) % cells, (y0 + 1) % cells, (t0 + 1) % frame_cells
        tx, ty, tt = smooth(fx - int(fx)), smooth(fy - int(fy)), smooth(ft - int(ft))

        def plane(t):
            top = grid[t][y0][x0] * (1 - tx) + grid[t][y0][x1] * tx
            bottom = grid[t][y1][x0] * (1 - tx) + grid[t][y1][x1] * tx
            return top * (1 - ty) + bottom * ty

        return plane(t0) * (1 - tt) + plane(t1) * tt

    return at


def water(rng):
    noise = looping_noise(rng, 4, WATER_FRAMES, 4)
    frames = []
    for frame in range(WATER_FRAMES):
        phase = 2 * math.pi * frame / WATER_FRAMES
        rows = []
        for y in range(SIZE):
            row = []
            for x in range(SIZE):
                # Ripples: two sets of bands, wider than tall, crossing at
                # a slant and drifting opposite ways (a whole wavelength a
                # loop, so the frames wrap), warped by the noise.
                warp = noise(x, y, frame) * 4
                a = math.sin((y + x / 2 + warp) * 2 * math.pi / 8 + phase)
                b = math.sin((y - x / 2 + warp) * 2 * math.pi / 8 - phase)
                value = 0.5 + 0.25 * (a + b) + 0.25 * (noise((x + 7) % SIZE, (y + 3) % SIZE, frame) - 0.5)
                row.append(px(min(5, max(2, 2 + int(value * 4)))))
            rows.append(row)
        frames.append(rows)
    return frames


def lava(rng):
    # Crust plates around random centres (wrapping); the seams between two
    # plates are molten and bright. Each centre sways round a small circle,
    # once a loop, from its own starting angle.
    def dist(x, y, c):
        dx = min(abs(x - c[0]) % SIZE, SIZE - abs(x - c[0]) % SIZE)
        dy = min(abs(y - c[1]) % SIZE, SIZE - abs(y - c[1]) % SIZE)
        return math.hypot(dx, dy)

    # Centres at least 4 pixels apart: two close together would leave a
    # wide molten patch between them as they sway.
    centres = []
    while len(centres) < 6:
        c = (rng.uniform(0, SIZE), rng.uniform(0, SIZE), rng.uniform(0, 2 * math.pi))
        if all(dist(c[0], c[1], other) >= 4 for other in centres):
            centres.append(c)
    noise = looping_noise(rng, 4, LAVA_FRAMES, 2)

    frames = []
    for frame in range(LAVA_FRAMES):
        turn = 2 * math.pi * frame / LAVA_FRAMES
        moved = [(cx + 0.7 * math.cos(turn + a), cy + 0.7 * math.sin(turn + a)) for cx, cy, a in centres]
        rows = []
        for y in range(SIZE):
            row = []
            for x in range(SIZE):
                d = sorted(dist(x + 0.5, y + 0.5, c) for c in moved)
                seam = d[1] - d[0]  # 0 on the border between two plates
                heat = 0.85 * max(0.0, 1.0 - seam / 2.2) + 0.5 * noise(x, y, frame)
                row.append(px(min(7, max(1, 1 + int(heat * 6)))))
            rows.append(row)
        frames.append(rows)
    return frames


def strip(frames):
    """A list of frames as one image, the first on top."""
    return [row for frame in frames for row in frame]


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


# A dead bush's twigs: each a line through these points, from the ground
# up. Forks start on the twig they leave.
DEAD_BUSH = [
    [(7, 15), (7, 12), (5, 9), (3, 6), (2, 3)],
    [(5, 9), (6, 6), (5, 2)],
    [(3, 6), (0, 5)],
    [(8, 15), (8, 12), (10, 9), (12, 6), (14, 4)],
    [(10, 9), (9, 6), (10, 3)],
    [(12, 6), (15, 7)],
    [(8, 12), (11, 13), (14, 11)],
    [(7, 13), (4, 13), (1, 11)],
]


def line(a, b):
    """The pixels from a to b, one a step along the longer axis."""
    (x0, y0), (x1, y1) = a, b
    steps = max(abs(x1 - x0), abs(y1 - y0))
    return [
        (round(x0 + (x1 - x0) * i / steps), round(y0 + (y1 - y0) * i / steps))
        for i in range(steps + 1)
    ]


def dead_bush():
    rows = [[CLEAR for _ in range(SIZE)] for _ in range(SIZE)]
    for twig in DEAD_BUSH:
        pixels = []
        for a, b in zip(twig, twig[1:]):
            pixels += line(a, b)[(1 if pixels else 0):]
        for i, (x, y) in enumerate(pixels):
            # Darker at the ground, lighter toward the tips, with a light
            # pixel every few for the bark catching the sun.
            t = (SIZE - 1 - y) / (SIZE - 1)
            level = 2 + round(t * 2) + (1 if i % 3 == 2 else 0)
            if rows[y][x] == CLEAR:
                rows[y][x] = px(level)
    return rows


def preview(path, images):
    """Each image's frames down a column (a still image is one frame)."""
    scale, gap = 8, 8
    columns = [[image[i:i + SIZE] for i in range(0, len(image), SIZE)] for image in images]
    width = len(columns) * (SIZE * scale + gap) + gap
    tallest = max(len(frames) for frames in columns)
    height = tallest * (SIZE * scale + gap) + gap
    mid = 128
    rows = [[(mid, mid, mid, 255)] * width for _ in range(height)]
    for n, frames in enumerate(columns):
        ox = gap + n * (SIZE * scale + gap)
        for f, image in enumerate(frames):
            oy = gap + f * (SIZE * scale + gap)
            for y in range(SIZE * scale):
                for x in range(SIZE * scale):
                    r, g, b, a = image[y // scale][x // scale]
                    t = a / 255
                    c = round(r * t + mid * (1 - t))
                    rows[oy + y][ox + x] = (c, c, c, 255)
    write_png(path, rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out_dir")
    parser.add_argument("--preview")
    args = parser.parse_args()
    rng = random.Random(0xDEFA41A)
    # Water and lava draw from their own seeds. The images after them were
    # drawn when the still water and lava took 16 and 28 draws from `rng`,
    # so skip as many to keep those the same.
    for _ in range(16 + 28):
        rng.random()
    images = {
        "glass": glass(),
        "stained_glass": glass(px(4, 150), px(7, 200)),
        "water": strip(water(random.Random(0xDEFA41A + 1))),
        "lava": strip(lava(random.Random(0xDEFA41A + 2))),
        "tuft": tuft(rng),
        "items/ingot": ingot(),
        "moss": moss(rng),
        "items/bucket": bucket(),
        "dead_bush": dead_bush(),
    }
    for name, rows in images.items():
        path = os.path.join(args.out_dir, f"{name}.png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        write_png(path, rows)
    if args.preview:
        preview(args.preview, list(images.values()))


if __name__ == "__main__":
    main()
