#!/usr/bin/env python3
"""Write the wire's models, and print the parts that turn them.

A wire lies flat on the side of its voxel it rests against (its `on`
property: the floor, a wall or the ceiling). Its models are drawn resting
on the floor and turned to the other sides by the block's parts:

- `wire_core`: the bit of wire in the middle of the floor, where its
  arms meet.
- `wire_arm`: from the core along the floor to the north side.
- `wire_riser`: from the core straight up through the middle of the block
  to the middle of the top side.
- `wire_climb`: `wire_arm`, then up the north side to the top.

Each model's boxes are the wire as drawn, to bump into and aim at. A
part reaches the side its `when` joins, which is what the wrench works on
when it's aimed at.

Bends always turn at the middle of a side a wire lies on, or go straight
through the middle of the block, so a wire never steps sideways. Where two
joined wires meet on the side between them, each ends at a point on it:
the middle of its edge toward what the wire rests on (an arm), or the
middle of the side (a riser, or the core, for the side it rests on). Wires
resting on the same side meet. Otherwise the parts drawn by their
neighbours (`next`) bend one of them onto the other's surface:

- Joined up (away from what it rests on) to a wire resting across, a
  wire climbs that wire's surface to it (`wire_climb`): a floor wire
  rides up the wall.
- Joined toward what it rests on draws nothing more: the core touches
  that side, and a wire rests only on a solid face (the block's
  `support`), so no wire is there.
- Two wires joined along what they rest on, on different surfaces: the
  one on the lesser surface (floor before ceiling before the walls,
  north, south, east, west) goes over to the other's. From a surface
  across it, it runs along its own to the corner and along the other's
  to the middle, then on as a wire there would (an arm to the side): a
  wall wire beside a floor wire comes down the wall and along the floor.
  From the opposite surface, it goes straight through the middle of the
  block (`wire_riser`) and on along the other's.

Usage:
    python3 scripts/wire-models.py            # writes the models, prints the parts

Writes into assets/mods/defaria/models/. Standard library only.
"""

import itertools
import os
import sys

# Sizes in pixels (sixteenths of a block).
WIDTH = 4  # across the wire, along the surface
THICK = 2  # off the surface
LO, HI = 8 - WIDTH // 2, 8 + WIDTH // 2
# The crosshair's boxes, a little bigger than the wire so it's easy to aim at.
PICK_WIDTH = 6
PICK_THICK = 3
PLO, PHI = 8 - PICK_WIDTH // 2, 8 + PICK_WIDTH // 2

SIDES = {
    "up": (0, 1, 0),
    "down": (0, -1, 0),
    "north": (0, 0, -1),
    "south": (0, 0, 1),
    "east": (1, 0, 0),
    "west": (-1, 0, 0),
}


def px(v):
    s = f"{v / 16:.4f}".rstrip("0").rstrip(".")
    return s if s not in ("", "-0") else "0"


X, Y, Z = 0, 1, 2


def face(side, lo, hi, texture, along, cull=None):
    """The face of the box from `lo` to `hi` (pixels) on `side`, its
    vertices counter-clockwise seen from the front, each showing the tile
    at its own position as a full block's face would, but turned a
    quarter if need be so the tile's u runs `along` the wire (an axis:
    X, Y or Z). The texture's diagonal strands then slant the same way
    on every face, seen from outside, as a twist does; turning a model
    keeps its faces' uvs, so they still do on every side it rests on."""
    n = SIDES[side]
    axis = next(i for i in range(3) if n[i])
    at = hi[axis] if n[axis] > 0 else lo[axis]
    # (u axis, u flipped, v axis, v flipped) per side, as models/cube.json5.
    uv_of = {
        "up": (0, False, 2, False),
        "down": (0, False, 2, True),
        "north": (0, True, 1, True),
        "south": (0, False, 1, True),
        "east": (2, True, 1, True),
        "west": (2, False, 1, True),
    }[side]
    ua, uf, va, vf = uv_of
    corners = []
    for a, b in [(0, 0), (1, 0), (1, 1), (0, 1)]:
        p = [0, 0, 0]
        p[axis] = at
        p[ua] = (lo[ua], hi[ua])[a ^ uf]
        p[va] = (lo[va], hi[va])[b ^ vf]
        corners.append(p)

    def uv(p):
        u = p[ua] / 16
        v = p[va] / 16
        u, v = (1 - u if uf else u), (1 - v if vf else v)
        # A quarter turn, not a mirror, so the strands keep their slant.
        return [v, 1 - u] if va == along else [u, v]

    # Counter-clockwise from the front: (b - a) x (c - a) points along n.
    a, b, c = corners[0], corners[1], corners[2]
    ab = [b[i] - a[i] for i in range(3)]
    ac = [c[i] - a[i] for i in range(3)]
    cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ]
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


