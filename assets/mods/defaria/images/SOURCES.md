# Layer sources

Where each grayscale layer image came from. Add a row for every layer made
with Retro Diffusion (see `.claude/skills/retro-diffusion/SKILL.md`), and
update the row when a layer is replaced.

| File | Source | Date | Style | Prompt | Request id | Plan usage left after |
|---|---|---|---|---|---|---|
| `stone.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even` | 2026-11-23 | `rd_plus__mc_texture` | solid bedrock stone made of a few large irregular rock chunks separated by thin dark cracks, chipped edges, subtle surface pitting, flat block surface texture, grayscale, even lighting | `741affb4-b362-4e1a-9485-f15e48029e62` | 99% |
| `dirt.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even` | 2026-11-23 | `rd_plus__mc_texture` | loose packed soil with small pebbles and crumbly clumps, flat block surface texture, grayscale, even lighting | `d1fa0950-5b6d-41c5-bbc3-d4e2c39dc04c` | 99% |
| `grass.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even` | 2026-11-23 | `rd_plus__mc_texture` | dense short grass seen from above, many small overlapping blades and tufts, flat block surface texture, grayscale, even lighting | `42b07dc0-f8eb-4168-827d-f431bf00c1ea` | 99% |
| `grass_side.png` | Cut by hand from `grass.png`: the top 4–8 rows of each column (ragged, edge columns equal so neighbouring blocks line up), the bottom pixel of each one level darker, transparent below. Stacked over `dirt.png` on grass side faces | 2026-11-23 | | | | |
| `sand.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even` | 2026-11-24 | `rd_plus__mc_texture` | fine loose sand with soft tiny grains and faint ripples, flat block surface texture, grayscale, even lighting | `f8574fb8-163a-4914-ba5c-d5c270ccf33d` | 98% |
| `gravel.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even` | 2026-11-24 | `rd_plus__mc_texture` | loose gravel made of many small rounded pebbles of different sizes packed together with dark gaps between them, flat block surface texture, grayscale, even lighting | `78b8a1ec-47f1-4ec3-b4e3-3f585c344e97` | 98% |
| `leaves.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even`, then by hand: the darkest pixels (level 1) became holes, each grown by one darker neighbour (down where possible), and pixels below or right of a hole one level darker. About 12% transparent | 2026-11-24 | `rd_plus__mc_texture` | dense clump of small tree leaves overlapping each other with dark gaps between them, flat block surface texture, grayscale, even lighting | `b2b29137-7faa-4882-b768-34f9b7e707d7` | 98% |
| `log_side.png` | Retro Diffusion, converted with `scripts/gray-layer.py --even` | 2026-11-24 | `rd_plus__mc_texture` | rough tree bark with long vertical grooves and ridges running top to bottom, flat block surface texture, grayscale, even lighting | `f4a15570-965f-4364-b0b3-9e5b41997282` | 98% |
| `log_top.png` | Retro Diffusion (a round slice with empty corners), luminance rounded to levels 1–6 by hand without stretching; the corners filled with level 4 (`log_top_bark.png` covers them) | 2026-11-24 | `rd_lite__default` | close-up of the cut end of a tree log filling the entire frame edge to edge, concentric growth rings around a small center, no outline, no border, no background, grayscale, even lighting | `fbc5af25-9fd7-4bb2-bbe5-054f51c000e6` | 98% |
| `log_top_bark.png` | Cut by hand from `log_side.png`: the outer pixel frame, the corners outside `log_top.png`'s slice and the slice's dark outline next to them; transparent inside. Stacked over `log_top.png` | 2026-11-24 | | | | |
| `planks.png` | Retro Diffusion, converted with `scripts/gray-layer.py` (no `--even`, which would erase the seams), then rebuilt by hand: four boards of three rows over a one-row seam (level 1), the source's grain marks kept, the bottom row of each board one level darker, a few level-6 highlights on top rows, and one staggered end joint (level 2) per board | 2026-11-24 | `rd_lite__default` | close-up of a wooden plank floor surface filling the entire frame edge to edge, four horizontal boards stacked with thin dark seams between them, subtle wood grain lines along each board, no outline, no border, no background, grayscale, even lighting | `b88a1319-3618-4224-948e-ed44963ebdd2` | 98% |
| `ore.png` | Retro Diffusion for where the nuggets go (in color, so the ore could be told from the stone); the generated nuggets were one or two pixels, so redrawn by hand as six small clusters at the same spots, lit from the top left (levels 6, 5, 4 and 3, 2 underneath), transparent elsewhere. An overlay stacked over `stone.png`, colored by the ore material's `ore` palette | 2026-11-25 | `rd_plus__mc_texture` | grey stone with several small clusters of shiny orange metal ore nuggets embedded in it, flat block surface texture, even lighting | `2fb46ee2-fd9c-4080-824f-6089fb27c603` | 98% |

## Drawn by script

`glass.png`, `water.png`, `lava.png` and `tuft.png` were drawn
procedurally by `scripts/draw-layers.py` (no Retro Diffusion), 2026-11-25.
Re-run the script to redraw them.

- `glass.png`: a solid one-pixel frame, lit from the top left, two short
  highlight streaks, and a nearly clear middle (alpha 40). Partly
  transparent, for translucent glass.
- `water.png`: soft, wrapping ripples, fully solid; the water material's
  palette carries its alpha.
