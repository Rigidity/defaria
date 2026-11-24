#!/usr/bin/env python3
"""Turn a 16x16 image into a block texture layer: 8-level grayscale.

Converts each pixel to gray by luminance, then stretches the image so its
darkest pixel lands on level LOW and its brightest on level HIGH (1 and 6 by
default), spreading the rest linearly and rounding to the nearest of the 8
levels the game accepts (see `gray()` in src/block_texture/layer.rs).
Fully transparent pixels stay transparent. This is the house processing
step for Retro Diffusion output; see .claude/skills/retro-diffusion/SKILL.md.

--soften blurs the gray values slightly before the stretch (each pixel
weighted 4 to 1 against each of its 4 neighbours, wrapping at the edges so
the layer still tiles). That merges single-pixel speckle into larger soft
patches; pair it with a narrower range such as --low 2 --high 5 for a calm
texture. It throws away detail; prefer a lower-contrast palette in code.

--even makes every row and every column average the same brightness. A
block texture repeats on every block, so a row or column that is lighter or
darker than the rest turns into a line along every block edge and outlines
the squares. Evening them out removes those lines without touching the
detail inside them. It runs on the gray values before quantizing, then
nudges a few pixels by one level where rounding brought a stripe back.

Only for opaque images.

Usage:
    python3 scripts/gray-layer.py IN.png OUT.png [--low 1] [--high 6]
                                  [--soften] [--even] [--preview PREVIEW.png]

--preview writes the layer tiled 3x3 and upscaled 8x, to check for seams.
Prints how many pixels use each level. Standard library only.
"""

import argparse
import struct
import zlib

LEVELS = 8
GRAY = [round(i * 255 / (LEVELS - 1)) for i in range(LEVELS)]
SIZE = 16


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos, idat = 8, b""
    while pos < len(data):
        (n,) = struct.unpack(">I", data[pos : pos + 4])
        kind, body = data[pos + 4 : pos + 8], data[pos + 8 : pos + 8 + n]
        pos += 12 + n
        if kind == b"IHDR":
            width, height, depth, ctype, _, _, interlace = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
    if depth != 8 or interlace or ctype not in (0, 2, 4, 6):
        raise SystemExit(f"{path}: only 8-bit non-interlaced gray/RGB(A) PNGs are supported")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}[ctype]
    raw, stride = zlib.decompress(idat), width * channels
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(height):
        kind, line = raw[i], bytearray(raw[i + 1 : i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - channels] if x >= channels else 0
            b = prev[x]
            c = prev[x - channels] if x >= channels else 0
            if kind == 1:
                line[x] = (line[x] + a) & 255
            elif kind == 2:
                line[x] = (line[x] + b) & 255
            elif kind == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        prev = line
        row = []
        for x in range(width):
            px = tuple(line[x * channels : (x + 1) * channels])
            if channels == 1:
                px = px * 3 + (255,)
            elif channels == 2:
                px = (px[0],) * 3 + (px[1],)
            elif channels == 3:
                px = px + (255,)
            row.append(px)
        rows.append(row)
    return rows


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


def means(grid):
    """Each row's and each column's mean, minus the overall mean."""
    total = sum(map(sum, grid)) / SIZE / SIZE
    rows = [sum(row) / SIZE - total for row in grid]
    cols = [sum(grid[y][x] for y in range(SIZE)) / SIZE - total for x in range(SIZE)]
    return rows, cols


def even_rows_and_columns(lum, rounds=4):
    """Shifts each row and column so they all have the same mean."""
    for _ in range(rounds):
        rows, _ = means(lum)
        lum = [[v - rows[y] for v in row] for y, row in enumerate(lum)]
        _, cols = means(lum)
        lum = [[v - cols[x] for x, v in enumerate(row)] for row in lum]
    return lum


def restore_even(levels, exact, low, high, passes=30):
    """Rounding to levels brings back some row/column error. Moves single
    pixels one level at a time, where it helps most and strays least from
    the unrounded value, until every row and column is within 0.1 levels."""
    levels = [row[:] for row in levels]
    for _ in range(passes):
        rows, cols = means(levels)
        best = None
        for y in range(SIZE):
            for x in range(SIZE):
                error = rows[y] + cols[x]
                if abs(error) < 0.1:
                    continue
                level = levels[y][x] + (-1 if error > 0 else 1)
                if not low <= level <= high or abs(level - exact[y][x]) > 1.0:
                    continue
                score = abs(error) - abs(level - exact[y][x]) * 0.5
                if best is None or score > best[0]:
                    best = (score, y, x, level)
        if best is None:
            break
        _, y, x, level = best
        levels[y][x] = level
    return levels


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("input")
    parser.add_argument("output")
    parser.add_argument("--low", type=int, default=1)
    parser.add_argument("--high", type=int, default=6)
    parser.add_argument("--soften", action="store_true")
    parser.add_argument("--even", action="store_true")
    parser.add_argument("--preview")
    args = parser.parse_args()
    if not 0 <= args.low < args.high < LEVELS:
        raise SystemExit(f"need 0 <= low < high < {LEVELS}")

    image = read_png(args.input)
    if (len(image[0]), len(image)) != (SIZE, SIZE):
        raise SystemExit(f"{args.input} is {len(image[0])}x{len(image)}, expected {SIZE}x{SIZE}")

    lum = [[0.2126 * r + 0.7152 * g + 0.0722 * b if a else None for r, g, b, a in row] for row in image]
    if (args.soften or args.even) and any(v is None for row in lum for v in row):
        raise SystemExit("--soften and --even need a fully opaque image")
    if args.soften:
        lum = [
            [
                (4 * lum[y][x] + lum[y - 1][x] + lum[(y + 1) % SIZE][x] + lum[y][x - 1] + lum[y][(x + 1) % SIZE]) / 8
                for x in range(SIZE)
            ]
            for y in range(SIZE)
        ]
    if args.even:
        lum = even_rows_and_columns(lum)
    values = [v for row in lum for v in row if v is not None]
    lo, hi = min(values), max(values)
    span = (hi - lo) or 1

    exact = [
        [None if v is None else args.low + (v - lo) / span * (args.high - args.low) for v in row]
        for row in lum
    ]
    levels = [[None if v is None else round(v) for v in row] for row in exact]
    if args.even:
        levels = restore_even(levels, exact, args.low, args.high)

    counts = [0] * LEVELS
    out = []
    for row in levels:
        line = []
        for level in row:
            if level is None:
                line.append((0, 0, 0, 0))
                continue
            counts[level] += 1
            line.append((GRAY[level],) * 3 + (255,))
        out.append(line)
    write_png(args.output, out)
    print("pixels per level:", {level: n for level, n in enumerate(counts) if n})

    if args.preview:
        scale, tiles = 8, 3
        big = [[px for px in row * tiles for _ in range(scale)] for row in out * tiles for _ in range(scale)]
        write_png(args.preview, big)


if __name__ == "__main__":
    main()
