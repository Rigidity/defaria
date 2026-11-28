"""Shared by the scripts that write conduits' models and parts
(pipe-models.py, wire-models.py): directions, quarter turns as the engine
makes them, faces and boxes in the model format, and writing the models
and a block's `parts` list in place.

Positions are in pixels (sixteenths of a block) until they're written.
Standard library only.
"""

import itertools
import os
import sys

SIDES = {
    "up": (0, 1, 0),
    "down": (0, -1, 0),
    "north": (0, 0, -1),
    "south": (0, 0, 1),
    "east": (1, 0, 0),
    "west": (-1, 0, 0),
}
OPPOSITE = {"up": "down", "down": "up", "north": "south", "south": "north", "east": "west", "west": "east"}
X, Y, Z = 0, 1, 2

# A full block's face on each side shows the tile at its own position:
# (u axis, u flipped, v axis, v flipped), as models/cube.json5.
BLOCK_UV = {
    "up": (0, False, 2, False),
    "down": (0, False, 2, True),
    "north": (0, True, 1, True),
    "south": (0, False, 1, True),
    "east": (2, True, 1, True),
    "west": (2, False, 1, True),
}

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
MOD = os.path.join(ROOT, "assets", "mods", "defaria")


def px(v):
    """Pixels as a written number of blocks."""
    s = f"{v / 16:.4f}".rstrip("0").rstrip(".")
    return s if s not in ("", "-0") else "0"


def block_uv(side):
    """The uv of a point on a face looking `side`: the tile at its own
    position, as a full block's face would show it."""
    ua, uf, va, vf = BLOCK_UV[side]

    def uv(p):
        u, v = p[ua] / 16, p[va] / 16
        return [1 - u if uf else u, 1 - v if vf else v]

    return uv


def face(side, lo, hi, texture, uv, cull=None):
    """The face of the box from `lo` to `hi` on `side`, its vertices
    counter-clockwise seen from the front, each with `uv(p)`."""
    n = SIDES[side]
    axis = next(i for i in range(3) if n[i])
    at = hi[axis] if n[axis] > 0 else lo[axis]
    a, b = [i for i in range(3) if i != axis]
    corners = []
    for ca, cb in [(0, 0), (1, 0), (1, 1), (0, 1)]:
        p = [0, 0, 0]
        p[axis] = at
        p[a] = (lo[a], hi[a])[ca]
        p[b] = (lo[b], hi[b])[cb]
        corners.append(p)
    e1 = [corners[1][i] - corners[0][i] for i in range(3)]
    e2 = [corners[2][i] - corners[0][i] for i in range(3)]
    cross = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]]
    if sum(cross[i] * n[i] for i in range(3)) < 0:
        corners.reverse()
    head = f'texture: "{texture}"' + (f', cull: "{cull}"' if cull else "")
    verts = ",\n".join(
        f"        {{ pos: [{', '.join(px(c) for c in p)}], uv: [{', '.join(px(t * 16) for t in uv(p))}] }}"
        for p in corners
    )
    return f"    {{\n      {head},\n      vertices: [\n{verts},\n      ],\n    }}"


def box(lo, hi):
    return f"{{ from: [{', '.join(px(c) for c in lo)}], to: [{', '.join(px(c) for c in hi)}] }}"


def model(script, comment, slots, faces, collision=None):
    """A model file: `comment` lines, its slots, its faces and, if given,
    its collision boxes (which the crosshair picks too)."""
    out = "\n".join(f"// {line}" if line else "//" for line in comment)
    out += f"\n// Written by scripts/{script}; change it there.\n{{\n"
    out += "  slots: { " + ", ".join(f"{s}: null" for s in slots) + " },\n" if slots else "  slots: {},\n"
    out += "  occludes: [],\n"
    if collision is not None:
        out += f"  collision: [{', '.join(box(*b) for b in collision)}],\n"
    out += "  faces: [\n" + "".join(f + ",\n" for f in faces) + "  ],\n" if faces else "  faces: [],\n"
    return out + "}\n"


# Quarter turns, as `content::bake::model::rotation`: about X, then Y, then Z.
def rotation(deg):
    def rx(v):
        return (v[0], -v[2], v[1])

    def ry(v):
        return (v[2], v[1], -v[0])

    def rz(v):
        return (-v[1], v[0], v[2])

    def apply(v):
        for turn, d in zip((rx, ry, rz), deg):
            for _ in range(d // 90):
                v = turn(v)
        return v

    return apply


ROTATIONS = sorted(
    itertools.product((0, 90, 180, 270), repeat=3),
    key=lambda d: (sum(1 for x in d if x), sum(d), d),
)


def turn_to(want):
    """The simplest rotation taking each direction in `want` to its target."""
    for deg in ROTATIONS:
        r = rotation(deg)
        if all(r(SIDES[a]) == SIDES[b] for a, b in want.items()):
            return list(deg)
    raise ValueError(want)


def part(when, name, turn, next=None):
    """A line of a block's `parts`. `when` is written as given (a dict of
    strings, or a written object)."""
    if isinstance(when, dict):
        when = "{ " + ", ".join(f'{k}: "{v}"' for k, v in when.items()) + " }" if when else None
    w = f"when: {when}, " if when else ""
    rot = "" if turn == [0, 0, 0] else f", rotate: {turn}"
    nxt = f", next: {next}" if next else ""
    return f"    {{ {w}model: \"{name}\"{rot}{nxt} }},"


def with_parts(text, parts, path):
    """`text`, a block's file, with its `parts` list replaced by `parts`
    (the lines between `  parts: [` and the `  ],` before `render`)."""
    start, end = "  parts: [\n", "  ],\n  render"
    a, b = text.find(start), text.find(end)
    if a < 0 or b < a:
        sys.exit(f"{path}: no `parts: [` ... `],` before `render` to write into")
    return text[: a + len(start)] + parts + "\n" + text[b:]


def write(models, blocks, check):
    """Writes each model (name: text) into the mod's models/ and each
    block's `parts` (block name: lines) into its file. With `check`, writes
    nothing and fails if any file differs from what it would write."""
    files = {os.path.join(MOD, "models", f"{name}.json5"): text for name, text in models.items()}
    for name, parts in blocks.items():
        path = os.path.join(MOD, "blocks", f"{name}.json5")
        with open(path) as f:
            files[path] = with_parts(f.read(), parts, path)
    stale = []
    for path, text in files.items():
        current = open(path).read() if os.path.exists(path) else None
        if current == text:
            continue
        if check:
            stale.append(os.path.relpath(path, ROOT))
        else:
            with open(path, "w") as f:
                f.write(text)
    if stale:
        sys.exit("out of date, re-run the script:\n  " + "\n  ".join(stale))
