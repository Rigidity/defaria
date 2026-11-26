# Decisions to revisit

Choices made deliberately simple for now, with what's wrong with them and
what should replace them. Newest first.

## Buckets: the hotbar names the kind, and a click picks which one (2026-11-26)

**Context.** Fluid containers (#84) keep their fluid in their item data,
so filled and empty buckets are separate inventory entries. The hotbar
names a kind of item (an item state), not one copy, so with a bucket in
hand a click has to pick which bucket acts, and whether it takes or
pours.

**Decision.** `src/fluid_container.rs`: right click takes if it can
(the crosshair is on fluid, or a machine's tank holds fluid, and some
bucket can take it: one part-filled with that fluid first, else an empty
one), and otherwise pours: onto fluid only a bucket of that fluid, onto a
block the fullest bucket, and onto a block that does something when used
(a machine with a screen, a scripted block) not at all, so the click uses
the block. Mining still aims through fluid with a bucket in hand, though
the outline is on the fluid (provisional, for the user: feel).
Shift pours (into the world, or a machine's tanks), never takes, so
pouring where there's already fluid (a pit you've started filling) needs
it. Taking and pouring reuse the pumps' `fluid_network::world::take`
and `pour` on a bucket seen as a one-off `Tank`, so the rules for which
voxels take what stay in one place. Buckets act on the inventory's own
buckets in creative too, so fluid is never made or lost; a fluid block's
item places only in creative (the user decided). The hotbar slot shows
all its buckets' fluid together in a gauge, the inventory each kind's.
The fill shows as a gauge down the slot's left edge in the fluid's fog
color, not in the icon's pixels (provisional, for the user to confirm:
controls and look).

**What's wrong with it.**

- Pouring onto a block picks the fullest bucket, whatever its fluid: with
  buckets of water and lava, a click on the ground may pour lava.
- Shift is also sneak, and fly down while flying, so pouring while flying
  sinks the player a little each click.