- `lava.png`: dark crust plates with bright molten seams between them,
  wrapping.
- `tuft.png`: nine blades of grass rising from the bottom edge and bending
  outward toward their tips, thicker near the ground, transparent around
  them.

## Networks and machines

The wire, pipe, tube and machine layers were drawn procedurally by
`scripts/draw-machines.py` (2026-11-26). Re-run it to redraw them:
`python3 scripts/draw-machines.py <out-dir> scripts/draw-machines-wrench.png`.
It draws the whole set; each layer is added here as the content that
uses it is.

Arms run left to right in a band across the middle of the tile and tile
horizontally, transparent above and below; a partial box samples the tile
at its own position, so the band is where an arm box of that height
samples it.

- `pipe_side.png`: an 8-pixel pipe (rows 4–11), shaded round, one level
  darker in the first and last column so joints show between blocks.
- `pipe_end.png`: a pipe end-on, 8 pixels across: a lit flange ring around
  a dark hollow.
- `pipe_core.png`: the junction box face, 10 pixels (3–12): a bevelled
  plate with four bolts and a round cap; transparent around it.

## Cracks (`cracks/`)

`stage_0.png` to `stage_9.png`, the cracks drawn over a block being mined,
were drawn procedurally by `scripts/crack-stages.py` (no Retro Diffusion):
one fixed network of jagged, branching lines spreading from a point near
the middle, each stage showing more of it. New crack pixels start at level
3 and deepen to level 1 over two stages; from stage 5 a few level-5 chips
appear beside the oldest lines. Transparent elsewhere. The game multiplies
the block under them by the gray. Re-run the script to redraw them
(2026-11-25).

## Items (`items/`)

Tools are split into one layer per part, so each part takes its own
palette (an iron head on a wooden handle, a gold guard, …). Every tool
points from the bottom-left corner to the top-right along the same axis.
The sprites were generated in color (wood and metal), split into parts by
hue, and each part's luminance rounded to the gray levels and remapped by
hand (wood 1→2, 2→4, 3→5; metal 2→2, 3→3, 4→5, 5→6), with no stretch.
Background transparency came from the style.

| File | Source | Date | Style | Prompt | Request id | Plan usage left after |
|---|---|---|---|---|---|---|
| `pickaxe_handle.png` | Retro Diffusion: the wooden handle of the generated pickaxe, extended by hand to meet the head. Also the base of the axe and shovel handles | 2026-11-24 | `rd_fast__mc_item` | a pickaxe with a straight wooden handle and a curved iron head, diagonal from bottom left to top right, on a plain white background | `901305c2-fbcf-494b-883e-2e14c79718c6` | 99% |
| `pickaxe_head.png` | Drawn by hand (the generated head was a blob): a two-pixel arc across the handle, lit from the top left, tips darker | 2026-11-24 | | | | |
| `axe_head.png` | Retro Diffusion, the metal of the generated axe; by hand: the back lobe trimmed to a small poll, the cutting edge brightened | 2026-11-24 | `rd_fast__mc_item` | an axe with a straight wooden handle and a wide single-bladed iron head on one side, diagonal from bottom left to top right, on a plain white background | `d07718f2-ff51-4a1a-be63-e1231889b3bb` | 99% |
| `axe_handle.png` | A copy of `pickaxe_handle.png`, its top pixel at (10, 5) removed where the axe head covers it | 2026-11-24 | | | | |
| `shovel_head.png` | Drawn by hand, because the generated shovel pointed the wrong way and its blade ran off the canvas: a rounded spade with a neck and a shallow middle ridge | 2026-11-24 | `rd_fast__mc_item` (unused) | a shovel with a straight wooden handle and a rounded flat iron spade blade at the top end, diagonal from bottom left to top right, on a plain white background | `05c3e134-ade2-43cb-b000-5acf2b4ffd1e` | 99% |
| `shovel_handle.png` | A copy of `pickaxe_handle.png`, its top three pixels trimmed to meet the shovel's neck | 2026-11-24 | | | | |
| `sword_blade.png`, `sword_guard.png`, `sword_grip.png` | Drawn by hand along the tool axis, because the generated sword had a smeared guard and its tip ran off the canvas; its shading levels were kept. The guard layer includes the pommel | 2026-11-24 | `rd_fast__mc_item` (shading only) | a short sword with a straight iron blade, a small crossguard and a wooden grip with a round pommel, pointing diagonally up to the top right, on a plain white background | `c9115adf-ce9b-4cfe-bbe6-bbab3ee591c8` | 99% |
| `knife_blade.png`, `knife_handle.png` | Retro Diffusion, split as generated | 2026-11-24 | `rd_fast__mc_item` | a small knife with a short pointed iron blade and a wooden handle, pointing diagonally up to the top right, on a plain white background | `4da18cd3-5179-41fe-8e3f-b6a653915c31` | 98% |
| `bow_limbs.png`, `bow_string.png` | Retro Diffusion, split as generated; the string extended by one pixel to meet the top limb | 2026-11-24 | `rd_fast__mc_item` | a simple curved wooden bow with a thin taut string, drawn diagonally with the bow arc bulging toward the top right and the string running from top left to bottom right, on a plain white background | `a44165d6-4535-445b-82ac-ec76b599b9a3` | 99% |
