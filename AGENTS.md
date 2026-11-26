# Agent guide

A Bevy 0.20 game, developed mostly by agents. Rust is pinned in
`rust-toolchain.toml`.

## Commands

| Task | Command |
|---|---|
| Build | `cargo build` |
| Tests | `cargo test` |
| Lint (must be clean) | `cargo clippy --all-targets -- -D warnings` |
| Format | `cargo fmt` |
| Play a scripted scenario, no window | `cargo headless <script>` |
| Shipping build | `cargo dist` |

`cargo run`, `cargo play` and `cargo shot` open the game window. They're for
the user; see "Don't open the game window" below.

Before you finish, run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`
and `cargo test`. CI runs the same checks, plus clippy with
`--no-default-features`.

## Build rules (these keep rebuilds at ~2s instead of ~10 min)

- Don't run `cargo clean` and don't delete `target/`.
- Don't change `rust-toolchain.toml`, the Bevy version, Bevy features or the
  `[profile.*]` sections unless the task is specifically about that. Each of
  these forces a full Bevy rebuild.
- Use the plain commands above. The `dev` feature (dynamic linking, hot
  reload, BRP) is on by default so every command shares one compiled Bevy.
  Don't add `--release` or `--no-default-features` while iterating; that builds
  a second copy of Bevy.
- `[profile.ci]` (unoptimized Bevy) is for CI only. Don't use `--profile ci`
  locally; it builds another copy of Bevy.
- If cargo prints `Blocking waiting for file lock`, another build is using this
  checkout. Wait for it; don't kill it.
- Every checkout builds into its own `target/`. Never share one between
  worktrees (`CARGO_TARGET_DIR`, or `target-dir` in a `.cargo/config.toml`):
  cargo names the game's artifacts the same in every checkout and decides
  they're fresh by mtimes alone, so tests and headless runs in one worktree
  would run another worktree's code. A new worktree gets Bevy already
  compiled from `scripts/seed-worktree-target.sh` (T3 Code runs it when it
  creates one), and its first build compiles only the game crate, in about
  30 seconds.
- After adding, removing or updating a dependency (anything that changes
  `Cargo.lock`, or Bevy's features), run
  `scripts/seed-worktree-target.sh --save` yourself once `cargo test` and
  `cargo clippy --all-targets -- -D warnings` have built it, without being
  asked. That adds the new dependencies to the seed, so other worktrees
  don't each compile them (about 10 minutes for a new Bevy). Saving only
  adds, so it's safe on a branch that hasn't merged yet.

## Don't open the game window

Don't launch the game window yourself: no `cargo run`, `cargo play`,
`cargo shot` or `--screenshot` runs. The user plays the game and judges how it
feels. Only open the window when the user explicitly asks. Use headless mode
(below) to see the game instead.

Likewise, leave the user's saved worlds alone: don't read, write or delete
anything in `~/Library/Application Support/Defaria` (see
`docs/world-format.md`). Headless runs and tests keep their worlds in memory
or in folders they're given.

## Verifying changes

Every change gets both kinds of check:

- **Tests** for everything that can be checked in code and gives the same
  answer every run: game logic, math, parsing, state changes, input handling.
  If a behaviour can be asserted, assert it in a test rather than eyeballing it.
- **The headless runner** for everything you have to see or measure: how the
  game looks, UI layout, whether a feature feels right in motion, and
  performance. Run it after any visible change and look at the screenshots
  before you finish.

The headless runner is part of the codebase, not a fixed tool. When a check
needs something it can't do yet (benchmarking frame times, UI interaction, new
input types, new kinds of output), extend the runner in `src/headless/`,
document the new command at the top of `src/headless.rs`, and add a test for
it. Keep `scenarios/` working when the game
changes.

1. **Logic:** write headless tests. Build an `App` with `MinimalPlugins`, add
   only the systems under test, step time with
   `TimeUpdateStrategy::ManualDuration`, and call `app.update()`. See the tests
   in `src/fly_camera.rs`, and in `src/player.rs` for systems in
   `FixedUpdate`. `Time<Virtual>` clamps each step to 250ms, so keep manual
   steps below that. Build the usual setups from `src/testing.rs` rather
   than copying them: `world_app` (the world streaming around a loader,
   with the feature's plugins added, then `build()`, which waits for it to
   `settle`), `patch_app` (the fixed test patch of terrain), `chunk_app`
   or `ChunkMapPlugin` with `insert_chunk` (chunks spawned by hand), and
   `test_content`/`test_mods` (the base game plus a `test` mod). When a
   plugin these add starts needing something, add it there once.
2. **Gameplay and visuals:** write a scenario script and run
   `cargo headless <script>`. It runs the whole game with no window, rendering
   offscreen, feeds it the script's input at a fixed 60 fps, and exits when the
   script ends. `screenshot` saves PNGs you can open; `inspect` prints
   components to stdout. Example (`scenarios/fly-around.txt`):
   ```text
   wait 5
   camera 64 56 92 0 -37.9  # the player's eyes here, flying
   hold MouseLeft 1      # lock the cursor
   hold KeyW 60          # fly forward for one second
   mouse 150 0           # turn right
   wait 1
   screenshot target/scenarios/fly-around/after.png
   inspect FlyCamera     # a component or resource, short or full name
   ```
   Commands: `wait`, `press`, `release`, `hold`, `mouse`, `screenshot`,
   `grain` (a texture-noise number for the frame; lower is smoother, and only
   comparable between runs with the same view and art), `inspect`, `fill`
   (set a box of voxels to a block, to build a test scene), `cursor` and
   `scroll` (point at and wheel over UI, such as the inventory; they drive
   Bevy's picking as a real mouse does), `type` (type text into whatever
   has the keyboard, such as a text field), `pour` (put an amount of a
   fluid in a voxel) and `fluids` (each fluid's total in the loaded world,
   pipes and tanks, and how much of it is active), `network` (the network
   the conduit at a voxel is part of: kind, size, frozen or running, its
   ports and their roles, on an energy network what it stored, produced
   and consumed, and what a fluid network holds),
   `put` (put items in a machine's slot group or fluid in its tank, as a
   tube or pipe would), `hotbar` (put an item, such as `wrench`, in a
   hotbar slot and in hand), `give` (put items in the player's
   inventory, a fluid container filled if a fluid is named), `camera` (put the
   camera, the player's eyes, at a position and angle; the player stops
   there, flying) and `timing` (mean and worst wall-clock
   frame time since the last `timing`, for performance comparisons, and
   how many voxels the `fill`s since then changed and how long they took) and
   `settle` (wait for the world around the camera to load and clear of fog,
   after a `camera` jump somewhere new, and print how many frames and
   seconds that took) and `memory` (what the loaded chunks hold: how many,
   how many are one block or light throughout, the bytes of their blocks,
   light, fluid and meshes, and how many chunks of one value are still
   kept dense) and `lookups` (time reading every voxel in a box of at
   most 256³ by world position and a chunk at a time) and `random-ticks` (pick more
   voxels a chunk for random ticks, so scripted blocks grow or spread
   quickly enough to watch). The full format is documented at the top of
   `src/headless.rs`. The world loads around the camera in the background;
   the script starts once the first view has loaded. Headless runs play a
   world kept in memory, so every run starts the same; add `--world <folder>`
   to play and save one on disk (put it in your scratch directory). Commands between two
   `wait`s run on the same frame, so put a `wait 1` after a `screenshot`
   before input meant to come after it. A rendered run fails if a shader
   fails to compile.
   Keys are `KeyCode` names; mouse buttons are `MouseLeft`/`MouseRight`/
   `MouseMiddle`. Add `--no-render` to run without a GPU (`screenshot` and
   `grain` are skipped), and `--radius <chunks>` to load a smaller world
   (the default is 16); CI runs every scenario in `scenarios/` that way,
   with `--radius 4`, so they must keep working with a small world. Write screenshots under `target/`. Keep reusable scenarios in
   `scenarios/`; put one-off ones in your scratch directory.

   **Don't run every scenario locally to check they pass.** CI runs them
   all on every push, unrendered and in parallel shards, in about a
   minute and a half; rendering all of them locally takes 15 minutes or
   more. `cargo test` also parses every scenario and checks the blocks
   its `fill`s name, which catches the usual breakage (a renamed block).
   Locally, render only the scenarios whose output your change affects,
   to look at their screenshots or put them in a PR, and leave
   pass/fail for the rest to CI.
3. **End-to-end tests:** the same scripts run inside `cargo test` with no GPU
   (screenshots are skipped there). Build the app with
   `headless::default_plugins(false)`, `GamePlugin` and `HeadlessPlugin`, step it
   until it exits, then assert on the world. See
   `script_drives_the_fly_camera` in `src/headless/tests.rs`; `headless::test_game`
   builds that app (loading a smaller world) and `headless::settle` steps it
   until the world has loaded. CI has no GPU, so tests must use
   `rendering: false`.
4. **Live state:** if the user has the game running and asks you to inspect
   it, the Bevy Remote Protocol listens on `127.0.0.1:15702` (or `BRP_PORT`).
   Example:
   ```sh
   curl -s localhost:15702 -d '{"jsonrpc":"2.0","id":1,"method":"world.query","params":{"data":{"components":["defaria::fly_camera::FlyCamera"]}}}'
   ```
   Components only show up in BRP and `inspect` if they derive `Reflect` with
   `#[reflect(Component)]`.

