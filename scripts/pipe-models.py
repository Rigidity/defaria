#!/usr/bin/env python3
"""Write the pipe's models (pipes and tubes share them), and print the
parts that turn them.

A pipe is a square rod, SIZE pixels across, through the middle of its
block: a cube in the middle and an arm out to each side it's joined on, as
thick as the cube, so it doesn't bulge where they meet.

- `pipe_core`: nothing drawn, the middle cube's boxes (collision and the
  crosshair's).
- `pipe_cap`: one face of the middle cube, its north one, with `side`, the
  band turned to run east to west. Drawn on each side the pipe isn't
  joined on, turned so the band runs along an arm that passes it, so a
  straight pipe reads as one rod. Where arms pass it on two neighbouring
  edges, three or four, the face is `pipe_cap_bend`, `pipe_cap_tee` or
  `pipe_cap_cross`, showing `bend`, `tee` or `cross` at its own position:
  the band turning to each of those edges (the tiles have it turning to
  their top and right, top, bottom and right, and every edge, and the
  parts turn the face to match). With no arm passing it, the face is the
  pipe's end, `pipe_cap_end`, with `end`.
- `pipe_arm`: from the middle cube's north face out to the block's north
  side. Its sides show `side` turned so the band runs along it, its end
  `end`, hidden when the neighbour covers that side.

The middle cube has no face where an arm leaves it, so a tube's glass
shows nothing inside it. Every `side` face shows the tile with u along
the rod (its position along it, so the tile's left and right columns are
where it meets the next block) and v across it, the higher side first.
Turning a model keeps its uvs, so that holds on every side.

Usage:
    python3 scripts/pipe-models.py      # writes the models, prints the parts

Writes into assets/mods/defaria/models/. Standard library only.
"""

import itertools
import os
import sys

SIZE = 6  # across the rod, in pixels
LO, HI = 8 - SIZE // 2, 8 + SIZE // 2

SIDES = {
    "up": (0, 1, 0),
    "down": (0, -1, 0),
    "north": (0, 0, -1),
    "south": (0, 0, 1),
    "east": (1, 0, 0),
    "west": (-1, 0, 0),
}
OPPOSITE = {"up": "down", "down": "up", "north": "south", "south": "north", "east": "west", "west": "east"}
ORDER = ["north", "south", "east", "west", "up", "down"]


def px(v):
    s = f"{v / 16:.4f}".rstrip("0").rstrip(".")
    return s if s not in ("", "-0") else "0"


def face(side, lo, hi, texture, uv, cull=None):
    """The face of the box from `lo` to `hi` (pixels) on `side`, its
    vertices counter-clockwise seen from the front, each with `uv(p)`."""
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


def along(axis, face_axis):
    """`side`'s uv on a face whose normal is on `face_axis`, for a rod
    along `axis`: u along the rod, v across it, the higher side first."""
    across = next(i for i in range(3) if i not in (axis, face_axis))
    return lambda p: [p[axis] / 16, 1 - p[across] / 16]


def end_uv(side):
    """`end`'s uv on a face looking `side`: the tile at its own position,
    as a full block's face would show it (models/cube.json5)."""
    ua, uf, va, vf = {
        "up": (0, False, 2, False),
        "down": (0, False, 2, True),
        "north": (0, True, 1, True),
        "south": (0, False, 1, True),
        "east": (2, True, 1, True),
        "west": (2, False, 1, True),
    }[side]

    def uv(p):
        u, v = p[ua] / 16, p[va] / 16
        return [1 - u if uf else u, 1 - v if vf else v]

    return uv


def box(lo, hi):
    return f"{{ from: [{', '.join(px(c) for c in lo)}], to: [{', '.join(px(c) for c in hi)}] }}"


def model(comment, slots, faces, collision=None):
    lines = [f"// {line}" if line else "//" for line in comment]
    out = "\n".join(lines) + "\n// Written by scripts/pipe-models.py; change it there.\n{\n"
    out += "  slots: { " + ", ".join(f"{s}: null" for s in slots) + " },\n" if slots else "  slots: {},\n"
    out += "  occludes: [],\n"
    if collision is not None:
        out += f"  collision: [{', '.join(box(*b) for b in collision)}],\n"
    if faces:
        out += "  faces: [\n" + "".join(f + ",\n" for f in faces) + "  ],\n"
    else:
        out += "  faces: [],\n"
    return out + "}\n"


X, Y, Z = 0, 1, 2


def core():
    return model(
        [
            f"The middle of a pipe or tube: nothing drawn, the box of the {SIZE}-pixel",
            "cube in the middle of the block, to bump into and aim at. Its faces",
            "are `pipe_cap`'s, on the sides it isn't joined on.",
        ],
        [],
        [],
        [((LO, LO, LO), (HI, HI, HI))],
    )


