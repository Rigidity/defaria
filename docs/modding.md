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
  categories/        headings items are listed under in the inventory
  networks/          kinds of network conduits form
  recipes/           what recipe machines take, make and need
  reactions/         what fluids do where they meet
  scripts/           Luau scripts, see scripting.md
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
  side: "both",                   // "both" (the default), "server" or "client"
}
```

Mods load after their dependencies, otherwise in order of id. A mod whose
dependency is missing, broken or circular isn't loaded. Two mods may not
share an id. The folder's name doesn't matter.

`side` says where the mod belongs, once a server plays with clients (see
[Multiplayer](#multiplayer)):

- `both`: blocks, items, everything below, and scripts both sides load.
  A server sends its `both` mods to every client that joins.
- `server`: scripts only the server runs, such as gameplay rules or admin
  tools. Never sent: clients don't see them.
- `client`: scripts only a client runs, kept on that client: a player's
  own, optional, and the server doesn't know about them.

A `server` or `client` mod holds only `scripts/` (and its manifest): a
definition or an image in one is an error, and the mod isn't loaded.
Everything a mod defines changes the block and item states both sides
must number the same, and a client never has the server's `server` mods,
nor the server its clients' `client` ones. Put content in a `both` mod.
For the same reason a `both` mod can't depend on a `server` or `client`
mod, nor one side's mod on the other's; any mod may depend on `both`
mods, and a side's mods on each other. In single player every mod loads,
each side's scripts on their own side.

## Images

`images/<name>.png`, referenced as `<name>` (or `<mod>:<name>`). An image
is 16×16 and grayscale: it may use only the 8 gray levels
`0, 36, 73, 109, 146, 182, 219, 255`, plus transparency. Each level is an
index into a palette, so an image is shape and shading only, and one image
can be stone in one texture and copper in another.
`scripts/gray-layer.py` turns any 16×16 image into one.

An image can also be a strip of animation frames: 16 wide, with frames of
16×16 stacked down it, the first on top. A texture using one needs an
`animation` (see [Animation](#animation)).

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
  // Other named numbers, for blocks and items to read.
  stats: { throughput: 0.5, max_temperature: 570 },
}
```

