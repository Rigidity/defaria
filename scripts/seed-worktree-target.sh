#!/bin/sh
# Give a worktree its own target/ with Bevy already compiled.
#
#   scripts/seed-worktree-target.sh          seed this worktree's target/
#   scripts/seed-worktree-target.sh --save   make this worktree's target/ the seed
#
# The seed is a target directory holding only the dependencies (Bevy and
# the rest), with none of the game crate's artifacts. Seeding clones it with
# APFS copy-on-write (`cp -c`): it takes a few seconds, and disk blocks stay
# shared until one side rewrites them. Without it a new worktree spends about
# 10 minutes compiling Bevy.
#
# Each worktree needs its own target/. Cargo names the game's artifacts with
# a hash that doesn't include the checkout's path, and decides they are fresh
# by comparing mtimes alone, so worktrees sharing a target directory run each
# other's builds of the game. For the same reason, seeding deletes any game
# artifacts it finds: a cloned build of other sources must never look fresh.
#
# After a Bevy or dependency change, build in a worktree that has it
# (`cargo test --no-run` and `cargo clippy --all-targets`) and run `--save`
# there, or every new worktree compiles the new dependencies itself.
#
# T3 Code runs this when it creates a worktree. DEFARIA_TARGET_SEED overrides
# where the seed lives.
set -eu

seed=${DEFARIA_TARGET_SEED:-$HOME/.cache/cargo-targets/defaria-seed}
here=$(git rev-parse --show-toplevel)
target=$here/target

# Every unit of the game crate (lib, bin, tests, build outputs) is named
# `defaria…` or `libdefaria…` in a profile directory and its subdirectories.
game_artifacts() {
    for profile in "$1"/*/; do
        [ -e "$profile.cargo-lock" ] || continue
        for dir in "$profile" "$profile"build/ "$profile"deps/ \
            "$profile"incremental/ "$profile".fingerprint/; do
            [ -d "$dir" ] || continue
            find "$dir" -maxdepth 1 -mindepth 1 \
                \( -name 'defaria*' -o -name 'libdefaria*' \)
        done
    done
}

# The target directory cargo uses here must be target/, not one shared with
# other checkouts through a parent .cargo/config.toml or CARGO_TARGET_DIR.
check_target_dir() {
    actual=$(cargo metadata --format-version 1 --no-deps |
        python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
    if [ "$actual" != "$target" ]; then
        echo "seed-worktree-target: cargo builds into $actual, not $target." >&2
        echo "  Remove the target-dir setting (or CARGO_TARGET_DIR) that shares it." >&2
        exit 1
    fi
}

save() {
    if [ ! -d "$target" ]; then
        echo "seed-worktree-target: $target doesn't exist; build first" >&2
        exit 1
    fi
    new=$seed.new.$$
    rm -rf "$new"
    mkdir -p "$new"
    [ -f "$target/CACHEDIR.TAG" ] && cp -c "$target/CACHEDIR.TAG" "$new/"
    # Only profile directories: screenshots and scenario output stay behind.
    for profile in "$target"/*/; do
        [ -e "$profile.cargo-lock" ] || continue
        name=$(basename "$profile")
        mkdir "$new/$name"
        # Clone build/ one package at a time, so a large build/defaria is
        # never cloned only to be deleted.
        for entry in "$profile".[!.]* "$profile"*; do
            [ -e "$entry" ] || continue
            base=$(basename "$entry")
            case $base in
            defaria* | libdefaria*) ;;
            build)
                mkdir "$new/$name/build"
                for package in "$entry"/*; do
                    case $(basename "$package") in
                    defaria*) ;;
                    *) cp -cR "$package" "$new/$name/build/" ;;
                    esac
                done
                ;;
            *) cp -cR "$entry" "$new/$name/" ;;
            esac
        done
    done
    game_artifacts "$new" | while read -r path; do rm -rf "$path"; done
    if [ -d "$seed" ]; then
        mv "$seed" "$seed.old.$$"
        mv "$new" "$seed"
        rm -rf "$seed.old.$$"
    else
        mkdir -p "$(dirname "$seed")"
        mv "$new" "$seed"
    fi
    echo "seed-worktree-target: saved $target as the seed ($seed)"
}

seed_here() {
    if [ ! -d "$seed" ]; then
        echo "seed-worktree-target: no seed at $seed yet, skipping"
        echo "  (build here, then run this with --save to make one)"
        exit 0
    fi
    if [ -d "$target/debug" ]; then
        echo "seed-worktree-target: $target/debug already exists, skipping"
        exit 0
    fi
    mkdir -p "$target"
    cp -cR "$seed/" "$target"
    game_artifacts "$target" | while read -r path; do rm -rf "$path"; done
    echo "seed-worktree-target: seeded $target from $seed"
}

case ${1-} in
--save)
    check_target_dir
    save
    ;;
"")
    check_target_dir
    seed_here
    ;;
*)
    echo "usage: $0 [--save]" >&2
    exit 2
    ;;
esac