# The tile's edges on a north face showing it at its own position: its top
# is up, its right west (seen from the north, west is on the right).
TILE_TOP, TILE_RIGHT = "up", "west"


def cap(kind):
    lo, hi = (LO, LO, LO), (HI, HI, HI)
    if kind == "side":
        f = face("north", lo, hi, "side", along(X, Z))
        comment = [
            "The north face of a pipe's middle cube, the band (`side`) running",
            "east to west, as on an arm running that way, so an arm passing",
            "it reads as one rod.",
        ]
    else:
        f = face("north", lo, hi, kind, end_uv("north"))
        comment = {
            "end": ["The north face of a pipe's middle cube, where no arm passes it:", "the pipe's end (`end`), at its own position."],
            "bend": ["The north face of a pipe's middle cube where it bends: `bend`, at", "its own position, the band turning from its top edge (up) to its", "right (west)."],
            "tee": ["The north face of a pipe's middle cube at a tee: `tee`, at its own", "position, the band running top to bottom (up and down) with a", "branch to the right (west)."],
            "cross": ["The north face of a pipe's middle cube at a cross: `cross`, at its", "own position, the band running to every edge."],
        }[kind]
    comment.append("Its box is `pipe_core`'s.")
    return model(comment, [kind], [f], [])


def arm():
    lo, hi = (LO, LO, 0), (HI, HI, LO)
    faces = [face(s, lo, hi, "side", along(Z, X if s in ("east", "west") else Y)) for s in ("east", "west", "up", "down")]
    faces.append(face("north", lo, hi, "end", end_uv("north"), cull="north"))
    return model(
        [
            f"A pipe's arm to the north side of its block: {SIZE} pixels across, from",
            "the middle cube (`pipe_core`) out to the north side. Its sides show",
            "`side`, the band running along it; its end shows `end` and is",
            "hidden when the neighbour covers that side. No face against the",
            "middle cube.",
        ],
        ["side", "end"],
        faces,
    )


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


def part(when, name, turn):
    rot = "" if turn == [0, 0, 0] else f", rotate: {turn}"
    pairs = ", ".join(f'{k}: "{v}"' for k, v in when.items())
    w = f"when: {{ {pairs} }}, " if when else ""
    return f"    {{ {w}model: \"{name}\"{rot} }},"


def cap_part(side, passing, arms):
    """The face of the middle cube on `side`, not joined there, with arms
    on `arms` of the four sides `passing` it."""
    when = {side: "false", **{d: "true" if d in arms else "false" for d in passing}}
    if not arms:
        return part(when, "pipe_cap_end", turn_to({"north": side}))
    if len(arms) == 1 or (len(arms) == 2 and arms[1] == OPPOSITE[arms[0]]):
        return part(when, "pipe_cap", turn_to({"north": side, "east": arms[0]}))
    if len(arms) == 2:
        a, b = arms
        try:
            turn = turn_to({"north": side, TILE_TOP: a, TILE_RIGHT: b})
        except ValueError:
            turn = turn_to({"north": side, TILE_TOP: b, TILE_RIGHT: a})
        return part(when, "pipe_cap_bend", turn)
    if len(arms) == 3:
        (missing,) = [d for d in passing if d not in arms]
        return part(when, "pipe_cap_tee", turn_to({"north": side, OPPOSITE[TILE_RIGHT]: missing}))
    return part(when, "pipe_cap_cross", turn_to({"north": side}))


def parts():
    lines = ['    { model: "pipe_core" },']
    for side in ORDER:
        lines.append(part({side: "true"}, "pipe_arm", turn_to({"north": side})))
    for side in ORDER:
        # Not joined on `side`: a face of the middle cube, showing how the
        # arms passing it run.
        passing = [d for d in ORDER if d not in (side, OPPOSITE[side])]
        for mask in range(16):
            arms = [d for i, d in enumerate(passing) if mask >> i & 1]
            lines.append(cap_part(side, passing, arms))
    return "\n".join(lines)


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    out = os.path.join(root, "assets", "mods", "defaria", "models")
    for name, text in [
        ("pipe_core", core()),
        ("pipe_cap", cap("side")),
        ("pipe_cap_end", cap("end")),
        ("pipe_cap_bend", cap("bend")),
        ("pipe_cap_tee", cap("tee")),
        ("pipe_cap_cross", cap("cross")),
        ("pipe_arm", arm()),
    ]:
        with open(os.path.join(out, f"{name}.json5"), "w") as f:
            f.write(text)
    sys.stdout.write(parts() + "\n")


if __name__ == "__main__":
    main()
