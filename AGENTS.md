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
- If cargo prints `Blocking waiting for file lock`, another build is using this
  checkout. Wait for it; don't kill it.

## Don't open the game window

Don't launch the game window yourself: no `cargo run`, `cargo play`,
`cargo shot` or `--screenshot` runs. The user plays the game and judges how it
feels. Only open the window when the user explicitly asks. Use headless mode
(below) to see the game instead.

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
input types, new kinds of output), extend `src/headless.rs`, document the new
command, and add a test for it. Keep `scenarios/` working when the game
changes.

1. **Logic:** write headless tests. Build an `App` with `MinimalPlugins`, add
   only the systems under test, step time with
   `TimeUpdateStrategy::ManualDuration`, and call `app.update()`. See the tests
   in `src/fly_camera.rs`. `Time<Virtual>` clamps each step to 250ms, so keep
   manual steps below that.
2. **Gameplay and visuals:** write a scenario script and run
   `cargo headless <script>`. It runs the whole game with no window, rendering
   offscreen, feeds it the script's input at a fixed 60 fps, and exits when the
   script ends. `screenshot` saves PNGs you can open; `inspect` prints
   components to stdout. Example (`scenarios/fly-around.txt`):
   ```text
   wait 5
   hold MouseLeft 1      # lock the cursor
   hold KeyW 60          # fly forward for one second
   mouse 150 0           # turn right
   wait 1
   screenshot target/scenarios/fly-around/after.png
   inspect FlyCamera     # short or full reflected type name
   ```
   Commands: `wait`, `press`, `release`, `hold`, `mouse`, `screenshot`,
   `inspect`. The full format is documented at the top of `src/headless.rs`.
   Keys are `KeyCode` names; mouse buttons are `MouseLeft`/`MouseRight`/
   `MouseMiddle`. Add `--no-render` to run without a GPU (screenshots are
   skipped); CI runs every scenario in `scenarios/` that way, so they must keep
   working. Write screenshots under `target/`. Keep reusable scenarios in
   `scenarios/`; put one-off ones in your scratch directory.
3. **End-to-end tests:** the same scripts run inside `cargo test` with no GPU
   (screenshots are skipped there). Build the app with
   `headless::default_plugins(false)`, `GamePlugin` and `HeadlessPlugin`, step it
   until it exits, then assert on the world. See
   `script_drives_the_fly_camera` in `src/headless.rs`. CI has no GPU, so tests
   must use `rendering: false`.
4. **Live state:** if the user has the game running and asks you to inspect
   it, the Bevy Remote Protocol listens on `127.0.0.1:15702` (or `BRP_PORT`).
   Example:
   ```sh
   curl -s localhost:15702 -d '{"jsonrpc":"2.0","id":1,"method":"world.query","params":{"data":{"components":["defaria::fly_camera::FlyCamera"]}}}'
   ```
   Components only show up in BRP and `inspect` if they derive `Reflect` with
   `#[reflect(Component)]`.

## Reviewed PRs

When asked to take work through review ("with reviews", "drive this to
completion", "review until clean"), follow `.claude/skills/review-loop/`. It
runs rounds of code review and headless playtesting, fixes what they find, and
ends in a PR. Agents other than Claude Code can read `SKILL.md` there and
follow it directly.

## Code layout

- `src/main.rs`: CLI flags and `App` setup only.
- `src/lib.rs`: `GamePlugin`, which registers one plugin per feature.
- `src/<feature>.rs` (or `src/<feature>/`): one plugin per gameplay feature,
  with its components, systems and tests. Put new features in new modules so
  parallel work doesn't collide. Touch `lib.rs` only to register the plugin.
- `src/devtools.rs`: CLI flags, windowed screenshot mode and BRP.
- `src/headless.rs`: headless mode and the scenario script runner.
- `scenarios/`: reusable scenario scripts for `cargo headless`.
- `assets/`: loaded through `AssetServer`. They hot reload in dev builds.

## Conventions

- Derive `Reflect` and add `#[reflect(Component)]` on gameplay components.
- Keep systems small and give each one a single job. Prefer events, messages
  and observers over systems that reach into many unrelated queries.
- Clippy's `type_complexity` and `too_many_arguments` are allowed project-wide,
  because Bevy queries trip them. Fix every other warning.