## Screenshots in pull requests

When a PR changes anything you can see, its description shows it:

1. Pick the one to four screenshots that best show the change, from the
   scenarios you ran. When the change is an improvement, include a before and
   after pair of the same view: run the scenario on the base branch in a
   scratch worktree that shares the target directory (see
   `.claude/skills/review-loop/references/reviewer-brief.md` for the
   commands).
2. Copy them somewhere temporary, with names that say what they show
   (`close-up.png`, `far-view-before.png`). The name becomes the alt text.
3. Upload them, which prints Markdown image links:
   ```sh
   scripts/pr-screenshots.sh <pr-number> <image.png>...
   ```
   The script commits the images to the `rigidity/pr-screenshots` branch and
   links them by commit. It doesn't touch your working tree or your branch.
4. Add a `## Screenshots` section to the PR description, with a line for each
   image saying what it shows and which scenario made it.

When a later push changes what's visible, upload new screenshots and update
the section.

## Issues and the roadmap

Planned work is tracked in issues, grouped under epics (an issue labelled
`epic`, with the work as its sub-issues), and ordered in the roadmap
project: https://github.com/users/Rigidity/projects/3. Keep the project in
step with the work, using `scripts/roadmap.sh` (needs the `project` scope:
`gh auth refresh -h github.com -s project`):

