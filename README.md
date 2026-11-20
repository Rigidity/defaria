<img src="assets/icon.svg" width="64" height="64" alt="">

# defaria-2026

Created in [T3 Code](https://t3.codes).

## Development

Built with [Bevy](https://bevy.org) 0.20.1 on Rust 1.97.1, pinned in `rust-toolchain.toml`.
See [AGENTS.md](AGENTS.md) for the full workflow.

```sh
cargo run                    # dev build: dynamic linking, asset hot reload, BRP on :15702
cargo test                   # headless tests
cargo shot                   # render 60 frames, save target/screenshot.png, exit
cargo play                   # optimized build without dev features
cargo dist                   # shipping build (fat LTO, stripped, panic=abort)
```

The `dev` feature is on by default so every cargo command shares one compiled
Bevy. Only `cargo play` and `cargo dist` turn it off. Never ship a dev build.

### Build performance notes

- Dependencies build at `opt-level = 3` once; our crate builds at `opt-level = 1`,
  so debug builds run at near-release speed while edit-to-rebuild stays ~2s on an M3 Max.
- macOS's default linker (ld-prime) is already as fast as lld/sold, so no linker override.
- `tracing` max levels compile out `trace` logs in debug and everything below `warn` in release.
- New git worktrees: `scripts/seed-worktree-target.sh` clones the main checkout's
  `target/` with APFS copy-on-write, so the worktree doesn't rebuild Bevy from scratch.
- Not enabled (nightly-only, optional): `-Zshare-generics=y`, `-Zthreads=0`, and the
  Cranelift backend (Bevy's docs warn it can crash Bevy apps on macOS).
