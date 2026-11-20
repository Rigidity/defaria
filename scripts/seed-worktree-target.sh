#!/bin/sh
# Seed a new git worktree's target/ from the main checkout using APFS
# copy-on-write clones (`cp -c`): near-instant, and disk blocks stay shared
# until either side rewrites them. Without this, a fresh worktree spends
# ~10 minutes compiling Bevy from scratch.
set -eu

here=$(git rev-parse --show-toplevel)
main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')

if [ "$here" = "$main" ]; then
    echo "seed-worktree-target: in the main checkout, nothing to do"
    exit 0
fi
if [ ! -d "$main/target" ]; then
    echo "seed-worktree-target: $main/target doesn't exist yet, skipping"
    exit 0
fi
if [ -e "$here/target" ]; then
    echo "seed-worktree-target: $here/target already exists, skipping"
    exit 0
fi

cp -cR "$main/target" "$here/target"
echo "seed-worktree-target: cloned $main/target"
