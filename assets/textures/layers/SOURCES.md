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
