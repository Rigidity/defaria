# Decisions to revisit

Choices made deliberately simple for now, with what's wrong with them and
what should replace them. Newest first.

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

## UI hit testing is our own, not `bevy_picking` (2026-11-24)

**Decision.** `src/ui/pointer.rs` finds the node under the cursor itself,
from each `Hoverable` node's laid-out rectangle, stack order and clipping,
and triggers its own `Clicked` event, rather than using `bevy_picking`.

**What's wrong with it.** It duplicates what Bevy's picking does, without
its extras (drag, press and release events, touch, several pointers), and
the stock `bevy_ui_widgets` that rely on picking won't respond to it.
Bevy's mouse pointer always targets the window, while headless runs draw
into an image, so picking would never hit anything there, and UI couldn't
be tested.

**What should replace it.** `bevy_picking` with a pointer the headless
runner drives at the image target (writing `PointerInput` itself and
turning off the mouse input plugin there), once that's worked out; then
`Clicked` can become picking's `Pointer<Click>`.

**When to revisit.** When the UI needs dragging (moving stacks), text
input, or Bevy's widgets.

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
