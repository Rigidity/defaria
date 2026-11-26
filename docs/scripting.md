# Scripting

Mods describe what blocks, items and machines are in JSON5
([modding.md](modding.md)). Scripts are for what data can't describe: what
something *does*. They're written in [Luau](https://luau.org), a fast,
sandboxed dialect of Lua 5.1 with optional types (why Luau: see
`docs/decisions.md`).

Blocks and items name a script with `script` (see
[modding.md](modding.md#scripts)), and its hooks run as things happen to
them. A machine is a block, so it names its script the same way, and has
hooks and an API of its own besides ([Machines](#machines)).

## Where scripts go

Each mod's scripts are `.luau` files in its `scripts/` folder. A script's
id is its mod's id and its path there, without the extension, like every
other id: `scripts/furnace.luau` in mod `defaria` is `defaria:furnace`,
and `scripts/machines/pump.luau` is `defaria:machines/pump`. In a mod's
files, `"furnace"` means `<this mod>:furnace`.

A `.luau` file anywhere else in a mod is reported and ignored.

## Hooks

A script implements **hooks**: global functions whose names start with
`on_`, which the engine calls when something happens. Scripts don't run
every frame, only when a hook is called; a script that doesn't define a
hook costs nothing for it.

```lua
-- scripts/counter.luau, a machine's script
local count = 0

function on_tick(m, dt: number)
	count += 1
	log.info("ticked", count, "times")
end
```

Every script in every mod loads when the game starts: its top level runs
once, defining its hooks (and anything else it keeps). Variables at a
script's top level keep their values between calls, but they're shared by
everything that uses the script and they're lost when the scripts reload,
so state that belongs to one block goes in its saved data ([Data](#data)).

The hooks themselves and what they're passed are below, for
[blocks and items](#block-and-item-hooks) and for
[machines](#machines).

## Block and item hooks

A block's script (`script` in its file) can define:

| Hook | When |
|---|---|
| `on_used(block, player)` | A player used the block: right click on it. Using a block whose script defines this doesn't place a block against it; sneaking with something in hand does. On a machine it takes the click from the machine's screen, and with a wrench in hand the wrench takes it. `player` may be nil for uses that aren't a player's. |
| `on_placed(block)` | The block was put in its voxel, by anyone: a player, a script, a scenario. |
| `on_broken(block, old)` | It was replaced by another block (air, when broken). `block` is the voxel as it is now, and `old` what was there: `{ id, name, state }`. Its block entity, and with it its `data`, is already gone (a machine's too: it gets a block handle, not its machine handle). |
| `on_neighbour_changed(block, face)` | The block on that face of it (`"up"`, `"north"`, ...) changed, or its properties did. |
| `on_random_tick(block)` | Now and then, at random: about once a minute on average. For things that grow or spread at their own pace. |

An item's script can define `on_use(item, player)`: a player used the
item in hand (right click) and the click didn't use the block it's on
(a block whose script defines `on_used`, a machine's screen, a wrench on
a conduit). Returning `true` uses one of it up, in survival.

```lua
-- scripts/switch_lamp.luau: using it turns it on and off.
function on_used(block, player)
	block:set_state({ lit = not block.state.lit })
	block.data.switched += 1
end
```

Hooks run with the simulation (60 times a second), on the tick after
what caused them, and what they change are ordinary edits: they're lit,
drawn and saved like a player's, and run the hooks they cause in turn, a
tick later. A block whose script doesn't define a hook costs nothing for
it. An `on_neighbour_changed` for a face, or an `on_random_tick`, that's
still waiting to run isn't queued again: when it runs, it sees the block
as it is by then. When a block's hook fails, that voxel's script is disabled until the
scripts reload or another block is placed there; when an item's does,
that item's script is.

### Blocks

A hook's `block` is a handle on a block by position:

| | |
|---|---|
| `block.pos` | Its position, a `vector`. |
| `block.id` | Its block's id: `"defaria:stone"`, or `"air"`. |
| `block.name` | Its state's name: `"Granite Iron Ore"`. |
| `block.state` | Its property values, a table: `true`/`false` for the properties whose values are `"true"`/`"false"`, text otherwise (`block.state.rock == "defaria:granite"`). |
| `block.data` | Its block entity's data, a table, or nil for a block with no `block_entity` (see [Data](#data)). |
| `block.light` | The light in its voxel, `{ sky, red, green, blue }`, each 0 to 15, or nil where none is worked out yet. A solid block's own voxel is dark: read the light of the voxel next to it (`block:neighbour("up").light`). |
| `block:stat(name)` | A stat of its state (`hardness`, `weight` or one of the block's `stats`), or nil. |
| `block:neighbour(face)` | The block next to it on that face, or nil if that isn't loaded. |
| `block:set_state(values)` | Sets some of its properties: `block:set_state({ lit = true })`. Returns whether it changed. A property or value the block doesn't have is an error. |
| `block:set(id, values?)` | Makes it another block, in its default state or with some properties set: `block:set("defaria:stone", { ore = "defaria:iron" })`. Returns whether it changed. `block:set("air")` breaks it, dropping nothing. It's an error to put fluid in a voxel or replace fluid (a machine's tanks pour and draw it), or to replace a block that has a block entity (a machine, a pipe, a block with `data`), which would lose what it holds, or to make one from another block, which only placing it does. Plain blocks it may make from air or turn into others (moss spreading over stone), so a script can make or remove what mining a block gives. |

A hook can change only its own block and the six next to it
(`block:neighbour(face):set(...)`); changing anything further is an
error. An item's `on_use` can change the block the player looks at and
the six next to it. Any loaded block can be read; the `data` of one out
of reach is a read-only copy. A position further
than 2^30 from the origin on any axis is an error.

Handles last as long as the hook they were given to. Keeping one in a
variable and using it in a later call is an error.

### The world

`world` is a global, read-only:

| | |
|---|---|
| `world.block(pos)` | A handle on the block at `pos` (a vector, or `x, y, z`), or nil if it isn't loaded. |
| `world.block_def(id)` | What a block is: `{ id, name, properties, script }`, with `properties` each property's values (`properties.lit == { "false", "true" }`), or nil if there's no such block. |
| `world.item_def(id)` | What an item is: `{ id, name, block, script }`, `block` the id of the block it places, or nil if there's no such item. |
| `world.material_def(id)` | What a material is: `{ id, name, tags, stats }`, `tags` a list and `stats` its stats by name (`hardness`, `weight` and its own), or nil if there's no such material. |
| `world.faces` | The six faces' names: `up`, `down`, `north`, `south`, `east`, `west`. |

### Players and items

`player` is a table: `pos` (the eyes) and `look` (the way they look)
are vectors, `mode` is `"creative"` or `"survival"`, `sneaking` a
boolean, and `target` the block they look at (a handle, or nil) with
`face`, the face of it they look at. `item` is `{ id, name, state }`.

### Data

A block with a `block_entity` keeps a table of its own between calls,
saved with the world: `block.data`. It starts as the block's
`block_entity.data`, or empty. Change it as any table; what's in it when
the hook returns is saved, if the hook succeeds (a hook that fails
leaves it as it was). It keeps only plain values: numbers, strings,
booleans, and tables of them with string keys or as lists, at most 32
deep and 16 KiB saved. Anything else (a function, a handle) is an error
when the hook returns.

Variables at a script's top level are shared by every block that uses
the script and are lost when the scripts reload, so anything that
belongs to one block goes in its data.

### Random ticks

Each tick, one voxel at random in each loaded chunk is picked, and if
its block's script defines `on_random_tick`, it runs: so each block is
ticked about once a minute on average. At most 64 run in a tick (picked
at random among those due), so a world full of growing things slows
them all evenly rather than slowing the game. None are picked while more
than a tick's worth of block hooks (1,024) is waiting. Chunks with no such block
in them cost nothing. In headless scenarios, `random-ticks <n>` picks `n`
voxels a chunk instead, to watch growth quickly.

`math.random` is seeded the same way every time the scripts load, so
headless runs repeat.

## Machines

A machine (a block with `machine`, see
[modding.md](modding.md#machines)) whose block names a `script` gets
every [block hook](#block-and-item-hooks), and two of its own:

| Hook | When |
|---|---|
| `on_tick(m, dt)` | Every network tick, 20 times a second, `dt` (0.05) the tick's length in seconds. Machines' ticks are spread over the frames between network ticks (by position), so some run on the network tick, after the networks moved things, and some a frame or two later. Not while any network on its ports is frozen (it reaches into a chunk that isn't loaded), as the solvers skip those too. |
| `on_recipe_finished(m, recipe)` | A recipe machine finished a recipe: its outputs went in. `recipe` is `{ id, type, ticks, energy }`, `energy` the joules it used (negative for a generator's recipe, which makes them). |

They run after the network solvers and the recipe machines on the same
tick, so a script sees what the tick moved, and what it makes or moves
reaches the networks on the next one. Machines tick in order of position,
so runs repeat. A tick's `on_recipe_finished`s share a pool of steps and
time the size of block hooks'; those that don't fit run on the next
network tick. A machine broken or replaced before its hook runs doesn't
get it.

In every hook of a machine, `on_used` and `on_placed` too, the block
handle is the machine's own **machine handle**, `m`: everything a
[block handle](#blocks) has, and

| | |
|---|---|
| `m.energy` | Its energy buffer, or nil if it has none (below). |
| `m.tanks` | Its tanks by name: `m.tanks.fuel` (below). |
| `m.slots` | Its slot groups by name: `m.slots.input` (below). |
| `m.ports` | Its ports, a list of `{ face, kind, role, network, frozen }`: the face (`"north"`), the network kind's id, the role (`"produce"`), the id of the network joined to it or nil, and whether that network is frozen. |
| `m.active` | Whether it's working. |
| `m:set_active(on)` | Sets it working or not, which puts its block in its `active_state` (or back). A recipe machine, or one with `energy.use`, sets this itself every tick, over the script's. Returns whether it changed. |

**Energy** is in joules, rates in watts:

| | |
|---|---|
| `m.energy.stored`, `.capacity`, `.room` | What it holds, could hold, and has room for. Read-only. |
| `m.energy.max_in`, `.max_out`, `.max_generate` | Its rates, from its definition. |
| `m.energy:take(n)` | Uses up to `n` of what it holds (it's spent, as a lamp spends it), and returns how much. |
| `m.energy:generate(n)` | Makes up to `n`, and returns how much: no more than its `max_generate` allows this tick (a watt of it is a twentieth of a joule a tick, spread so a second adds up exactly), nor than it has room for. Only in `on_tick`; in any other hook it makes nothing. A machine with no `max_generate` makes nothing. |

**Tanks** count fluid in thousandths of a block (1000 is a block):

| | |
|---|---|
| `tank.name`, `.fluid`, `.amount`, `.capacity`, `.room` | Its name, the id of the fluid it holds (nil while empty), how much, and how much more it holds. |
| `tank:move_to(other, n?)` | Moves up to `n` (as much as it can, if left out) into another of the machine's tanks (a tank or its name), if that takes the fluid. |
| `tank:pour(target, n?)` | Pours up to `n` into the voxel `target`: a face (`"up"`) or a block handle, next to the machine. Into air, or onto the same fluid, as much as the voxel holds (a block). |
| `tank:draw(target, n?)` | Takes up to `n` of the fluid in the voxel `target`, if the tank takes it. A voxel it empties becomes air. |

**Slots** hold items, up to 64 of one item each (one of a unique item):

| | |
|---|---|
| `slots.name`, `#slots` | Its name, and how many slots it has. |
| `slots:get(i)` | What slot `i` (counting from 1) holds, `{ id, name, state, count }`, or nil. |
| `slots:count(id?)` | How many items it holds, of item `id` or in all. |
| `slots:move(i, to, n?)` | Moves up to `n` (all it can, if left out) out of slot `i` into another of the machine's slot groups (a group or its name), onto slots of the same kind first. |

Each of these returns how much moved.

**Nothing is made or lost.** These go through the same code the
engine's own machines use, and they only move things: fluid and items
between the machine's own tanks and slots, or between a tank and the
world next to it; energy only out of the machine, spent. A script makes
energy only with `generate`, within the `max_generate` its definition
gives it; fluid it can't make at all, and items only as any script can,
by `set`ting a plain block that's then mined (that's what recipes and a
definition's `emits` are for). Moving items into a recipe machine's
input slots takes only what its recipes use, as a tube or the player's
hand does. Fields are read-only: `m.energy.stored =
1000` is an error. A handle works only in the hooks of its own machine.

What a failing hook moved before it failed stays moved (each move is
whole on its own), but its `data` is left as it was before the hook, and
the machine's script is disabled, as a block's is. Other machines carry
on.

A scripted generator, the base game's solar panel:

```lua
-- scripts/solar_panel.luau: up to max_generate (60 W) in open sky.
function on_tick(panel, dt)
	local above = panel:neighbour("up")
	local light = above and above.light
	local sky = if light then light.sky else 0
	local energy, data = panel.energy, panel.data
	local owed = energy.max_generate * dt * sky / 15 + data.carry
	local whole = math.floor(owed)
	local made = energy:generate(whole)
	data.carry = owed - whole
	data.made += made
	panel:set_active(made > 0)
end
```

Each read of a handle's field or method that reaches the world
(`m.energy`, `m.data`, `block.light`, `block:neighbour(...)`) asks the
engine, and costs about half a microsecond; read each once into a local,
as above, when a script uses it more than once. A thousand scripted
machines a tick add up.

```json5
// blocks/solar_panel.json5 (in part)
{
  block_entity: { data: { made: 0, carry: 0 } },
  script: "solar_panel",
  machine: {
    ports: [{ faces: ["down", "north", "south", "east", "west"], network: "electric", role: "produce" }],
    energy: { capacity: 2000, max_out: 1000, max_generate: 60 },
  },
}
```

## Modules

`require` loads another script and returns what it returns, so shared code
can live in a module:

```lua
-- scripts/util.luau
local util = {}
function util.double(x: number): number
	return x * 2
end
return util

-- scripts/machines/doubler.luau
local util = require("util")            -- this mod's `util`
local shared = require("library:maths") -- a dependency's script
```

A script can require scripts from its own mod and from the mods its
`mod.json5` lists in `dependencies`, nothing else. Each module runs once
per mod that requires it. A script that requires itself, directly or
through others, is an error.

## Logging

`log.debug`, `log.info`, `log.warn` and `log.error` write to the game's
log, under the mod's id and the file and line they're called from. Their
arguments are turned into text and joined with spaces, as `print` would.
(`print` itself isn't available.) Debug lines only show in dev builds;
shipping builds keep only warnings and errors.
A message is cut to 4 KiB (ending in `…(n bytes)`, the length it had),
and one hook call (or a script's top level) may log 100 lines; past
that, one warning says the rest were dropped, and they are. An error's
message is cut the same way.

```text
INFO script: [mymod mods/mymod/scripts/counter.luau:5] ticked 3 times
```

## The sandbox

Scripts get Luau's standard libraries except those that reach outside
the game: `coroutine`, `table`, `string`, `utf8`, `bit32`, `math`,
`buffer` and `vector`, and the base functions. There's no `os` (so no
clock or date), `io`, `debug`, `loadstring`, `getfenv`/`setfenv`,
`collectgarbage` or `print`. Scripts can't read files, start programs or
use the network.

Each mod's scripts run in a Luau state of their own, and each script has
its own globals within it: a global one script sets, no other script sees,
in its mod or any other. The shared globals (the libraries, `log`,
`require`) are read-only.

Scripts change only game state, through what the hooks hand them, never
rendering or the UI. That's so they can run on the server once the game
is split into a client and a server.

## Limits

- **Steps.** Each hook call, and each script's top level as it loads, may
  take a budget of steps (100,000 by default, `ScriptSettings::step_budget`).
  A step is about one loop iteration or one function call, so the budget
  is roughly a millisecond of work. A call that runs out is stopped;
  catching the error with `pcall` doesn't get past it.
- **Steps a tick.** Block hooks may use 250,000 steps between them in one
  tick (`HookSettings::steps_per_tick`), about 3 ms of work. Once they're
  spent, the hooks still waiting run on the next tick, oldest first, so
  scripted blocks that keep setting each other off slow down rather than
  slowing the game. Machines' `on_tick`s get as many again each network
  tick, split between the frames they're spread over; once a frame's
  share is spent, the rest skip that tick, and the next one starts with
  them. Their `on_recipe_finished`s get as many again each network tick,
  a pool of their own; those that don't fit wait for the next network
  tick, oldest first (and past 65,536 waiting, new ones are dropped).
- **Memory.** Each mod's scripts may use 32 MiB between them by default
  (`ScriptSettings::memory_limit`). An allocation past that fails with an
  error.
- **Time.** Steps don't measure the time library functions take inside
  one step: a `table.sort` of a big table, or `string.rep` with a huge
  count, is one step. So each call may also run for only 20 ms of wall
  clock (`ScriptSettings::time_limit`), checked every 16 steps, and is
  stopped the same way when it runs past. The steps are the limit to
  write scripts against, the same on every machine; the time limit is a
  safety net that a slow computer may hit sooner than a fast one.
- **Time a tick.** For the same reason, block hooks may take only 4 ms of
  wall clock between them in one tick (`HookSettings::time_per_tick`);
  once that's passed, the rest wait for the next tick, as when the steps
  run out (the first hook of a tick always runs). Machines' `on_tick`s
  get as much again each network tick, split as their steps are, and
  their `on_recipe_finished`s as much again, waiting oldest first once
  it's passed. A call stopped for
  running out of steps or time counts as its whole step budget against
  the tick's steps, however few it used.

The limits are in the `ScriptSettings` resource (and the per-tick ones
in `HookSettings`); changing `ScriptSettings` reloads the scripts.

## Errors

When a script fails to load (it doesn't parse, or its top level raises
an error or runs out of steps), the error is logged with the mod, the
file and the line, and anything that uses that script is disabled:

```text
ERROR mods/mymod/scripts/broken.luau:3: Expected identifier when parsing expression, got ')' (mod `mymod`, loading `mymod:broken`)
```

When a hook fails (an error, running out of steps, or memory), the error
is logged once, the same way, and whatever called it (one machine, say)
is disabled until the scripts reload. Everything else carries on.
Running out of memory names the script and hook but not the line: Luau
can't say where an allocation failed.

## Reloading

In dev builds, editing a script while the game runs reloads every
script, as editing a texture reloads it: top-level state starts afresh,
and everything a script error disabled is enabled again. A newly added
script file needs a restart. Shipping builds load scripts once.

## For engine code

The runtime is `ScriptRuntime` (`src/script/`), a resource. Hooks are
called with `ScriptRuntime::call(caller, script_id, hook, args)`: `args`
gets the call's `mlua::Scope` and wraps engine state as userdata in it
(`scope.create_userdata_ref_mut(&mut tank)`, or `scope.create_userdata`
for a handle that borrows), which stops working when the call returns, so
a script can't keep engine state past the call. `call_with` takes plain
arguments. A `Caller` is what gets disabled when a call fails: an entity,
a block position, or anything else. Call hooks from `FixedUpdate`, with
the rest of the simulation. `ScriptsReloaded` is sent after a reload.

Blocks' and items' hooks are called in `script::hooks`, with
`ScriptWorld::call` (`src/script/api/`): a system parameter with the
voxels, the content and blocks' data, which puts the world in reach of
the script for the call and saves the `data` it changed after. Its
`origin` is the block the call may change around. Handles are made with
`BlockRef::create(lua, pos)`. To hand a script engine state of your own
as well (a machine's buffers), set the call up with `ScriptWorld::begin`
and make it in `ScriptRuntime::call_scoped`'s scope:

```rust
let call = world.begin(Some(pos));
runtime.call_scoped(caller, script, "on_tick", |lua, scope, hook| {
    let machine = scope.create_userdata_ref_mut(&mut machine)?;
    call.run(lua, scope, hook, |lua, _content| Ok((BlockRef::create(lua, pos)?, machine)))
})
```

`ScriptIndex` says which blocks and items have scripts and which hooks
they define, rebuilt when the scripts reload. Random ticks are in
`script::random_tick` (`RandomTicks` sets their rate and cap).

Machines' hooks are called in `script::machine` (`MachineScriptPlugin`,
in `MachineScriptSystems`), with a `MachineRef` made by
`MachineRef::create(lua, pos)` for the call's origin. `ScriptWorld` reaches
the machine's `EnergyBuffer`, `Tanks`, `Slots`, `Ports` and
`MachineActive`; `api::handle(lua, pos, machine)` makes a machine handle
or a block handle. `WorldCall::allow_generating(n)` lets the call's
machine make up to `n` joules (`on_tick` gives it the tick's share of
`max_generate`); without it, `generate` makes nothing.
`RecipeFinished` messages, from the recipe machines, run
`on_recipe_finished`.

Tests use the test mods in `tests/fixtures/script-mods/`, and the hooks'
and machines' tests test mods of their own (`src/script/hooks/tests.rs`,
`src/script/machine/tests.rs`).
