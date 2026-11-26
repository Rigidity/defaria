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
  hardness: 1,                     // required: how hard it is to break
  weight: 750,                     // required: kg per block of it
}
```

`hardness` and `weight` are what blocks and items made of the material
count toward their own (see [Hardness and weight](#hardness-and-weight)).
Hardness has no unit: dirt is about 0.5, stone 1.5 and iron 3. Weight is in
kilograms for a whole block of the material, a cube a metre on each side,
so it's the material's density in kg/m³: oak 750, stone 2600, iron 7870.
Every material needs both.

Each palette is 2 to 8 hex colors, darkest first: `#rrggbb`, or
`#rrggbbaa` with an alpha, for see-through materials like water. They're
spread evenly over
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
below. An opaque block's faces should end up fully opaque; see-through
blocks (see [Rendering](#rendering)) and item icons may be transparent.

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

An input can instead be a **tag**, `#name`: an input called `name` that
takes only materials with that tag. A texture whose inputs are all tags is
made for every combination of tagged materials as the game loads, so every
tile it could need exists, and a mod adding a material with the tag gets
its tiles without writing a texture. And `{name}` in a layer's `image`
(for a tag input) is the name of that input's material, so each material
can bring its own image:

```json5
// textures/soil_turf.json5: the side of any soil under any turf. Sand's
// is sand.png with grass_side.png over it, colored sand and grass.
{
  layers: [
    { image: "{soil}", material: "#soil" },
    { image: "grass_side", material: "#turf" },
  ],
}
```

`{name}` gives the material's id without its mod (`sand` for
`defaria:sand`), and the image is looked up in that material's mod, unless
the image names a mod of its own (`"defaria:{soil}"`). A missing image is
reported, and the tiles that use it show the placeholder. A texture's
inputs are all `$name`s or all `#tag`s, not a mix.

All mods together have room for 2048 tiles, counting every combination a
tagged texture makes. A texture whose combinations don't fit is reported
and left out.

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
  // against one of these is hidden, and light doesn't pass through them
  // (unless the block is see-through: see Rendering).
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
`occludes`), going in or out, but it passes through every other side, and
through every side of a see-through block (see [Rendering](#rendering)). So a
slab, which covers only its bottom, is lit from above and from the sides,
and a roof of slabs shades what's under it like a full roof. A block that
gives off light shines out of every side, covered or not. A block whose
model covers all six sides is opaque: no light gets in, and it shades the
corners of the faces next to it (ambient occlusion).

### Rendering

`render` says how the block's faces are drawn:

```json5
// blocks/leaves.json5
{
  name: "{wood} Leaves",
  properties: { wood: { materials: ["oak", "#wood"] } },
  model: "cube",
  render: "cutout",
  textures: { all: { texture: "leaves", materials: { wood: "{wood}" } } },
}
```

- **`"opaque"`**, the default: every pixel is solid, and alpha is ignored.
  A side the model covers hides the neighbour's face against it, unless
  the block is a fluid (see [Fluids](#fluids)).
- **`"cutout"`**: pixels with alpha under one half are holes, the rest
  solid. For leaves and plants. Nothing is hidden next to it, not even
  the same block, since you can see through its holes.
- **`"translucent"`**: pixels blend with what's behind them by their
  alpha, from the image or the palette (`#rrggbbaa`). For glass and
  water. A side it covers hides only the same block's translucent faces
  against it, so a wall of glass or a pond of water looks like one body.

Variants can set `render`; a later one replaces an earlier one's. Light
passes through every side of a cut-out or translucent block, and they
cast no ambient occlusion.

`double_sided: true` draws each face from behind as well, lit as its front
is, for blocks you can be inside: water's surface shows from underneath,
and lava's from inside it. It's set on the block, not in variants.

### Fluids

`fluid: true` makes a block a fluid: nothing bumps into it, whatever its
model's `collision`, the crosshair passes through it (so you can't mine
it), and placing a block where it is replaces it. Since you can be inside
one, even an opaque fluid like lava doesn't hide the faces of solid opaque
blocks next to it, so the floor and walls of a lava pool show from inside;
it still hides its own faces, and those of anything else see-through
(water, glass, leaves), so only one face is drawn where they meet. Make a
fluid `double_sided` too, so its surface shows from inside. Fluids don't
flow yet.

```json5
// blocks/water.json5
{
  name: "Water",
  model: "cube",
  render: "translucent",
  double_sided: true,
  fluid: true,
  textures: { all: "water" },
}
```

### Hardness and weight

Every block state and item state has a hardness (how long it takes to
break; no unit, dirt about 0.5 and stone 1.5) and a weight in kilograms.
Each is a number:

```json5
{ name: "Lamp", model: "cube", textures: { all: "lamp" }, hardness: 0.3, weight: 500 }
```

or, for something made of materials, a `base` times the weighted average
of its material properties' values:

```json5
// blocks/stone.json5: mostly rock, so ore counts a quarter.
hardness: { materials: { rock: 3, ore: 1 } },
weight: { materials: { rock: 3, ore: 1 } },

// blocks/stone_slab.json5: half a block of rock.
weight: { base: 0.5, materials: { rock: 1 } },
```

- **`materials`** names material properties and how much each counts.
  Stone with iron ore is `(3 × 1.5 + 1 × 3) / 4 = 1.875` hard. A property
  at `none` doesn't count, so plain stone is just its rock's 1.5. Every
  state needs a material in at least one of them, so counting only
  optional properties is an error (in the state where all are `none`,
  there'd be nothing to average); count one that isn't optional, or
  write a number.
- **`base`** multiplies the average; it defaults to 1. A material's weight
  is per whole block, so a slab's base is 0.5, and a tool's is its volume
  in cubic metres (a 0.7-litre pickaxe is `0.0007`).
- Left out, a block's hardness and weight are 0.

Each state's values are worked out once, at load.

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

Tag inputs (see [Textures](#textures)) suit material properties: soil is
any `#soil` material, with or without a `#turf` growing on it, and dirt,
sand and gravel each have their own image:

```json5
// blocks/soil.json5
{
  name: "{soil}",
  properties: {
    soil: { materials: ["dirt", "#soil"] },
    turf: { materials: "#turf", optional: true },
  },
  model: "cube",
  textures: { all: { texture: "soil", materials: { soil: "{soil}" } } },
  variants: [
    {
      when: { turf: { not: "none" } },
      name: "{soil} with {turf}",
      textures: {
        up: { texture: "turf", materials: { turf: "{turf}" } },
        side: { texture: "soil_turf", materials: { soil: "{soil}", turf: "{turf}" } },
      },
    },
  ],
}
```

A mod adds a soil, say `clay`, with a material tagged `soil` and an image
`clay.png`; a turf, say `moss`, needs only a material tagged `turf`.

The long form works for plain values too: `{ values: ["y", "x", "z"] }`.

### Placement

A plain property can be set as the block is placed, from where you click
or where you look, with `place`:

```json5
// A log runs along the face it's placed against.
properties: {
  axis: { values: ["y", "x", "z"], place: "face" },
},
```

- **`"face"`** uses the side of the block you click: placing on top of a
  block is `up`, against its east side `east`.
- **`"look"`** uses the way you look, along its strongest axis: looking
  down is `down`, looking mostly west is `west`.
- Either one gives each direction the value named after its axis (`x`,
  `y` or `z`), or else after itself (`up`, `down`, `north`, `south`,
  `east`, `west`). A property whose values are neither needs `values`.
- **`{ from, values }`** says which value each direction gives; `from` is
  `"face"` or `"look"`. A direction it leaves out keeps the value the
  block would have had (from its item, or the default), but it must name
  at least one:

  ```json5
  // A slab half: on top of a block it sits low, under one it sits high.
  half: {
    values: ["bottom", "top"],
    place: { from: "face", values: { up: "bottom", down: "top" } },
  },
  ```

`place` is for blocks' plain properties only, and not with `item: true`:
the item doesn't carry a value that placing picks.

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

The item is as hard and as heavy as the block state it places. To change
the item (its name, icon, stack size, hardness or weight), add an item file with
the same name: what it leaves out comes from the block. It can't add
properties or a `block`.

## Items

`items/<name>.json5`.

```json5
{
  name: "{head} Pickaxe",   // defaults to the id, e.g. "Pickaxe"
  properties: {
    head: { materials: ["iron", "#metal"] },
    handle: { materials: ["oak", "#wood"] },
  },
  // The icon.
  texture: { texture: "pickaxe", materials: { head: "{head}", handle: "{handle}" } },
  max_stack: 1,          // default 64
  block: "stone",        // optional: the block it places
  // As for blocks: a number, or a base times its materials' average.
  hardness: { materials: { head: 3, handle: 1 } },
  // 0.7 litres, a quarter of it the head.
  weight: { base: 0.0007, materials: { head: 1, handle: 3 } },
}
```

`hardness` and `weight` work as they do for
[blocks](#hardness-and-weight), with the item's own properties; weight is
what one of the item weighs, in kilograms. Left out, they're those of the
block state the item places, or 0 for an item that places none.

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
- Placement can't read where on a face you click yet, so a slab placed
  against the side of a block can't pick its half from the upper or lower
  part of that side.
- Item data that isn't a fixed list of values, such as durability.
- Weight is worked out but nothing uses it yet. Variants can't change
  hardness or weight.
- Fluids don't flow, and their textures don't move.
- See-through blocks let all light through: leaves and water don't dim it.
- Models are block-only so far: no bones or animation for entities.
- A mod can only add definitions in its own namespace, not replace another
  mod's.
