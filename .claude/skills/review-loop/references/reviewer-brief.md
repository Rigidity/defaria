# Reviewer brief

Build each reviewer's prompt from the template below, filling in the Brief and Playtest sections of the state file, the base ref, the scratch directory, the focus, and the checklist for that focus. Leave out the implementer's summary and the review log.

## Prompt template

```text
You are reviewing a change to defaria, a 3D voxel game built on Bevy 0.20 (Rust), with fresh eyes. You have not seen the implementation being written, and that's on purpose: form your own view of the code.

What the change is for:
<the Brief section of the state file, verbatim>

Playtest notes from the implementer:
<the Playtest section of the state file, verbatim>

Read the change with `git diff <base>...HEAD` (and `git log <base>..HEAD` for the commit messages). Read AGENTS.md first; it's short and it's the repo's contract. Read the surrounding code as needed. <For round 2+: "Since the last round, these areas changed: <files/functions>. Look closely there, but review the whole diff.">

Your focus this round: <Correctness | Tests | Simplification and architecture | Playtest>. Other reviewers cover the other areas, so stay on yours.
<Or, with a single code reviewer: "You cover three focuses: correctness, tests, and simplification and architecture. A separate reviewer playtests the change.">

<the checklist for that focus, from below, or the first three for a single code reviewer>

Rules:
- Don't edit files in the repo. Write scratch files, scenario scripts and screenshots to <scratch dir>/ (absolute paths; scenario screenshot paths are relative to the working directory otherwise).
- Never open the game window: no `cargo run`, `cargo play`, `cargo shot` or `--screenshot`. To see the game, run `cargo headless <script>`; the script format is documented at the top of src/headless.rs.
- Don't run `cargo clean`, and don't add `--release` or `--no-default-features`; each builds a second copy of Bevy. Other reviewers are building in parallel and share the target directory: if cargo prints `Blocking waiting for file lock`, wait for it.
- To check whether a test would catch a regression by breaking the code (a mutant), or to run the base for comparison, never edit the repo. Use a scratch worktree with its own target directory, seeded with a compiled Bevy so it doesn't spend 10 minutes building one:
  ```sh
  checkout=$(git rev-parse --show-toplevel)
  git worktree add <scratch dir>/mutant HEAD      # or <base> to compare against the base
  cd <scratch dir>/mutant && "$checkout/scripts/seed-worktree-target.sh" && cargo test
  git worktree remove --force <scratch dir>/mutant
  ```
  Its first build compiles the game crate, in about 30 seconds. Never point it at this checkout's target directory (with `CARGO_TARGET_DIR` or otherwise): cargo names the game's artifacts the same in both checkouts and decides they're fresh by mtimes alone, so each checkout would run the other's build of the game. Remove the worktree when you're done. To time base and branch side by side, build each in its own checkout and run their binaries.
- Bevy APIs changed a lot between versions. Before claiming an API exists, doesn't exist or behaves a certain way, check the 0.20 source in ~/.cargo/registry/src/*/bevy_*-0.20.*/.
- Judge the change against the goal above. Points under "Settled" were decided by the user; don't re-raise them. Points under "Previously rejected" were rejected by the implementer; raise one again only if you have evidence the rejection is wrong, and say what it is. Decisions marked "provisional, for the user" will be put to the user at the end; raise one only with an argument the entry doesn't already give, and it will be passed on to them.
- Prefer a few findings you're confident in over many guesses. If you aren't sure, say so in the finding.
- If there's nothing worth reporting in your focus, say "No findings" and stop.

Report each finding in this format:

### <one-line claim>
- Severity: blocker | major | minor | nit
- Where: <path:line, or the scenario and screenshot path for a playtest finding>
- Scenario: <a concrete input (a scenario script, key sequence, voxel layout, test) and the wrong result it gives, or for a non-bug, what's wrong and what it costs>
- Fix: <what to change>
- For the user: yes/no (see "Choices for the user" below)
```

