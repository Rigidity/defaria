//! Random ticks: now and then, a block whose script defines
//! `on_random_tick` gets it called, so plants grow and moss spreads at
//! their own pace without running every block every tick.
//!
//! Each tick (`FixedUpdate`, 60 a second) picks [`RandomTicks::per_chunk`]
//! voxels at random in each loaded chunk that has such a block in it, and
//! queues the hook for those that are one, at most
//! [`RandomTicks::max_per_tick`] a tick (chosen at random among them, so a
//! busy world slows every block down evenly rather than starving some).
//! With one per chunk a tick, a block is ticked about once a minute on
//! average, as in Minecraft. While more than a tick's worth of block
//! hooks ([`HookSettings::max_per_tick`]) is waiting, no random ticks are
//! picked, so they don't add to a backlog.
//!
//! Which chunks have random-ticking blocks is kept as they load and as
//! blocks are placed, so chunks without any cost nothing. A chunk whose
//! last such block is broken stays on the list until it unloads; picking
//! in it finds nothing.

use bevy::{platform::collections::HashSet, prelude::*};

use super::hooks::{Hook, HookSettings, HookSystems, PendingHooks, ScriptIndex};
use crate::{
    content::Content,
    terrain::{ChunkCoord, ChunkMap},
    voxel::{CHUNK_VOLUME, Chunk},
};

pub(super) fn plugin(app: &mut App) {
    app.register_type::<RandomTicks>()
        .init_resource::<RandomTicks>()
        .init_resource::<TickingChunks>()
        .add_systems(
            FixedUpdate,
            (find_ticking_chunks, pick_random_ticks)
                .chain()
                // After the edits and uses, so the queue's order is fixed.
                .after(super::hooks::collect_uses)
                .in_set(HookSystems::Collect),
        );
}

/// How often blocks get random ticks.
#[derive(Resource, Reflect, Debug, Clone, PartialEq)]
#[reflect(Resource)]
pub struct RandomTicks {
    /// Voxels picked in each chunk each tick.
    pub per_chunk: u32,
    /// The most `on_random_tick` calls in one tick.
    pub max_per_tick: usize,
    /// The random number generator's state.
    pub seed: u64,
}

impl Default for RandomTicks {
    fn default() -> Self {
        Self {
            per_chunk: 1,
            max_per_tick: 64,
            seed: 0x0005_EED0_FDEF_A41A,
        }
    }
}

impl RandomTicks {
    /// The next random number (SplitMix64).
    fn next(&mut self) -> u64 {
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A random number below `n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The loaded chunks that may have a random-ticking block in them.
#[derive(Resource, Debug, Default)]
pub struct TickingChunks(HashSet<ChunkCoord>);

impl TickingChunks {
    /// Notes that the voxel at `pos` ticks randomly.
    pub fn add(&mut self, pos: IVec3) {
        self.0.insert(ChunkCoord::split(pos).0);
    }

    pub fn contains(&self, coord: ChunkCoord) -> bool {
        self.0.contains(&coord)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Looks through each chunk that's loaded (every loaded chunk, when the
/// scripts reloaded) for random-ticking blocks.
fn find_ticking_chunks(
    chunks: Query<(&ChunkCoord, Ref<Chunk>)>,
    content: Res<Content>,
    index: Res<ScriptIndex>,
    mut ticking: ResMut<TickingChunks>,
) {
    if !index.any_random_tick() {
        return;
    }
    let all = index.is_changed();
    for (coord, chunk) in &chunks {
        if !(all || chunk.is_added()) || ticking.0.contains(coord) {
            continue;
        }
        if chunk
            .voxels()
            .iter()
            .any(|&state| index.ticks_randomly(&content, state))
        {
            ticking.0.insert(*coord);
        }
    }
}

/// Picks this tick's random ticks.
fn pick_random_ticks(
    mut settings: ResMut<RandomTicks>,
    mut ticking: ResMut<TickingChunks>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    content: Res<Content>,
    index: Res<ScriptIndex>,
    mut pending: ResMut<PendingHooks>,
    hooks: Res<HookSettings>,
) {
    // A tick's worth of hooks is already waiting: random ticks would only
    // add to the backlog.
    if ticking.is_empty() || pending.len() > hooks.max_per_tick {
        return;
    }
    let settings = &mut *settings;
    let mut picked = Vec::new();
    ticking.0.retain(|&coord| {
        let Some(chunk) = map.get(coord).and_then(|entity| chunks.get(entity).ok()) else {
            // Unloaded: it's looked through again if it loads.
            return false;
        };
        for _ in 0..settings.per_chunk {
            let voxel = settings.below(CHUNK_VOLUME);
            if index.ticks_randomly(&content, chunk.voxels()[voxel]) {
                picked.push(coord.origin() + Chunk::position_of(voxel));
            }
        }
        true
    });
    // Too many: a random few, by a partial shuffle.
    let keep = picked.len().min(settings.max_per_tick);
    for i in 0..keep {
        let j = i + settings.below(picked.len() - i);
        picked.swap(i, j);
    }
    for &pos in &picked[..keep] {
        pending.push(Hook::RandomTick(pos));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_numbers_cover_the_range_evenly() {
        let mut ticks = RandomTicks::default();
        let mut counts = [0u32; 8];
        for _ in 0..8000 {
            counts[ticks.below(8)] += 1;
        }
        assert!(
            counts.iter().all(|&c| (800..1200).contains(&c)),
            "{counts:?}"
        );
    }
}
