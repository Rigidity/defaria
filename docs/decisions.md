# Decisions to revisit

Choices made deliberately simple for now, with what's wrong with them and
what should replace them. Newest first.

## Connections are block states, matched by rule name (2026-11-26)

**Context.** Pipes and wires join whatever is next to them that they can
connect to (#108), without the player choosing. Network kinds and conduit
groups (#111), machine ports (#114) and a wrench that blocks a face
(#121) build on it.

**Decision.** A `connections` property becomes six boolean properties,
so a joined side is part of the block state, stored in the chunk and
saved with it like any other property, and drawn by multipart models.
The engine derives them: `Content::connected` sets each side from
`Content::accepts`, the one check of whether the neighbour takes the
rule on the facing side, which for now is "it's a block with the same
rule name", on any side. The `connections` plugin runs it for the
voxels around every `VoxelChanged` and for connecting blocks in chunks
that just loaded and the faces of their neighbours, in `Update` after
edits and before remeshing, so a placed pipe is drawn joined on the same
frame. A side facing an unloaded chunk keeps its value, so unloading
changes nothing and loading catches up. Updates go through
`Voxels::update` and send `PropertiesUpdated`, not `VoxelChanged`: they
don't wake fluid, keep any fluid in the voxel, aren't edits to mining
(which now compares blocks, not states), and don't cascade, because a
neighbour's sides depend on which block is next to it, not on that
block's sides. Light relights them only if the two states differ for
light, and the chunk is saved with them.

**What's wrong with it.** Every combination of sides is a separate
state, 64 per combination of the block's other properties. Rules match
by exact name, and every face of a block accepts its rule, so a machine
can't take a pipe on one face only and two kinds of pipe can't share a
network yet. Being derived from the world, the sides can't hold
anything the player chose: a face blocked by a wrench has to live
elsewhere (the block entity) and be read by `accepts`. A chunk saved
before its neighbour changed loads stale and is fixed a frame later.
Placing checks only the placed block's default state against the bodies
around it (`block_edit`), not the arms it grows once joined, nor the
arms its neighbours grow toward it: a pipe placed beside one the player
stands against can push an arm into the player. Deferred: checking the
joined state, and the neighbours' new states, before placing.

**What should replace it.** `accepts` grows into the conduit and port
rules: network kinds and groups (#111), ports accepting a rule on a
face (#114) and faces blocked in the block entity (#121), all in that
one function, so every caller agrees. If states run short, the sides
move out of the state into per-voxel data the mesher reads.

**When to revisit.** With #111, #114 and #121, or when the state limit
is hit.

## Multipart states are flattened at load, and parts don't cull each other (2026-11-26)

**Context.** Blocks have `parts` (#107): models added to a state where
their `when` matches, so a pipe is a core and an arm per joined side
rather than 64 hand-made models.

**Decision.** Each state is baked at load into one list of faces, one
list of collision boxes and one set of covered sides, the union of its
parts', so meshing, culling, light and physics read a state exactly as
before. The parts aren't thrown away: `State::parts` keeps each part's
model, rotation, covered sides and its ranges in those lists. A part's
`rotate` turns only that part; the block's (and its variants') `rotate`
turns only the block's own model. Parts in one voxel don't hide each
other's faces: a model meant to join another leaves out the faces it
would cover (the pipe arm has no face against the core). Every
combination is still a separate state, baked up front, so the six
connection properties of #108 cost 64 states per combination of a
block's other properties.

**What's wrong with it.** Faces that one part hides inside another are
drawn unless the model author leaves them out. A directional block with
parts must repeat its turn on every part. States multiply: a block with
many independent parts and a couple of material properties nears the
4096-state limit, and every state's faces are stored. The parts can only
come from the block's properties, not from data in the voxel, so a wire
and a pipe can't share one voxel and there's no Chisel & Bits-style
building from small pieces.

**What should replace it.** A voxel that holds several parts of its own
(per-voxel data, or a block entity, #109), meshed part by part with
culling between parts in the same voxel, and parts baked once per model
and rotation rather than per state. `State::parts` is the list that work
should start from.

**When to revisit.** When a wire and a pipe should share a voxel, when
the state limit is hit, or when small-part building starts.

## Material stats share one namespace, and their units are loose (2026-11-26)

**Context.** Materials have named stats (#106), which blocks and items
average into their own numbers with attributes: a wire's capacity from its
metal's `conductivity`, a pipe's throughput and the hottest fluid it
carries from its material's `throughput` and `max_temperature`.

**Decision.** Stat names aren't qualified with a mod id, unlike every
other reference: a mod's copper writes `conductivity` and works in the base
game's wires, with nothing to import. A mod's own stats are expected to
start with its id by convention. The base game's are relative numbers:
`conductivity` is 1 for copper, `throughput` 1 for iron, and only
`max_temperature` has a real unit (kelvin, roughly where each material
melts or chars; kelvin so no stat is ever below 0). A material without a
stat an attribute reads is an error unless the attribute gives a
`default`; a stat no material has at all is an error too, as it's most
likely misspelt. Blocks and items have `stats` of their own (per-state
numbers for systems and scripts to read), and an item takes the stats of
the block state it places under its own.

**What's wrong with it.** Two mods can pick the same stat name for
different things and nothing notices. Relative units mean what a
`conductivity` of 0.17 does is up to whatever reads it (#110, #112), so
the numbers may need rescaling once wires carry power. The "no material
has it" check depends on which mods are loaded, so a block reading a stat
only an optional mod's materials define fails without that mod. Variants
can't change stats.

**What should replace it.** Stats declared once (a `stats/<name>.json5`
with a unit, a default and a description), qualified like other ids, so
clashes are errors, a missing stat can fall back to the declared default,
and tools can show units. Real units for power once #110 decides them.

**When to revisit.** When a second mod defines stats, when #110 decides
power units, or when fluids get temperatures (#113).

## Inventories count items, limited by a provisional weight (2026-11-25)

**Context.** The inventory (#28) is limited by weight rather than slots
or stack sizes: there are no stacks to arrange, any number of an item fits
as long as the total weight does, and screens sort and filter what's
there.

**Decision.** An `Inventory` (`src/inventory/model.rs`) is a list of
entries, each an item state, its `ItemData` (empty for now) and a count,
with an optional weight limit. Equal entries merge; a `unique` item gets an
entry per copy. It's a component, so it lives on an entity: the player's is
on the player (with `LocalPlayer`). The hotbar (`Hotbar`) names kinds of
items, persisting at 0 when they run out. The player can carry
`PLAYER_MAX_WEIGHT`, 10,000 tonnes (`src/inventory/player.rs`), in
survival; creative has no limit. There are no dropped items yet, so in
survival a block too heavy to carry doesn't break (`src/game_mode.rs`).

**What's wrong with it.** Blocks weigh what a cubic metre of their material
does (stone 2,500 kg), so 10,000 tonnes is about 4,000 stone: a number picked
so the system can be played with, not a design (#87). An entity has at most
one inventory, so a machine with input, fuel and output would need child
entities, and an item holding items (a bag in your inventory) has nowhere to
keep them yet. A hotbar slot names an item state, so it can't pick out one
particular unique item (one tool of several). Mining something you can't
carry should drop it, not refuse to break it.

**What should replace it.** Weights and the limit decided against real
content (#87). Inventories described by data (sections with rules: take
only, one kind), so mods can define machines. `ItemData` carrying wear and
contents, including a nested inventory, and hotbar slots that can name one
unique item.

**When to revisit.** With #87, with chests or machines, with dropped
items, or with items that hold items or wear out.

## Fluid flows a voxel at a time (2026-11-25)

**Context.** Fluid is volumetric: each voxel holds an amount, and flow
moves amounts between neighbouring voxels (`src/fluid/flow.rs`). Only
active voxels step, so settled fluid is free. Each fixed tick visits at
most `FluidSettings::cells_per_tick` (4096) of them, a fluid's step is
spread evenly over the ticks until its next one, and the step's changes
are made all at once when it's done.

**Decision.** Keep flow local: a voxel only ever looks at the voxels
near it, and gives each lower side at most an eighth of the difference
between them, so levels drift together rather than swinging back and
forth. Where an eighth rounds down to nothing, a single unit trickles if it
can't overshoot, so a wide pool settles within a drawn height of flat
rather than in a slope. A surface counts as level once neighbours are
within the fluid's `ripple`.

**What's wrong with it.** Sideways, local flow is diffusion. Measured in
`scenarios/sea-breach.txt` (a cut 64 long, 31 wide and 12 deep opened onto
the sea): the sea pours in slowly, its front creeping rather than
rushing, and after 20 seconds the cut is still mostly dry. The sea around
the breach drops in a shallow cone, and that disturbance keeps spreading
out across the loaded sea: 1,000
active voxels after one second, 3,900 after twenty, under a microsecond
each. Frame times stay in budget (mean 3.2 to 3.8 ms, worst under 10 ms,
against 2.5 to 3 ms with the same cut on `main`, where water doesn't
flow), but a longer drain wakes more and more of the sea until the budget
caps it, and then flow slows down. A settled sea costs nothing: flying
over one is as fast as on `main`. Small scenes take a while to come to
rest too: the last units trickle for up to half a minute after water
poured down `scenarios/fluid-flow.txt`'s stairs has visibly stopped,
though they cost little. Chunks whose fluid changed are remeshed
on the frame it changes, with no cap of their own; the breach didn't need
one, but a flood across many chunks at once might.

**What should replace it.** Treat a settled, connected body of a fluid as
one reservoir with a shared level: water leaving it lowers the whole
surface evenly, without stepping each voxel. That also makes a dam break
fast, and it's what pumps drawing from a lake or an oil pool will need
(#96 looks at this and other ways to speed flow up). If floods turn out to
remesh too much at once, cap the chunks remeshed for fluid alone per
frame. Films thinner than `film` stay on dry ground for good: see #97
(creeping toward a drop) and #98 (evaporation).

## A fluid's kind is its block; only its amount is stored apart (2026-11-25)

**Context.** Water and lava are blocks with `fluid` set. A voxel of one
also holds an amount, finer than any block property could hold.

**Decision.** The voxel's block says which fluid it is, and the chunk
keeps the amounts of the fluid voxels that aren't full in a sparse map
(`Chunk::partial_at`; a fluid voxel not in it is full), saved in the chunk
format. Generated seas are full, so they cost no extra memory or disk.

**What's wrong with it.** A voxel holds one block, so it can't hold a
fluid and a block that doesn't fill it, such as a slab in a pond
(waterlogging, #75). And a fluid can only flow into air: flow's
`cell_of` in `src/fluid.rs` gives every other block no room.

**What should replace it.** Move the fluid's kind into the same per-voxel
layer as its amount, so a voxel holds a block and a fluid, and let
`cell_of` give a block that doesn't fill its voxel room for some fluid.

## Fluid surfaces are flat steps (2026-11-25)

**Context.** A fluid voxel is drawn at one of 16 heights by its amount.

**Decision.** Its top is flat at that height, and its sides are cut to
it.

**What's wrong with it.** Flowing water looks like terraces rather than a
slope.

**What should replace it.** Average each corner's height with the voxels
around it, as Minecraft does, so the surface slopes toward the flow.
## Images are named after their materials (2026-11-25)

**Context.** Each soil (dirt, sand, gravel) has its own image, not just its
own colors, so the soil textures pick the image by material: `{soil}` in a
layer's `image` is the soil material's name (see Textures in
`docs/modding.md`).

**Decision.** A material's image is found by name: a soil `clay` in mod
`clay` is drawn with `clay:clay`, so a mod adding a soil names its image
after the material.

**What's wrong with it.** It's a convention: nothing in the material file
says which image is its own, and a material can have only one image per
mod name.

**What should replace it.** If materials ever need several images, or
images shared between them, let a material name its images by role, the
way it names palettes (`images: { soil: "sand" }`), and have `{soil}` look
there first.

## See-through blocks let all light through (2026-11-25)

**Context.** Leaves, glass, water and plants are drawn see-through
(`render` in `docs/modding.md`). Light used to stop at any side a block
covers; see-through blocks cover their sides too, so their faces against
each other can be hidden.

**Decision.** Light ignores the covered sides of see-through blocks: it
passes through leaves, glass, water and plants as if they were air
(`State::blocks_light` in `src/content.rs`), and they cast no ambient
occlusion.

**What's wrong with it.** Under a tree is as bright as open ground, and
the bottom of a deep lake is in full sky light. Minecraft dims light a
level or more through leaves and water.

**What should replace it.** A per-block light filter: how many levels light
loses passing through (leaves 1, water 2 or 3), applied in
`src/light/propagate.rs` as light steps into the block. Sky light coming
straight down should lose it too, so a deep lake darkens with depth.

## Translucent faces are sorted per chunk, on the CPU (2026-11-25)

**Context.** Translucent faces (glass, water) blend with what's behind
them, so they must be drawn back to front.

**Decision.** Each chunk's translucent faces are one mesh on a child
entity, which Bevy sorts against other meshes by its centre. Inside it,
`sort_translucent_faces` (`src/terrain.rs`) orders the faces farthest
first from the camera by their centres, rewriting the index buffer when
the camera has moved a block (near chunks) or a chunk (far ones).

**What's wrong with it.** Sorting by face centres and by chunk centres is
an approximation: big faces at grazing angles, and translucent faces in
neighbouring chunks that overlap on screen, can blend in the wrong order.
The second shows plainly in anything built of water and glass across a
chunk border, seen at an angle: a hard-edged band of a different shade
along the border, since a whole chunk's faces draw before or after the
next chunk's. Each re-sort uploads the chunk's mesh again. To keep that
cheap, a mesh whose faces all lie in one plane (open water) is never
sorted, and far chunks re-sort at staggered places (`sort_position`).
There's no tint or fog underwater.

Double-sided faces' backs are flagged, and the shader draws them a hair
farther away than they are (a depth bias of 2e-5 of their distance), so
a neighbour's face in the same plane that only covers part of it (a
slab's side against water or lava) wins the depth test instead of
flickering. Moving the backs themselves left hairline gaps or overlaps
at block edges. A back sorts as if a little inside its block, so water
blends over the glass it touches.

**What should replace it.** Order-independent transparency (Bevy has an
OIT pass), or sorting only the index buffer on the GPU. For water, add an
underwater view when fluids get physics (#63).

## Every material combination is its own tile and state (2026-11-25)

**Context.** Blocks and items can be made of materials
(`docs/modding.md`): a log of any wood, stone with any ore. A material is a
property value like any other, so every combination is its own block or
item state, and every combination of materials a texture is used with is
baked on the CPU into its own layer of the block texture array.

**Decision.** Keep expanding combinations up front, and keep baking each
texture's combinations into finished tiles. The base game has 41 block
states, 80 item states and 78 tiles. Baking stays even when tiles run
short: coloring in the shader instead (gray layers plus a palette per
face) doesn't fit layered textures. Each face would need a variable-length
list of layers to color and blend per pixel, and filtering breaks: mips
and linear filtering average gray levels and alpha before the palette
lookup, giving the wrong colors at a distance, where the baked tiles' mips
are averaged in linear color (`src/block_texture.rs`).

**What's wrong with it.** It multiplies. A block's states are the product
of its properties' values (4096 at most per block, 65536 block states in
all), and the tiles are the product of the materials each texture is used
with. Metal allows 2048 array layers; a few mods adding woods, rocks and
ores, with stairs, walls and tools for each, would get there. Item states
only carry values from fixed lists, so nothing like durability yet.

**What should replace it.** Room for more baked tiles, by either:

- **Several texture arrays.** Bind more than one 2048-layer array (or a
  binding array of them) and give each face an array index beside its
  layer. Filtering and mips stay exactly as they are.
- **Atlases.** Pack tiles into one or more large 2D textures. Far more
  tiles per texture, but smaller mips blend neighbouring tiles, so each
  tile needs padding (or a mip chain cut short) and the shader has to
  clamp and wrap within its tile.

If block states run short, store materials beside the state (a per-chunk
palette of material ids) instead of in it. Items get a data field next to
their state for values that aren't a fixed list.

**When to revisit.** When the tile count nears the GPU's array layer limit,
or block states near the 16-bit limit, or items need open-ended data.

## The fog's cylinder rides in Bevy's atmospheric fog (2026-11-24)

**Context.** The fog (`src/fog.rs`) is a cylinder: how far it reaches
sideways, up and down. Bevy's `DistanceFog` reaches every material's shader
in the view's bindings, which is cheap to change every frame, but none of
its falloffs is a cylinder.

**Decision.** Put the cylinder's three reaches in
`FogFalloff::Atmospheric`'s `extinction` and where it starts thickening in
`inscattering`, which Bevy passes to shaders unchanged, and read them in
our chunk shader (`assets/shaders/voxel.wesl`).

**What's wrong with it.** Anything drawn with Bevy's own PBR shader
(`StandardMaterial`) on this camera would read those numbers as real
atmospheric fog, hundreds of times too thick, and come out solid sky
colour. Nothing uses it in the world today.

**What should replace it.** A fog uniform of our own that every material
in the world reads, or the cylinder written into Bevy's fog in a form its
shader understands.

**When to revisit.** With the first thing in the world not drawn by the
chunk shader (held items, dropped items, mobs).

## The fog is the sky's colour everywhere (2026-11-24)

**Context.** Distance fog (`src/fog.rs`) hides the edge of the drawn
world: a cylinder around the camera, sized to fit inside what's loaded,
that shrinks while chunks inside it are missing. It fades into the sky
colour (`ClearColor`).

**Decision.** One fog colour, the sky's, wherever the camera is.

**What's wrong with it.** Underground, far cave walls fade to bright sky
blue rather than darkness, and a day-night cycle would leave the fog
behind. The cylinder's bottom is 160 blocks below the camera and its top
160 above (ten chunks: the world loads twelve each way, less one that's
never drawn and one to spare). Mountains rise nearly 300 blocks above
their valleys, so from a summit the valleys are lost in the fog, and
from a valley so are the tops of the peaks.

**What should replace it.** Fog coloured by the sky light where the camera
is (dark in caves), following the sky colour once it changes over the
day. Loading chunks around the ground's surface rather than a fixed
number of levels around the camera, so whole mountainsides are loaded
(and buried stone and empty sky aren't).

**When to revisit.** With a day-night cycle, when caves get big enough
to see far across, or if flying high should keep the ground in view
(load further down, `StreamingSettings::below`).

## Walking is smoothed, not stepped per tick (2026-11-24)

**Context.** Walking's top speed (4.317 blocks a second) and how quickly
it's reached and lost come from a per-tick model, Minecraft's: each 50 ms
tick adds a fixed push, moves, then applies ground friction of 0.546.
`WALK` in `src/player.rs` turns that into a smooth response time, so it
works the same at any physics step rate.

**Decision.** Keep the smooth version.

**What's wrong with it.** The smooth curve trails the per-tick one: after
100 ms the per-tick model has walked 0.25 blocks to our 0.18, and it stays
about 0.1 blocks ahead. Letting go, ours slides 0.36 blocks to its 0.26,
so stopping is a little looser than the numbers suggest. Jumps have the
same kind of gap: the right height, but about 6% quicker. Sneaking also
only lowers the eyes (to 1.27); the box stays 1.8 tall, so the player
can't sneak under a gap a block and a half high.

**What should replace it.** If starts and stops should feel tighter: step
horizontal movement per 50 ms tick (every third 60 Hz step) in
push-move-friction order, and draw between ticks as now. Shrink the
player's box while sneaking, when there's room to stand back up.

**When to revisit.** If walking or stopping still feels loose, or when
sneaking under low gaps matters.

## Physics is precise far away, but drawing isn't (2026-11-24)

**Context.** The player is a box that collides with block shapes
(`src/physics.rs`, `src/player.rs`). Its position is an `f64` and boxes are
compared with a tolerance, so walking, standing and sliding work the same
millions of blocks out. The camera's `Transform`, the chunk meshes'
positions and the shaders are `f32`, as Bevy's are.

**Decision.** Keep drawing in `f32` world coordinates for now; only the
physics is exact far out.

**What's wrong with it.** `f32` has about 7 significant digits. A few
hundred thousand blocks out, the camera and the meshes start to visibly
wobble and the outline drifts off its block; around 16 million blocks out,
a block is smaller than the gap between two `f32` values. The crosshair's
raycast is `f32` too.

**What should replace it.** Camera-relative drawing: keep positions in
`f64` (or chunk plus offset), and each frame move the world, not the
camera, so the camera stays at the origin and nearby meshes are drawn with
small coordinates. Bevy's big-space crate does this. The raycast should take
the `f64` eye position and step in chunk-local coordinates.

**When to revisit.** When anyone plays far out, or before adding anything
that sends the player a long way (teleports, long travel).

## Sky light comes in above whatever's loaded (2026-11-24)

**Context.** The world has no top or bottom: chunks are cubes, loaded in a
cylinder around the player (`src/streaming.rs`). Sky light falls straight
down at full strength until it hits an opaque block, then spreads and fades
like block light (`src/light.rs`). Without a top to the world, something
has to say where sky light starts.

**Decision.** Wherever a loaded chunk has no loaded chunk above it, full sky
light comes into its top layer. Unloaded chunks above are assumed to be open
sky. So the loaded chunks shade each other as usual, and nothing outside
them casts a sky shadow.

**What's wrong with it.**

- A cave or shaft that reaches the top of the loaded chunks is lit all the
  way down, however deep you are. More chunks are loaded above the player
  than strictly needed partly to make this rarer.
- Blocks above the loaded chunks, like a platform built very high up, cast
  no sky shadow until they load.
- The light changes as you fly up and down: when a roof unloads above you,
  what's under it is lit; when it loads again, it's shaded.

**How it's built so it can be replaced.** One trait decides it:
`OutsideSky::shines_into(chunk, x, z)` in `src/light/sky.rs`, asked only when
the chunk above isn't loaded. The flood fill, the chunks lit on loading
tasks and the light updates on loading, unloading and editing all go
through it. The current answer is `AssumeOpenSky` (always yes). A
replacement implements the trait and is inserted as the `Sky` resource.
`light::propagate`'s tests already run the light under two different
`OutsideSky`s and check it against a fresh fill after random loads, unloads
and edits.

**What should replace it.** A saved per-column record of the highest opaque
block, kept as a tree like CubicChunks2's surface tracker: each chunk a leaf
holding its 16×16 top heights, each node above holding the highest of its 16
children, saved with the regions, so "is anything opaque above this spot?"
is answered without loading anything. It covers generated and saved chunks.
Above them, up to the world's atmosphere top (a hard limit on what the
generator places, structures included), chunks the generator says could hold
something are generated, saved and added to the tree without being drawn;
chunks it says are certainly empty are skipped. Its answers change when
chunks above are generated or edited, so it also needs to queue light
updates for the loaded chunks below them.

**When to revisit.** When caves leaking sky light or tall builds not
shading the ground start to matter, or when worlds get an atmosphere top.

## The terrain is a heightmap, shaped by splines (2026-11-25)

**Context.** The ground's height in each column comes from
continentalness, erosion and ridged peaks noise, each mapped through a
spline as Minecraft's terrain is, with the old rolling hills on top
(`src/terrain/shape.rs`). The land near the origin is kept to the old hills.

**Decision.** Keep it a heightmap: one height per column, stone below,
air above.

**What's wrong with it.** No overhangs, arches, caves or floating
islands; cliffs are steep stairs rather than sheer faces. Changing the
generator changed every chunk not yet saved, so worlds saved before it
have seams where saved land meets newly generated land (the save format
doesn't record which generator made a chunk).

**What should replace it.** A 3D density function shaped by the same
splines (density falling off with height above the spline's surface),
with caves carved from 3D noise, and the generator's version recorded in
the world's `world.json5` so old worlds keep generating as they did.

**When to revisit.** When caves or overhangs are wanted, or before the
generator changes again for worlds anyone cares about.

## A fixed range of chunks is loaded around the player (2026-11-24)

**Decision.** Chunks within 16 sideways, 12 above and 12 below the
player's chunk are loaded (`StreamingSettings` in `src/streaming.rs`),
wherever the player is.

**What's wrong with it.** The mountains (see the terrain entry) stand
about 350 blocks from the lowest valleys to the highest peaks, more than
the 176 drawn above or below the camera. From a summit the valleys aren't
loaded, so the mountain stands over empty sky; from a valley the peaks'
tops are cut off; flying more than about 190 blocks over the ground loses
it. The fog hides the edge, but it looks like missing land. Loading
further up and down everywhere costs memory and loading time for chunks
that are nearly all solid rock or empty sky: about 20,000 chunks and 1 GB
already.

**What should replace it.** Load what can be seen rather than a fixed box:
for example, below the player, down to the surface in each column (which
the surface tracker in the decision above would know), with far chunks at
a lower level of detail.

**When to revisit.** When flying high or seeing far matters, or alongside
the surface tracker.

## Item icons are drawn on the CPU, unlit (2026-11-24)

**Decision.** Each item's icon is drawn once on the CPU
(`src/ui/icon/raster.rs`): a block's model in isometric view, each face
shaded by a fixed amount, or an item's own tile, flat. They redraw only
when the block texture is rebuilt.

**What's wrong with it.** Icons don't go through the world's shader, so
they don't match its lighting exactly, and lamps don't glow. Anything the
shader learns later (animated or emissive textures, tinting) won't show in
icons. The block icons are drawn at 128 pixels (enough for a scale-2
window) and scaled down to the slot, which softens them slightly.

**What should replace it.** Rendering icons on the GPU with the voxel
material: an offscreen camera drawing each block's mesh into an atlas,
lit the same way as the world, redrawn when the texture or shader changes.

**When to revisit.** When icons need to match the world (emissive blocks,
animated textures), or for entity or held-item models, which will need a
3D renderer for items anyway.

## The HUD is kept from picking by a camera marker (2026-11-25)

**Decision.** The UI uses `bevy_picking` (`src/ui/pointer.rs`). While no
screen is open, `UiPickingSettings::require_markers` is turned on, and no
camera carries `UiPickingCamera`, so the UI backend finds nothing under the
pointer and the HUD can't be hovered or clicked through a locked cursor.

**What's wrong with it.** It leans on a setting meant for something else
(opting nodes and cameras into picking), so a camera that ever gets
`UiPickingCamera` would make the HUD clickable while playing.

**What should replace it.** One owner of the cursor (see the next entry)
that also says whether the UI has the pointer, and a picking input filter
there: while the game has the mouse, the mouse pointer gets no input at all.

**When to revisit.** With the cursor's single owner, or when anything needs
`UiPickingCamera`.

## The cursor has two owners (2026-11-24)

**Decision.** `fly_camera` locks the cursor on a click and frees it on
Escape while playing; `ui::screen` frees it when a screen opens and locks
it when the last one closes. Gameplay input checks `ui::playing` (no screen
open), and some of it (looking, breaking, placing) also checks that the
cursor is locked.

**What's wrong with it.** "The game has the input" is decided two ways.
With no screen open but the cursor freed by Escape, the mouse wheel still
steps through the hotbar, while the pointer doesn't hover the hotbar
either, since the UI only takes the mouse while a screen is open. Escape is
handled in two places, which only works because those systems don't run on
the same frames.

**What should replace it.** One owner of `CursorOptions` in `ui::screen`:
the free-cursor state after Escape becomes a screen (a pause menu), so
`playing` alone says whether the game has the input, and `fly_camera` only
reads the cursor.

**When to revisit.** With the first pause or settings menu.

## Crack images are fixed files in the base mod (2026-11-25)

**Decision.** `cracks` loads its ten stages from
`mods/defaria/images/cracks/stage_<n>.png` by path, and draws them with
Bevy's `StandardMaterial` (unlit, multiply blending) on a mesh of the
mined block's faces, not through the block texture array or the voxel
shader.

**What's wrong with it.** Mods can't replace or add crack images, or give
a block its own cracks, and the overlay doesn't share the block texture's
filtering: its 16×16 images are sampled nearest with no mips, so far away
or at a grazing angle the cracks shimmer where the block under them
doesn't.

**What should replace it.** Crack stages as a texture kind in the mod
format, built into the block texture array (or an array of their own with
mips), drawn by the voxel material's shader over the block's own layers.

**When to revisit.** When mods add mining effects, or if the shimmer shows
in play.
