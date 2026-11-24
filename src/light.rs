//! Block light and sky light, stored per voxel and spread through the world
//! the way Minecraft does it.
//!
//! Every voxel has four light levels from 0 to [`MAX_LIGHT`]: the sky's, and
//! red, green and blue block light. They're independent: each spreads on its
//! own and the shader mixes them.
//!
//! - **Block light** starts at blocks that give off light (their `light` in
//!   the mod files) and drops by one level per block it travels, around
//!   corners but not through opaque blocks, so a level-15 lamp lights blocks
//!   up to 14 away.
//! - **Sky light** comes down from the top of the world at full strength and
//!   stays at full strength straight down until it hits an opaque block. From
//!   every voxel it reaches it spreads sideways and into caves like block
//!   light, dropping by one per block.
//!
//! Light is kept up to date incrementally by [`propagate`]'s flood fill:
//! when a chunk loads, its own sources and its loaded neighbours' light
//! spread into it; when a voxel changes, the light that came through it is
//! taken back and spread again around it. A chunk that isn't loaded lets no
//! light through, and a chunk that loads later fills in, so the result
//! doesn't depend on the order chunks load in. Because light reaches less
//! than a chunk's width, a change only ever touches the chunk it's in and the
//! chunks around it.
//!
//! Chunk meshes bake the light into their vertices (see `voxel`), so a chunk
//! is remeshed when its light, or the light just across its border,
//! changes. Its `ChunkLight` is marked changed for that.

mod propagate;

use bevy::{platform::collections::HashSet, prelude::*};

pub use crate::content::MAX_LIGHT;
use crate::{
    content::Content,
    terrain::{
        ChunkCoord, ChunkMap, RemeshChunks, VoxelChanged, WORLD_CHUNKS, remesh_changed_chunks,
    },
    voxel::{CHUNK_VOLUME, Chunk},
};
pub use propagate::{ChunkStore, LightQueue, LightTable};

/// The sky's channel in a [`Light`]; red, green and blue follow it.
pub const SKY: usize = 0;
/// Light channels: the sky, then red, green and blue block light.
pub const CHANNELS: usize = 4;

pub struct LightPlugin;

impl Plugin for LightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightTable>()
            .init_resource::<LightQueue>()
            .init_resource::<SkyLight>()
            .add_systems(
                Update,
                (queue_loaded_chunks, queue_changed_voxels, propagate_light)
                    .chain()
                    .in_set(RemeshChunks)
                    .before(remesh_changed_chunks),
            );
    }
}

/// The colour of sky light at full strength. One colour for the whole world.
#[derive(Resource, Reflect, Debug, Clone, Copy)]
#[reflect(Resource)]
pub struct SkyLight(pub Color);

impl Default for SkyLight {
    fn default() -> Self {
        Self(Color::WHITE)
    }
}

/// The light levels at one voxel: four channels of 4 bits, the sky's in the
/// lowest bits, then red, green and blue.
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Light(pub u16);

impl Light {
    pub const DARK: Self = Self(0);

    pub const fn new(sky: u8, [red, green, blue]: [u8; 3]) -> Self {
        Self(sky as u16 | (red as u16) << 4 | (green as u16) << 8 | (blue as u16) << 12)
    }

    /// The level in one channel: [`SKY`], or 1 to 3 for red, green and blue.
    pub const fn get(self, channel: usize) -> u8 {
        ((self.0 >> (channel * 4)) & 0xF) as u8
    }

    pub fn set(&mut self, channel: usize, level: u8) {
        debug_assert!(level <= MAX_LIGHT);
        let shift = channel * 4;
        self.0 = (self.0 & !(0xF << shift)) | (level as u16) << shift;
    }

    #[cfg(test)]
    pub const fn sky(self) -> u8 {
        self.get(SKY)
    }

    /// Red, green and blue block light.
    #[cfg(test)]
    pub const fn block(self) -> [u8; 3] {
        [self.get(1), self.get(2), self.get(3)]
    }
}

