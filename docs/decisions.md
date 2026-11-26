# Decisions to revisit

Choices made deliberately simple for now, with what's wrong with them and
what should replace them. Newest first.

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

The network tick is fixed at **20 Hz** (`TICKS_PER_SECOND`), counted
out of `FixedUpdate`'s time, and fluid and item networks should run at
the same rate. A rate per second is split into whole joules per tick that
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
port) at the scale of a few thousand ports. Ports must be the same in a
machine's active and idle states for conduits to follow, as the
connection properties don't re-evaluate on a property update (only on
edits). A machine facing up or down takes west as its left.

**What should replace it.** Update the index per network from
`NetworkChanged` and the voxels that changed, once worlds have many
machines. Re-evaluate neighbours' connections on property updates if
ports ever change with state.

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
and loading take it back, or networks should run unloaded as numbers
(#123). Ports get roles and rates from machine definitions (#114).

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
