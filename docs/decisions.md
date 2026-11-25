# Decisions to revisit

Choices made deliberately simple for now, with what's wrong with them and
what should replace them. Newest first.

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