- With full and empty buckets both to hand, a click on fluid always
  takes: pouring onto fluid (a pond, a pit already part filled, a
  machine's tank holding fluid) needs Shift. A hotbar that names
  one copy, or an item in hand apart from the hotbar, would let the
  player choose which bucket.
- The icon is the same for an empty and a full bucket; only the gauge
  and the tooltip tell them apart.
- A bucket pours into at most two voxels (the one aimed at, then the one
  in front of the face): what fits in neither stays in the bucket.
- Of equally full buckets, the one poured onto a block is the first in
  the inventory, which the player can't see or choose.
- The gauge is the fluid's fog color, lightened; water's reads slate
  rather than blue (provisional, for the user: look).
- Fluid weight is the fluid block's `weight` per full block; a filled
  bucket can take the inventory over its limit only by rounding.

**What should replace it.** An item in hand that is one copy (a held
stack), so a click uses that bucket, and an icon drawn per fluid and
fill (the fluid's texture inside the bucket's mouth), once item icons
can vary with item data.

## The player looks with yaw and pitch on itself; the camera only follows (2026-11-26)

**Decision.** The player is its own entity (#67). Its `Transform` is at
its feet and turned by the yaw; both yaw and pitch live in its `Look`,
and gameplay aims from its `Eyes` (feet plus the drawn eye height,
looking along the `Look`). The camera (`FlyCamera`) is a child at eye
height that copies the pitch, with the swaying, widening `EyeView` under
it. The mouse turns the `Look` of the player the camera belongs to, so
first-person look is still the camera module's input. The usual
split, with pitch kept only on the camera, would make aiming depend on
the camera, so a spectator or third-person camera couldn't exist without
the player losing where it looks. Mobs need a head pitch anyway.

Movement reads `Controls` rather than the keys, and only the local
player's come from the keys, so another body walks with the same
physics from its own.

The chunk loader and the held light are children of the player at
`EYE_HEIGHT`, not on the player at its feet: loading from the feet put
the loaded levels one chunk lower whenever the feet and eyes were in
different chunks, which changed what scenarios showed. They don't follow
the eyes down while sneaking (0.35 blocks), which doesn't matter yet.

**Revisit** when there's a second camera mode (third person, spectator)
or other players: a camera that isn't the player's eyes should take its
pitch from wherever it's pointed, not the `Look`, and the mouse input
then belongs to a controller rather than the camera module.

## Chunk storage: uniform chunks hold one value; voxels are still found by hash (2026-11-26)

**Context.** Measured (#71) with `scenarios/chunk-storage.txt` at the default
radius (16 by 12 up and down), headless, the `dev` profile (the game
crate at opt-level 1), on an Apple Silicon Mac that other builds were
loading heavily (load average 40 to 80), so the times are rough:

- 21,519 chunks loaded. 11,083 (52%) are air throughout and 9,015 (42%)
  one other block throughout; 20,299 (94%) have one light level
  throughout.
- With a flat array of block states and one of light (16 KiB a chunk),
  chunk storage was 336 MiB, 316 MiB of it in chunks of one value, and
  the run peaked at 914 MB of memory (unrendered). Chunk meshes keep
  another 101 MiB in the main world (`RenderAssetUsages::default()`).
- Loading the first view takes about 940 frames, 6 s on a quiet machine.
  Idle frames average 3 to 4 ms.
- Reading voxels by world position (`Voxels::get`: `ChunkMap` hash plus
  query, per voxel) costs about 15 ns a voxel; reading the same box a
  chunk at a time costs about 0.9 ns.
- A 344,064-voxel `fill` takes 9 to 19 ms itself (26 to 55 ns a voxel),
  and its frame takes 100 to 400 ms. Sampling those frames (macOS
  `sample`) puts about 46% of the time in light propagation, 23% in fluid
  waking around the edits, 10% in network updates and about 10% in the
  `ChunkMap` hash and query lookups spread across all of them.

**Decision.** Blocks and light are kept in a `ChunkArray`
(`src/world/chunk_array.rs`): one value while a chunk is one value throughout,
a dense array once a voxel differs, compacted back once a frame for the
chunks that changed. Chunk storage at the default radius went from 336
MiB to 21 MiB and peak memory from 914 MB to 416 MB. Frame times around
the big fills came out the same within the noise (best of six runs at
radius 8: pit 300 → 216 ms, roof 341 → 364 ms, refill 1,167 → 1,043 ms).

**What's wrong with it.** Voxels are still found one at a time through
`ChunkMap` (a hash) and a query, in `Voxels::get`, light propagation,
fluid flow and physics. That's 16 times slower than reading a chunk
directly, but about a tenth of a big edit's frame, and nothing in normal
play. Mixed chunks are dense (8 KiB each for blocks and for light), but
they're 6% of chunks and 21 MiB. And chunk meshes keep a copy of their
vertices and indices in the main world (101 MiB), more than all of
chunk storage now.

**What should replace it.**

- Looking a chunk up once per run of voxels: a cache of the last chunk
  in `Voxels`, or a dense array of the loaded chunks around each loader,
  starting with light propagation, which does the most lookups.
- A palette for dense chunks (4 or 8 bits a voxel).
- Chunk meshes kept in the render world only (`RENDER_WORLD`), if
  nothing reads them back on the CPU: #161.

**When to revisit.** Any of:

- a profile of real play (fluids, machines, a server's chunks) shows
  voxel lookups (`ChunkMap` hashing and `Query::get`) above about 25% of
  a frame, or a common action (a fluid spreading, an explosion) takes
  more than a frame's budget in lookups: the per-run lookup;
- mixed (dense) chunks pass about 100 MiB (a much larger radius, or a
  server keeping several players' areas): the palette;
- the meshes' main-world copy is already due, and tracked in #161.

## Shale and loam don't generate in the terrain yet (2026-11-26)

**Context.** #39 added shale (a `#rock`) and loam (a `#soil`) to the
base mod. The terrain is made of one rock and one soil: every rock
column is stone and every soil is dirt or, at the shore, sand, chosen in
`TerrainBlocks` (`src/worldgen.rs`). Granite and slate don't generate
either.

**Decision.** Shale and loam are only placeable, like granite and slate.
Generating them means choosing where each rock and soil goes (by noise,
depth or height), which is a terrain feature of its own, not a material.

**When to revisit.** When the terrain gets variety in its rocks and
soils: layers or patches of granite, slate and shale underground, and
loam in lowlands or under forests, with `TerrainBlocks` holding a state
for each.

## Beyond the loaded area: chunk loaders first, then networks that remember unloaded conduits (2026-11-26)

**Context.** Machines run only in loaded chunks, and a network that
reaches into an unloaded chunk freezes (#111), so a factory stops when
the player walks away and a power line to an outpost stops whenever any
stretch of it is out of reach. #123 weighed four ways out: chunk
loaders, a simulation distance apart from the drawn one, abstract
simulation of unloaded networks as numbers, and catching up on load;
and asked how each fits multiplayer (#49, #50, #51), what it costs, and
how scripted machines (#118) would catch up.

What the code has now: streaming already loads the union of cylinders
around every `ChunkLoader` entity (the camera has the only one), each
radius 16 by 12 up and 12 down, and draws them a chunk in. A loaded
chunk without its mesh is about 16 KB (8 KB of block states, 8 KB of
light), so the player's area is about 20,000 chunks and 300 MB before
meshes, and a 3×3×3 patch is 27 chunks and under half a megabyte. Every
solver (energy, fluid, items, recipes, scripts) runs on whatever is
loaded, with no idea of time passing outside it; the world keeps no
clock. Energy networks hold nothing in their wires, fluid networks keep
their fluid in one pool per network (`FluidNet`) with the unloading
pipes' share written to their block entities, and item networks hold
nothing in transit.

**Decision.** Machines run only in loaded chunks, always, and the ways
to keep more running are, in order:

1. **First step: chunk loaders** (#125, #126). A `ChunkLoader` gets its
   own area and a flag for whether what it loads is drawn. A chunk-loader
   block keeps a small patch around it (3×3×3 chunks) loaded and
   simulated but never meshed, unless a drawing loader also reaches it.
   The world remembers where its loader blocks are (a file in the world
   folder), so they load their patches at startup. This needs nothing
   new in the solvers: a loaded patch is simulated exactly as the
   player's area is, so it can't disagree with how things run when
   watched. It's also what a server does anyway: after the split (#50)
   the server's loaded set is the union of every player's area and every
   loader's, and "drawn" becomes "sent to a client", so a loader is one
   more source of wanted chunks and needs no rethinking for #49.
2. **Next: networks that remember their conduits in unloaded chunks**
   (#127). Loaders keep compact bases running, but a power line or pipe
   between two loaded areas still freezes at the gap. Rather than run
   whole networks as numbers, keep only the *topology* abstract: when a
   chunk unloads, its conduits stay in `Networks` (position, kind,
   capacity, throughput, the links between them) and are saved with the
   world, so a network spanning unloaded stretches keeps running between
   its loaded parts. Its machines in unloaded chunks are simply absent
   (their ports don't take part). A network freezes only where a conduit
   joins a chunk the world has never had loaded. The solvers need no
   change for energy and items (wires and tubes hold nothing); fluid
   networks keep their pool whole instead of handing an unloading pipe's
   share to its block entity.
3. **Later, and only if wanted: catching up on load** (#128, #129). A
   world clock, counted in network ticks and saved, is the prerequisite
   (and day and night will want it). With it, a chunk can record when it
   was last simulated, and a machine whose behaviour is a closed form
   (a recipe machine with its inputs and its own buffer, a battery) can
   be fast-forwarded by the time it missed, capped, when its chunk and
   every chunk of its networks load. Scripted machines don't catch up:
   they resume where they were, unless their script opts in with a hook
   that's handed the seconds missed (to be offered by #118's API, not
   guessed by the engine).

**Not chosen.** A **simulation distance** larger than the drawn one costs
in proportion to its area (two more chunks of radius is a quarter more
chunks, all simulated), keeps running what the player doesn't care about
and still stops a far outpost; a server can have one as a setting after
#50, from the same per-loader area #125 adds. **Abstract simulation of
machines** (a network's machines' rates saved and run as numbers) works
for steady producers and consumers only: recipe machines run in bursts
set by their inputs, item networks route by what's in each slot, and
scripts have no rate at all, so the numbers would disagree with what
happens when loaded, and the disagreement would show the moment the
player returns. Catching up covers the useful part of it, per machine,
with the real rules.

**What's wrong with it.** Loaders are Minecraft's answer, with its
problems: a player who wants a far base running has to place one, and a
spread-out base needs several. Each loader's patch is simulated in full
(fluid, random ticks, scripts) though nobody sees it, so a world with
many costs CPU every tick; #126 caps how many a world has. A patch is
a separate island of loaded world, lit and flowing as if its edges were
the world's edges, as the edge of the player's area already is. Until
#127, a network crossing the edge of a patch freezes. After #127,
remembered conduits cost memory and save space per conduit (a few dozen
bytes each, far less than a chunk), and a network can run between two
loaded ends through a stretch nobody has seen for days, which is the
point but means breaking a wire there (when a player reaches it) is the
only way to cut it. Nothing catches up until #129, so a far furnace
without a loader simply stops.

**What should replace it.** If loaders feel like chores, a loader could
be a machine with a cost (fuel or energy) and a configurable area, or
bases could be loaded automatically around claimed land, which is a
multiplayer design question (#49). If catching up proves reliable for
recipe machines, it could replace most loaders: a base far away then
needs only its networks remembered (#127) and its machines caught up on
load (#129).

**When to revisit.** With the server split (#50), which moves loaders
and remembered conduits into the server; if loaders show in frame
times; and when #118's machine script API settles what a script would
be told about missed time.

## Machine scripts only move things, and make energy only up to `max_generate` (2026-11-26)

**Context.** Machines' scripts (#118) need hooks at the network tick and
when a recipe finishes, and an API onto the machine's energy buffer,
tanks and slots, without letting a script break the rule the solvers
keep: energy, fluid and items are never made or lost except where a
definition says so (a generator's `generate`, a creative `emits`,
recipes). The issue sketched `fill(fluid, n)`/`drain(n)` on tanks and
`take`/`put` on slots, which as written make and lose things.

**Decision.** A machine's script is its block's `script` (#122), not a
separate `machine.script`: one script per block, which gets every block
hook as well. `on_tick(m, dt)` runs on every network tick, after the
energy, fluid and item solvers and the recipe machines, for every
machine whose script defines it, in order of position, and not while
any network on its ports is frozen. The calls are spread over the
fixed steps between two network ticks (three at 60 Hz), each machine
on the step its position hashes to, so no one frame runs them all; each
still gets the network tick's `dt` and share of `max_generate`, and a
third of them run on the network tick's own step, after the solvers.
They share a pool of steps a tick the size of block hooks' (250,000,
`HookSettings::steps_per_tick`, split evenly between the steps; its
size provisional, for the user to confirm), and of wall clock (4 ms,
`HookSettings::time_per_tick`, split the same way; provisional too):
once a step's share is
spent, the rest of its machines skip the tick, and its next turn starts
with them, so the same machines don't always miss out. `on_recipe_finished(m, recipe)`
runs from a `RecipeFinished` message the recipe machines send, oldest
first, from a pool of steps and time a tick as big as block hooks' (so
furnaces fed together, finishing together, don't all run at once); the
rest wait for the next network tick, in a queue that isn't saved, so
a hook still waiting when the world closes is lost. Past 65,536 waiting,
new ones are dropped with a warning, as block hooks are, and a hook
whose machine was broken (or replaced by another) before it ran is
dropped too, so a new machine never hears of the old one's recipe. In
every hook of a machine (`on_used` too) the handle is a `MachineRef`,
which reaches the machine's own parts through the call's world
function, as block handles do, and only in its own hooks. Every API
call only moves things, through the components' own methods:

- energy: `take(n)` spends what the buffer holds (as `use` does), and
  `generate(n)` makes energy only in `on_tick`, at most the tick's share
  of a new `energy.max_generate` rate (0 unless given). `generate` (the
  creative generator's) stays automatic and apart, so a script can't
  raise it.
- tanks: `move_to` another of its tanks, `pour` into and `draw` from a
  voxel within one step of the machine (through `Voxels`, so flow wakes
  around it). No `fill` or `drain` from nowhere.
- slots: `get`, `count`, and `move` between its own slot groups. No
  `take` that hands a script items to hold, which a failing hook would
  lose.

What a failing hook moved stays moved (each move is whole), while its
`data` is left as it was, and the machine's script is disabled
(`Caller::Block`, as for blocks). Scripts read light (`block.light`) and
materials (`world.material_def`) for machines that depend on their
surroundings; the base game's example is a solar panel.

**What's wrong with it.** A script can't hand anything to another
machine, or to a network, except through its ports and the solvers, nor
drop items into the world; a sorter that sends items to a neighbour
needs tubes. Energy a script makes waits a tick in the buffer before the
network sees it. Every scripted machine is a Lua call every tick even
when it has nothing to do, and each call costs: the base game's solar
panel took about 11 µs a call (1,024 panels, 3.7 ms on one frame of
every three) and takes about 7.5 µs now (about 2.6 ms, spread over
three frames), in a dev build on an M3 Max. What's left is mlua's
round trips: an empty `on_tick` costs about 1 µs, reading a handle's
field about 0.3 µs, and each field or method that asks the world
(`data`, `energy`, `light`, `neighbour`) about 0.5 µs more, through
the call's world function (a Lua function the field getter calls); and
reading `data` converts it to a table and back. A script that keeps an
`energy`, `data` or `neighbour` it reads twice in a local pays once. A
script keeping a fraction in its `data`
marks its chunk edited every tick. A machine's script can't make fluid
or items at all, even for a creative or test machine. `set_active` is
overwritten each tick on recipe machines and those with `use`. `on_broken`
still runs after the block entity is gone, so a broken machine's script
can't see (or drop) what it held. A machine whose tick is skipped,
when the pool runs out, misses it: a script that makes energy by the
tick makes none that tick. Since the pool is time as well as steps,
which machines miss out on a busy tick depends on the computer's speed.

**What should replace it.** Let scripts offer to and ask of the
networks on their ports (through the solvers, as a scripted port role),
and give items to a neighbouring machine's slots or drop them as item
entities once those exist. Let `on_tick` return how long to sleep. Hand
`on_broken` the block entity's parts before it's despawned. Run machine
hooks before the solvers when the simulation has an ordering of events.
If machine scripts' cost matters, make the handles scoped userdata that
borrow the call's world directly (one Rust call per field, not two and
a Lua call between), and skip converting `data` back when a script
didn't write it.

**When to revisit.** When item entities exist, when a mod needs a
scripted sorter or pump, or if scripted machines show up in frame
times.

## Block and item scripts: hooks a tick later, writes within one step, capped random ticks (2026-11-26)

**Context.** Blocks and items name scripts (#122) whose hooks run when
they're used, placed, broken, when a neighbour changes, and on random
ticks. Scripts must change the world only through the engine's own paths,
stay cheap with thousands of scripted blocks loaded, and run with the
simulation so they can move to a server.

**Decision.** Hooks run in `FixedUpdate`, from the `VoxelChanged` and
`PropertiesUpdated` messages and use messages of the frames before, so
a hook runs a tick after its cause, and what it changes runs the next
hooks a tick later again. Scripts reach the world through `ScriptWorld`:
`set_state` goes through `Voxels::update` (so a lamp switching keeps its
block entity and fluid, and isn't an edit that wakes fluid), and `set`
through `Voxels::set`. `set` refuses to put fluid in a voxel, to replace
fluid, to replace a block that has a block entity, and to make one (a
different block with a block entity), so no script makes or loses fluid,
a machine (a creative generator), its contents or a block's data (provisional, for
the user to confirm: the conservative reading of "scripts only move
things"). It does make plain blocks from air and turn plain blocks into
others, which moss or a growing crop needs, so a script can make or
remove block items: mining what it set gives an item (provisional, for
the user to confirm; the alternatives are letting `set` only replace a
block with one of the same material, or never fill air, or only make
blocks their definition lists as ones scripts may set). A hook may change only its own block and the six
next to it (an item's, the block looked at and its six), and read any
loaded block. Block handles reach the world through a function the call
puts in the Lua state's app data, made in the call's scope, and carry the call's
serial, so a kept handle fails. A block's `data` is its block entity's
`BlockData`, turned into a Lua table when first read in a call and saved
back when the hook succeeds; out of the hook's reach, it's a read-only
copy, never saved. Random ticks pick one voxel per loaded
chunk per tick, only in chunks known to hold a random-ticking block, and
run at most 64 hooks a tick, chosen at random among those due; other
block hooks run at most 1,024 a tick, and until they've used 250,000
steps between them (each call still gets its own budget, so a tick can
go over by one call's) or taken 4 ms of wall clock
(`HookSettings::time_per_tick`, a safety net for library calls, which do
any amount of work in a step, and for logging; the first hook of a tick
always runs), the rest waiting; a call stopped for running out counts as
its whole step budget. A neighbour hook (for a face)
or random tick already waiting isn't queued again, so blocks that keep
setting each other off faster than their hooks run queue at most one
hook per face rather than more every tick; random ticks aren't picked
while more than a tick's worth (1,024) of hooks is waiting; and past
65,536 waiting hooks new ones are dropped, with a warning (given again
only once the queue has drained below half of that), so scripts
that set off ever more edits can't use up memory. One pool for every
mod, and its size and the tick's 4 ms, are provisional, for the user to
confirm. A block's errors
disable its voxel (`Caller::Block`) until another block is placed there.
Using a scripted block is a right click handled in `BlockUseSystems`,
as machine screens and the wrench are, taking the click: after the
wrench (an item's action in hand wins) and before a machine's screen
(so a scripted machine's `on_used` replaces its screen); using a
scripted item comes after every block use, so a block that does
something when used (a machine's screen) wins over the item in hand.
Luau's `math.random` is seeded with 0 when the scripts load, and hooks
are queued in a fixed order (edits, then uses, then random ticks),
after fluid flow and the fluid solver (the other systems that edit
voxels in `FixedUpdate`), so headless runs repeat.

**What's wrong with it.** A tick's delay per step means a chain of
scripted blocks reacting to each other moves a block a tick. The one-step
write limit rules out scripts that build anything bigger than their
neighbourhood (a tree growing from a sapling). A script can't break a
block with a block entity, its own included, make one (a machine
growing another), or make a fluid source, even where a mod means it to (a sponge, a spring). `on_broken` comes after
the block entity is gone, so a broken block's script can't read its
data (to drop what it held). Chunks that lose their last random-ticking
block stay on the list until they unload. Every random tick and hook is
a Lua call even when the script returns at once. The tick's pool of
steps is shared by every mod, so one mod's busy blocks slow every mod's
hooks. The tick's time limit makes how many hooks run in a tick depend
on the computer's speed when hooks are slow, so heavy scripted worlds
don't repeat exactly between runs. A call can still run for its own 20
ms past the tick's 4 ms, and a single call may log 100 lines of 4 KiB.
Dropping hooks once the queue is full loses them, `on_placed`
and `on_broken` included, and a backlog delays every block's hooks, in
every mod. Two handles' world
calls go through Lua for each field read, which is slower than a typed
API.

**What should replace it.** Run hooks the same tick when the
simulation has a proper ordering of events; let scripts ask for writes
further away through a budgeted, deferred edit queue (with a check that
the target chunk is loaded and owned); hand `on_broken` the block
entity's parts before it's despawned (machines, #118, will need this to
drop their contents); count random-ticking blocks per chunk to drop
chunks that have none.

**When to revisit.** With machines' scripts (#118), when trees or
structures want to grow from scripts, or if random ticks show up in
frame times.

## Blocked conduit faces: a mask in the block entity, drawn as no arm (2026-11-26)

**Context.** A wrench (#121) blocks a conduit's face so it doesn't join
what's next to it. Where that's kept decides what it costs: as block
state properties it'd be three values per face (open, joined, blocked),
729 states per combination of the block's other properties.

**Decision.** Every conduit has a block entity (`Block::has_block_entity`
is true for blocks that connect by their `conduit`), which while some
face is blocked has a `blocked_faces` part, `BlockedFaces`, a six-bit
mask, defined in `connections.rs` and registered by the wrench's plugin
(the wrench adds it with the first face it blocks and takes it off with
the last it opens). `connect` passes it to `Content::connected_unless`, so
a face blocked on either side is never joined, and every change to it
(and a conduit loading with some blocked, or going with some) brings that
voxel and its neighbours up to date, as an edit does. The networks follow
the connection properties, so they split and join with no code of their
own. The wrench (`src/wrench.rs`, an item with `action: "wrench"`)
toggles the face aimed at: on an arm (a collision box that reaches one
side of the block) that arm's side, otherwise the face clicked. Aimed at
a conduit's open face toward another conduit that has blocked its face
back, it opens that one's instead, so a face blocked between two
conduits opens from either side, as it does from a machine's face
(provisional, for the user to confirm: controls).

**What's wrong with it.**

- A blocked face is drawn only as the arm missing: no cap, and nothing
  shows it's blocked rather than just facing nothing. Drawing one needs
  the mesher to read block entities (or a state property only for
  drawing), which it doesn't.
- With no arm there, there's nothing to aim at where the face was
  blocked: it's opened again by aiming at the core's face on that side,
  which for a thin wire is a small target, or at the face of the block
  the arm reached (a machine's port), which the wrench takes as the
  conduit's. With a wrench in hand, right click on any block is the
  wrench's, so it never opens a machine's screen or uses a scripted
  block (provisional, for the user to confirm: controls).
- Every conduit has a block entity, an entity each in memory, though a
  wire or tube keeps nothing in it until a face is blocked (one with
  nothing to save isn't written to disk). Deferred: giving a voxel a
  block entity only once it has a part needs block entities that come
  and go per voxel rather than per block, and the save and load to
  follow; a long wire run costs an entity a voxel until then.
- The face's change takes effect in `Update` with the other connection
  updates, a frame after a chunk with blocked faces loads at worst,
  while its block entity spawns: the networks may join through the face
  for that frame and split again.

**What should replace it.** A cap model drawn on blocked faces, from a
property the mesher can see or a per-voxel overlay like the crack
overlay, which would also give the face a box to aim at. Saving block
entities per voxel, only while they hold something, would remove the
entity a bare wire or tube has.

## Items move instantly along tubes, round-robin, inputs first (2026-11-26)

**Context.** The item solver (#120) needs to decide how items get from a
machine's output to another's input: whether they travel visibly through
the tubes or jump, which input gets each, and what's in a tube that's
broken or a network that freezes.

**Decision.** Items move instantly, from one machine's slot group straight
into another's, in `FixedUpdate` on network ticks (20 Hz, as energy and
fluid). A network moves as many items a second as the `throughput` of
its slowest tube (worked out when the networks change, in `ItemRates`),
as credit it saves up a tick at a time (in thousandths of an item,
handed out per tick as energy is, so a rate holds exactly over each
second; at most a tick's worth and one item, so an idle network doesn't
burst), and at
most 64 in one tick. Each item goes from a giver (`output`, `store`) to a
taker (`input`, `store`), never `store` to `store` nor into its own slot
group; inputs before stores; round-robin between givers and between
takers, with a cursor per network kept in `NetworkMap<ItemRouting>`,
which merging keeps one network's of and splitting resets (it holds no
items). Item ports can have a `filter` by item id or material (and
material tag). A recipe machine's input groups take only what one of its
type's recipes takes, as they do by hand (#119): an item no taker takes
stays where it is and the giver's next item is tried, so a furnace whose
output tube also runs past a second furnace sends its glass on to a
crate rather than jamming the second with it (provisional, for the user,
with #119's rule). With instant transport, nothing is ever in a tube, so
nothing is dropped, returned or held when one is broken or the network
freezes: a frozen network just moves nothing. A tube's `capacity` (1)
is unused. The creative item source is a machine with `emits`, a slot
group kept full out of nothing; it's the only thing that makes items
until recipes (#115).

**What's wrong with it.** You can't see items move, and a long tube is as
fast as a short one: distance means nothing. The rate is the slowest
tube's, not the narrowest path's between the two machines, so one iron
tube anywhere on a gold network slows all of it. Round-robin is per item,
not per stack, which is fine at a few items a second and fair, but
moves at most 64 a tick. Routing is greedy: a giver's first slot that
some taker accepts goes first, and there are no priorities beyond inputs
before stores. Items aren't tagged, so filters go by item id and
material only.

**What should replace it.** Items travelling through tubes at a speed,
as entities or instanced meshes, with a tube's capacity limiting how many
are in it at once, and then deciding what happens to those in a broken
tube (#124). Port priorities, and path-based rates, if networks
grow large. Item tags, for filters, once items have them.

## Fluid networks are one shared tank of one fluid, and pumps are stats (2026-11-26)

**Context.** The fluid solver (#113) moves fluid between machines on
pipe networks, and pumps and outlets join them to the world's fluid. The
issue left open whether a network holds one fluid, what pipes too cool
for a fluid do, and how a tank shows how full it is.

**Decision.** A network is one shared tank (`FluidNet`), as big as its
pipes' capacities together, with no pressure or flow along the pipes, as
Factorio 2.0 does. Each network tick (20 Hz, as energy's), outputs give
to it, inputs take from it, and stores make up the difference (give when
inputs go short, take what's left otherwise); what goes in and what comes out are each limited
by the slowest pipe's throughput. **One fluid per network**: it takes in
only the fluid it holds. Networks can't be kept from joining (the
connection properties know nothing of what's inside), so two holding
different fluids that join hold both, take nothing in and only give out,
first fluid first, until one is used up. That matches the world today,
where two fluids that meet just stop (#85 is still open); if #85 makes
them react, a mixed network could react the same way. **Temperature**: a
fluid block's `temperature` stat and a pipe's `max_temperature` stat (from
its metal, #106), in kelvin; a network takes in no fluid hotter than its
coolest pipe, but keeps what it holds if a cooler pipe joins it later.
**Pumps and outlets** are machines with a `pump_rate` or `outlet_rate`
stat, working on their first tank and the voxel their `facing` names. A
pump reaches through the fluid and air in front of it (64 voxels, 8
blocks, not upward), so it drains the film a pond leaves on its bed,
which flow never brings to it. **Tanks** show how full they are through
a `level` property (and `fluid`), which the engine sets from the first
tank, so the block draws it with ordinary variants and textures.

**What's wrong with it.** Throughput ignores distance: a long thin pipe
carries as much as a short one, and fluid crosses a network in a tick.
Stores drain the pipes every tick, so pipes hold fluid only on the way
through. Marking pumps and gauges by stat and property names is
convention, not schema. A pump can drink from a whole sea, one voxel at
a time, waking flow across it (no reservoir, #96).

**What should replace it.** Throughput falling with distance from the
source, if networks feel too instant. Pump and outlet as a `machine`
field, or scripts (#118, #122), once machines have behaviour in data. A
pump drawing from a settled body's level as a reservoir (#96).

## Pipe contents are written into block entities just before their chunk is saved (2026-11-26)

**Context.** #111 left what conduits held to the solvers: an unloading
chunk's conduits are handed to the solver as lost (`removed`). Fluid in
pipes must survive unloading, quitting and breaking pipes.

**Decision.** A network's fluid lives in its `FluidNet`, not per pipe,
and every conduit now has a block entity (as the wrench will need). Just
before chunks are saved, streaming says which (`ChunksSaving`: those
unloading, and whether an autosave or exit saves the loaded ones), and
systems in `BeforeChunkSave` write into block entities: each unloading
pipe gets its exact share of its network by volume (`PipeFluid`, saved
as `pipe`), taken out of the network; on an autosave or exit every pipe
gets a copy of its share, which the network keeps. A pipe loaded with
fluid gives it back to its network, once that's known, and is emptied.
A broken pipe takes its share out before the network splits and pours it
into its voxel. So a split never has anything to lose, and the solver's
`split` ignores `removed`. A share written as a copy is harmless: a
loaded pipe gives back only what it was loaded with.

**What's wrong with it.** Fluid moves between pipes of the same network
as a chunk unloads and loads (it's shared out by volume, not kept where
it was), which is invisible now there's no pressure. A crash between
autosaves loses what went into networks since the last one, as it does
any edit. Writing every pipe's share on an autosave costs a pass over
every fluid pipe loaded (only changed ones are rewritten). A pipe broken
where a block takes its place, or holding a second fluid, loses that
fluid (counted in `FluidNetworkStats::lost`).

**What should replace it.** Networks that remember their conduits in
unloaded chunks (#127, as #123 decided) keep their whole pool and need
this only for saving. Energy
and items in transit (#112, #120) can use `BeforeChunkSave` the same
way.

## Machine screens are laid out from the definition, and inputs take only what a recipe takes (2026-11-26)

**Context.** Using a machine (#119) opens a screen showing its slots,
tanks, energy and recipe progress, and the player moves items in and
out. Mods' machines need one without writing UI, and later layers add
other things right click does on a block (the wrench, #121; scripted
blocks' `on_used`, #122).

**Decision.** The screen is generated from the `machine` definition: a
row of slot groups (a recipe machine's inputs, its progress bar and
status, then its outputs and the rest), a vertical bar per tank in its
fluid's fog color and one for energy, each under its capitalised name,
and the player's inventory list under it, as in the inventory screen.
The player's items go into a recipe machine's **input groups, but only
items one of its type's recipes takes** (`Content::slot_group_takes`, the
rule tubes follow too, #120), or any group of another machine; any slot
gives its items back. Taking anything, as the screen first did, let a
player or a tube put glass into a furnace, where it sat for good: two
furnaces chained on a tube line jammed, the second full of the first's
glass. *Provisional, for the user to confirm*: the same rule for hands
and tubes is the conservative pick; the other way is letting hands put
anything in (a player can take it back out) and holding only tubes to
it. Clicks work as in the inventory (one,
all with Shift, half with right click), and an inventory entry dragged
onto the machine moves them all; dropped on a slot, they go into that
slot's group first, so a group no recipe names (one a script reads) can
be filled by hand (any group but a recipe machine's outputs;
provisional, for the user to confirm: the other way is a filter in the
definition saying which groups hands fill). Fluid shows in litres (a
block of fluid is a cubic metre). Using a block is a right click handled in
`BlockUseSystems`, before placing, which takes the click so nothing is
placed; **Shift with right click places** against a machine instead.
Only machines with slots, tanks or recipes have a screen: one with just
an energy buffer (a powered lamp, a battery) would show a single bar, and
taking right click from it would make placing against it need Shift, so
it has none (provisional, for the user to confirm; a battery's charge
shows in the headless `network` command for now, a tooltip later).
The screen closes when the machine's block entity goes (broken,
replaced, or its chunk unloaded). A slot group of more than three rows
(a slot group can have up to 256 slots) scrolls, three rows tall, so the
screen fits in the window and stays the height it is for a furnace
(provisional, for the user to confirm: the other way is a taller screen
for big groups, scrolling only past what fits).

**What's wrong with it.** An input takes an item if any recipe of the
machine's type takes it, even one this machine can't run, or one whose
other inputs aren't there, so a furnace can still hold something it
never starts on. Machines without recipes take anything in every group.
A machine's slots can't be
dragged, nor bound to the hotbar with the number keys. Fluid and energy
can't be moved by hand (no buckets or batteries in slots). One generated
row is fine for a few groups but doesn't suit a machine with many (it
wraps onto more lines, and scrolls once those don't fit), and a mod
can't lay one out itself. Other mods' "use"
actions and the screen compete for the same click with no priority
beyond system order.

**What should replace it.** Slot filters in the definition, for
machines without recipes and for inputs narrower than their recipes;
dragging
and number keys on machine slots; item containers of fluid and energy;
optional UI layouts in mod files (or from scripts, #118) that map onto
the same widgets; and a single "use" event with a priority order once
the wrench and block scripts need it.

**When to revisit.** With the wrench (#121) and block scripts (#122),
when mods add machines with many slot groups, or when fluid containers
arrive.

## Recipes take their inputs when they start, and stop rather than slow down (2026-11-26)

**Context.** Recipe machines (#115) turn what's in their slots and tanks
into other things over time, using energy, and generators turn fuel into
energy. Whatever fills them (a player through a screen, #119; tubes,
#120; pipes, #113; scripts, #118) must work the same, and their state
must save with the chunk.

**Decision.** Recipes are data (`recipes/<name>.json5`), each with a
plain `type` that machines name in `machine.recipes`: types aren't
namespaced, so any mod's recipes run in any machine of the type. A
recipe with `materials` is baked once per combination, and combinations
naming something that isn't there are skipped (so coal, an ore with no
ingot, makes no smelting recipe). A machine tries its type's recipes in
load order on each network tick (20 Hz) and starts the first whose
inputs it holds, whose outputs would fit right now, and whose first tick
it can pay for; it **takes the inputs when it starts**, so a running
recipe can't be starved by something taking its input away, and the
progress part (`RecipeProgress`: recipe id and ticks done) is all that
needs saving. Energy is drawn (or, for a generator, made) a tick's exact
share at a time from the machine's own buffer, so a recipe uses exactly
its energy; without it, or without room, the machine **waits** where it
is rather than running slower. It stays active (lit) while it holds a
recipe, waiting or not, so short power doesn't make it blink; this is
provisional, for the user to confirm (going dark after a second or so of
waiting would show a stalled machine, but a full generator feeding a
small load runs one tick in fifty and would still blink). Outputs go in
when every tick has run, or it waits until they fit. Generators are
recipe machines with an `{ energy }` output, so energy enters the world
at them; the base game's combustion generator burns logs, planks, coal
ore (rock and all, as breaking ore gives the stone item) and lava. The
headless `put` command fills slots and tanks, a fluid going to a tank
and anything else to a slot group when a machine has both by one name.

**What's wrong with it.** Taken inputs are lost if the machine is
broken mid-recipe (as everything in a machine is, for now) or the recipe
disappears from the mods. A machine waiting for power or room looks the
same as one working, and a furnace whose output is full stays lit until
it's emptied. Outputs are checked against
the slots at the start and again at the end, so something filling the
output meanwhile leaves the finished recipe waiting. The first recipe
that fits wins, so two recipes with overlapping inputs depend on load
order. Progress changes every tick, so a running machine's chunk is
saved again every frame it's marked edited. Coal ore burning as a block
of stone is a stand-in for a coal item. Recipes have no chances, tools
that aren't used up, or by-products that are optional.

**What should replace it.** Recipe machines that run at a fraction of
their speed on a fraction of their power (with the screen showing it),
dropping their held inputs when broken, a coal item from coal ore, and a
priority or explicit selection (a recipe picked on the machine's screen)
where inputs overlap. Save progress less often than every tick if it
shows up in saving.

**When to revisit.** With machine screens (#119), with item networks
(#120) feeding machines faster than they run, and when mods add
overlapping recipes.

## Power is one pool per network, capped by its weakest wire, in joules and watts (2026-11-26)

**Context.** Before the energy solver (#112), the spike (#110) had to
decide how much electrical detail the game has: one pool per network,
per-path limits (a max-flow solve), voltage tiers (GregTech,
IndustrialCraft), or loss over distance; what an overloaded wire does;
the units; and the network tick rate.

**Decision.** One pool per network, as the issue recommended. Each
network tick, producers offer (up to their `max_out` and what they
hold), consumers ask (up to their `max_in` and their room), and storage
does both: consumers are served from producers first, then from storage;
what producers have left charges storage (`balance` in
`src/network/energy.rs`). What consumers *need* to run this tick (their
`use`, and an `EnergyNeed` set by whatever runs them: a recipe's share
of its energy), less what its buffer already holds, is served first;
only what's left tops their buffers up, in proportion to the room each
has. A machine that can pay for its tick from its buffer needs nothing,
so one that can't run on its own is served before it, rather than power
going into a furnace holding 5 kJ while an empty one never starts. In a
shortage the needs are served
whole, the one needing least first, until the power runs out
(`serve_smallest_first`): some machines run and the rest wait, dark.
Shared in proportion to what each asked, as the solver first did, a
long line of lamps (20 W) beside furnaces (500 W) on a 1000 W wire got
under a joule a tick each, which rounds to nothing, so every lamp went
dark and the furnaces ran in bursts: a network using all its power
looked dead. Served whole by what each asked rather than what each
needed, as it did next, a machine filling its buffer took the power one
that was working needed: two electric furnaces (#115, 500 W recipes) on
a 1000 W generator, one asking its `max_in` of 1000 W to fill a 10 kJ
buffer, left the other waiting about 20 seconds, then dropping out
between recipes. *Provisional, for the user to decide*: needs first,
served whole and smallest first, then top-ups shared, is the
conservative pick; the other ways are keeping it all proportional (with
fractions carried over ticks so small loads aren't rounded away),
capping a recipe machine's demand at its recipe's rate plus a little
headroom, or a priority set per machine. Everything moved on a network in a tick is
capped by its *throughput*, its weakest conduit's: one iron wire (170 W)
limits a copper network (1000 W) everywhere. An overloaded wire caps
throughput; nothing burns out. No voltage tiers and no loss over
distance.

Units are **joules and watts**, with game-scaled numbers rather than
realistic ones (as Factorio does): #114's integer energy units are
joules and its per-second rates watts, so nothing changed but the
names. Real names give screens (#119) prefixes for free (kJ, MW) and
need no explaining, and nobody minds that a lamp is 20 W and a battery
holds 100 kJ. Energy is held only in machines' buffers: wires hold none
(capacity 0), so merging and splitting moves no energy and a chunk
unloading takes none with it.

The network tick is fixed at **20 Hz** (`TICKS_PER_SECOND`, in
`src/network/tick.rs`), counted out of `FixedUpdate`'s time by the
network clock (`NetworkClock`), which every solver runs on, fluid and
item networks included. A rate per second is split into whole joules per tick that
add up to exactly that rate over each second (`per_tick`). Energy enters
the world only at machines with `generate` (the creative generator) and
leaves only at machines with `use` (the powered lamp), which are active
exactly while their buffer pays for the tick. A machine on two networks
keeps to its rates over both.

**What's wrong with it.** A thin wire is a bottleneck for the whole
network, not where it is, so a network that's one long copper trunk and
one iron spur is limited to 170 W everywhere. Wires have nothing to do
with distance or layout, so where generators go doesn't matter. Batteries
on one network don't even out between themselves, and a battery between
two networks passes energy through in one tick. In a shortage the
smallest machines always win, whatever matters more to the player, and
which of equal ones run goes by position; the one the power runs out on
gets part of its demand and may turn on and off (relighting each time).
`use` is all or
nothing: a machine can't run slower on less power. Every buffer that
changes is saved again on that frame (the block entities' way), and the
throughputs are worked out again for every energy network on any frame a
conduit changes. The powered lamp's item icon is drawn from its default,
unlit state, so in the inventory it looks like grey stone; an item can
only swap that for a flat `texture`, not a lit 3D block (deferred: a way
for a block's item to name the state its icon shows).

**What should replace it.** If players want to plan wiring: per-path
limits (a max-flow solve when the network changes, cached), then loss
over distance from `conductivity`. Voltage tiers only if the game grows
into big tiered machines, with burning out as a choice per network kind.
A rolling average or a little hysteresis on machines' active states so
short power doesn't flicker, and recipes (#115) that slow down rather
than stop.

**When to revisit.** When players build networks big enough that one
thin wire is a surprise, when machine screens (#119) show power, or when
power shows in frame times.

## Machines hold items in slots, not by weight (2026-11-26)

**Context.** The player's inventory has no slots or stack sizes, only a
weight limit (#87). Machines (#114) need somewhere to keep items too, and
#87 asked whether they should work the same way.

**Decision.** Machines have named groups of slots (`slots: { input: 1,
output: 1 }`), each slot holding up to `SLOT_SIZE` (64) of one kind of
item (an `ItemStack`, the inventory's own type), or one of a `unique`
item. A recipe machine's input and output are then fixed places that
recipes (#115), item networks (#120) and machine screens (#119) can name,
and a full output stops a machine in an obvious way. Weight plays no
part in a machine.

**What's wrong with it.** Two ways of holding items: a crate holds 9×64
of anything, so 576 gold blocks (11,000 tonnes) as easily as 576 pebbles,
while the player is limited by weight. One slot size for every machine
and item.

**What should replace it.** Once #87 settles the weight scale: a slot
size per machine (or per group) in the `machine` definition, and
possibly a weight limit on storage blocks (crates) as well as slots.

## Machine ports are worked out from the state, and indexed whole (2026-11-26)

**Context.** Machine definitions (#114) give each block state ports
(faces, network kind, role), an energy buffer, tanks and slot groups.
Solvers (#112, #113, #120) need, for each network, its machine ports and
their roles, cheaply, every tick.

**Decision.** Ports are baked per block state (`Content::machine`), faces
turned by the state's `rotate` and the relative ones resolved from its
`facing` property. `Content::has_port` reads them, so `Content::accepts`
(and so connection properties and networks) join conduits to port faces
only. The `Ports` and `MachineActive` block entity parts aren't saved:
they're made from the voxel's state when the block entity is (parts can
now say `SAVED = false`), and follow the state when it changes. Stored
energy, tanks and slots are saved; their sizes come from the state. The
index of each network's machine ports (`MachinePorts`) is rebuilt whole,
in `Update`, on frames when the networks or block entities change,
reading each network's port faces and the block there. Energy is counted
in whole integer units, rates per second, with no physical meaning yet
(#110 decides the units). A machine with a `facing` faces whoever
places it (`place: { from: "look", values: { north: "south", ... } }`),
so you see its front as you place it, as in Minecraft. *Provisional, for
the user to confirm*: the other way, facing the way you look, aims a
pump or an outlet at what you're looking at, but shows you every
machine's back.

**What's wrong with it.** Rebuilding the index costs time in proportion
to every port in the loaded world on any frame a conduit or block
entity changes (mining a block anywhere counts). It's small (a lookup per
port) at the scale of a few thousand ports. A machine's ports may
differ between its states (active and idle, or turned by a script's
`set_state`): since #122 a property update that changes what a voxel
offers its sides rechecks its neighbours' connections
(`around_updates`), so conduits follow. A machine facing up or down
takes west as its left.

**What should replace it.** Update the index per network from
`NetworkChanged` and the voxels that changed, once worlds have many
machines.

## Networks follow connection properties, live only in loaded chunks, and split by search (2026-11-26)

**Context.** Conduits (wires, pipes, tubes) form networks (#111) that
the solvers (#112 energy, #113 fluid, #120 items) run. The networks
must stay right as blocks change and chunks load and unload, and what a
network holds (fluid, energy, items) mustn't be made or lost as it
merges and splits.

**Decision.** A conduit is a block with a `conduit` (network kind,
optional group, capacity and throughput as attributes). Its connection
rule is derived from it (`connections: true`): two conduits join when
they're on the same kind and group, decided in `Content::accepts`, with
`Content::has_port` deciding machine ports (filled in by #114). The
`network` module doesn't decide joins itself: a conduit's six edges are
read from its connection properties (joined to a conduit joined back is
a link; joined into an unloaded chunk is open; joined to anything else is
a port), so anything that changes the sides, like a wrench (#121),
changes the networks. `Networks` (a resource) is updated in `Update`
after the connection properties, for the voxels around every edit and
property update and for chunks that load or unload. A new link merges
the two networks, smaller into larger. A lost link or conduit starts a
search from each conduit that lost a link, all in step; a search that
runs dry without meeting another is a separate part and moves to a new
network, and the last search left keeps the id unexplored. A network
with an open face is frozen. Networks are rebuilt from chunks as they
load; nothing is saved. Solvers keep per-network state in a
`NetworkMap<T>`, which applies the `NetworkChanged` messages (merged;
split into parts with each part's share, plus the share of the conduits
lost) right after the networks update; `divide_by_share` divides whole
units exactly, by capacity (or by count where nothing has capacity).

**What's wrong with it.** Breaking the middle of a long line searches
both halves: about 2 ms for a 20,000-pipe line (measured in a test
build), which is fine for anything a player builds by hand but grows
with the network. A whole chunk unloading moves each conduit in it
separately. When a chunk unloads, what its conduits held is handed to
the solvers as the lost share (`removed`), and nothing keeps it for when
the chunk loads again, so a solver that holds amounts in conduits must
save them itself (in block entities, #109) or accept the loss; frozen
networks keep their own share. Every network a new conduit makes is
merged at once, so loading a chunk full of conduits makes and merges a
network per conduit (about 1 µs each). Ports are any joined face that
isn't a link, with nothing about what the machine does there. A
conduit's item icon is drawn from its default state, joined nowhere, so
it's only the core: a wire's is a dot, a pipe's and a tube's a grey
cube. Deferred: an icon state per item (a straight run, say), or icons
drawn from the item's own model.

Each solver works out its networks' weakest conduit itself, when the
networks change: energy (`find_limits`, #112), items (`measure_rates`,
#120) and fluid (`FluidLimits`, #113), each with its own copy of the
lookup from a position to its chunk's voxel, as do other systems that
read voxels by position. Deferred: one helper in `network` giving a
network's least of a conduit attribute (throughput, `max_temperature`),
kept per network until the networks change, for all three, and one
shared voxel lookup. Any change to the networks also has each of those,
and the index of machine ports (`index_ports`, #114), work out every
network again, however few changed: a lookup per conduit or port in the
loaded world per change. Deferred too: a set of the networks each
change touched (from its `NetworkChanged` messages and the voxels it
changed), so each refresh redoes only those.

**What should replace it.** If searches get slow, keep a spanning
forest (a dynamic connectivity structure) so most breaks are found
without a search, or spread a search over frames with the network frozen
meanwhile. Unloading should hand the unloaded share to block entities
and loading take it back, or networks should remember their conduits
in unloaded chunks (#127, as #123 decided). Ports get roles and rates from machine definitions (#114).

**When to revisit.** When the solvers land (#112, #113, #120), if a
break or a chunk load shows in frame times, and with #123.

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
## Block entities: kind is the block, saved forms kept as parts change (2026-11-26)

**Context.** Block entities (#109) hold per-voxel data beyond the block
state: machines' buffers (#114), a conduit's blocked faces (#121), scripts'
data (#118). Each needs a kind, saved with it, and a way for features to
add data and save it.

**Decision.** A block entity's kind is the id of the block that has it,
and a block has one when its definition has `block_entity`. Features add
*parts*: components implementing `BlockEntityPart` (`src/block_entity.rs`),
registered with `register_block_entity_part`, each saved under a key as a
plain `Value` with its own version. The saved form of every part is
written to a `SavedParts` component whenever the part changes, rather than
read from the components when the chunk saves, because the streaming
systems that save chunks can't take a query over arbitrary components
next to their mutable resources (in Bevy 0.20 resources are components,
and `EntityRef` conflicts with them). Lifecycle follows `VoxelChanged`:
a different block removes the old one and makes a new one. The base game
has no block with a block entity yet; the tests use a test mod.

**What's wrong with it.** A part that changes every tick is re-serialised
every frame it changes, and its saved form is kept in memory twice. A part
changed after `BlockEntitySystems` on the frame the game exits isn't in
that save. Kinds can't be shared between blocks, and data for a block
that changes into another block (a furnace becoming a different furnace)
is lost.

**What should replace it.** Saving that reads parts directly when the
chunk is saved (an exclusive save step, or queries built for the
registered parts), so nothing is serialised until it's needed. Named kinds
separate from blocks if blocks ever need to hand data on to each other.

**When to revisit.** When machine simulation (#114, #112) shows up in
frame times, or when a feature needs data to survive a block swap.

## Mod scripts are Luau, run through mlua (2026-11-26)

**Context.** Mods need scripts for what data can't describe (#105).
The spike (#116) prototyped three runtimes outside the repo, each running
the same `on_tick` (read a machine's stored energy, give some, set a
state) for 1,000 machines, 200 ticks, release build, Apple M3 Max:

| | 1,000 `on_tick`s | per engine call | clean build added | binary added |
|---|---|---|---|---|
| Luau (mlua 0.12) | 0.38 ms mean, 0.41 worst | 35 ns | 14 s wall, 31 s CPU | ~2 MB |
| Rhai 1.27 | 0.95 ms mean, 2.4 worst | 94 ns | 13 s wall, 39 s CPU | ~4 MB |
| wasmtime 40 (hand-written WAT guest) | 0.03 ms mean | 4 ns | 36 s wall, 290 s CPU | ~9 MB |

Luau's figures are with the instruction-budget interrupt on and the
machine passed as userdata borrowed for the call (`Lua::scope`), as the
engine will; 'static userdata made once is 0.22 ms. At 20 Hz that's
under 1% of a core for 1,000 scripted machines. A Luau state with the
script and 1,000 userdata uses about 650 KiB. The runtime as built
(`src/script/`: a scope and a budget per call, a globals table per
script, in a dev build) takes 0.58 ms mean and 1.0 ms worst for 1,000
calls of a hook that fills a tank.

**Decision.** Luau through `mlua` (`features = ["luau", "send"]`). It has
a sandbox built in (read-only globals and libraries), an interrupt for
budgets and a per-state memory limit; scripts reload by just loading the
new text; modders need no toolchain, and Lua is what game modders know.
Errors read well: `mods/m/scripts/broken.luau:3: Expected identifier when
parsing expression, got ')'`. Luau's C++ is compiled by `luau-src` once
(it's a dependency, so it never rebuilds Bevy) and linked statically,
so `cargo dist` still makes one binary: on macOS it needs only
`libc++`, which every Mac has (checked with `otool -L` on the
prototype), on Linux `libstdc++` like any C++ program, and on Windows
the MSVC runtime the Rust binary already uses. Each mod gets its own
Luau state (own globals, own memory cap) and each script its own
globals table within it. The runtime is `src/script/`; how it looks to
modders is `docs/scripting.md`.

Rhai is pure Rust but 2.5 times slower per tick and per call, with worse
worst cases, and few modders know it. Wasmtime is ten times faster than
Luau, but that's a hand-written WAT guest with no strings or tables; a
real guest pays for marshalling. It costs 290 s of CPU to build, 9 MB,
and every modder a compiler toolchain, and hot reloading means
rebuilding the module.

**What's wrong with it.** The budget counts Luau's interrupts (loop back
edges and calls), not instructions, so a step is a rough unit: a long
straight run of code with no loops or calls costs one step, and a
library call such as `string.rep` with a huge count can take far longer
than its one step (a loop of 200 `table.sort`s of a 200,000-entry table
took 1.3 s in 402 steps). So each call also has a wall-clock limit
(`ScriptSettings::time_limit`, 20 ms), read every 16 steps; it's a
safety net, not deterministic, and one library call between two reads
can still overrun it (bounded by what the memory cap lets it build).
Memory is capped per mod, not per script, so one
script can use up its mod's share. Determinism across platforms is
likely but unproven: Luau numbers are doubles and `math.sin` and friends
come from the platform's C library, which can differ in the last bit;
`math.random` is seeded the same everywhere. Each mod's state starts with
Luau's libraries (about 400 KiB), so a hundred scripted mods cost 40 MB.

**What should replace it.** If the server needs bit-identical results,
replace the transcendental math functions with Rust implementations. If
step counts turn out too coarse, charge library functions steps for
the work they do (by wrapping `table.sort`, `string.rep` and the like),
so the deterministic limit covers them too. WASM stays an option
for heavy, performance-critical mods later, beside Luau rather than
instead of it.

**When to revisit.** With the server split (#50), if mods' scripts show
up in frame times, or if modders ask for another language.

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

## Fluid surfaces slope between corners averaged from the same fluid (2026-11-26)

**Context.** A fluid voxel is drawn at one of 16 heights by its amount.
Its top slopes: each corner is at the average height of the voxels of the
same fluid around it, or a full block beside a column (`FluidTop`, in
`src/world/fluid_top.rs`). Swimming and the fog inside fluid use the same
corners.

**Decision.** Only the same fluid counts toward a corner; air and other
blocks around it are left out. The top is drawn as two triangles, but
swimming and fog blend the four corners (bilinear), which can differ from
the drawn triangles by up to the slope inside one voxel. The sloped top
keeps a straight-up normal, so its lighting doesn't see the slope.

**What's wrong with it.** Water at the edge of a drop doesn't dip toward
it, as it would if air beside a corner pulled it down. And when a thin
voxel empties, it stops counting toward its corners, so they jump up to
the average of what's left: a draining edge can step up a little before
going on down. Counting air as empty (height 0) would fix both, but on
its own it turns a lone block of water into a low tent; Minecraft weights
nearly full voxels far more heavily to avoid that. The eyes can be a hair
above or below the drawn surface right at it. A slope isn't shaded as
one.

**What should replace it.** Count open air beside a corner as an empty
voxel, weighted so a lone full block stays nearly full and flat pools
against walls stay flat; follow the drawn triangles exactly in
`FluidTop::at`; give a sloped top its real normal.

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

## Light filters are per block and dim every colour alike (2026-11-26)

**Context.** Leaves, glass, water and plants are drawn see-through
(`render` in `docs/modding.md`) and light ignores their covered sides
(`State::blocks_light` in `src/content.rs`). Without anything more, under
a tree was as bright as open ground and a deep lake's floor was in full
sky light.

**Decision.** A block's `light_filter` (water 2, leaves 1, glass and
plants 0) is how many levels light loses coming into it, on top of the
level per block, for block and sky light (`LightTable::entering` in
`src/light/propagate.rs`). Sky light straight down keeps full strength
only through unfiltered blocks, as in Minecraft. The sky that comes into
the loaded chunks from above (`OutsideSky`) is full strength; only loaded
voxels filter it.

**What's wrong with it.** The filter is the block's, not the state's or
the fluid amount's: a thin film of water dims light as much as a full
block. It dims red, green, blue and sky alike, so water doesn't tint what's
under it blue. And a lake or forest above the loaded chunks doesn't shade
what's below them, since the sky from outside doesn't know about it.

**What should replace it.** A filter per channel (a tinted filter, so
water passes more blue), scaled by how full a fluid voxel is, and an
`OutsideSky` that knows the columns above the loaded area (see "Sky light
comes in above whatever's loaded").

## Translucent faces cull by the texture where they meet (2026-11-26)

**Context.** A translucent block hid every translucent face of the same
block next to it, so with stained glass as one block whose color is a
property, red glass beside blue lost the seam between them (#82). The
options were to compare states, to compare the faces' textures, or to let
a block name the property that splits it (`cull_by`).

**Decision.** Compare textures: two faces of the same translucent block
that meet hide each other only when they have the same baked tile. It
needs nothing in the mod format and works whether a material or a plain
property picks the color, while states that only differ in something
invisible there (a connection, a rotation that keeps the texture) still
join into one pane. It's decided per side of a state, not per face: each
state bakes, for each side, the one tile of every face it culls against
that side (`SideTexture`). A side whose faces have different tiles
matches nothing, and a covered side with no faces matches anything.

**What's wrong with it.** Connected textures (a frame that opens toward a
neighbour) would give the two faces different tiles, and their seam would
show. Two different blocks with the same texture (glass and a future
glass slab) never hide each other. A side made of several faces with
different tiles draws them all against the same block.

**What should replace it.** If connected textures come, let a block say
what splits it (`cull_by: "color"`), or compare a per-state "look" key
that ignores connection properties, beside the texture rule.

**When to revisit.** Connected glass, or translucent blocks whose sides
mix textures.

## Translucent faces are sorted per chunk, on the CPU (2026-11-25)

**Context.** Translucent faces (glass, water) blend with what's behind
them, so they must be drawn back to front.

**Decision.** Each chunk's translucent faces are one mesh on a child
entity, which Bevy sorts against other meshes by its centre. Inside it,
`sort_translucent_faces` (`src/meshing.rs`) orders the faces farthest
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
texture's combinations into finished tiles. The base game has 733 block
states, 232 item states and 239 tiles. Baking stays even when tiles run
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
(`src/worldgen/shape.rs`). The land near the origin is kept to the old hills.

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

## Plants say what they stand on in data; losing support drops nothing, and loading checks nothing (2026-11-26)

**Decision.** A block's `support` (in its JSON5) lists what it can stand
on, and the engine enforces it (`src/support.rs`, with the placement check
in `block_edit` and the harvest in `mining`), rather than plant scripts
(`on_placed`, `on_neighbour_changed`). A script can't refuse a placement:
the item would be used up and the plant vanish a tick later. A plant that
loses its support gives its item only to a player mining the block under
it in survival; broken any other way (a script, a `fill`, an inventory
too full) it gives nothing. Nothing is checked when chunks load.

**What's wrong with it.** There are no dropped items, so a plant knocked
off by a script or a machine is lost. A plant whose support was edited
while the plant's chunk wasn't loaded (only possible at the edge of the
loaded area) keeps standing when it loads. Support is only ever the block
below: nothing hangs from a ceiling or clings to a wall.

**What should replace it.** Dropped item entities, so every break drops
what it gives, wherever it comes from; a support check on chunks as they
load, next to the connection check; and `support` naming a side, for
hanging and climbing plants.

**When to revisit.** With dropped items, or with the first plant that
hangs or climbs.
