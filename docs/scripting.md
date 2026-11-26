# Scripting

Mods describe what blocks, items and machines are in JSON5
([modding.md](modding.md)). Scripts are for what data can't describe: what
something *does*. They're written in [Luau](https://luau.org), a fast,
sandboxed dialect of Lua 5.1 with optional types (why Luau: see
`docs/decisions.md`).

The runtime is in place; the hooks that call scripts come with machines
(#118) and with scripted blocks and items (#122). This page grows with
them.

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
-- scripts/counter.luau
local count = 0

function on_tick(dt: number)
	count += 1
	log.info("ticked", count, "times")
end
```

Every script in every mod loads when the game starts: its top level runs
once, defining its hooks (and anything else it keeps). Variables at a
script's top level keep their values between calls, but they're shared by
everything that uses the script and they're lost when the scripts reload,
so state that belongs to one machine goes in its saved data (#118).

The hooks themselves (`on_tick`, `on_placed`, `on_broken`, ...) and what
they're passed are documented with the features that call them.

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

The limits are in the `ScriptSettings` resource; changing them reloads
the scripts.

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

Tests use the test mods in `tests/fixtures/script-mods/`.
