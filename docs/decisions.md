# Decisions to revisit

Choices made deliberately simple for now, with what's wrong with them and
what should replace them. Newest first.

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

## A fixed range of chunks is loaded around the player (2026-11-24)

**Decision.** Chunks within 10 sideways, 6 above and 8 below the player's
chunk are loaded (`StreamingSettings` in `src/streaming.rs`), wherever the
player is.

**What's wrong with it.** Fly more than about 130 blocks above the highest
ground and the ground drops out of the loaded range, so the view below is
empty sky; well before that, the edge of the loaded disc shows. Loading
further down everywhere costs memory and loading time for chunks that are
mostly solid rock.

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
