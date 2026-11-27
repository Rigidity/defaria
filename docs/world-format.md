# World save format

A saved world is a folder. The game plays
`<data folder>/worlds/world` unless it's started with `--world <folder>`.
The data folder is:

| System | Data folder |
|---|---|
| macOS | `~/Library/Application Support/Defaria` |
| Windows | `%APPDATA%\Defaria` |
| Linux | `$XDG_DATA_HOME/defaria`, or `~/.local/share/defaria` |

Headless runs (`cargo headless`) keep their world in memory and save nothing,
unless they're given `--world`.

```text
world/
  world.json5          the world's seed and format version
  player.json5         what the player carries
  regions/
    r.0.0.0.dfr        chunks 0..16 on every axis
    r.-1.0.0.dfr       chunks -16..0 in X, 0..16 in Y and Z
    ...
```

The code is in `src/save.rs` (folders and the store), `src/save/region.rs`,
`src/save/chunk.rs` and `src/save/player.rs`. Every format here has a version
number. When a format changes, bump its version and keep reading the old one. A build that finds a
version newer than it knows leaves the file alone: a newer `world.json5` means
the world is played without saving; a newer region file or chunk means that
region is played (generated afresh) but never written. A region file the game
can't read at all is treated the same way.

## `world.json5`

```json5
{
  format: 1,          // this file's format
  seed: 233808922,    // the terrain's seed
}
```

Written when the world is created. A world keeps its seed for good, whatever
seed the game would otherwise use.

## `player.json5`

```json5
{
  format: 2,          // this file's format
  game_mode: "survival", // or "creative" (the default)
  active_slot: 2,     // the hotbar slot in hand, 0 to 8
  // Each hotbar slot's item, or null for an empty slot.
  hotbar: ["defaria:dirt", null, "defaria:pickaxe[handle=defaria:oak,head=defaria:iron]"],
  inventory: [        // what the inventory holds, how many of each
    { item: "defaria:dirt", count: 1250 },
    { item: "defaria:pickaxe[handle=defaria:oak,head=defaria:iron]", count: 1 },
    { item: "defaria:pickaxe[handle=defaria:oak,head=defaria:iron]", count: 1 },
    { item: "defaria:bucket", count: 2 },
    // Two more, each holding 600 thousandths of a block of water.
    { item: "defaria:bucket", count: 2, fluid: { fluid: "defaria:water", amount: 600 } },
  ],
}
```

An inventory has no slots: it's how many of each item it holds, limited only
by weight. A unique item (see `docs/modding.md`) is listed once per copy. A
fluid container holding fluid has `fluid`: the fluid's block id and how much,
in thousandths of a block, as chunks count fluid; containers holding
different amounts are listed apart. Fluid an item can't hold any more (it
isn't a container now, the fluid is gone, or it's more than it holds) is
dropped with a warning, the container kept. The
hotbar names kinds of items rather than holding them, so a slot can name
something the inventory has none of.

Items are saved by name, like blocks in chunks: the item's id, then its
property values in brackets, sorted by property name, if it has any. An item
no mod defines any more is left out when the file is read (with a warning); one
whose properties changed comes back in its default state.

Written every 30 seconds and when the game exits, to `player.json5.tmp` and then
renamed over the old file. A world without one (a new world) gives the player
a new player's inventory and hotbar. A `player.json5` that can't be read (newer
than the build, or damaged) is reported and left alone: the player starts as a
new player, and nothing is saved over the file while the world is open.

## Chunks and regions

The world has no edges, up and down included. It's stored a **chunk** at a
time: 16×16×16 voxels, chunk `(cx, cy, cz)` covering voxels `16·cx ..= 16·cx +
15` on X and the same on Y and Z.

Chunks are grouped into **regions** of 16×16×16 chunks, one file per region:
chunk `c` is in region `floor(c / 16)` (on each axis), named
`r.<x>.<y>.<z>.dfr`, at index `(y · 16 + z) · 16 + x` within it, where `x`,
`y` and `z` are `c mod 16` (always positive).

Every chunk the game has generated is saved, edited or not, when it unloads and
when the game exits, so the world stays as it was first generated even if the
terrain generator changes. Edited chunks are also saved every 30 seconds. A
region file is rewritten whole, to `<name>.tmp` and then renamed over the old
file, so it's never left half written.

### Region file (`.dfr`), format 1

All numbers little-endian.

| Bytes | Field |
|---|---|
| 4 | Magic: `DFRG` |
| 2 | Format: `1` |
| 2 | Region size in chunks along each axis: `16` |
| 4 | Number of chunks saved, `n` |
| 10 × `n` | Table, sorted by index: `u16` index, `u32` offset of the chunk's blob from the start of the file, `u32` its length |
| … | The chunk blobs |

Chunks that aren't in the table have never been saved, and are generated when
they're loaded. A region file that's damaged is renamed to `<name>.broken`
(and logged), and the region starts over.

### Chunk blob, format 5

| Bytes | Field |
|---|---|
| 1 | Format: `5` |
| rest | The payload below, zlib-compressed |

The payload uses unsigned LEB128 varints (seven bits a byte, lowest first, the
top bit set on every byte but the last).