- **Every open issue is in the project, in priority order,** except epics,
  which stay out (each item's Parent issue shows its epic). The Order field
  is the priority: work goes in increasing order, so an issue goes after the
  work it needs and before the work that builds on it, and work in progress
  sits with the rest of its epic.
- **Filing an issue:** add it to the project where it belongs in the order,
  with `scripts/roadmap.sh add <issue> <order>` (`list` shows the order;
  `7.5` goes between 7 and 8). Put it under its epic if it has one, or
  start an epic when it begins a new group of work. In Claude Code, a hook
  (`.claude/hooks/roadmap-reminder.sh`) reminds you after `gh issue create`.
  The project's own auto-add workflow can add sub-issues without an Order;
  give them one.
- **Labels:** give every issue the labels that fit it from `gh label list`:
  the areas it touches (`rendering`, `ui`, `world` and so on), `enhancement`
  or `refactor`, and the flags (`mod-format`, `save-format`, `art`,
  `needs-decision`). If an issue needs a label that doesn't exist yet,
  because it starts a new area or needs a new flag, create it with
  `gh label create <name> --color <hex> --description <what it's for>`
  rather than leaving the issue without one.
- **Checking:** `scripts/roadmap.sh check` lists open issues missing from
  the project, items with no Order, epics in the project, Orders that two
  items share and open epics with every sub-issue closed, and fails if
  there are any. Run it after filing, closing
  or reordering issues, and fix what it reports.
- **Starting work on an issue:** `scripts/roadmap.sh status <issue> doing`.
- **Finishing:** say `Closes #<issue>` in the PR description, so merging
  closes the issue, and the project marks it Done on its own. Work that
  ends without a merged PR: close the issue, or set it back with
  `status <issue> todo` if it's left unfinished.
- **Epics close on their own:** when an issue closes and every sub-issue of
  its epic is closed, `.github/workflows/close-finished-epics.yml` closes
  the epic (`scripts/close-finished-epics.sh`). Adding work to a closed
  epic: reopen it with `gh issue reopen <epic>`.
- **Changing the plan:** when priorities change, or an issue turns out to
  depend on another, move it with `scripts/roadmap.sh order <issue> <order>`.

## Reviewed PRs

When asked to take work through review ("with reviews", "drive this to
completion", "review until clean"), follow `.claude/skills/review-loop/`. It
runs rounds of code review and headless playtesting, fixes what they find, and
ends in a PR. Agents other than Claude Code can read `SKILL.md` there and
follow it directly.

## Art and Retro Diffusion

Block textures are 16×16 grayscale layer images in a mod's `images/` folder
(the base game's are in `assets/mods/defaria/images/`), colored by material
palettes and stacked at runtime as the mod's texture files say
(`src/block_texture.rs`). Layers use only the 8 gray levels that `cargo test`
checks for.

The Retro Diffusion MCP runs on a monthly subscription with a limited
allowance of plan usage. Read
`.claude/skills/retro-diffusion/SKILL.md` before any Retro Diffusion call.
In short: generate only when the user asks for that specific art, check
plan usage first, one call at a time and never in parallel, always the
house style for the material (see the skill) at 16×16, converted with
`scripts/gray-layer.py`, and record every result in
`assets/mods/defaria/images/SOURCES.md`.

## Content and mods

Blocks, items, models, textures and materials are data, not code: JSON5
files in mod folders under `assets/mods/`, loaded by `src/content.rs` into
the `Content` resource. The base game is the `defaria` mod. To add or
change a block, edit or add files there rather than Rust. The formats are
documented in `docs/modding.md`; keep that file, the serde types in
`src/content/schema.rs` and the base mod in step. Chunks store
`BlockState`s, which index `Content::states`; look blocks up by id
(`content.default_state("defaria:stone")`), and in tests use
`content::base()` and `content::block("stone")`.

## Code layout

- `src/main.rs`: CLI flags and `App` setup only.
- `src/lib.rs`: `GamePlugin`, which registers one plugin per feature.
- `src/<feature>.rs` (or `src/<feature>/`): one plugin per gameplay feature,
  with its components, systems and tests. Put new features in new modules so
  parallel work doesn't collide. Touch `lib.rs` only to register the plugin.
- `src/devtools.rs`: CLI flags, windowed screenshot mode and BRP.
- `src/headless.rs` (and `src/headless/`): headless mode and the scenario
  script runner; the script format is documented at the top of
  `headless.rs`. `script.rs` parses scripts into `Command`s; `runner.rs`
  is `HeadlessPlugin`, which steps through them a frame at a time and
  exits when they end; `input.rs` the keys, characters and pointer
  target that simulated input uses; `world_commands.rs` the commands
  that set up a scene (`fill`, `pour`, `put`, `hotbar`, `camera`, ...)
  and the `fluids` and `network` reports; `capture.rs` the offscreen
  image, screenshots, `grain`, `timing` and `inspect`; `pipelines.rs`
  failing the run on a broken shader; `memory.rs` the `memory` and
  `lookups` commands; `test_app.rs` the test helpers (`test_game`,
  `run_more`, `settle`); `tests.rs` end-to-end tests of whole scripts.
- `src/testing.rs`: helpers for tests (`#[cfg(test)]`): world, patch
  and chunk apps, settling streaming, and content with a `test` mod.
- `src/world.rs` (and `src/world/`): the world's voxels, which
  everything else reads and writes: `Chunk` (`world/chunk.rs`, with the
  fluid amounts it keeps), `ChunkCoord`, the loaded chunks (`ChunkMap`,
  `spawn_chunk`), `Voxels` (`world/voxels.rs`) for reading and writing by
  world position, and the messages edits send (`VoxelChanged`,
  `FluidChanged`, `PropertiesUpdated`). `world/chunk_array.rs`:
  `ChunkArray`, a value per voxel of a chunk, kept as a single value
  while the chunk is one value throughout; what `Chunk` keeps its block
  states in and `ChunkLight` its light. Changed chunks are compacted once
  a frame.
- `src/meshing.rs` (and `src/meshing/`): drawing chunks. Remeshing the
  ones that changed (`RemeshChunks`, `MeshTask`, `AwaitingNeighbours`) and
  showing the meshes: opaque faces on the chunk's entity, cut-out and
  translucent ones on child entities (`ChunkParts`), with the translucent
  faces sorted back to front. `meshing/build.rs` builds the meshes from a
  `Neighborhood` (culling, face shading, the vertex attributes).
- `src/worldgen.rs` (and `src/worldgen/`): terrain generation (`Terrain`,
  `TerrainSettings`), with the land's shape and the splines it's made of,
  and `worldgen/trees.rs` where trees grow and their shapes.
  Only `streaming`, which runs it, uses it outside tests.
- `src/ui.rs` (and `src/ui/`): the UI toolkit that the HUD and screens are
  built from: the theme, screens (`UiScreen`, and the `playing` run
  condition gameplay input runs under), the pointer (Bevy's picking),
  scrolling, the pixel font, buttons, text fields (and the `typing` run
  condition), item icons, item slots, bars and tooltips. Build new UI from these,
  in the feature's own module. F10 opens a gallery of the widgets in dev
  builds (`src/ui/gallery.rs`), for working on how they look.
- `src/block_edit.rs`: the crosshair, its target and placing blocks.
  `src/mining.rs`: breaking blocks by holding left click, for a time set
  by their hardness (`Mining`, on the local player, has the target and
  progress).
  `src/cracks.rs`: the crack overlay on the block being mined.
  `src/support.rs`: blocks that need support (plants, a block's
  `support`) break when what's under them goes; `block_edit` places
  them only where they stand (`Content::supports`), and `mining` gives
  a survival player what breaks with the block mined.
- `src/connections.rs`: keeps blocks' connection properties (a pipe's
  joined sides) in step with their neighbours, after edits and on chunk
  load, through `Voxels::update`. Whether a neighbour takes a connection
  is decided in one place, `Content::accepts` (`src/content/connect.rs`).
  A conduit's faces blocked with a wrench (`BlockedFaces`, in its block
  entity) never join; `src/wrench.rs` is the wrench.
- `src/network.rs` (and `src/network/`): networks of conduits (wires,
  pipes, tubes; blocks with a `conduit`): `Networks` keeps which conduits
  form which network, following the connection properties, merging and
  splitting as blocks change and chunks load and unload, and freezing a
  network that reaches into an unloaded chunk. Solvers keep what each
  network holds in a `NetworkMap` (`NetworkState`), which merges and
  splits with it. Network kinds are data (`networks/` in a mod).
  `NetworkPlugin` adds the solvers too (energy, fluid, items);
  `NetworkCorePlugin` is the networks and clock alone, for tests.
  `src/network/tick.rs`: the network clock (`NetworkClock`), 20 network
  ticks a second counted out of `FixedUpdate`; every ticked system runs
  in a phase of `NetworkTick` (`Clock`, then `Solve` for the solvers,
  then `Machines` for recipes and machine scripts) under the
  `on_network_tick` run condition.
  `src/network/energy.rs`: the `energy` solver, first in
  `NetworkTick::Solve`: moves energy between machines' `EnergyBuffer`s
  by their ports' roles, capped by the network's weakest wire, makes it
  in generators and uses it up in machines with `use`, and reports each
  network's (`EnergyReport`).
  `src/network/item.rs` is the item solver: tubes move items instantly
  between machines' slot groups, round-robin, inputs before stores,
  through port filters, at the slowest tube's rate; and creative
  machines with `emits` (`ItemSource`) keep a slot group full.
- `src/game_mode.rs`: creative and survival (`GameMode` on the player,
  C switches): whether breaking gives items and placing uses them up.
- `src/inventory/`: inventories. `model.rs` has `Inventory` (how many of
  each item, limited only by weight), a component any entity can have;
  `player.rs` the local player's (on the entity with `LocalPlayer`, saved
  in `player.json5`); `transfer.rs` moving items between inventories and
  the creative catalog; `list.rs` sorted item lists on screen, grouped
  by category and filtered by the search; `search.rs` search queries and
  the search box any screen can have;
  `gesture.rs` the mouse on lists and the hotbar (clicks, drags, binding
  the hotbar); `screen.rs` the inventory screen (E).
  `src/hotbar.rs`: the hotbar (`Hotbar`, nine slots naming kinds of item,
  showing how many the player has) and `ActiveSlot`.
- `src/physics.rs`: boxes colliding with block shapes (swept, sliding,
  stepping up), in `FixedUpdate`. `src/player.rs`: the player's
  Minecraft-style walking, jumping, sneaking, sprinting and flying: a
  physics body on its own entity (the `LocalPlayer` one is this game's
  player), its `Transform` at its feet, moved by its `Controls` (the
  keys fill the local player's) and facing its `Look` (yaw turns the
  body, pitch the head). Gameplay aims from the player's `Eyes`, never
  from the camera. `src/fly_camera.rs`: the camera, a child of the local
  player at its eyes, and the mouse turning its `Look`.
- `src/fluid.rs` (and `src/fluid/`): fluid flow. A fluid voxel holds an
  amount (`Chunk::partial_at`), and `src/fluid/flow.rs` moves amounts
  between voxels, never making or losing any. Also what the player
  swims in and the fog inside fluid.
- `src/streaming.rs`: loading, unloading and saving chunks around the
  camera. The world has no edges, up and down included.
- `src/fog.rs`: distance fog that follows the edge of the drawn world.
- `src/light.rs` (and `src/light/`): block and sky light. `src/light/sky.rs`
  decides where sky light comes in above the loaded chunks; see
  `docs/decisions.md`.
- `src/block_entity.rs` (and `src/block_entity/`): block entities, the
  data a voxel keeps beyond its state (`BlockEntity`, under the chunk's
  entity while it's loaded). Features add parts with
  `register_block_entity_part`; `BlockEntities` finds the one at a voxel.
- `src/machine.rs` (and `src/machine/`): machines (blocks with a
  `machine`; baked per state in `src/content/machine.rs`). Their block
  entities get `Ports`, `EnergyBuffer`, `Tanks`, `Slots` and
  `MachineActive` (which puts the voxel in its `active_state`), and
  `MachinePorts` lists each network's machine ports and their roles for
  the solvers. `MachinePlugin` adds the screen; `MachineCorePlugin` is
  machines without it, for tests. `src/machine/screen.rs`: the screen
  using a machine opens, laid out from its definition;
  `src/machine/exchange.rs`: moving items between the player and a
  machine. Using a block (right click) is
  handled in `block_edit::BlockUseSystems`, before placing.
- `src/recipe.rs` (and `src/recipe/`): recipe machines (a `machine` with
  `recipes`), in `FixedUpdate` on network ticks: start a recipe on what's
  in their input slots and tanks, draw its energy (or make a generator's)
  a tick at a time, and put out its outputs; `RecipeProgress` is the saved
  part. Recipes are data (`recipes/` in a mod), baked in
  `src/content/recipe.rs`, templated ones once per material combination.
- `src/fluid_network.rs` (and `src/fluid_network/`): the fluid solver. A
  network is one shared tank (`FluidNet`, in a `NetworkMap`) moving fluid
  between machines' tanks by role, on network ticks (20 Hz); pumps and
  outlets (`pump_rate`, `outlet_rate` stats) draw from and pour into the
  world; tanks with a `level` property show how full they are. A broken
  pipe spills its share; an unloading or saved chunk's pipes keep theirs
  in their block entity (`PipeFluid`), written in
  `streaming::BeforeChunkSave`.
- `src/fluid_container.rs`: fluid containers (items with a
  `fluid_container`, the bucket): what one holds is its `ItemData::fluid`
  (a `FluidAmount`); right click takes from or pours into the world or a
  machine's tanks, through `fluid_network::world`'s `take` and `pour`.
- `src/save.rs` (and `src/save/`): the world's folder on disk and its region
  and chunk formats, documented in `docs/world-format.md`. Keep the two in
  step, and bump a format's version when it changes.
- `src/script/`: mod scripts (Luau, through `mlua`): loading each mod's
  `scripts/`, the sandbox, budgets and errors. `api/` is what hooks are
  handed (block handles, the `world` global, players, items, blocks'
  `data`) and `ScriptWorld::call`, which calls a hook with it; `hooks.rs`
  calls blocks' and items' scripts (`on_used`, `on_placed`, `on_broken`,
  `on_neighbour_changed`, items' `on_use`) and `random_tick.rs` gives
  blocks random ticks; `using.rs` turns right clicks into uses of
  scripted blocks (in `block_edit::BlockUseSystems`) and items (after
  every block use). Documented in `docs/scripting.md`.
- `scenarios/`: reusable scenario scripts for `cargo headless`.
- `src/content.rs` (and `src/content/`): mod loading and the `Content`
  registry.
- `assets/`: loaded through `AssetServer`. They hot reload in dev builds.
  Mods are in `assets/mods/`; their JSON5 files load once at startup.
- `docs/modding.md`: the mod file formats.
- `docs/world-format.md`: the save format.
- `docs/decisions.md`: deliberate shortcuts to revisit, and what should
  replace them. Add to it when you make one.
- `scripts/`: helper scripts, such as `pr-screenshots.sh` for PR
  descriptions, `gray-layer.py` for turning images into texture layers,
  `draw-layers.py` for the procedurally drawn layers and
  `roadmap.sh` for keeping the roadmap project in step with the issues.

## Conventions

- The game must be playable without function keys (many laptop keyboards
  need Fn for them): bind gameplay actions to letters, digits and the like
  (C switches creative and survival, E opens the inventory). F-keys are
  only for developer tools, such as the widget gallery (F10, dev builds).

- Breaking changes are fine at this stage: to the mod file formats, the
  save formats, saved worlds, Rust APIs, anything. Don't add backwards
  compatibility, migrations, defaults or fallbacks just to keep old files,
  worlds or code working; change them, and bump a format's version if it
  has one.

- Derive `Reflect` and add `#[reflect(Component)]` on gameplay components.
- Keep systems small and give each one a single job. Prefer events, messages
  and observers over systems that reach into many unrelated queries.
- Clippy's `type_complexity` and `too_many_arguments` are allowed project-wide,
  because Bevy queries trip them. Fix every other warning.