/// The light at every voxel of a [`Chunk`], in the same order. Every chunk
/// has one; it starts dark and [`propagate_light`] fills it in.
#[derive(Component, Reflect, Debug, Clone, PartialEq)]
#[reflect(Component)]
pub struct ChunkLight {
    levels: Vec<Light>,
}

impl Default for ChunkLight {
    fn default() -> Self {
        Self {
            levels: vec![Light::DARK; CHUNK_VOLUME],
        }
    }
}

impl ChunkLight {
    /// The light at a chunk-local position.
    pub fn at(&self, pos: IVec3) -> Light {
        self.levels[Chunk::index_of(pos)]
    }

    #[cfg(test)]
    pub fn set_at(&mut self, pos: IVec3, light: Light) {
        self.levels[Chunk::index_of(pos)] = light;
    }

    fn get(&self, index: usize) -> Light {
        self.levels[index]
    }

    fn set(&mut self, index: usize, light: Light) {
        self.levels[index] = light;
    }
}

/// Queues the light for chunks that just loaded: their own sources, the sky
/// for chunks at the top of the world, and their loaded neighbours' light.
fn queue_loaded_chunks(
    chunks: Query<(&ChunkCoord, &Chunk), Added<Chunk>>,
    map: Res<ChunkMap>,
    table: Res<LightTable>,
    mut queue: ResMut<LightQueue>,
) {
    for (coord, chunk) in &chunks {
        let under_sky = coord.0.y == WORLD_CHUNKS.y - 1;
        queue.chunk_loaded(coord.0, chunk, &table, under_sky, |neighbour| {
            map.get(ChunkCoord(neighbour)).is_some()
        });
    }
}

/// Queues the light around every voxel that changed.
fn queue_changed_voxels(mut changes: MessageReader<VoxelChanged>, mut queue: ResMut<LightQueue>) {
    for change in changes.read() {
        queue.voxel_changed(change.pos);
    }
}

/// The chunks, for the flood fill: change detection is left alone, and the
/// chunks whose light changed are marked afterwards.
struct EcsChunks<'a, 'w, 's> {
    map: &'a ChunkMap,
    chunks: &'a mut Query<'w, 's, (&'static Chunk, &'static mut ChunkLight)>,
}

impl ChunkStore for EcsChunks<'_, '_, '_> {
    fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool {
        let Some(entity) = self.map.get(ChunkCoord(coord)) else {
            return false;
        };
        let Ok((chunk, mut light)) = self.chunks.get_mut(entity) else {
            return false;
        };
        f(chunk, light.bypass_change_detection());
        true
    }
}

/// Runs the flood fill for everything queued, then marks the chunks whose
/// meshes show light that changed.
pub fn propagate_light(
    mut queue: ResMut<LightQueue>,
    table: Res<LightTable>,
    map: Res<ChunkMap>,
    mut chunks: Query<(&'static Chunk, &'static mut ChunkLight)>,
) {
    if queue.is_empty() {
        return;
    }
    let remesh: HashSet<IVec3> = queue.run(
        &mut EcsChunks {
            map: &map,
            chunks: &mut chunks,
        },
        &table,
    );
    for coord in remesh {
        if let Some(entity) = map.get(ChunkCoord(coord))
            && let Ok((_, mut light)) = chunks.get_mut(entity)
        {
            light.set_changed();
        }
    }
}

impl FromWorld for LightTable {
    fn from_world(world: &mut World) -> Self {
        Self::new(world.resource::<Content>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_packs_four_channels() {
        let mut light = Light::new(15, [1, 2, 3]);
        assert_eq!((light.sky(), light.block()), (15, [1, 2, 3]));
        light.set(2, 9);
        assert_eq!((light.sky(), light.block()), (15, [1, 9, 3]));
        light.set(SKY, 0);
        assert_eq!(light, Light::new(0, [1, 9, 3]));
    }
}