```text
varint  palette length P (at least 1)
P ×     varint name length, then the name in UTF-8
packed  4096 voxels, B bits each, B = ceil(log2 P)
        (0 bits when P is 1: the chunk is one block)
varint  fluid palette length Q (0 for a chunk with no fluid)
Q ×     string: a fluid's block name
packed  4096 voxels' fluid, C bits each, C = ceil(log2 (Q + 1)):
        0 for none, else 1 + its index in the fluid palette
        (0 bits when Q is 0)
varint  F, the number of voxels with fluid that isn't a full block
F ×     varint voxel index, then varint amount (1 to 999)
varint  E, the number of block entities
E ×     varint voxel index
        string kind: the id of the block that has it
        varint part count N
        N ×  string key, varint version, value
```

Strings are a varint length in bytes, then UTF-8.

A **block entity** is data a voxel keeps beyond its block state (see
`docs/modding.md`). Entries are in increasing index order. Each has
*parts*, one per feature that adds one (the block's plain `data` is the
part `data`), sorted by key, each with the version of its own form, so a
feature can change how it saves without the chunk format changing. When
loading, an entry whose kind no mod defines any more is dropped, as is one
whose voxel isn't that block any more, or whose block no longer has a
block entity; a part no feature adds is dropped, and one at a version its
feature can't read starts over from the block's defaults. A voxel whose
block has a block entity but that has no entry gets a new one.

A **value** is a tag byte and what it says follows:

| Tag | Value | Followed by |
|---|---|---|
| 0 | false | |
| 1 | true | |
| 2 | integer | varint, zigzag-encoded (`(n << 1) ^ (n >> 63)`) |
| 3 | float | 8 bytes, an IEEE 754 double |
| 4 | string | string |
| 5 | list | varint count, then that many values |
| 6 | table | varint count, then that many string keys, each followed by a value |

Lists and tables nest at most 32 deep.

The parts the base game saves:

| Key | Version | Who has it | Value |
|---|---|---|---|
| `data` | 1 | blocks with `block_entity: { data }`, and scripts | a table of plain values |
| `energy` | 1 | machines with `energy` | the integer energy stored |
| `tanks` | 1 | machines with `tanks` | a table by tank name of `{ fluid, amount }`, the fluid's block id and the amount in thousandths of a block; an empty tank is left out |
| `slots` | 2 | machines with `slots` | a table by group name of lists, a table per slot: `{}` when empty, or `{ item, count }`, the item by its saved name (as in `player.json5`), with `fluid` (a block id) and `amount` for a fluid container holding some |
| `recipe` | 1 | machines with `recipes` | `{ recipe, ticks }` while one is running: the recipe's id (`defaria:smelt_ore[ore=defaria:iron]` for one of a templated recipe's materials) and how many of its network ticks have run; `{}` when none is. Its inputs were taken when it started, so they're in no slot |
| `pipe` | 1 | conduits on a network the `fluid` solver runs (pipes) | a list of `{ fluid, amount }`, the fluid's block id and the amount in thousandths of a block: the pipe's share of its network's fluid when its chunk was saved; an empty list when it held none |
| `blocked_faces` | 1 | conduits with a face blocked with a wrench | an int, a bit for each side, `1 << side` with up 0, down 1, north 2, south 3, east 4, west 5; only while some face is blocked: a conduit with every face open has none |

Every conduit (wire, pipe, tube) has a block entity, but a block entity
with no parts to save (a wire or tube with every face open) gets no
entry: it's made afresh when its chunk loads.
A fluid network's fluid is kept for the network while it's loaded, not
per pipe: just before a chunk is saved, each of its pipes gets its share
of its network's fluid (divided by capacity) in `pipe`, and when the
chunk loads, each pipe gives what it holds back to its network and is
emptied. A machine's ports and whether it's working aren't saved: they're worked
out from the voxel's state. A tank or slot group the block doesn't have
any more is dropped, as are slots past a group's count and items that no
longer exist. A running recipe that doesn't exist any more is dropped, and
the inputs it took with it.

A voxel holds a fluid (a block with `fluid` set, such as water) in a layer
of its own, beside its block: a voxel of water alone is air with water in
it, and a slab in a pond is the slab with half a block of water. The
fluid's amount is in thousandths of a block. Most fluid is full, so only
the voxels that aren't are listed, in increasing index order; a voxel
with fluid that isn't listed holds a full block of it. A listed voxel
with no fluid is an error. On loading, a fluid that's gone (its mod was
removed, or it isn't a fluid any more) is dropped from every voxel it
was in, a voxel's fluid is cut to the room its block leaves now
(`fluid_room`), and dropped where that's none, and a block that's become
a fluid is air holding a full voxel of it.

Voxels are in the order `index = (y · 16 + z) · 16 + x`. Each voxel's value is
its index into the palette, packed least significant bit first into
`ceil(4096 · B / 8)` bytes.

The palette lists block states by name: the block's id, plus its property
values in the block's property order (sorted by name) if it has any, as in
`defaria:lamp` and `defaria:log[axis=x,wood=defaria:birch]`. A material
property's value is the material's id, or `none`. Air is `air`. Names, rather than the game's internal
state numbers, keep saves valid when mods are added, removed or reordered. When
loading, a block that no longer exists becomes air, and a state whose
properties changed becomes its block's default state.

A voxel holding two halves (a double slab, see "Halves" in
`docs/modding.md`) is named by its two halves joined by a `+`, the one
on the low side (down, north or west) first:
`defaria:stone_slab[half=bottom,rock=defaria:stone]+defaria:planks_slab[half=top,wood=defaria:oak]`.
When loading, each half is read as above; if one is gone (it became
air) the voxel keeps the other, and if the two don't fit together any
more it keeps the first.

An all-air or all-dirt chunk takes about 15 bytes and a chunk of hillside a
few hundred, against 8 KiB in memory; a freshly explored area averages about
45 bytes a chunk.