def model(comment, faces, collision):
    """A model's file: its faces and its boxes (`collision`: the wire as
    drawn), which are also what the crosshair picks."""
    body = "".join(f + ",\n" for f in faces)
    boxes = ", ".join(box(*b) for b in collision)
    return (
        "\n".join(f"// {line}" if line else "//" for line in comment)
        + "\n// Written by scripts/wire-models.py; change it there.\n"
        + "{\n"
        + "  slots: { wire: null },\n"
        + "  occludes: [],\n"
        + f"  collision: [{boxes}],\n"
        + "  faces: [\n"
        + body
        + "  ],\n}\n"
    )


def core():
    lo, hi = (LO, 0, LO), (HI, THICK, HI)
    faces = [face("up", lo, hi, "wire", X)] + [
        face(s, lo, hi, "wire", X) for s in ("north", "south", "east", "west")
    ]
    faces.append(face("down", lo, hi, "wire", X, cull="down"))
    return model(
        [
            f"The middle of a wire lying on the floor: a box {WIDTH} pixels wide and {THICK}",
            "high in the middle of the bottom of the block. Turned to the side",
            "the wire rests on by the block's parts.",
        ],
        faces,
        [(lo, hi)],
    )


def arm():
    lo, hi = (LO, 0, 0), (HI, THICK, LO)
    faces = [face(s, lo, hi, "wire", Z) for s in ("up", "east", "west")]
    faces.append(face("down", lo, hi, "wire", Z, cull="down"))
    faces.append(face("north", lo, hi, "wire", Z, cull="north"))
    return model(
        [
            "A wire lying on the floor, from its core (`wire_core`) to the",
            "north side, along the floor. It has no face against the core; its",
            "end is hidden when the neighbour covers the north side.",
        ],
        faces,
        [(lo, hi)],
    )


def riser():
    lo, hi = (LO, THICK, LO), (HI, 16, HI)
    faces = [face(s, lo, hi, "wire", Y) for s in ("north", "south", "east", "west")]
    faces.append(face("up", lo, hi, "wire", Y, cull="up"))
    return model(
        [
            "A wire lying on the floor, joined up: from the top of its core",
            "(`wire_core`) straight up to the middle of the block's top side.",
            "Drawn unless the wire above rests across it (a part's `next`).",
        ],
        faces,
        [(lo, hi)],
    )


