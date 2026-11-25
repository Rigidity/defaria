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

`materials/<name>.json5`: what things are made of. A material colors
grayscale images with its palette, and blocks and items can be made of
any material with a tag: one log file gives an oak, a birch and a spruce
log, and a mod adding a wood gets its logs, planks and tool handles too.

```json5
{
  name: "Oak",                     // defaults to the id, e.g. "Oak"
  tags: ["wood"],                  // groups that properties choose from
  palette: ["#5a3e22", "#8a643a", "#a87e4c", "#be945e"],
  // Other palettes for particular uses, by role. A layer with
  // `role: "bark"` uses this one; materials without it use `palette`.
  palettes: {
    bark: ["#2e2014", "#483422", "#5e462e", "#74583a"],
  },
}
```

Each palette is 2 to 8 hex colors, darkest first. They're spread evenly over
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

A layer's `material` can be an **input**, `$name`, which whoever uses the
texture fills in. `role` picks one of the material's `palettes` (its main
one if it has none by that name):

```json5
// textures/log_side.json5: bark of any wood.
{
  layers: [{ image: "log_side", material: "$wood", role: "bark" }],
}
```

A block or item names a texture with inputs together with its materials:
`{ texture: "log_side", materials: { wood: "birch" } }`. Every input needs
a material, and only inputs can be given. Each combination in use becomes
its own tile, made once.

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
  // against one of these is hidden, and light doesn't pass through them.
  occludes: ["up", "down", "north", "south", "east", "west"],
  // The boxes the player bumps into and the crosshair picks. Optional:
  // left out, it's the box around every face (here the whole block).
  collision: [{ from: [0, 0, 0], to: [1, 1, 1] }],
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
  front. Only the front is drawn. It's lit by the light in front of it: the
  voxel past its `cull` side, or the block's own voxel if it has no `cull`.
- **`cull`** is for faces that lie on the block's boundary: the face is
  hidden when the neighbour on that side `occludes` the side facing it.
  Leave it out for faces that should always draw.
