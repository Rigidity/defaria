# Agent guide

A Bevy 0.20 game, developed mostly by agents. Rust is pinned in
`rust-toolchain.toml`.

## Commands

| Task | Command |
|---|---|
| Build / run (dev) | `cargo build` / `cargo run` |
| Tests | `cargo test` |
| Lint (must be clean) | `cargo clippy --all-targets -- -D warnings` |
| Format | `cargo fmt` |
| See the game | `cargo shot` → open `target/screenshot.png` |
| Optimized run | `cargo play` |
| Shipping build | `cargo dist` |

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

## Verifying changes

Nobody is watching the window, so check your work yourself:

1. **Logic:** write headless tests. Build an `App` with `MinimalPlugins`, add
   only the systems under test, step time with
   `TimeUpdateStrategy::ManualDuration`, and call `app.update()`. See the test
   in `src/spinner.rs`.
2. **Visuals:** `cargo run -- --screenshot <path.png> [--frames N]` renders N
   frames (default 60), saves a PNG, and exits. Open the PNG to look at it.
   Don't minimize or cover the window while it runs, or the capture can come
   out black.
3. **Live state:** `cargo run` starts the Bevy Remote Protocol on
   `127.0.0.1:15702` (set `BRP_PORT` to use another port when several games
   run at once). Example:
   ```sh
   curl -s localhost:15702 -d '{"jsonrpc":"2.0","id":1,"method":"world.query","params":{"data":{"components":["defaria_2026::spinner::Spin"]}}}'
   ```
   Components only show up over BRP if they derive `Reflect` with
   `#[reflect(Component)]`.

## Code layout

- `src/main.rs`: CLI flags and `App` setup only.
- `src/lib.rs`: `GamePlugin`, which registers one plugin per feature.
- `src/<feature>.rs` (or `src/<feature>/`): one plugin per gameplay feature,
  with its components, systems and tests. Put new features in new modules so
  parallel work doesn't collide. Touch `lib.rs` only to register the plugin.
- `src/devtools.rs`: screenshot mode and BRP.
- `assets/`: loaded through `AssetServer`. They hot reload in dev builds.

## Conventions

- Derive `Reflect` and add `#[reflect(Component)]` on gameplay components.
- Keep systems small and give each one a single job. Prefer events, messages
  and observers over systems that reach into many unrelated queries.
- Clippy's `type_complexity` and `too_many_arguments` are allowed project-wide,
  because Bevy queries trip them. Fix every other warning.