def climb():
    # `wire_arm` along the floor, then up the north side to the top.
    arm_lo, arm_hi = (LO, 0, THICK), (HI, THICK, LO)
    up_lo, up_hi = (LO, 0, 0), (HI, 16, THICK)
    faces = [face(s, arm_lo, arm_hi, "wire", Z) for s in ("up", "east", "west")]
    faces.append(face("down", arm_lo, arm_hi, "wire", Z, cull="down"))
    faces += [face(s, up_lo, up_hi, "wire", Y) for s in ("south", "east", "west")]
    faces.append(face("north", up_lo, up_hi, "wire", Y, cull="north"))
    faces.append(face("up", up_lo, up_hi, "wire", Y, cull="up"))
    faces.append(face("down", up_lo, up_hi, "wire", Y, cull="down"))
    return model(
        [
            "A wire lying on the floor, joined up to one resting on the north",
            "side: from the core along the floor to the north side, then up it",
            "to the top, where the other's arm comes down the same side. Drawn",
            "where that wire is (a part's `next`).",
        ],
        faces,
        [(arm_lo, arm_hi), (up_lo, up_hi)],
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


OPPOSITE = {"up": "down", "down": "up", "north": "south", "south": "north", "east": "west", "west": "east"}
ORDER = ["down", "up", "north", "south", "east", "west"]


def flat(sides):
    return "[" + ", ".join(f'"{s}"' for s in sides) + "]"


def where(on):
    return "the floor" if on == "down" else "the ceiling" if on == "up" else f"the {on} wall"


def part(when, model_name, turn, next=None):
    rot = "" if turn == [0, 0, 0] else f", rotate: {turn}"
    nxt = f", next: {next}" if next else ""
    return f"    {{ when: {when}, model: \"{model_name}\"{rot}{nxt} }},"


def wire_on(side, ons):
    on = f'"{ons[0]}"' if len(ons) == 1 else flat(ons)
    return f'{{ {side}: {{ block: "wire", when: {{ on: {on} }} }} }}'


def wins(b, a):
    """Whether a wire resting on `b` stays put where it meets one resting
    on `a`, which comes over to it."""
    return ORDER.index(b) < ORDER.index(a)


def parts():
    lines = []
    for on in ORDER:
        lines.append(f"    // On {where(on)}.")
        lines.append(part(f'{{ on: "{on}" }}', "wire_core", turn_to({"down": on})))
        for side in ORDER:
            when = f'{{ on: "{on}", {side}: "true" }}'
            across = [d for d in ORDER if d not in (side, OPPOSITE[side])]
            if side == OPPOSITE[on]:
                # Away from what it rests on: a riser to the middle, unless
                # the wire there rests across it, when it climbs to its edge.
                not_across = f'{{ {side}: {{ not: {{ block: "wire", when: {{ on: {flat(across)} }} }} }} }}'
                lines.append(part(when, "wire_riser", turn_to({"down": on}), not_across))
                for d in across:
                    lines.append(
                        part(when, "wire_climb", turn_to({"down": on, "north": d}), wire_on(side, [d]))
                    )
            elif side == on:
                # Toward what it rests on, a solid face (its `support`):
                # the core already touches it, and no wire is there to
                # bend to.
                continue
            else:
                # Along what it rests on: an arm to the edge, unless the
                # wire there rests on a surface that wins, when this one
                # goes over to that surface and on along it.
                turn = turn_to({"down": on, "north": side})
                over = [d for d in across if d != on and wins(d, on)]
                if over:
                    not_over = f'{{ {side}: {{ not: {{ block: "wire", when: {{ on: {flat(over)} }} }} }} }}'
                    lines.append(part(when, "wire_arm", turn, not_over))
                else:
                    lines.append(part(when, "wire_arm", turn))
                for d in over:
                    rule = wire_on(side, [d])
                    if d == OPPOSITE[on]:
                        lines.append(part(when, "wire_riser", turn_to({"down": on}), rule))
                    else:
                        lines.append(part(when, "wire_arm", turn_to({"down": on, "north": d}), rule))
                        lines.append(part(when, "wire_core", turn_to({"down": d}), rule))
                        lines.append(part(when, "wire_arm", turn_to({"down": d, "north": on}), rule))
                    lines.append(part(when, "wire_arm", turn_to({"down": d, "north": side}), rule))
    return "\n".join(lines)


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    out = os.path.join(root, "assets", "mods", "defaria", "models")
    for name, text in [
        ("wire_core", core()),
        ("wire_arm", arm()),
        ("wire_riser", riser()),
        ("wire_climb", climb()),
    ]:
        with open(os.path.join(out, f"{name}.json5"), "w") as f:
            f.write(text)
    sys.stdout.write(parts() + "\n")


if __name__ == "__main__":
    main()