- **`collision`** is a list of boxes, each from one corner to the
  opposite one, in the same block coordinates. They must be inside the
  block and not flat. Left out, it's the one box around all the faces
  (`models/slab.json5` gets `[0, 0, 0]` to `[1, 0.5, 1]` that way), or
  nothing if the faces are all in one plane. `collision: []` makes a block
  you walk through; the crosshair still picks it, as a whole block. Boxes
  turn with the block's `rotate`.

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
the model says. A texture is its id, or its id with materials for its
inputs (see [Textures](#textures)). A face whose slot ends up with no texture is an error.
Textures for slots a state's model doesn't have are ignored for that state,
so a block can set textures for several models and switch between them in
variants; naming a slot that none of the block's models has is an error.

### Light

```json5
{
  name: "Red Lamp",
  model: "cube",
  textures: { all: "red_lamp" },
  light: [15, 3, 2],               // red, green, blue: 0 to 15 each
}
```

`light` makes the block give off light: a level from 0 to 15 for each of
red, green and blue. Each colour spreads on its own and drops one level per
block, so a level-15 colour reaches 14 blocks and colours shift as they
fade. A block that gives off light is drawn fully lit. Variants can set
`light` too; a later one replaces an earlier one's.

Light doesn't pass through a side a block covers (its model's
`occludes`), going in or out, but it passes through every other side. So a
slab, which covers only its bottom, is lit from above and from the sides,
and a roof of slabs shades what's under it like a full roof. A block that
gives off light shines out of every side, covered or not. A block whose
model covers all six sides is opaque: no light gets in, and it shades the
corners of the faces next to it (ambient occlusion).

### States, properties and variants

A block can have **properties**, each with a list of values; the first is
the default. Every combination of values is a separate block state, which
can look different. Chunks store block states, so a block carries its
property values wherever it is.

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

For each state the game starts from the block's own `model`, `textures`,
`rotate` and `light`, then applies every variant whose `when` matches, in
order. A later variant's `model`, `rotate` and `light` replace earlier
ones, and its `textures` are merged in slot by slot. A `when` value may be one value or
a list of any that match: `{ when: { axis: ["x", "z"] } }`. A variant with
no `when` always applies.

- **`rotate`** turns the model about its center: `[x, y, z]` degrees, in
  multiples of 90, applied about X first, then Y, then Z. Faces, their
  `cull` sides and `occludes` all turn with it.
- **`{property}`** in a model or texture name (or a texture's materials)
  is replaced by the state's value. One line can cover every color:

  ```json5
  {
    properties: { color: ["white", "red", "blue"] },
    model: "cube",
    textures: { all: "wool_{color}" },
  }
  ```

A variant can also set `name`, which replaces the block's.

A block may have up to 4096 states.

### Material properties

A property can be a material instead of a list of words:

```json5
// blocks/stone.json5: any rock, with or without ore in it.
{
  name: "{rock}",
  properties: {
    rock: { materials: ["stone", "#rock"] },
    ore: { materials: "#ore", optional: true },
  },
  model: "cube",
  textures: { all: { texture: "stone", materials: { rock: "{rock}" } } },
  variants: [
    {
      when: { ore: { not: "none" } },
      name: "{rock} {ore} Ore",
      textures: { all: { texture: "ore", materials: { rock: "{rock}", ore: "{ore}" } } },
    },
  ],
}
```

- **`materials`** is a material, `#tag` for every material with that tag
  (in order of id), or a list of them. A material that comes up again is
  skipped, so `["stone", "#rock"]` makes stone the default.
- **`optional: true`** adds the value `none`, first, for no material.
- The values are the materials' ids (`defaria:granite`), and that's what
  `{rock}` gives in texture and model names and in `materials`.
- **Names.** In `name`, `{property}` gives the material's name
  ("Granite"), nothing for `none` (spaces left over are dropped), or a
  plain property's value as it's written.

The long form works for plain values too: `{ values: ["y", "x", "z"] }`.

### Conditions

A `when` gives each property it cares about one of:

- a value, or a list of values any of which match: `"x"`, `["x", "z"]`;
- for a material property, a material (`"granite"`), `"#tag"` for any
  material with that tag, or `"none"`;
- `{ not: … }` with any of those, for every other value.

Each value must match something the property can be.

### The block's item

Every block gets an item with the same id. The item has the block's
material properties (and any property with `item: true`), and places the
block with those values and the rest at their defaults: there's an Oak
Log and a Birch Log item, but no item per axis. Set `item: false` on a
property to leave it out, or on the block to have no item at all.

To change the item (its name, icon or stack size), add an item file with
the same name: what it leaves out comes from the block. It can't add
properties or a `block`.

## Items

`items/<name>.json5`.

```json5
{
  name: "{head} Pickaxe ({handle})",   // defaults to the id, e.g. "Pickaxe"
  properties: {
    head: { materials: ["iron", "#metal"] },
    handle: { materials: ["oak", "#wood"] },
  },
  // The icon.
  texture: { texture: "pickaxe", materials: { head: "{head}", handle: "{handle}" } },
  max_stack: 1,          // default 64
  block: "stone",        // optional: the block it places
}
```

Items have properties as blocks do, plain or material, and every
combination is a separate item state: an item carries its property values
in the hotbar and the inventory. `{property}` works in the name and the
texture as it does for blocks. An item that places a block passes on the
values of the properties the block has too (by name), and leaves the rest
at their defaults.

The icon (in the hotbar and the inventory) is the `texture` tile, drawn
flat and scaled up. An item with no `texture` that places a block shows
that block instead, drawn in 3D from its model in its default state, seen
from above with its top, south and east faces showing. Icons redraw when
the textures hot reload. Every item state shows in the inventory.

## Not yet

- JSON5 changes need a restart; images hot reload.
- Placement rules (such as setting a log's `axis` from the face you click)
  aren't data yet, so placed blocks are in their item's state.
- Material attributes (hardness, weight) and other item data that isn't a
  fixed list of values, such as durability.
- Cut-out textures (leaves with holes): block faces are drawn opaque.
- Models are block-only so far: no bones or animation for entities.
- A mod can only add definitions in its own namespace, not replace another
  mod's.
