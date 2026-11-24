---
name: retro-diffusion
description: Rules for making defaria art with the Retro Diffusion MCP (mcp__claude_ai_Retro_Diffusion__*). Read before ANY Retro Diffusion call, metered or free, and whenever the user asks for new or replacement block textures, layer images or other pixel art in this repo. Covers when generating is allowed at all, checking usage first, the house styles (one per kind of material), the 16×16 size, converting results to the 8 gray levels locally (no palette is sent), one job at a time, and how results land in assets/textures/layers/.
---

# Retro Diffusion rules

The account is on a monthly Retro Diffusion subscription (the Creator plan).
Jobs draw down a fixed monthly allowance of plan usage, shown as a
percentage, instead of being billed per image; it resets each billing
period. The allowance is limited, and once it's gone generation stops until
the next period (or falls back to the account's few one-off credits). These
rules exist to keep every generation deliberate, light on plan usage and
consistent with the art already in the game. They apply to you and to any
subagent; agents other than Claude Code can read this file and follow it
directly.

## How the art works

Block textures are stacks of **grayscale layer images**, colored at runtime
(see `src/block_texture.rs` and `src/block_texture/layer.rs`):

- Each layer is a 16×16 PNG in `assets/textures/layers/`.
- It may use only the 8 gray levels `0, 36, 73, 109, 146, 182, 219, 255`
  (`gray()` in `layer.rs`), plus fully transparent pixels.
- Each gray level is an index into a palette, darkest first. The game gives
  every level its own color, then stacks the colored layers into a tile.
- So a layer is about **shape and shading only**. Color, and which layers
  stack into which tile, live in code (`Tile::layers`, and later in json5
  files). Never ask Retro Diffusion for color.

`cargo test` checks every layer on disk (`layers_on_disk_build`): wrong size,
a color, or a gray outside the 8 levels fails the build with the pixel's
position.

## The rules

1. **Generate only when the user asks, for what they asked.** A generation
   needs an explicit request in the current conversation naming what to make
   ("generate a stone layer"). Never generate as a side effect of other work,
   to test the pipeline, to try an idea, to fill in a missing asset, or
   because a placeholder looks bad. Placeholders are drawn by hand (or by a
   throwaway script) and are free. When unsure whether a request covers a
   generation, ask.

2. **Check usage first, every session, before the first metered call.**
   - Call `get_balance` and report the plan usage left (`plan_usage_left_pct`)
     and when the period ends (`period_end`) to the user.
   - Call `estimate_inference_cost` (free) with the exact parameters you
     intend to send. Its `would_use` must be `plan`. Its dollar figures are
     the pay-as-you-go price, not what the subscription charges, so don't
     quote them as a cost.
   - Stop and ask if plan usage left is under 10%, or if `would_use` is
     anything other than `plan` (the job would spend credits or balance).

3. **One job at a time, never in parallel.** No two Retro Diffusion calls in
   the same message, no subagents or workflows making calls alongside you,
   no batch of styles "to compare". Use `num_images: 1` unless the user asks
   for several takes. Wait for each result and look at it before deciding
   on the next call.

4. **Don't retry on your own.** If a result is unusable, show it to the user
   and say why, and let them decide whether to use plan usage on another
   attempt.
   The one exception: a job that *failed* (no image, auto-refunded) may be
   retried once with identical parameters.

5. **Only the house styles and the fixed format.** Every layer is generated
   the same way so they look like one set. Pick the row by material:

   | Material | `prompt_style` | Prompt shape |
   |---|---|---|
   | Organic or noisy: stone, dirt, gravel, sand, grass | `rd_plus__mc_texture` | `<material and its details>, flat block surface texture, grayscale, even lighting` |
   | Masonry with clear shapes: cobblestone, bricks, tiles | `rd_lite__default` | `close-up of a <material> surface filling the entire frame edge to edge, <shapes and details>, no outline, no border, no background, grayscale, even lighting` |

   And for every generation:

   | Parameter | Value |
   |---|---|
   | `width`, `height` | `16`, `16` |
   | `num_images` | `1` |
   | `input_palette`, `input_image`, `tile_x`, `tile_y`, `remove_bg`, `bypass_prompt_expansion` | leave unset |

   Don't switch style, size or model to work around a bad result. Changing
   the house styles is the user's decision; if they make it, update these
   tables.

6. **Expect texture from Retro Diffusion, structure from us.** What the
   first stone attempts (November 2026) showed, at 16×16:
   - `rd_plus__mc_texture` tiles cleanly but ignores structure in the
     prompt (cracks, chunks): it makes fine speckle, whatever you ask.
     Hand-added structure (cracks) didn't work for stone: anything that
     stands out repeats on every block and lines up into a pattern across
     a surface, and at low contrast it just looks like noise. Prefer
     texture without landmarks.
   - `rd_lite__default` follows the prompt, but draws "stone" as a
     framed object with an outline unless the prompt says the surface
     fills the frame edge to edge (the masonry prompt shape above). Even
     then it reads as cut blocks, and its tile edges don't match: fix the
     seams by hand.
   - Constraining output to the 8 grays (`input_palette`) collapses the
     shading into two or three levels. Convert locally instead.
   - Image-to-image (`rd_core__default` with a sketch as `input_image`, at
     strength 0.6) returned the sketch nearly unchanged, with broken
     seams, at twice the plan usage. Not worth it at this size.

   In every prompt: no color words (color comes from palettes), no "pixel
   art" (the style does that), no light direction, no borders or frames.

7. **Overlays and variants come from existing layers, not new
   generations.** A layer that covers only part of a tile (like
   `grass_side.png`, grass hanging over dirt) is cut from a full layer by
   hand with transparency. Another color of the same material is a new
   palette in code, not a new image.

## After a generation

1. Download the output into your scratch directory, not straight into
   `assets/`.
2. Check it is 16×16 (`file <download>.png`). The house styles return
   16×16 natively; if one doesn't, stop and tell the user rather than
   resizing it or paying for `fix_pixel_art`.
3. Convert it to a layer with `scripts/gray-layer.py`:
   ```sh
   python3 scripts/gray-layer.py <download>.png <scratch>/<name>.png --even --preview <scratch>/<name>-tiled.png
   ```
   Use `--even` for every layer that fills the whole tile. It makes each
   row and column average the same brightness; without it, a slightly
   darker or lighter row or column turns into a line along every block
   edge, and a surface visibly breaks into squares.
   It converts to gray by luminance and stretches the image so its darkest
   pixel is level 1 and its brightest level 6, rounding the rest to the
   nearest of the 8 levels. Levels 0 and 7 stay free for rare accents
   placed by hand, but use them sparingly: anything high-contrast repeats
   on every block. Every layer goes through the
   same stretch, so one palette looks the same on any layer. If the result
   looks too grainy, lower the contrast of its palette in code (fewer
   steps between the darkest and lightest stops) rather than the layer:
   the layer keeps all its detail and the palette sets how strong it
   looks. That's how the stone is done. `--soften` and a narrower range
   such as `--low 2 --high 5` also calm a layer down, but they throw
   away detail; use them only if the user asks. Don't use `palette_converter` for any of this.
4. Look at the preview (the layer tiled 3×3, upscaled) and show it to the
   user next to the untouched download. Small seams are fixed by hand, locally;
   don't spend plan usage on `seam_tiling` unless the user asks. A bad
   seam or a lopsided light direction is a reason to report back, not to
   regenerate unasked.
5. Only when the user is happy: save it as `assets/textures/layers/<name>.png`
   (lowercase snake case). Never overwrite an existing layer unless the user
   said to replace it.
6. Record it in `assets/textures/layers/SOURCES.md`: file, date, style,
   prompt, the response's `request_id` (the API doesn't return a seed) and
   the plan usage left afterwards (`billing.plan_usage_left_pct` in the
   response; `get_balance` rounds it). Reuse a recorded prompt shape when
   making a related layer, so the set stays consistent.
7. Run `cargo test` (the layer checks) and a headless scenario
   (`cargo headless scenarios/texture-distances.txt`) and look at the
   screenshots.

## Free calls

These use no plan usage and are fine whenever they help: `get_balance`,
`list_available_styles`, `get_style_usage`, `list_edit_tools`,
`estimate_inference_cost`, `estimate_edit_tool_cost`, `get_service_status`.
Everything else (`create_inference`, `start_inference_job`, `run_edit_tool`,
`fix_pixel_art`, style creation, anything Low-Poly) draws on plan usage
(or credits) and falls under the rules above.