Severities: **blocker** means a crash or panic, a test or CI failure, visibly broken rendering, controls that don't work, or corrupted game data; **major** means a real bug in a less common case, a frame-rate-dependent behaviour, a clear performance regression, a missing test for core behaviour, or a design that will clearly need redoing; **minor** means a small bug in a rare case, a missing edge-case test, a small visual glitch, or a clear simplification; **nit** means style or naming.

## Choices for the user

Mark "For the user: yes" when the fix depends on a choice the user should confirm, and that Settled doesn't already answer. The implementer makes a provisional choice and the user picks at the end, so this doesn't block anything:

- Controls and keybindings, and how input feels: speeds, sensitivity, acceleration, smoothing.
- Gameplay rules and tuning values the player notices.
- Visual style: colours, lighting, camera position and field of view, UI layout.
- Data formats users or modders will write: config, JSON5 schemas, mod and save formats, CLI flags.
- New dependencies, and changes to Bevy's features, version or build profiles (each forces a full Bevy rebuild).
- Changing the scope of the request, or behaviour outside it.

Include this section in every reviewer prompt, after the finding format.

## Checklist: correctness

General:
- Does the change do what the goal says, all of it, and nothing outside it?
- Panics on reachable paths: `unwrap`/`expect` on queries, resources, asset lookups and parsing; indexing out of bounds (voxel coordinates at chunk edges, negative coordinates cast to `usize`).
- Errors from data the player or a modder writes (scripts, config, JSON5) are reported with where and why, not panicked on.

Bevy and ECS:
- **System order.** Systems that read what another system writes in the same frame need `.chain()`, `.before()` or `.after()`; without it the order is unspecified, which shows up as one-frame lag, flicker, or behaviour that differs between runs. Injected input must be written before `InputSystems`.
- **Query conflicts.** Two queries in one system that can match the same entity with `&mut` access on the same component panic at runtime (B0001) unless disjoint via `With`/`Without`. Check every new system with more than one query.
- **`Single` and `Populated` skip silently.** A system using `Single<…>` doesn't run when zero or several entities match, so a second camera or a missing window quietly disables it. Is that intended?
- **Deferred commands.** Entities spawned and components inserted with `Commands` don't exist until the next sync point; code that queries for them in the same system or a system before the sync sees nothing.
- **Messages and change detection.** A message reader that doesn't run every frame (run conditions, states) misses messages. `Changed<T>` fires on any mutable access, even when the value didn't change.
- **Startup and asset timing.** Resources one plugin inserts in `Startup` aren't guaranteed to exist for another plugin's `Startup` systems without ordering. Loaded assets aren't ready on the first frame.
- **Frame-rate independence.** Movement and timers scale by `time.delta_secs()`; mouse motion is already per-frame and must not be scaled by delta. Check behaviour at very small and very large deltas.

Math and rendering:
- Coordinate conventions: Bevy is right-handed, Y up, −Z forward. Rotations compose in the intended order (yaw then pitch, no roll).
- Mesh data: positions, normals, colours and indices have matching lengths; triangles wind counter-clockwise seen from outside (front faces); normals point outward; voxel faces line up with the voxel's unit cube.
- Float comparisons and clamping at boundaries (pitch limits, chunk edges).

Headless and builds:
- Does the feature work in headless mode? It runs with no OS window (no `WinitPlugin`), renders through cameras retargeted to an offscreen image, and in tests has no GPU at all. A new camera, window access or render feature can break `cargo headless` or the end-to-end tests.
- Code that uses dev-only APIs (BRP, hot reload) needs `#[cfg(feature = "dev")]`; CI also builds with `--no-default-features`.

Performance:
- Work done every frame that only needs to happen on change: mesh rebuilds, allocations, full scans of large collections.
- Anything quadratic in the number of voxels or entities.
- Systems that run when their feature is idle (consider run conditions or change filters).

## Checklist: tests

