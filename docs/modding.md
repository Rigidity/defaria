# Modding

Everything you can see in the world (blocks, items, the models and textures
they're drawn with, and the colors those textures use) is defined in
[JSON5](https://json5.org) files, grouped into mods. The base game is a mod
too: `assets/mods/defaria/` is the best example of every file kind below.

Mods load once, at startup. A broken file is logged as an error naming the
file (and the line, for syntax errors) and left out, along with anything
that refers to it; everything else still loads. Unknown fields are errors,
so a misspelt key never fails silently.

## A mod folder

```text
assets/mods/<folder>/
  mod.json5          the manifest
  images/            16×16 grayscale PNGs
  materials/         palettes
  textures/          images and materials, layered into tiles
  models/            mesh data with texture slots
  blocks/            blocks and their states
  items/             items (blocks get one automatically)
```

Every folder except the manifest is optional, and each may have
subfolders.

## Ids and references

A definition's id is its mod's id, a colon, and its path in its kind's
folder without the extension. In mod `defaria`, `blocks/stone.json5` is block
`defaria:stone`, and `blocks/ores/iron.json5` is block `defaria:ores/iron`.
Each kind has its own ids, so block `defaria:stone`, texture
`defaria:stone` and material `defaria:stone` are three different things.

Names are lowercase letters, digits, `_`, `-` and `.`, with `/` between
folders. Mod ids are lowercase letters, digits and `_`.

Inside a file, a reference without a colon means the same mod: in mod
`defaria`, `"stone"` means `"defaria:stone"`. To use another mod's
definition, write its full id, `"othermod:stone"`, and list that mod as a
dependency.

## `mod.json5`

```json5
{
  id: "moreores",                 // required: the namespace of everything here
  name: "More Ores",              // required
  version: "1.2.0",
  description: "Adds copper and tin.",
  authors: ["Someone"],
  dependencies: ["defaria"],      // these load first
}
```

Mods load after their dependencies, otherwise in order of id. A mod whose
dependency is missing, broken or circular isn't loaded. Two mods may not
share an id. The folder's name doesn't matter.

## Images

`images/<name>.png`, referenced as `<name>` (or `<mod>:<name>`). An image
is 16×16 and grayscale: it may use only the 8 gray levels
`0, 36, 73, 109, 146, 182, 219, 255`, plus transparency. Each level is an
index into a palette, so an image is shape and shading only, and one image
can be stone in one texture and copper in another.
`scripts/gray-layer.py` turns any 16×16 image into one.

Images load through the asset server, so in dev builds editing one updates
the game while it runs. An image that's missing, won't load, is the wrong
size or uses other colors is logged, and the tiles that use it show a
magenta and black checkerboard until it's fixed.

## Materials

`materials/<name>.json5`: a palette, which colors a grayscale image.

```json5
{
  palette: ["#33353d", "#52535a", "#6f6f74", "#95938f"],
}
```

`palette` is 2 to 8 hex colors, darkest first. They're spread evenly over
the 8 gray levels and blended in between (in Oklab, so the steps look
even). With exactly 8 colors, each level gets its own color as given. Few
steps between the darkest and lightest colors give a calm texture; many
give a busy one.

## Textures

`textures/<name>.json5`: one 16×16 tile, made by coloring images and
stacking them, bottom layer first.

```json5
// Grass hanging over the edge of a dirt side face.
{
  layers: [
    { image: "dirt", material: "dirt" },
    { image: "grass_side", material: "grass" },
  ],
}
```

Each layer has an `image` and either a `material` or a `palette` of its own
(the same format as a material's). Transparent pixels show the layers
below. Block faces should end up fully opaque; item icons may be
transparent.

## Models

`models/<name>.json5`: the vertex data a block (and later an entity) is
drawn with. Faces don't name textures; they name slots, which the block
fills.

```json5
{
  // Each slot, and the slot it falls back to when the block leaves it
  // empty (or null). Here a block can fill `all`, or `side` and `end`, or
  // single faces.
  slots: {
    all: null,
    side: "all",
    end: "all",
    up: "end",
    down: "end",
    north: "side", south: "side", east: "side", west: "side",
  },
  // Sides of the block this model covers completely. A neighbour's face
  // against one of these is hidden.
  occludes: ["up", "down", "north", "south", "east", "west"],
  faces: [
    {
      texture: "up",       // the slot whose texture this face shows
      cull: "up",          // hidden when the neighbour above covers its bottom
      vertices: [
        { pos: [0, 1, 1], uv: [0, 1] },
        { pos: [1, 1, 1], uv: [1, 1] },
        { pos: [1, 1, 0], uv: [1, 0] },
        { pos: [0, 1, 0], uv: [0, 0] },
      ],
    },
    // ...one per side for a cube. See models/cube.json5.
  ],
}
```

- **Positions** are in blocks: a full block runs from `[0, 0, 0]` to
  `[1, 1, 1]`. Up is +Y, south is +Z and east is +X, so north is −Z and
  west is −X.
- **UVs** are within the tile, from `[0, 0]` at its top left to `[1, 1]`
  at its bottom right.
- A face has 3 or 4 **vertices**, counter-clockwise when you look at its
  front. Only the front is drawn, and its shading comes from the direction
  it faces.
- **`cull`** is for faces that lie on the block's boundary: the face is
  hidden when the neighbour on that side `occludes` the side facing it.
  Leave it out for faces that should always draw.

## Blocks

`blocks/<name>.json5`.

```json5
{
  name: "Stone",                    // defaults to the id, e.g. "Stone"
  model: "cube",
  textures: { all: "stone" },       // slot → texture
}
```

`textures` fills the model's slots; each slot it leaves empty falls back as
the model says. A face whose slot ends up with no texture is an error.
Textures for slots a state's model doesn't have are ignored for that state,
so a block can set textures for several models and switch between them in
variants; naming a slot that none of the block's models has is an error.

### States, properties and variants

A block can have **properties**, each with a list of values; the first is
the default. Every combination of values is a separate block state, which
can look different.

```json5
// A log: `axis` is the direction the trunk runs.
{
  name: "Log",
  properties: { axis: ["y", "x", "z"] },
  model: "cube",
  textures: { side: "log_side", end: "log_top" },
  variants: [
    { when: { axis: "x" }, rotate: [0, 0, 90] },
    { when: { axis: "z" }, rotate: [90, 0, 0] },
  ],
}
```

For each state the game starts from the block's own `model`, `textures`
and `rotate`, then applies every variant whose `when` matches, in order. A
later variant's `model` and `rotate` replace earlier ones, and its
`textures` are merged in slot by slot. A `when` value may be one value or
a list of any that match: `{ when: { axis: ["x", "z"] } }`. A variant with
no `when` always applies.

- **`rotate`** turns the model about its center: `[x, y, z]` degrees, in
  multiples of 90, applied about X first, then Y, then Z. Faces, their
  `cull` sides and `occludes` all turn with it.
- **`{property}`** in a model or texture name is replaced by the state's
  value. One line can cover every color:

  ```json5
  {
    properties: { color: ["white", "red", "blue"] },
    model: "cube",
    textures: { all: "wool_{color}" },
  }
  ```

A block may have up to 4096 states.

### The block's item

Every block gets an item with the same id, which places the block in its
default state. Set `item: false` to opt out. To change the item (its name,
icon or stack size), add an item file with the same name: what it leaves
out comes from the block.

## Items

`items/<name>.json5`.

```json5
{
  name: "Pickaxe",       // defaults to the id, e.g. "Pickaxe"
  texture: "pickaxe",    // the icon
  max_stack: 1,          // default 64
  block: "stone",        // optional: the block it places
}
```

## Not yet

- JSON5 changes need a restart; images hot reload.
- Placement rules (such as setting a log's `axis` from the face you click)
  aren't data yet, so placed blocks are always in their default state.
- Item icons aren't drawn anywhere yet.
- Models are block-only so far: no bones or animation for entities.
- A mod can only add definitions in its own namespace, not replace another
  mod's.
