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
  regions/
    r.0.0.0.dfr        chunks 0..16 on every axis
    r.-1.0.0.dfr       chunks -16..0 in X, 0..16 in Y and Z
    ...
```

The code is in `src/save.rs` (folders and the store), `src/save/region.rs` and
`src/save/chunk.rs`. Every format here has a version number. When a format
changes, bump its version and keep reading the old one. A build that finds a
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

### Chunk blob, format 1

| Bytes | Field |
|---|---|
| 1 | Format: `1` |
| rest | The payload below, zlib-compressed |

The payload uses unsigned LEB128 varints (seven bits a byte, lowest first, the
top bit set on every byte but the last).

```text
varint  palette length P (at least 1)
P ×     varint name length, then the name in UTF-8
packed  4096 voxels, B bits each, B = ceil(log2 P)
        (0 bits when P is 1: the chunk is one block)
```

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

An all-air or all-dirt chunk takes about 15 bytes and a chunk of hillside a
few hundred, against 8 KiB in memory; a freshly explored area averages about
45 bytes a chunk.