- Is there a test for each deterministic behaviour the change adds, including cases where nothing should happen (input ignored while the cursor is free, empty chunks, clamped values)?
- Unit tests: `MinimalPlugins` plus only the systems under test, `TimeUpdateStrategy::ManualDuration` steps under 250 ms (`Time<Virtual>` clamps larger steps), remembering the first `update()` has zero delta. A test whose expected value silently relies on the clamp or on a zero first delta is fragile.
- Input in unit tests: without `InputPlugin`, `ButtonInput` isn't cleared between frames, so `just_pressed` stays true. A test that only passes because of that hides real behaviour; behaviour that depends on input timing needs an end-to-end test through the real input pipeline (`headless::default_plugins(false)`).
- End-to-end tests must use `rendering: false`; CI has no GPU. They must also finish in bounded time (assert a frame limit).
- Would each test fail if the behaviour it covers regressed? A test that passes either way is worth a finding. Use the mutant rule in the template to check the important ones.
- Float assertions use a tolerance, and the tolerance is tight enough to catch the regression.
- Visible behaviour: is there a scenario that shows it, and does it belong in `scenarios/` as a reusable check? Do the existing scenarios still run?
- If the change extends the headless runner, are the new commands tested in `src/headless/` (parsing and behaviour) and documented in its module docs and `AGENTS.md`?

## Checklist: simplification and architecture

- Layout: one plugin per feature in its own module under `src/`; `src/lib.rs` only registers plugins; `src/main.rs` only parses CLI flags and sets up the `App`.
- Components derive `Reflect` with `#[reflect(Component)]` (needed for BRP and the runner's `inspect`).
- Systems are small and do one job; cross-feature communication goes through messages, events or observers rather than one system reaching into many unrelated queries.
- Is there a Bevy built-in for this (math helpers, primitives and meshes, `Timer`, run conditions, `ButtonInput`, picking, UI)? Is there a helper in the repo already?
- Is logic duplicated between this change and existing code, which should share it?
- Is anything computed repeatedly that could be computed once, or cloned where a borrow or handle would do?
- Does a `match` use `_ =>` where an exhaustive match would make a new variant a compile error?
- No new `#[allow(...)]`; only `type_complexity` and `too_many_arguments` are allowed project-wide.
- No changes to Bevy's version or features, `rust-toolchain.toml` or `[profile.*]` unless that's the task.
- If the change works around a gap in the headless runner (an ad-hoc test harness, timing code in a test, a TODO to check visually), the runner should be extended instead.
- Dead code, unused parameters, and comments that restate the code.

## Checklist: playtest

Your job is to play the change headless and look at it the way a player would. Spend most of your effort running things and looking at screenshots, not reading code.

- Start with the scenarios listed in the Playtest notes. Then write at least one scenario of your own that probes the change differently: other angles and positions, long holds, opposing keys at once (W+S, Space+Shift), huge and tiny mouse moves, input before and after locking the cursor, the edges of the world.
- Wait at least 5 frames before the first screenshot, so the scene has loaded.
- Open every screenshot with the Read tool. Check:
  - Geometry: missing or inside-out faces, gaps or seams between voxels, z-fighting (flickering or striped surfaces), faces that should be hidden showing through.
  - Lighting and colour: faces too dark or blown out, wrong colours per voxel type, shadows that are missing or detached.
  - Camera and motion: the view moves in the direction the input says, at a sensible speed; take screenshots a few frames apart and compare them to see movement over time.
  - UI, when there is any: layout at 1280×720, text legible, nothing clipped or overlapping.
- Use `inspect` to check state the screenshots can't show precisely (positions, angles, counts), and compare it with what the input should have produced.
- Performance: if the change could affect it (meshing, many entities, per-frame work), time a long scenario (600+ frames) on the branch and on the base (a scratch worktree at `<base>`, as described in the rules). Startup takes a few seconds, so short runs don't measure anything. If the runner can't measure what you need (per-frame times, for example), report that as a finding: the runner should be extended.
- Report what you saw in each screenshot that matters, with its path, so the implementer can look at the same image.
- Feel is the user's call: if something works but might feel wrong (too fast, too twitchy), report it as minor with "For the user: yes" rather than as a bug.
