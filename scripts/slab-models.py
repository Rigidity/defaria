#!/usr/bin/env python3
"""Writes the six slab models, assets/mods/defaria/models/slab_{half}.json5.

Each face gets the UVs a full cube's face has at the same place, so halves
side by side or in one voxel line up. Run: scripts/slab-models.py
"""
import sys
out = sys.argv[1] if len(sys.argv) > 1 else "assets/mods/defaria/models"
H = 0.5
halves = {
  "bottom": ((0,0,0),(1,H,1),"down","The bottom half of a block.",""),
  "top":    ((0,H,0),(1,1,1),"up","The top half of a block.",""),
  "north":  ((0,0,0),(1,1,H),"north","The north half of a block, against its north side.",""),
  "south":  ((0,0,H),(1,1,1),"south","The south half of a block, against its south side.",""),
  "east":   ((H,0,0),(1,1,1),"east","The east half of a block, against its east side.",""),
  "west":   ((0,0,0),(H,1,1),"west","The west half of a block, against its west side.",""),
}
def n(v):
    return str(int(v)) if v == int(v) else str(v)
def faces(lo, hi):
    x0,y0,z0 = lo; x1,y1,z1 = hi
    # (slot, side, plane test, vertices) using the cube's corner order and UV mapping.
    fs = []
    def uv_e(p): return (1-p[2], 1-p[1])
    def uv_w(p): return (p[2], 1-p[1])
    def uv_u(p): return (p[0], p[2])
    def uv_d(p): return (p[0], 1-p[2])
    def uv_s(p): return (p[0], 1-p[1])
    def uv_n(p): return (1-p[0], 1-p[1])
    fs.append(("east", x1==1, [(x1,y0,z1),(x1,y0,z0),(x1,y1,z0),(x1,y1,z1)], uv_e))
    fs.append(("west", x0==0, [(x0,y0,z0),(x0,y0,z1),(x0,y1,z1),(x0,y1,z0)], uv_w))
    fs.append(("up", y1==1, [(x0,y1,z1),(x1,y1,z1),(x1,y1,z0),(x0,y1,z0)], uv_u))
    fs.append(("down", y0==0, [(x0,y0,z0),(x1,y0,z0),(x1,y0,z1),(x0,y0,z1)], uv_d))
    fs.append(("south", z1==1, [(x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1)], uv_s))
    fs.append(("north", z0==0, [(x1,y0,z0),(x0,y0,z0),(x0,y1,z0),(x1,y1,z0)], uv_n))
    return fs
for name,(lo,hi,side,what,extra) in halves.items():
    lines = []
    lines.append(f"// {what}")
    lines.append("// One of six, one per half (`slab_{half}`),")
    lines.append("// written by scripts/slab-models.py. Its faces show the part of their")
    lines.append("// texture a full block's would there, so halves side by side, or two in")
    lines.append("// one voxel, look like one block. The face inside the block has no")
    lines.append(f"// `cull` and is lit by the slab's own voxel. Only the {side} side is")
    lines.append("// covered completely, so light passes through every other side.")
    lines.append("{")
    lines.append("  slots: {")
    for k,v in [("all","null"),("side",'"all"'),("end",'"all"'),("up",'"end"'),("down",'"end"'),("north",'"side"'),("south",'"side"'),("east",'"side"'),("west",'"side"')]:
        lines.append(f"    {k}: {v},")
    lines.append("  },")
    lines.append(f'  occludes: ["{side}"],')
    lines.append(f"  // The box around the faces: [{', '.join(map(n,lo))}] to [{', '.join(map(n,hi))}].")
    lines.append("  faces: [")
    for slot, boundary, verts, uv in faces(lo, hi):
        cull = f', cull: "{slot}"' if boundary else ""
        lines.append("    {")
        lines.append(f'      texture: "{slot}"{cull},')
        lines.append("      vertices: [")
        for p in verts:
            u,v = uv(p)
            lines.append(f"        {{ pos: [{', '.join(map(n,p))}], uv: [{n(u)}, {n(v)}] }},")
        lines.append("      ],")
        lines.append("    },")
    lines.append("  ],")
    lines.append("}")
    open(f"{out}/slab_{name}.json5","w").write("\n".join(lines)+"\n")
