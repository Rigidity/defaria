#!/usr/bin/env python3
"""Generate the placeholder block texture atlas, assets/textures/blocks.png.

Stand-in art until real textures exist. Deterministic, standard library only.
The atlas is one row of 16x16 tiles; the order must match `Tile` in
src/voxel.rs:

    0 stone   1 dirt   2 grass top   3 grass side

Run: python3 scripts/placeholder-textures.py
"""

import random
import struct
import zlib
from pathlib import Path

TILE = 16
OUT = Path(__file__).resolve().parent.parent / "assets" / "textures" / "blocks.png"

STONE = [(112, 112, 116), (124, 124, 128), (136, 136, 140), (98, 98, 102), (84, 84, 88)]
DIRT = [(121, 85, 58), (134, 96, 67), (108, 75, 50), (148, 108, 76), (92, 63, 42)]
GRASS = [(86, 150, 58), (98, 164, 66), (76, 136, 50), (110, 176, 74), (66, 120, 44)]


def speckled(rng, palette, weights):
    return [[rng.choices(palette, weights)[0] for _ in range(TILE)] for _ in range(TILE)]


def stone(rng):
    tile = speckled(rng, STONE[:4], [5, 4, 2, 3])
    # A few short dark cracks.
    for _ in range(3):
        x, y = rng.randrange(TILE), rng.randrange(TILE)
        for _ in range(rng.randint(2, 4)):
            tile[y][x] = STONE[4]
            x = (x + rng.choice([-1, 0, 1])) % TILE
            y = (y + 1) % TILE
    return tile


def dirt(rng):
    tile = speckled(rng, DIRT[:3], [6, 3, 3])
    # Pebbles.
    for _ in range(5):
        tile[rng.randrange(TILE)][rng.randrange(TILE)] = rng.choice(DIRT[3:])
    return tile


def grass_top(rng):
    return speckled(rng, GRASS, [6, 4, 3, 2, 1])


def grass_side(rng, top):
    tile = dirt(rng)
    # Grass hangs over the top edge by a ragged 2-5 pixels.
    for x in range(TILE):
        depth = rng.randint(2, 5) if x % 3 else rng.randint(2, 3)
        for y in range(depth):
            tile[y][x] = top[y][x] if y < depth - 1 else GRASS[4]
    return tile


def write_png(path, rows):
    height, width = len(rows), len(rows[0])
    raw = b"".join(b"\x00" + bytes(c for px in row for c in px) for row in rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def main():
    rng = random.Random(0xDEFA)
    top = grass_top(rng)
    tiles = [stone(rng), dirt(rng), top, grass_side(rng, top)]
    rows = [sum((tile[y] for tile in tiles), []) for y in range(TILE)]
    write_png(OUT, rows)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
