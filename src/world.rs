//! The world's voxels: chunks, where they are, and reading and writing
//! them. What everything else depends on; `worldgen` decides what goes in
//! new chunks and `meshing` draws them.
//!
//! The world goes on forever in every direction, up and down too, in
//! 16×16×16 chunks ([`Chunk`]). Chunks are loaded and unloaded around the
//! player (see `streaming`), so [`ChunkMap`] only holds the loaded ones.
//! Editing a chunk through [`Voxels`] marks it, and the chunks that show
//! the voxel, changed, which is all it takes to update what's drawn.

use bevy::{platform::collections::HashMap, prelude::*};

use crate::content::BlockState;

mod chunk;
mod chunk_array;
mod fluid_top;
mod voxels;

#[cfg(test)]
pub use chunk::split_fluid;
pub use chunk::{
    CHUNK_SIZE, CHUNK_VOLUME, Chunk, FLUID_STEPS, VoxelFluid, axis_of, filled, fluid_step,
    fluid_surface, offsets_within, touching_offsets,
};
pub use chunk_array::ChunkArray;
pub use fluid_top::FluidTop;
pub use voxels::{MARGIN, Voxels};

/// Where changed chunks are relit and redrawn. Systems that edit chunks
/// run before it, so their edits show on the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemeshChunks;

/// A chunk with a chunk around it that isn't loaded yet, so it isn't
/// drawn: its faces against the missing chunks would be wrong. Whatever
/// loads the chunks takes it off once they are. Chunks without it are
/// drawn as soon as they spawn.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct AwaitingNeighbours;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<FluidChanged>()
            .add_message::<PropertiesUpdated>()
            .add_systems(
                Last,
                chunk_array::compact_changed_chunks.in_set(ChunkCompaction),
            );
    }
}

/// Where the chunks that changed this frame are compacted (see
/// [`ChunkArray::compact`]), in [`Last`]. Systems that measure chunk
/// storage go after it, to see this frame's edits compacted.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChunkCompaction;

/// Which chunk of the world grid this is. Chunk `c` covers voxels
/// `c * CHUNK_SIZE` up to (not including) `(c + 1) * CHUNK_SIZE`.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[reflect(Component)]
pub struct ChunkCoord(pub IVec3);

impl ChunkCoord {
    /// The world position of the chunk's `(0, 0, 0)` voxel.
    pub fn origin(self) -> IVec3 {
        self.0 * CHUNK_SIZE as i32
    }

    /// The chunk holding world voxel `pos`, and `pos` within that chunk.
    pub fn split(pos: IVec3) -> (Self, IVec3) {
        let size = IVec3::splat(CHUNK_SIZE as i32);
        (Self(pos.div_euclid(size)), pos.rem_euclid(size))
    }

    /// The chunks whose meshes show world voxel `pos`: its own, and every
    /// chunk across the edges and corners it touches (a voxel shades the
    /// corners of faces next to it, and those can be in any of them).
    pub fn touching(pos: IVec3) -> impl Iterator<Item = ChunkCoord> {
        let (coord, local) = Self::split(pos);
        touching_offsets(local).map(move |offset| ChunkCoord(coord.0 + offset))
    }

    /// The chunks within `reach` voxels of the voxel at `pos`, its own
    /// among them: see [`offsets_within`].
    pub fn within(pos: IVec3, reach: i32) -> impl Iterator<Item = ChunkCoord> {
        let (coord, local) = Self::split(pos);
        offsets_within(local, reach).map(move |offset| ChunkCoord(coord.0 + offset))
    }

    /// The 26 chunks around this one.
    pub fn neighbours(self) -> impl Iterator<Item = ChunkCoord> {
        (-1..=1)
            .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| IVec3::new(x, y, z))))
            .filter(|&offset| offset != IVec3::ZERO)
            .map(move |offset| ChunkCoord(self.0 + offset))
    }
}

/// A voxel was set to a different block through [`Voxels`].
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelChanged {
    pub pos: IVec3,
    /// The block it was.
    pub before: BlockState,
    /// The block it is now.
    pub after: BlockState,
    /// The fluid it held before.
    pub fluid_before: Option<VoxelFluid>,
    /// The fluid it holds now: as much of `fluid_before` as the new block
    /// leaves room for.
    pub fluid_after: Option<VoxelFluid>,
}

impl VoxelChanged {
    /// How much fluid the new block took the place of: what it held
    /// before that it doesn't now.
    pub fn displaced(&self) -> Option<VoxelFluid> {
        let before = self.fluid_before?;
        let left = self
            .fluid_after
            .filter(|after| after.kind == before.kind)
            .map_or(0, |after| after.amount);
        VoxelFluid::of(before.kind, before.amount.saturating_sub(left))
    }
}

/// The fluid in a voxel changed, without its block changing, through
/// [`Voxels::set_fluid`] or flow.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidChanged {
    pub pos: IVec3,
    /// Whether flow moved it, rather than something else setting it. Flow
    /// wakes the fluid around what it moves itself.
    pub by_flow: bool,
    pub before: Option<VoxelFluid>,
    pub after: Option<VoxelFluid>,
}

/// The engine set some of a voxel's properties from the world around it,
/// through [`Voxels::update`], such as which sides a pipe joins (see
/// `connections`). The block is the same one, so this isn't an edit: it
/// doesn't wake fluid, and a fluid in the voxel stays.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertiesUpdated {
    pub pos: IVec3,
    pub before: BlockState,
    /// Another state of the same block.
    pub after: BlockState,
}

/// Every loaded chunk's entity, by its coordinate.
#[derive(Resource, Debug, Default)]
pub struct ChunkMap(HashMap<ChunkCoord, Entity>);

impl ChunkMap {
    pub fn get(&self, coord: ChunkCoord) -> Option<Entity> {
        self.0.get(&coord).copied()
    }

    pub fn insert(&mut self, coord: ChunkCoord, chunk: Entity) {
        self.0.insert(coord, chunk);
    }

    pub fn remove(&mut self, coord: ChunkCoord) -> Option<Entity> {
        self.0.remove(&coord)
    }
}

/// Spawns a chunk entity at its place in the world and adds it to `map`.
pub fn spawn_chunk(
    commands: &mut Commands,
    map: &mut ChunkMap,
    coord: ChunkCoord,
    chunk: Chunk,
    extra: impl Bundle,
) -> Entity {
    let IVec3 { x, y, z } = coord.0;
    let entity = commands
        .spawn((
            Name::new(format!("Chunk ({x}, {y}, {z})")),
            Transform::from_translation(coord.origin().as_vec3()),
            // Its parts (see `ChunkParts`) inherit it.
            Visibility::default(),
            coord,
            chunk,
            extra,
        ))
        .id();
    map.insert(coord, entity);
    entity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_finds_the_chunk_and_local_position() {
        assert_eq!(
            ChunkCoord::split(IVec3::new(17, 0, 15)),
            (ChunkCoord(IVec3::new(1, 0, 0)), IVec3::new(1, 0, 15))
        );
        assert_eq!(
            ChunkCoord::split(IVec3::new(-1, 0, 0)),
            (ChunkCoord(IVec3::new(-1, 0, 0)), IVec3::new(15, 0, 0))
        );
    }
}
