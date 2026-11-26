#!/usr/bin/env python3
"""Split the generated plant sprites into layer images, one per part.

The flowers, fern and mushroom were generated in color by Retro Diffusion
(see assets/mods/defaria/images/SOURCES.md). Each sprite is split by hue
into its parts (a stem with its leaves, a bloom, a daisy's center, a
mushroom's cap and stalk), so each part takes its own palette and one
shape gives flowers of several colors. Each part's luminance is then
stretched to its own range of gray levels (green parts to 2..6, like
tuft.png, so they sit in with the grass tufts) and rounded to the 8
levels. Fixes by hand, in code:

- every plant stands on the bottom row, as tuft.png does;
- the tulip's bloom floated a pixel above its stem: a stem pixel joins
  them;
- the mushroom stood on a patch of grass: the grass is cut away, and
  the stalk's foot filled in where the grass covered it.

Writes into OUT_DIR: tulip_stem, tulip_bloom, daisy_stem, daisy_petals,
daisy_center, bluebell_stem, bluebell_bloom, fern, mushroom_cap and
mushroom_stalk (.png).

Usage:
    python3 scripts/split-plants.py OUT_DIR [RAW_DIR]

RAW_DIR holds the untouched downloads: tulip.png, daisy.png,
bluebell.png, fern.png and mushroom.png; by default
scripts/split-plants-raw/, where they're kept. Standard library only.
Deterministic.
"""

import argparse
import colorsys
import importlib.util
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ beside the scripts
HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("draw_machines", os.path.join(HERE, "draw-machines.py"))
_machines = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_machines)
read_png, save_levels = _machines.read_png, _machines.save_levels

SIZE = 16


def lum(r, g, b):
    return 0.299 * r + 0.587 * g + 0.114 * b


def hsv(pixel):
    r, g, b, _ = pixel
    h, s, v = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
    return h * 360, s, v


def split(rows, part_of):
    """{part: {(x, y): luminance}}, each opaque pixel in the part
    `part_of(x, y, hue, saturation)` names (None drops it)."""
    parts = {}
    for y, row in enumerate(rows):
        for x, pixel in enumerate(row):
            if pixel[3] == 0:
                continue
            h, s, _ = hsv(pixel)
            part = part_of(x, y, h, s)
            if part is not None:
                parts.setdefault(part, {})[(x, y)] = lum(*pixel[:3])
    return parts


def levels(pixels, low, high):
    """Stretches the part's luminance to levels low..high and rounds."""
    darkest, brightest = min(pixels.values()), max(pixels.values())
    span = max(1e-6, brightest - darkest)
    return {
        pos: round(low + (value - darkest) / span * (high - low))
        for pos, value in pixels.items()
    }


def grid(pixels, drop=0):
    """A layer from {(x, y): level}, moved down `drop` rows."""
    out = [[None] * SIZE for _ in range(SIZE)]
    for (x, y), level in pixels.items():
        out[y + drop][x] = level
    return out


def bottom(*parts):
    """How far down the parts must move to stand on the bottom row."""
    return SIZE - 1 - max(y for part in parts for (_, y) in part)


GREEN = (2, 6)
BLOOM = (1, 6)


def tulip(rows):
    parts = split(rows, lambda x, y, h, s: "bloom" if h > 300 or h < 20 else "stem")
    stem, bloom = levels(parts["stem"], *GREEN), levels(parts["bloom"], *BLOOM)
    # The bloom floated: a stem pixel joins it to the stem's top, at (8, 7).
    stem[(7, 7)] = 3
    drop = bottom(stem, bloom)
    return {"tulip_stem": grid(stem, drop), "tulip_bloom": grid(bloom, drop)}


def daisy(rows):
    def part_of(x, y, h, s):
        if s < 0.25:
            return "petals"
        if 20 <= h <= 60:
            return "center"
        return "stem"

    parts = split(rows, part_of)
    stem = levels(parts["stem"], *GREEN)
    petals = levels(parts["petals"], 2, 7)
    center = levels(parts["center"], 2, 6)
    drop = bottom(stem, petals, center)
    return {
        "daisy_stem": grid(stem, drop),
        "daisy_petals": grid(petals, drop),
        "daisy_center": grid(center, drop),
    }


def bluebell(rows):
    parts = split(rows, lambda x, y, h, s: "bloom" if 200 <= h <= 280 else "stem")
    stem, bloom = levels(parts["stem"], *GREEN), levels(parts["bloom"], *BLOOM)
    drop = bottom(stem, bloom)
    return {"bluebell_stem": grid(stem, drop), "bluebell_bloom": grid(bloom, drop)}


def fern(rows):
    parts = split(rows, lambda x, y, h, s: "fern")
    fern = levels(parts["fern"], *GREEN)
    return {"fern": grid(fern, bottom(fern))}


def mushroom(rows):
    def part_of(x, y, h, s):
        if 60 <= h <= 180:
            return None  # the grass it stood on
        return "cap" if y <= 7 else "stalk"

    parts = split(rows, part_of)
    cap, stalk = levels(parts["cap"], *BLOOM), levels(parts["stalk"], 2, 6)
    # The grass covered the left of the stalk's foot.
    stalk[(6, 12)] = stalk[(6, 11)]
    drop = bottom(cap, stalk)
    return {"mushroom_cap": grid(cap, drop), "mushroom_stalk": grid(stalk, drop)}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out_dir")
    parser.add_argument("raw_dir", nargs="?", default=os.path.join(HERE, "split-plants-raw"))
    args = parser.parse_args()
    out, raw = args.out_dir, args.raw_dir
    os.makedirs(out, exist_ok=True)
    layers = {}
    for name, split_sprite in [
        ("tulip", tulip),
        ("daisy", daisy),
        ("bluebell", bluebell),
        ("fern", fern),
        ("mushroom", mushroom),
    ]:
        layers.update(split_sprite(read_png(os.path.join(raw, name + ".png"))))
    for name, layer in layers.items():
        save_levels(os.path.join(out, name + ".png"), layer)
    print(f"wrote {len(layers)} layers to {out}")


if __name__ == "__main__":
    main()