`hardness` and `weight` are what blocks and items made of the material
count toward their own (see [Hardness, weight and stats](#hardness-weight-and-stats)).
Hardness has no unit: dirt is about 0.5, stone 1.5 and iron 3. Weight is in
kilograms for a whole block of the material, a cube a metre on each side,
so it's the material's density in kg/m³: oak 750, stone 2600, iron 7870.
Every material needs both.

`stats` are any other numbers a material has, by name, read the same way
(blocks and items average them over their materials). A name is lowercase
letters, digits and `_`, and isn't `hardness` or `weight`; a value is at
least 0. Names aren't qualified with the mod: a mod's materials give the
same stats the base game's do, so its metals work in the base game's
wires. A mod adding stats of its own should start their names with its id
(`mymod_mana`) so they don't clash. The base game's:

| Stat | Meaning | Materials |
|---|---|---|
| `conductivity` | How well it carries power, copper being 1 | copper 1, gold 0.7, iron 0.17 |
| `throughput` | How much a pipe or tube of it carries, iron being 1 | gold 1.5, copper 1.25, iron 1, woods 0.5 |
| `max_temperature` | The hottest it holds, in kelvin | iron 1811, copper 1358, gold 1337, woods 570 |

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
tagged texture makes and every animation frame after the first. A
texture whose combinations don't fit is reported and left out; one whose
frames don't fit shows only its first.

### Animation

A texture whose images are strips of frames (see [Images](#images)) steps
through them, with `animation` saying how:

```json5
// textures/lava.json5
{
  layers: [{ image: "lava", material: "lava" }],
  animation: {
    frame_time: 0.5, // seconds each frame shows for: needed with frames
    blend: true,     // fade each frame into the next (default false)
    scroll: 0.25,    // blocks a second it moves along flowing fluid (default 0)
  },
}
```

The animation loops: after the last frame comes the first. Layers can
hold different numbers of frames, and repeat until they line up (two
frames over three make six); a still layer shows in every frame. A
texture has at most 64 frames. Item icons show the first. `blend` suits
slow motion, like lava's; without it, frames switch at once.

`scroll` is for fluids: a fluid's faces that flow move their texture
along the flow at that speed: its top downhill, its sides down while it
falls, and across along the flow otherwise. Faces of still fluid don't.
The flow is smooth from block to block (each corner of a face takes the
average of the voxels around it, fading out toward still fluid), and a
fluid's texture is laid on by where it is in the world, one tile a
block, so it runs on across the faces rather than restarting at each.
A texture without frames can scroll too: `animation: { scroll: 1 }`.
The game's clock restarts every hour, so a loop (frames times
`frame_time`) and a scroll that fit a whole number of times into an hour
never jump.

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
  // The boxes the crosshair picks, if not the collision boxes. Optional.
  // target: [{ from: [0.125, 0, 0.125], to: [0.875, 0.875, 0.875] }],
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
  (`models/slab_bottom.json5` gets `[0, 0, 0]` to `[1, 0.5, 1]` that way), or
  nothing if the faces are all in one plane. `collision: []` makes a block
  you walk through; the crosshair still picks it, as a whole block. Boxes
  turn with the block's `rotate` (or the part's: see [Parts](#parts)).
- **`target`** is the boxes the crosshair picks (to mine the block, use
  it or place against it), in the same form, if they aren't the
  collision boxes: a plant you walk through has a smaller box than its
  whole block, so you can aim past it at what's behind. Left out, the
  crosshair picks the `collision` boxes, or the whole block if there are
  none. It can't be empty. `models/cross.json5` picks a box around the
  middle of its crossed quads, 14/16 high; `models/low_cross.json5`, the
  mushroom's, one 11/16 high.

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
variants (or fill every model of its [parts](#parts)); naming a slot that
none of the block's models has is an error.

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

```json5
{
  name: "Water",
  render: "translucent",
  light_filter: 2,                 // 0 to 15; left out, 0
}
```

`light_filter` dims light coming into the block: it loses that many
levels on top of the one per block, from lamps and the sky alike. Sky
light shining straight down stays at full strength only through blocks
with no filter: coming into water it drops 2 levels, and from there 3
levels a block, like any light that isn't at full strength, so a deep
lake's floor is dark. The base game's water has 2 and leaves 1, so the
ground under a tree is shaded; glass and plants have none. It's the
block's, the same for all its states; variants can't change it. A fluid
that doesn't fill its voxel leaves the voxel's own light alone, so a plant
standing in a puddle is lit as on dry ground, and dims the light it
passes on, and the faces under and beside it, in proportion to how full
it is: a film not at all, a voxel nearly full as a full one.

### Rendering

`render` says how the block's faces are drawn:

```json5
// blocks/leaves.json5
{
  name: "{wood} Leaves",
  properties: { wood: { materials: "#wood", default: "oak" } },
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
  against it, so a wall of glass or a pond of water looks like one body,
  and only where the two faces that meet have the same texture (the same
  tile, materials included). So a seam shows between red and blue stained
  glass, whether a material property or a plain one picks the color, but
  not between two of a block's states that look the same there (other
  properties, such as a connection, don't split it). Where a state's
  faces against one side have different textures, none of them is
  hidden; where the neighbour covers the side but has no faces against
  it, there's no seam to keep, and they're hidden.

Variants can set `render`; a later one replaces an earlier one's. Light
passes through every side of a cut-out or translucent block, and they
cast no ambient occlusion.

`double_sided: true` draws each face from behind as well, lit as its front
is, for blocks you can be inside: water's surface shows from underneath,
and lava's from inside it. It's set on the block, not in variants.

### Fluids

`fluid` makes a block a fluid: nothing bumps into it, whatever its model's
`collision`, and the crosshair passes through it (so you can't mine it; a
[fluid container](#items) in hand aims at it). A fluid isn't a voxel's
block but sits beside it, in a layer of its own: a voxel of water is air
with water in it. A block placed where it is keeps as much of it as the
block leaves room for (its `fluid_room`, below), and pushes the rest
aside: sideways first, into the nearest voxels on the same level with
room for it, reached through the same fluid (so a block dropped into a
full pond raises the pond around it a little, rather than piling water
on top); only when none within a short reach has room, into the voxel
above, then the one below. It flows on from there. Only what none of
those has room for is deleted. Breaking the block leaves the fluid.
Since you can
be inside one, even an opaque fluid like lava doesn't hide the faces of
solid opaque blocks next to it, so the floor and walls of a lava pool show
from inside; it still hides its own faces, and those of anything else
see-through (water, glass, leaves), so only one face is drawn where they
meet. Make a fluid `double_sided` too, so its surface shows from inside.
A fluid's item places it only in creative: in survival that would make
fluid from nothing, so fluid comes into the world only from containers
and the terrain.

A voxel of a fluid holds an amount of it, from a sliver to a full block,
and is drawn that high, at one of 16 heights (a full block with the same
fluid above it). Fluid flows: it only ever moves between voxels, so none
is made or lost. Each step, a voxel's fluid first falls into the voxel
below, as much as fits; then, if that's full, it spreads to its four
sides, giving each lower side an eighth of the difference, and half of
itself over any edge it can run off, straight into the voxel below the
edge. Levels only ever move toward each
other, so a draining pond goes down smoothly. Fluid only flows into the
room blocks leave (all of air's voxel, half a slab's) and into the same
fluid: two different fluids that touch stop there, unless they react
(see [Reactions](#reactions)).
A fluid has no `model` (nor `rotate`): the engine draws it as a box as
high as it's full, its sides cut to fit with their texture cut off rather
than squashed. Give it textures as for a full block: `all`, or `side` and
`end`, or each face's own (`up`, `down`, `north` and so on).

`fluid` says how it flows and what it's like to be in, with amounts in
blocks (1 is a full block):

```json5
// blocks/water.json5
{
  name: "Water",
  render: "translucent",
  double_sided: true,
  fluid: {
    rate: 20,          // flow steps a second
    fall: 1,           // the most that falls into the voxel below in a step
    film: 0.0625,      // the thinnest layer that spreads onto dry ground
    ripple: 0.002,     // the smallest difference that flows sideways
    move_speed: 0.5,   // how fast you move in it, as a fraction of walking
    view: { color: "#1d3f73", distance: 14 }, // the fog with your eyes in it
    evaporation: 0.00025, // how fast a film of it dries up under open sky
  },
  textures: { all: "water" },
}
```

- **`rate`**: how many times a second it flows. Lava's 2 to water's 20
  makes it creep.
- **`fall`**: how much falls into the voxel below in a step, from a
  sliver to 1. Under 1, a stream poured off a ledge falls as a thin
  column.
- **`film`**: fluid thinner than this stays where it is on dry ground
  (it still runs off an edge). Water's sixteenth lets a block of it spread
  thin over a wide puddle; lava's quarter keeps it in a thick blob.
- **`ripple`**: two voxels of it closer than this count as level, so
  nothing flows between them (nor between voxels a few thousandths apart,
  where a trickle could overshoot). At least 0.002. Small, and surfaces settle
  flat; bigger, and flow settles sooner, but surfaces can be left sloping
  by up to this much a voxel, which adds up across a wide pool.
- **`move_speed`**: how fast the player walks and swims in it, from 0 to
  1 of walking speed. It also scales swimming up and sinking.
- **`view`**: the fog's `color` (`#rrggbb`) and how far you can see
  (`distance`, in blocks) with your eyes under its surface.
- **`evaporation`**: how fast a film of it (thinner than `film`, so it no
  longer spreads) dries up under open sky, in blocks a second, from 0 to
  1. Left out, it's 0 and it never does: lava. Water's 0.001 dries a
  sixteenth of a block in about a minute. Open sky means full sky light
  just above it, so a roof, or water above, stops it. A film in a
  block's room (a slab's half) dries too, leaving the block. It's gone
  for good: nothing brings it back as rain.

#### Room for fluid

`fluid_room` says how much of a block's voxel fluid can share with it,
in blocks, from 0 to 1. Left out, it's 0: the block fills its voxel, and
placing it into fluid pushes all of it aside. A block that leaves room
keeps that much of any fluid when it's placed into it, and fluid flows
into the room it leaves, as into air; breaking the block leaves the fluid
behind. Any fluid works, not just water.

```json5
// blocks/stone_slab.json5
{
  model: "slab_{half}",
  fluid_room: 0.5,   // half its voxel: a slab in a pond stays under water
  // ...
}
```

The base game gives slabs half their voxel, and plants all of it. The
fluid is drawn in the part of the voxel the block doesn't cover: a slab's
half full of water reaches the top of its voxel, so the pond's surface
runs on unbroken over it, and less stands that much higher than the
slab. Light treats the voxel as the block and the fluid together (a slab
in lava glows), collision and the crosshair are the block's alone, and
flow compares levels by how high the fluid stands, the room the block
takes counting as filled. A fluid can't have a `fluid_room`.

### Hardness, weight and stats

Every block state and item state has a hardness (how long it takes to
break; no unit, dirt about 0.5 and stone 1.5) and a weight in kilograms.
Each is an attribute: a number,

```json5
{ name: "Lamp", model: "cube", textures: { all: "lamp" }, hardness: 0.3, weight: 500 }
```

or, for something made of materials, a `base` times the weighted average
of one stat of its material properties' materials:

```json5
// blocks/stone.json5: mostly rock, so ore counts a quarter.
hardness: { materials: { rock: 3, ore: 1 } },
weight: { materials: { rock: 3, ore: 1 } },

// blocks/stone_slab.json5: half a block of rock.
weight: { base: 0.5, materials: { rock: 1 } },

// A wire's stat (see below): 500 for each unit of its metal's
// conductivity, and 50 for a metal that doesn't say.
stats: { capacity: { base: 500, stat: "conductivity", materials: { metal: 1 }, default: 0.1 } },
```

- **`materials`** names material properties and how much each counts.
  Stone with iron ore is `(3 × 1.5 + 1 × 3) / 4 = 1.875` hard. A property
  at `none` doesn't count, so plain stone is just its rock's 1.5. Every
  state needs a material in at least one of them, so counting only
  optional properties is an error (in the state where all are `none`,
  there'd be nothing to average); count one that isn't optional, or
  write a number.
- **`stat`** is the materials' stat it averages: `hardness`, `weight` or
  one of their [`stats`](#materials). Left out, it's the stat named like
  the field, so `hardness` averages hardness and a block stat
  `max_temperature` averages `max_temperature`. A stat no material has is
  an error (it's likely misspelt).
- **`default`** is the value for a material without the stat. Left out,
  every material a counted property can be must have it: one that doesn't
  is an error naming the material and the stat.
- **`base`** multiplies the average; it defaults to 1. A material's weight
  is per whole block, so a slab's base is 0.5, and a tool's is its volume
  in cubic metres (a 0.7-litre pickaxe is `0.0007`).
- Left out, a block's hardness and weight are 0.

A block can have other numbers too, by name, in `stats`, each an
attribute:

```json5
// blocks/pipe.json5
stats: {
  throughput: { base: 2, materials: { material: 1 } },   // the stat `throughput`
  max_temperature: { materials: { material: 1 } },
  size: 0.25,                                            // the same for every state
},
```

Stat names are as for materials, and a stat a block leaves out it
doesn't have. Any other field that works a number out from materials
takes an attribute too, read the same way.

Each state's values are worked out once, at load, so the game never
averages while it runs.

### States, properties and variants

A block can have **properties**, each with a list of values; the first is
the default, unless the long form names one (see below). Every
combination of values is a separate block state, which can look
different. Chunks store block states, so a block carries its property
values wherever it is. A state is named `id[property=value,…]` (in saves
and scenarios), so property names and values can't be empty or contain
`[`, `]`, `=` or `,`.

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

### Parts

A block's `parts` add models to a state, each where its `when` matches,
so a pipe can be a core with an arm for every side it joins rather than
a model for each of the 64 ways its arms can go:

```json5
// models/pipe_arm.json5 points north; each part turns it to its side.
// `connect` becomes the six properties `north` to `down` (see
// Connections), which the engine sets as pipes join.
{
  properties: { connect: { connections: true } },
  conduit: { network: "fluid", capacity: 0.5, throughput: 1 },
  textures: { all: "pipe_core", side: "pipe_side", end: "pipe_end" },
  parts: [
    { model: "pipe_core" },                          // no `when`: always
    { when: { north: "true" }, model: "pipe_arm" },
    { when: { south: "true" }, model: "pipe_arm", rotate: [0, 180, 0] },
    { when: { west: "true" }, model: "pipe_arm", rotate: [0, 90, 0] },
    { when: { east: "true" }, model: "pipe_arm", rotate: [0, 270, 0] },
    { when: { up: "true" }, model: "pipe_arm", rotate: [90, 0, 0] },
    { when: { down: "true" }, model: "pipe_arm", rotate: [270, 0, 0] },
  ],
}
```

- A state is made of its own model (the block's `model`, or whatever its
  variants chose), if it has one, then every part whose `when` matches,
  in order. A block with `parts` doesn't need a `model`. `when` works as
  a variant's (see [Conditions](#conditions)).
- **`rotate`** turns only that part, about the block's center, as a
  block's `rotate` does. The block's own `rotate` (and its variants')
  turns only its own model, not the parts.
- **`textures`**: a part's slots are filled from the state's textures
  (the block's, with its variants'), then the part's own `textures`, which
  are for that part only and must name slots its model has.
- **`model`** may use `{property}`, as anywhere else.
- The state is the union of its parts: all their faces, all their
  collision and target boxes, and every side any of them `occludes`.
  Each part's faces cull against the neighbours as they would on their
  own; parts in the same block don't hide each other's faces, so leave
  out faces one part covers inside another (the pipe's arm has no face
  against the core).
- A fluid can't have parts.

### Material properties

A property can be a material instead of a list of words:

```json5
// blocks/stone.json5: any rock, with or without ore in it.
{
  name: "{rock}",
  properties: {
    rock: { materials: "#rock", default: "stone" },
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
  skipped.
- **`default`** is the value the block has unless something sets it: a
  material (`"stone"`), or `none` for an optional one. Left out, it's the
  first: `none` if the property is optional, else the first material, which
  for a tag is whichever id sorts first, and a mod can change that. So give
  a tagged property a `default`.
- **`optional: true`** adds the value `none`, for no material.
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
    soil: { materials: "#soil", default: "dirt" },
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

The long form works for plain values too, and can name the default the
same way: `{ values: ["x", "y", "z"], default: "y" }`. The default comes
first among the values; the others keep their order.

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
- **`"nearest"`** uses the side of the placed block's voxel nearest
  where you click: the side that touches the block clicked, unless the
  property has no value for that side, when it's the nearest of those
  it has. So a property with only the four sides, placed on a block's
  north side, is `south` wherever on that side you click, and placed on
  a floor is the side of its voxel the click is toward. One with only
  `up` and `down`, placed on a block's side, is `up` on its upper half.
  Sides equally near (a click in the very middle of a face) go to the
  way you look, or across the ground, if that's one of them, and else
  to the later one in the list `up`, `down`, `north`, `south`, `east`,
  `west`: a slab clicked in the middle of a block's side is a bottom
  slab.
- **`"look"`** uses the way you look, along its strongest axis: looking
  down is `down`, looking mostly west is `west`. If the property has no
  value for `up` or `down` and you look more up or down than across, it
  uses the way you face across the ground instead, so a machine placed
  on the floor in front of you faces the way you look.
- Each gives each direction the value named after its axis (`x`,
  `y` or `z`), or else after itself (`up`, `down`, `north`, `south`,
  `east`, `west`). A property whose values are neither needs `values`.
- **`{ from, values }`** says which value each direction gives; `from` is
  `"face"`, `"nearest"` or `"look"`. A direction it leaves out keeps the value the
  block would have had (from its item, or the default), but it must name
  at least one:

  ```json5
  // A slab half: on top of a block it sits low, under one it sits high,
  // and on a block's side in the half you click.
  half: {
    values: ["bottom", "top"],
    place: { from: "nearest", values: { down: "bottom", up: "top" } },
  },
  ```

  A vertical slab is a block of its own, standing against the block
  whose side you click, and on a floor or under a ceiling in the half
  of its voxel you click:

  ```json5
  half: { values: ["north", "south", "east", "west"], place: "nearest" },
  ```

`place` is for blocks' plain properties only, and not with `item: true`:
the item doesn't carry a value that placing picks.

### Replaceable blocks

`replaceable: true` makes a block one that placing replaces, as it does
air (with or without fluid in it): a block placed against it goes into
its voxel instead of beside it, and a block placed beside another can go
where it stands. The plants in the base game are, so you can build over
a meadow.

```json5
// blocks/tuft.json5 (in part)
replaceable: true,
```

- The crosshair still picks it, unlike a fluid, so you can mine it: give
  its model a small [`target`](#models) so it's easy to aim past.
- In survival, its item goes into the inventory of the player who builds
  over it, if it fits; otherwise it's lost.
- A block isn't placed over another state of the same block: a tuft
  placed against a tuft goes beside it, as against any other block, so
  a row of them can be planted.

### Halves

A state that fills exactly one half of its voxel, such as a slab, can
share the voxel with another half that fills the other side: a stone
bottom under a planks top, or two vertical slabs back to back. The game
sees that from the state itself; there's nothing to declare. A state is a
**half** when:

- its only collision box is one half of the voxel: `[0, 0, 0]` to
  `[1, 0.5, 1]` for the bottom, `[0, 0.5, 0]` to `[1, 1, 1]` for the top,
  or the same split along X or Z;
- its model `occludes` the side of the voxel that half is against;
- the crosshair picks it by that same box (no smaller `target`);
- it's drawn `opaque`;
- and it's a plain block: not a fluid, a conduit or a machine, not
  `replaceable`, and with no `block_entity`, `script`, connections or
  `support`.

Any two halves on opposite sides of the same axis fit together, whatever
their blocks: a slab with a slab, a vertical slab with a vertical slab.
Placing a block whose item places a half into a voxel holding the other
half fills it: clicking the open side of a half (the
top of a bottom slab), or placing against a face with a half just in
front of it. The new half is the state of the placed block that fills
the open side, with the item's values (its rock, say) and the values
its `place` properties pick from the face and view, all but its half.

The voxel is then one block made of both: it covers every side, so it
hides its neighbours' faces, stops light and shades corners like a full
block, and has no room for fluid. Each half's faces are drawn, but not
the two between them, so their models should show the part of the
texture a full block's face would there (the base game's `slab_{half}`
models do), to leave no seam where the halves meet. The crosshair picks
the half it's on, and mining breaks that half alone, giving its item and
leaving the other. It glows as brightly as the brighter half, and dims
light coming in by the larger `light_filter`.

The game bakes a state for every pair when it loads, in an engine block
called `combined` (scripts see that id, with no state values). Their
number is the product of the halves on each side, so a mod with many
slab materials adds many; if they don't all fit in the 65536 states
there's room for, the game reports an error and no halves combine.

### Connections

A block that joins whatever is next to it, such as a fence, has a
property with `connections`, naming the rule it connects by:

```json5
properties: {
  connect: { connections: "fence" },   // a rule, like an id: `fence` is `<this mod>:fence`
},
```

A [conduit](#conduits) connects by its network instead, and says so with
`true`:

```json5
properties: {
  metal: { materials: "#metal" },
  connect: { connections: true },   // joins by its `conduit`'s network and group
},
```

- It becomes six plain properties, `north`, `south`, `east`, `west`, `up`
  and `down`, each `"false"` or `"true"`, which `when` and `{property}`
  use like any other. Its own name isn't used. They count toward the 4096
  states: 64 for each combination of the block's other properties.
- The engine sets them, not the player: each side is `"true"` when the
  block on that side accepts the connection on the side facing it. By a
  rule, that's a block with a `connections` property naming the same
  rule, on any side. By a network, it's a conduit on the same network
  kind and in the same group (or both in none), on any side, or (once
  machines have them) a block with a port of that kind on that face. So
  pipes join pipes (of any metal), and wires join wires, but not each
  other. They're set as the block is placed, whenever a neighbour
  changes, and when a chunk loads beside it; a side facing a part of the
  world that isn't loaded keeps its value.
- A `connections` property has nothing else: no `values`, `default`,
  `item` or `place`. A block has at most one, and no other property
  called one of the six sides. It's for blocks only; the item doesn't
  carry the sides. `true` is only for a conduit, and a conduit must have
  one with `true`.

### Conduits

A wire, pipe or tube is a block with a `conduit`: it carries one kind of
network, and the conduits joined to each other form a network.

```json5
// blocks/wire.json5
{
  name: "{metal} Wire",
  properties: {
    metal: { materials: "#metal", default: "copper" },
    connect: { connections: true },
  },
  conduit: {
    network: "electric",                     // networks/electric.json5
    capacity: 0,                             // holds nothing
    // 1000 for each unit of its metal's conductivity.
    throughput: { base: 1000, stat: "conductivity", materials: { metal: 1 } },
  },
  // ...model, parts and textures as for any block
}
```

- **`network`** is a [network kind](#networks), a reference like any
  other.
- **`group`**, optional, keeps conduits of the same kind apart: two
  conduits join only if both have the same group or neither has one. It
  can use `{property}`, filled with the state's value (a material's id),
  so one cable file with a `color` property and `group: "{color}"` gives
  cables of each colour that run side by side without joining.
- **`capacity`** is what one of it holds and **`throughput`** what it
  passes on a second, both in its network's solver's units (the base
  game's pipes: blocks of fluid; wires: energy; tubes: items). Each is an
  [attribute](#hardness-weight-and-stats), so it can come from the block's
  materials' stats. A network's capacity is the sum of its conduits'.

A network follows the connection properties: two conduits are on the same
network when their sides facing each other are both joined. A conduit's
joined side facing any other block is a port on the network (a machine's
face). A network with a joined side facing a chunk that isn't loaded is
frozen, as it may go on out there: its solver skips it until the chunk
loads. Networks are worked out from the world as chunks load; nothing
about them is saved but what pipes hold (see [Fluid networks](#fluid-networks)). A conduit joins a block that isn't a conduit only
on a face where that block has a [port](#machines) of its kind.

Every conduit has a [block entity](#block-entities), which keeps which of
its faces are blocked: a [wrench](#items) closes a face, and a blocked
face is never joined, from either side, whatever is next to it. So two
pipes side by side can be kept apart, and a wire kept off a machine's
face. The block doesn't have to ask for the block entity.

### Conditions

A `when` gives each property it cares about one of:

- a value, or a list of values any of which match: `"x"`, `["x", "z"]`;
- for a material property, a material (`"granite"`), `"#tag"` for any
  material with that tag, or `"none"`;
- `{ not: … }` with any of those, for every other value.

Each value must match something the property can be.

### Support

A plant says what it must stand on with `support`: a list of blocks, each
in all its states or, with a `when` (see [Conditions](#conditions)), only
those that match.

```json5
// blocks/tuft.json5 (in part): on dirt or loam, grassy or bare, and on
// any soil under grass, but not on bare sand.
support: [
  { block: "soil", when: { soil: ["dirt", "loam"] } },
  { block: "soil", when: { turf: "grass" } },
],

// blocks/mushroom.json5 (in part): on dirt or loam, any stone or
// mossy stone.
support: [
  { block: "soil", when: { soil: ["dirt", "loam"] } },
  { block: "stone" },
  { block: "mossy_stone" },
],
```

- A player can place it only on top of one of these; anywhere else the
  click does nothing, and uses nothing up.
- When the block under it stops being one (it's broken, or replaced, by
  anyone: a player, a script, flowing fluid), it breaks, on the same
  frame, and so does anything standing on it in turn. A player who mines
  the block under it in survival gets its item too, if it fits; broken
  any other way, it gives nothing. A block a script or a scenario's
  `fill` puts where it can't stand breaks the same way.
- It isn't checked while the block under it is in a chunk that isn't
  loaded, nor when chunks load: terrain only grows plants where they
  stand.

A block without `support` stands anywhere, in mid-air too.

### The block's item

Every block gets an item with the same id. The item has the block's
material properties (and any property with `item: true`), and places the
block with those values and the rest at their defaults: there's an Oak
Log and a Birch Log item, but no item per axis. Set `item: false` on a
property to leave it out, or on the block to have no item at all.

The item is as hard and as heavy as the block state it places. To change
the item (its name, icon, variants, uniqueness, hardness, weight or
stats), add an item file with the same name: what it leaves out comes
from the block. It can't add properties or a `block`.

### Block entities

A block with `block_entity` gives each of its voxels data of its own,
beyond its state, saved with its chunk: what a machine stores, or a
script's table. Placing the block makes a new one; changing only its
state (a furnace turning lit) keeps it; breaking or replacing the block
removes it.

```json5
{
  name: "Counter",
  model: "cube",
  textures: { all: "lamp" },
  block_entity: {
    // Plain data a new one starts with: numbers, strings, booleans,
    // and lists and tables of them. Optional.
    data: { count: 0, label: "start" },
  },
}
```

`block_entity: {}` gives the block one with no data of its own (or an
empty table, for a block with a [script](#scripts)). Features of the
game (machines, conduits, scripts) add their own parts to the block
entities of the blocks they apply to.

### Scripts

`script` names a [Luau script](scripting.md) whose hooks run for the
block: when it's used (right click), placed, broken, when a block next
to it changes, and on random ticks. `"lamp"` is this mod's
`scripts/lamp.luau`; `"other:lamp"` is another mod's.

```json5
// blocks/switch_lamp.json5
{
  name: "Switched Lamp",
  properties: { lit: { values: ["false", "true"] } },
  model: "cube",
  textures: { all: "lamp_off" },
  variants: [{ when: { lit: "true" }, textures: { all: "lamp" }, light: [15, 14, 12] }],
  block_entity: { data: { switched: 0 } },  // its script keeps a count here
  script: "switch_lamp",
}
```

A machine's script also runs on network ticks (`on_tick`) and when it
finishes a recipe, and moves energy, fluid and items through its
buffer, tanks and slots ([scripting.md](scripting.md#machines)).

A scripted block changes its state (`block:set_state { lit = true }`)
and the blocks next to it. Only a block with a `block_entity` keeps data
of its own (`block.data`) between calls. The block's item doesn't take
its script: give the item its own with an item file.

### Machines

A block with `machine` takes part in networks: conduits join it on its
*ports*, and it holds energy, fluid and items in its block entity (it
gets one without saying `block_entity`). The solvers of each network
move things between machines' ports; recipes and scripts decide what a
machine does with what it holds.

```json5
// blocks/oven.json5
{
  properties: {
    lit: ["false", "true"],
    facing: { values: ["north", "south", "east", "west"], place: "look" },
  },
  // ...model and textures as for any block
  machine: {
    ports: [
      { faces: ["back"], network: "electric", role: "consume" },
      { faces: ["up"], network: "item", role: "input", slots: "input" },
      { faces: ["down"], network: "item", role: "output", slots: "output" },
      { faces: ["left", "right"], network: "fluid", role: "input" },
    ],
    // Joules; rates are per second (watts).
    energy: { capacity: 20000, max_in: 400, max_out: 0 },
    // In blocks of fluid; `accepts` left out takes any fluid.
    tanks: { water: { capacity: 4, accepts: ["water"] } },
    // How many slots in each group.
    slots: { input: 1, output: 1 },
    // Its state while it works; the defaults when it stops.
    active_state: { lit: "true" },
    // The recipes it runs (see Recipes).
    recipes: "baking",
  },
}
```

Every field is optional; `machine: {}` is a machine with nothing.

- **`ports`**: each is one or more **`faces`** on one **`network`** kind
  (a [network kind](#networks) reference), with a **`role`**. Faces are
  sides (`up`, `down`, `north`, `south`, `east`, `west`), which turn with
  the state's `rotate`, or sides relative to the block's `facing`
  property (`front`, `back`, `left`, `right`): `front` is the side
  `facing` names, and `left` and `right` are the machine's own, looking
  out of its front (a machine facing up or down has west on its left).
  A port with relative faces needs a `facing` property whose values are
  all sides. A face has at most one port of each network kind, and
  conduits of that kind join the machine on it, and only there.
- **Roles** say what a port does, and each solver takes its own (how
  items move is under [Networks](#networks)):
  - energy networks: `produce` (gives energy from the buffer: a
    generator), `consume` (takes energy into it: a lamp) and `store`
    (both, as the network has too much or too little: a battery). An
    energy port needs `energy`;
  - fluid and item networks: `input` (takes from the network), `output`
    (gives to it) and `store` (both: a tank, a crate). A fluid port fills
    or drains a **`tank`**, and an item port a slot group, named with
    **`slots`**; either can be left out when the machine has only one.
- **`energy`**: `capacity` (the most it holds), `max_in` and `max_out`
  (the most it takes and gives in a second, 0 if left out), in joules
  and watts. Two more rates, also 0 if left out, make and use energy:
  **`generate`**, what it makes on its own each second while its buffer
  has room (a creative generator), and **`use`**, what it uses up each
  second while it works: it works, and is in its `active_state`, exactly
  while its buffer pays for that (a powered lamp). Machines that do work
  for recipes or scripts leave `use` out and are made active by those.
  **`max_generate`**, also 0 if left out, is the most the machine's
  [script](scripting.md#machines) may make each second, for a generator
  whose output depends on its surroundings (the base game's solar panel
  makes up to 60 W as the sky above it allows). Without it a script
  can't make energy.
- **`tanks`**: each by name, with a **`capacity`** in blocks of fluid and
  the fluids it **`accepts`** (blocks with `fluid`); left out, any. A tank
  holds one fluid at a time.
- **`slots`**: each group by name, with how many slots (1 to 256). A slot
  holds up to 64 of one item, or one of a `unique` item.
- **`active_state`**: property values, other than their defaults, the
  machine has while working, set by the engine and undone when it stops. Its
  other properties (`facing`) stay as they are.
- **`recipes`**: makes it a recipe machine, running the
  [recipes](#recipes) of this type. The short form takes inputs from the
  slot group and tank called `input` and puts outputs in those called
  `output`; the long form names its own, slot groups and tanks alike:
  `recipes: { type: "combustion", inputs: ["fuel"], outputs: [] }` (a
  name that's both a slot group and a tank means both). A recipe machine
  uses energy for its recipes itself, so it has no `energy.use`.
- **`filter`**, on an item port: the items it passes, by **`items`** (item
  ids) and **`materials`** (material ids, or tags as `"#metal"`; an item
  passes if one of its material properties is one). An item passes if it
  matches either list. An `input` with a filter takes only those from the
  network, and an `output` offers only those. Left out, any item passes.
  `filter: { materials: ["#wood"] }` takes planks and logs of every wood.
- **`emits`**, for creative and test machines: `{ item, slots }` keeps
  the slot group `slots` (left out, the only one) full of `item`, in its
  default state, out of nothing, so tubes can take it away. The base
  game's Creative Item Source emits planks.

A machine with slots, tanks or `recipes` has a **screen**,
which opens when the player uses it (right click) and is laid out from
these fields: its slot groups (a recipe machine's inputs, its progress,
then its outputs and other groups; a group of more than four slots is
a square-ish grid, and one more than three rows tall scrolls; parts too
wide for the window wrap onto more lines), a bar per
tank and one for its energy with their numbers under them, each under
its name with the first letter capitalised (name them to read well:
`input`, `fuel`). The player's items go into a recipe machine's input
slot groups (only what one of its type's recipes takes; see
[Recipes](#recipes)), or any group of another machine, and come out of
any slot. Dropped on a slot, they go into that slot's group first (any
group but a recipe machine's outputs), so a group no recipe names, which
a script reads, can be filled by hand.

Every number (capacities and rates) is an
[attribute](#hardness-weight-and-stats), so a machine of better metal
can hold more. Names of tanks and slot groups are lowercase letters,
digits and `_`. A port naming a network kind, tank or slot group that
doesn't exist, a role the network's solver doesn't take, two ports on
one face and network kind, an empty filter, or a filter or `emits` naming
an item or material that doesn't exist, is an error.

## Items

`items/<name>.json5`.

```json5
{
  name: "{head} Pickaxe",   // defaults to the id, e.g. "Pickaxe"
  properties: {
    head: { materials: "#metal", default: "iron" },
    handle: { materials: "#wood", default: "oak" },
  },
  // The icon.
  texture: { texture: "pickaxe", materials: { head: "{head}", handle: "{handle}" } },
  variants: [],          // optional: other names and icons (see below)
  unique: true,          // each one its own entry; default false
  block: "stone",        // optional: the block it places
  // As for blocks: a number, or a base times its materials' average.
  hardness: { materials: { head: 3, handle: 1 } },
  // 0.7 litres, a quarter of it the head.
  weight: { base: 0.0007, materials: { head: 1, handle: 3 } },
  stats: { reach: 4.5 },  // other numbers, by name
  action: "wrench",      // optional: what using it (right click) does
  script: "pickaxe",      // optional: its `on_use` runs when it's used
  fluid_container: { capacity: 1 },  // optional: it holds fluid (a bucket)
}
```

`action` gives the item something built in to do when it's used:

- `"wrench"`: on a [conduit](#conduits), blocks the face aimed at, or
  opens it if it's blocked. On one of the conduit's arms, that's the side
  the arm reaches; on its core, the face clicked. A blocked face has no
  arm, so it's opened again from the core's face there, or from the face
  of the block next to it (a machine's port). It places nothing, and
  while it's in hand right click on any block is its own: it doesn't
  open machines' screens or use scripted blocks.

`hardness`, `weight` and `stats` work as they do for
[blocks](#hardness-weight-and-stats), with the item's own properties;
weight is what one of the item weighs, in kilograms. Left out, hardness
and weight are those of the block state the item places, or 0 for an
item that places none. An item has the stats of the block state it
places as well as its own, its own winning where both have one.

`fluid_container` makes it hold some of one fluid, up to `capacity`
blocks of it (a bucket holds 1). Right click with it in hand:

- on fluid (the crosshair stops at fluid while a container is in hand,
  at the height its surface is drawn), takes from that voxel, if some
  container can take that fluid: as much as one has room for and the
  weight limit lets in, a full block's worth or whatever a part-full
  voxel holds. A container part-filled with that fluid is topped up
  before an empty one is used.
- on fluid with Shift held, or with no container that can take it, pours
  a container of that fluid (never another) into the voxel, as much as
  fits, and the rest into the voxel in front of the face aimed at.
- on a block, pours the fullest container into the voxel in front of the
  face aimed at. On a block that does something when used (a machine
  with a screen, a scripted block) only with Shift held; without, the
  click uses the block.
- Pouring fills the room a voxel's block leaves for fluid (all of air's,
  a slab's half: see `fluid_room`), and taking takes the fluid a voxel
  holds beside its block (a slab in a pond), the block staying put.
- on a machine with [tanks](#machines), takes from a tank holding a
  fluid a container can take, or else (or with Shift) pours into a tank
  that takes the fluid and has room. With nothing to move, the click
  goes on to the machine.

What doesn't fit where it's poured stays in the container, so no fluid is
made or lost.

What a container holds is part of the item: containers count together
only with ones holding the same fluid and amount (empty ones together),
the fluid adds its weight (its block's `weight` per full block), and it's
saved with the inventory. Its slot shows how full it is in a gauge down
its left edge, in the fluid's color, and says what it holds in its
tooltip. A container acts on the containers in the inventory, in
creative too; the hotbar names the kind, and the click picks which one.

`script` names a [Luau script](scripting.md) whose `on_use` hook runs
when a player uses the item (right click with it in hand) instead of
placing its block. The base game's Moss turns bare stone mossy.

Inventories count items, limited only by weight, so any number of an
item fits in one entry. A `unique` item is the exception: every copy is
its own entry, never counted with the others, since it will carry data of
its own (such as wear). Tools are unique.

Items have properties as blocks do, plain or material, and every
combination is a separate item state: an item carries its property values
in the hotbar and the inventory. `{property}` works in the name and the
texture as it does for blocks.

`variants` change the name or icon of the states a `when` matches (see
[Conditions](#conditions)). Every variant that matches applies, in order,
so a later one wins; what no variant sets is the item's own. A tool takes
a rock or a metal head, and a rock head has its own, chunkier image:

```json5
// items/pickaxe.json5
{
  name: "{head} Pickaxe",
  properties: {
    head: { materials: ["#metal", "#rock"], default: "iron" },
    handle: { materials: "#wood", default: "oak" },
  },
  texture: { texture: "pickaxe", materials: { head: "{head}", handle: "{handle}" } },
  variants: [
    {
      when: { head: "#rock" },
      texture: { texture: "pickaxe_rock", materials: { head: "{head}", handle: "{handle}" } },
    },
  ],
}
```

A block's item file can have variants too. Their `when` names the item's
properties, so not a block property with `item: false`.

An item that places a block passes on the values of the properties the
block has too (by name), and leaves the rest at their defaults.

The icon (in the hotbar and the inventory) is the `texture` tile, drawn
flat and scaled up. An item with no `texture` that places a block shows
that block instead, drawn in 3D from its model in its default state, seen
from above with its top, south and east faces showing. Icons redraw when
the textures hot reload. Every item state shows in the inventory.

## Categories

`categories/<name>.json5`: a heading the creative inventory lists items
under.

```json5
{
  name: "Building",   // required: the heading
  order: 20,          // lowest first; ties go by id. Default 0
}
```

A block or item names its category with `category`, a reference like any
other (`"building"`, or `"othermod:machines"`):

```json5
{ name: "Planks", model: "cube", textures: { all: "planks" }, category: "building" }
```

A block's item is in the block's category, unless an item file for it
says otherwise. Items in no category come last, under "Other". The base
game's are Natural (10), Materials (15), Building (20), Lighting (30),
Machines (35) and Tools (40);
leave gaps in `order` so other mods can slot theirs between.

## Fluid networks

The `fluid` solver runs each network as one shared tank: it holds as much
as its pipes' `capacity` together (half a block per base-game pipe), and
moves fluid between it and the tanks of the machines on it, each network
tick (20 a second, as energy networks):

1. `output` ports give their tank's fluid to the network, as much as
   there's room for;
2. `input` ports take it into theirs, as much as they hold;
3. if those still want more, `store` ports give theirs to the network
   for them; otherwise the stores take what's left in the pipes.

What goes in and what comes out in a second are each at most the
network's throughput: the `throughput` of its slowest pipe, in blocks of
fluid a second. A network holds one fluid: it takes in only the one it
holds (any while it's empty). Two networks holding different fluids that
are joined hold both, and take nothing in, only give out, until one is
used up. A network doesn't take a fluid hotter than its coolest pipe can
carry: a fluid block's `temperature` stat against a pipe's
`max_temperature` stat, both in kelvin (either left out: no limit).

```json5
// blocks/lava.json5
stats: { temperature: 1400 },
// blocks/pipe.json5: copper (1358) won't carry lava, iron (1811) will.
stats: { max_temperature: { materials: { metal: 1 } } },
```

A broken pipe pours its share of its network (by volume) into its voxel,
for flow to spread. A network's fluid is kept with its pipes in their
block entities while their chunk isn't loaded.

Machines meet the world's fluid through stats:

- **`pump_rate`**: a pump. Each second it draws that many blocks of fluid
  into its first tank, from the voxel in front of it (the side its
  `facing` property names) and the fluid reached from there through
  fluid and air, no higher than that voxel, up to 64 voxels and 8 blocks
  away: so it drains a pond's bed, not just the voxel in front.
- **`outlet_rate`**: an outlet. Each second it pours that many blocks of
  fluid from its first tank into the voxel in front of it, if that's air
  or the same fluid, as much as fits, and flow spreads it.

A machine with a `level` property, whose values are numbers from `0`,
has it set to how full its first tank is (rounded up, so any fluid
shows), and one with a `fluid` property to the fluid it holds, where the
fluid's name (without its mod) is one of its values. The base game's
tank draws its sight glass from them:

```json5
// blocks/tank.json5
properties: {
  level: { values: ["0", "1", "2", "3", "4"], item: false },
  fluid: { values: ["water", "lava"], item: false },
},
variants: [
  {
    when: { level: { not: "0" } },
    textures: { side: { texture: "tank_level_{level}", materials: { fluid: "{fluid}" } } },
  },
],
```

## Networks

`networks/<name>.json5`: a kind of network that [conduits](#conduits)
form.

```json5
// networks/electric.json5
{
  name: "Electric",   // required
  solver: "energy",   // required: "energy", "fluid" or "item"
}
```

The solver is the engine's code that runs the network: `energy` (power
from producers to consumers), `fluid` (one shared tank of one fluid) or
`item` (items moved between machines).

The `energy` solver runs 20 times a second. Energy is in joules, rates
in watts, and is held only in machines' buffers (wires hold none). Each
tick, `produce` ports offer what their machine holds, up to its
`max_out`; `consume` ports ask for what it has room for, up to its
`max_in`; `store` ports do both. Consumers are served from producers
first, then from storage; what producers have left charges storage.
What a consumer needs to keep running this tick (its `use`, or a
running recipe's share) comes before filling buffers, which share what's
left by their room; in a shortage, those needs are served whole, the
smallest first, until the power runs out, so some machines run and the
rest wait.
All that's moved on a network is capped by its **throughput**: its
weakest conduit's (a copper wire passes 1000 W, an iron one 170 W), so
one iron wire anywhere limits the whole network. A frozen network moves
nothing, so its batteries keep their charge. A mod can add a kind on any of
them, a steam network on the `fluid` solver say, and conduits on it join
only each other. The base game's kinds are `electric`, `fluid` and
`item`, with wires, pipes and tubes.

On an `item` network, items move straight from one machine's slots to
another's, none in the tubes between: a network moves as many items a
second as its slowest tube's `throughput`. Each goes from a port that
gives (`output` or `store`) to one that takes (`input` or `store`), but
never from a `store` to a `store`, nor back into the slot group it came
from. Inputs are filled before stores, so machines are fed first and
storage takes the rest, and items go round-robin between the ports that
take them, so two crates on one tube fill evenly. A full slot group
takes no more, and the items wait where they are.

## Recipes

`recipes/<name>.json5`: something a recipe machine does. A machine with
`recipes` (see [Machines](#machines)) runs those of its type: it takes
the inputs from its input slots and tanks, uses energy from its buffer
over the recipe's time, and puts the outputs in its output slots and
tanks. Its input slot groups take only items that one of its type's
recipes takes, whatever puts them there, so nothing sits in a furnace
that it can't smelt.

```json5
// recipes/smelt_sand.json5
{
  type: "smelting",                 // run by machines with `recipes: "smelting"`
  inputs: [{ item: "soil", when: { soil: "sand", turf: "none" } }],
  outputs: [{ item: "glass" }],
  energy: 1500,                     // joules, drawn evenly over its time
  time: 3,                          // seconds
}
```

- **`type`** is a plain name (lowercase letters, digits and `_`), not an
  id: any mod's recipes of a type run in every machine of that type, so a
  mod's `smelting` recipes work in the base game's furnace.
- **`inputs`** (at least one) are each an item or a fluid:
  - `{ item, when, count }`: `count` (default 1, at most 64) of an item.
    **`when`** says what its properties must be, as a variant's `when`
    does (see [Conditions](#conditions)); properties it leaves out can be
    anything, so `{ item: "log" }` takes a log of any wood.
  - `{ fluid, amount }`: `amount` blocks of a fluid (a block with
    `fluid`), as little as a thousandth.
- **`outputs`** are each an item, a fluid or energy:
  - `{ item, properties, count }`: `count` of an item, with these
    property values and the rest at its defaults;
  - `{ fluid, amount }`;
  - `{ energy }`: joules made into the machine's buffer, evenly over the
    recipe's time. This is how a generator works: its recipes burn fuel
    into energy.
- **`energy`**: joules it uses, evenly over its time; 0 if left out.
- **`time`**: how long it takes, in seconds, counted in network ticks (20
  a second).

**Materials.** `materials` names materials the inputs and outputs use:
the recipe is made once for each combination, with `{name}` in them
filled with the material's id. A combination that names something that
isn't there is skipped, so one file smelts every ore that has an ingot
into it, and coal (an `#ore` but not a `#metal`) makes no recipe:

```json5
// recipes/smelt_ore.json5
{
  type: "smelting",
  materials: { ore: "#ore" },       // a material, `#tag` or a list of them
  inputs: [{ item: "stone", when: { ore: "{ore}" } }],
  outputs: [{ item: "ingot", properties: { metal: "{ore}" } }],
  energy: 2000,
  time: 4,
}
```

Each combination is its own recipe, with an id of the file's and its
materials: `defaria:smelt_ore[ore=defaria:iron]`. A recipe none of whose
combinations work is an error.

Once everything has loaded, a recipe machine that can't run any recipe
of its type is an error naming the block and why for each recipe: none
of the type at all, items or fluids with no input or output slot group or
tank of the machine's to take or hold them, more of a fluid than its
tanks for it hold together, inputs or outputs needing more tanks (one
fluid each) than the machine has for them, or more slots (64 of one kind
of item, or one unique item, each) than its
slot groups have slots, energy with no buffer or nothing to fill it (no
`consume` or `store` energy port with a `max_in`, and no `generate` or
`max_generate`), or
more energy in one tick than the buffer holds. A machine that can run
only some of them, and a recipe type no machine runs, are warnings.

**Running.** On each network tick, a machine that isn't running a recipe
tries its type's recipes in order (each mod's by id, mods in load order)
and starts the first whose inputs it holds, whose outputs would fit, and
whose first tick of energy it can pay for (and, for a generator, has room
for). It takes the inputs then. Each tick it pays its share of the
energy; while its buffer can't pay, or a generator's has no room, it
waits where it is, and goes on when it can. When every tick has run, the
outputs go in, or it waits until they fit, and the next recipe starts
straight away. The machine is in its `active_state` while it holds a
recipe, running or waiting for energy or room, so it doesn't blink while
power is short. Whatever put the inputs in its slots
and tanks (a player, a tube, a pipe, a script), it runs on them the same.

## Reactions

`reactions/<name>.json5`: what two fluids do where they meet. Fluids
without one just stop where they touch, as at a wall.

```json5
// reactions/lava_water.json5
{
  fluid: "lava",    // the fluid whose voxel is used up whole
  with: "water",    // the fluid it meets
  makes: "stone",   // what that voxel turns into...
  at_least: 0.5,    // ...if it holds at least this much, in blocks
  uses: 0.25,       // how much of the other fluid it uses up, per block
}
```

Where flowing fluid touches the other one, beside, above or below it,
the two voxels react once: `fluid`'s voxel turns into `makes` if it holds
at least `at_least` of a block (a whole block of it: a slab whose room
held lava turns to stone too), and otherwise its fluid cools away, and
`uses` of a block of `with` for each block of `fluid` there was is taken
from `with`'s voxel (all of it, if it holds less): lava turning a full
block to stone uses a quarter of a block of water, and a sliver cooling
away uses a quarter of the sliver. It's the same whichever fluid flows
onto which. Fluids react where they flow into each other, and where they
already touch when a chunk loads. `makes` is a solid block, so a
boundary reacts once and then it's a wall. `fluid` doesn't spread
sideways onto a voxel resting on `with` (it would only creep out an
eighth at a time, each too thin to make anything), though it still runs
off edges onto it; `with` spreads onto `fluid` as usual, reacting as it
goes. Two fluids can only react one way: a second reaction naming the
same pair, either way round, is an error.

What a reaction uses up is the only fluid flow ever loses, besides what
blocks placed into fluid delete, having nowhere to push it; the headless
`fluids` report counts both (`displaced` and `reacted`).

## Multiplayer

A server sends its mods to the clients that join it, so a player needs
none of a server's mods to play there. On connecting, the server lists
its `both` mods (each one's id, version and a SHA-256 hash of its files),
and the client fetches the ones it doesn't have: a mod of its own with
the same files (the base game, usually) is used as it is, and one it
fetched before is in its cache. It then builds its blocks and items from
exactly the server's mods, plus its own `client` mods, and the two sides
check they number every state the same; if they don't (a different
version of the game), the client is refused, saying so. Leaving the
server, the client has its own mods again.

What's sent, and what's taken:

- Only the manifest, definitions (`.json5`), images (`.png`) and scripts
  (`.luau`). Nothing else in a mod folder ever leaves it (notes, sources,
  art files).
- A file at most 1 MiB, a mod at most 16 MiB and 4,096 files, a server at
  most 256 mods and 64 MiB of them in all, and a mod's `version` at most
  64 printable characters (its id is checked as any mod's). A server
  whose mods break a limit doesn't start, saying which; a client refuses
  a server whose list breaks one, and a mod that does.
- Paths are lowercase letters, digits, `_`, `-` and `.`, in folders split
  by `/`, none starting with `.` or named as Windows names devices
  (`con`, `nul`, `prn`, `aux`, `com1` to `com9`, `lpt1` to `lpt9`, with
  any extension), at most 128 bytes and 16 folders deep. A server with a
  file named otherwise doesn't start; a client refuses a mod with one.
- JSON5 files may nest arrays and objects at most 64 deep (anywhere, not
  just in multiplayer: deeper is an error naming the file), and an
  image's PNG header must say it's 16 wide and at most 64 frames tall
  before it's decoded.
- What a client receives is untrusted, and checked again before it's
  used or cached: every path, size, nesting and image header, the hash,
  that it's the size the server listed, and that its `mod.json5` names
  the mod the server listed. Its scripts run in the same sandbox, with
  the same limits, as any other (see `docs/scripting.md`, "Limits").

The cache is the data folder's `mod-cache/`, a folder per mod named by
its hash (see `docs/world-format.md`). A cached mod is checked against
its hash each time it's used, and fetched again if it changed. It keeps
at most 256 MiB: past that, the mods used longest ago go (never those
the game is playing with).

## Not yet

- JSON5 changes need a restart; images hot reload. A server's mods
  don't hot reload on its clients.
- A `client` mod can't restyle the server's blocks (a resource pack): it
  holds only scripts. A `server` mod can't add recipes or other content
  only the server needs.
- Vertical slabs are blocks of their own rather than a slab placed
  another way, since a click on a block's side can't tell a top or
  bottom slab from a vertical one.
- A voxel holds at most two halves, both whole halves of one axis: no
  quarter blocks, stairs or small pieces yet, and a half that's
  see-through (a glass slab) or a plant doesn't share its voxel.
- Item data that isn't a fixed list of values, such as durability.
- Variants can't change hardness, weight, stats or `light_filter`.
- A light filter dims every colour alike, so water doesn't tint what's
  under it, and a fluid's is the same however full it is.
- Variants can't add or remove parts; a part's `when` does that.
- Fluids' textures don't move.
- A block's room for fluid is one amount, not a shape: fluid in a slab's
  voxel falls through its solid bottom, and stands on the slab whichever
  half the slab is.
- Fluids that don't react don't float on each other (a lighter one
  spreading over a heavier one), and reactions show no steam.
- Models are block-only so far: no bones or animation for entities.
- A wire carrying more than it can doesn't burn out; it caps what the
  network moves. Wires lose nothing over distance, and there are no
  voltages.
- A pump or outlet reaches only the world in front of it, and a tank
  shows its first tank only.
- Items don't visibly travel along tubes; they jump from machine to
  machine.
- Item ports can't be told apart by priority beyond inputs before stores.
- A slot's size (64) is the same for every machine.
- A recipe machine with too little power stops and starts rather than
  running slower; recipes can't take a tool without using it up, or give
  something only some of the time.
- A mod can only add definitions in its own namespace, not replace another
  mod's.
