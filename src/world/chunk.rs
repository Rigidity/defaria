//! 16×16×16 voxel chunks. A chunk holds a [`BlockState`] per voxel; what
//! each state looks like comes from the mods (see `content`), and the
//! terrain generator (`worldgen`) decides what goes in new ones.

use bevy::{platform::collections::HashMap, prelude::*};

use crate::{
    block_entity::BlockEntityData,
    content::{BlockState, Content, FULL_FLUID},
    light::ChunkLight,
    world::ChunkArray,
};

/// Voxels along each axis of a chunk.
pub const CHUNK_SIZE: usize = 16;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// A cube of voxels. Voxel `(x, y, z)` occupies the unit cube from
/// `(x, y, z)` to `(x + 1, y + 1, z + 1)` in the chunk's local space.
///
/// A fluid voxel (a block with `fluid` set) also holds an amount of its
/// fluid, from 1 to [`FULL_FLUID`]. Most fluid is full, so only the amounts
/// of the voxels that aren't are kept, in `partial`; a fluid voxel that
/// isn't there is full. Setting a voxel to another block forgets its
/// amount.
///
/// The blocks are kept in a [`ChunkArray`], so a chunk of one block
/// throughout holds just that block.
///
/// A chunk that isn't spawned also holds its voxels' block entities (see
/// `block_entity`), by voxel index. Once it's spawned they're entities
/// under it instead, and are written back into a copy of it when it's
/// saved.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Component)]
#[require(ChunkLight)]
pub struct Chunk {
    voxels: ChunkArray<BlockState>,
    /// Each fluid voxel that isn't full, by voxel index: its amount in the
    /// low [`AMOUNT_BITS`] bits, and above them the height it's drawn at
    /// (see [`sticky_step`]), which fits in the bits left over.
    partial: HashMap<u16, u16>,
    /// Block entities, by voxel index, while the chunk isn't spawned.
    block_entities: HashMap<u16, BlockEntityData>,
}

/// The bits of a [`Chunk`]'s kept fluid that hold the amount: enough for
/// anything under [`FULL_FLUID`].
const AMOUNT_BITS: u32 = 10;
const AMOUNT_MASK: u16 = (1 << AMOUNT_BITS) - 1;
const _: () = assert!(FULL_FLUID <= AMOUNT_MASK + 1 && FLUID_STEPS < 1 << (16 - AMOUNT_BITS));

impl Default for Chunk {
    fn default() -> Self {
        Self::filled(BlockState::AIR)
    }
}

impl Chunk {
    /// A chunk with these voxels, in [`Chunk::index_of`] order, any fluid
    /// in them full. `None` unless there are exactly [`CHUNK_VOLUME`].
    pub fn from_voxels(voxels: Vec<BlockState>) -> Option<Self> {
        (voxels.len() == CHUNK_VOLUME).then(|| Self {
            voxels: ChunkArray::from_vec(voxels),
            partial: HashMap::new(),
            block_entities: HashMap::new(),
        })
    }

    /// A chunk of one block throughout.
    pub fn filled(state: BlockState) -> Self {
        Self {
            voxels: ChunkArray::Uniform(state),
            partial: HashMap::new(),
            block_entities: HashMap::new(),
        }
    }

    /// The amount of fluid kept for the voxel at a chunk-local position:
    /// `None` when none is kept, which for a fluid voxel means it's full.
    /// See [`Chunk::fluid_amount_at`] for the amount itself.
    pub fn partial_at(&self, pos: IVec3) -> Option<u16> {
        self.partial
            .get(&(Self::index_of(pos) as u16))
            .map(|&kept| kept & AMOUNT_MASK)
    }

    /// Which of the [`FLUID_STEPS`] heights the fluid voxel at a
    /// chunk-local position is drawn at (see [`sticky_step`]).
    pub fn fluid_step_at(&self, pos: IVec3) -> u16 {
        self.partial
            .get(&(Self::index_of(pos) as u16))
            .map_or(FLUID_STEPS, |&kept| kept >> AMOUNT_BITS)
    }

    /// The amount of fluid in the voxel at a chunk-local position: 0 if it
    /// isn't a fluid.
    pub fn fluid_amount_at(&self, pos: IVec3, content: &Content) -> u16 {
        fluid_amount(content, self.at(pos), self.partial_at(pos))
    }

    /// Sets the amount of fluid in the voxel at a chunk-local position,
    /// which should be a fluid, from 1 to [`FULL_FLUID`], and returns
    /// whether the height it's drawn at changed (see [`sticky_step`]).
    pub fn set_fluid_at(&mut self, pos: IVec3, amount: u16) -> bool {
        self.store_fluid(pos, amount, self.fluid_step_at(pos))
    }

    /// [`Chunk::set_fluid_at`] for fluid that's just arrived in the voxel,
    /// with no height drawn before to stick to: it's drawn at its amount's
    /// own height (see [`fluid_step`]).
    pub fn start_fluid_at(&mut self, pos: IVec3, amount: u16) -> bool {
        self.store_fluid(pos, amount, fluid_step(amount))
    }

    fn store_fluid(&mut self, pos: IVec3, amount: u16, before: u16) -> bool {
        debug_assert!((1..=FULL_FLUID).contains(&amount), "fluid amount {amount}");
        let index = Self::index_of(pos) as u16;
        if amount >= FULL_FLUID {
            self.partial.remove(&index);
            return before != FLUID_STEPS;
        }
        let step = sticky_step(amount, before);
        self.partial.insert(index, amount | step << AMOUNT_BITS);
        step != before
    }

    /// Every voxel that isn't full of fluid, by index, with its amount.
    pub fn partial_fluid(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.partial
            .iter()
            .map(|(&index, &kept)| (index as usize, kept & AMOUNT_MASK))
    }

    /// The block entity kept for the voxel at a chunk-local position, while
    /// the chunk isn't spawned.
    pub fn block_entity_at(&self, pos: IVec3) -> Option<&BlockEntityData> {
        self.block_entities.get(&(Self::index_of(pos) as u16))
    }

    /// Keeps a block entity for the voxel at a chunk-local position.
    pub fn set_block_entity_at(&mut self, pos: IVec3, data: BlockEntityData) {
        self.block_entities.insert(Self::index_of(pos) as u16, data);
    }

    /// Every block entity kept, by voxel index.
    pub fn block_entities(&self) -> impl Iterator<Item = (usize, &BlockEntityData)> + '_ {
        self.block_entities
            .iter()
            .map(|(&index, data)| (index as usize, data))
    }

    /// Takes the block entities kept, leaving none, by voxel index.
    pub fn take_block_entities(&mut self) -> HashMap<u16, BlockEntityData> {
        std::mem::take(&mut self.block_entities)
    }

    /// Every voxel, in [`Chunk::index_of`] order.
    pub fn voxels(&self) -> &ChunkArray<BlockState> {
        &self.voxels
    }

    /// Drops the voxels if they're all one block (see [`ChunkArray`]).
    pub fn compact(&mut self) -> bool {
        self.voxels.compact()
    }

    #[cfg(test)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        self.voxels.get(Self::index(x, y, z))
    }

    /// Sets a voxel's block. A different block than it was forgets its
    /// amount of fluid: a fluid set this way is full.
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) {
        let index = Self::index(x, y, z);
        if self.voxels.get(index) != state {
            self.voxels.set(index, state);
            self.partial.remove(&(index as u16));
        }
    }

    /// Sets a voxel to another state of the same block, keeping its
    /// amount of fluid.
    pub fn update_at(&mut self, pos: IVec3, state: BlockState) {
        self.voxels.set(Self::index_of(pos), state);
    }

    /// [`Chunk::get`] for a chunk-local position given as an `IVec3`.
    pub fn at(&self, pos: IVec3) -> BlockState {
        self.voxels.get(Self::index_of(pos))
    }

    /// [`Chunk::set`] for a chunk-local position given as an `IVec3`.
    pub fn set_at(&mut self, pos: IVec3, state: BlockState) {
        self.set(pos.x as usize, pos.y as usize, pos.z as usize, state);
    }

    /// The voxel at an index from [`Chunk::index_of`].
    pub fn get_index(&self, index: usize) -> BlockState {
        self.voxels.get(index)
    }

    /// Where the voxel at a chunk-local position is stored. Chunk light
    /// uses the same order.
    pub fn index_of(pos: IVec3) -> usize {
        Self::index(pos.x as usize, pos.y as usize, pos.z as usize)
    }

    /// The chunk-local position of the voxel stored at `index`.
    pub fn position_of(index: usize) -> IVec3 {
        let size = CHUNK_SIZE;
        IVec3::new(
            (index % size) as i32,
            (index / (size * size)) as i32,
            (index / size % size) as i32,
        )
    }

    fn index(x: usize, y: usize, z: usize) -> usize {
        assert!(
            x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE,
            "voxel ({x}, {y}, {z}) is outside the chunk"
        );
        (y * CHUNK_SIZE + z) * CHUNK_SIZE + x
    }

    pub(crate) fn positions() -> impl Iterator<Item = (usize, usize, usize)> {
        (0..CHUNK_SIZE).flat_map(|y| {
            (0..CHUNK_SIZE).flat_map(move |z| (0..CHUNK_SIZE).map(move |x| (x, y, z)))
        })
    }
}

/// How many heights a fluid's surface is drawn at, from a sixteenth of a
/// block up to a full block.
pub const FLUID_STEPS: u16 = 16;

/// Which of the [`FLUID_STEPS`] heights `amount` of fluid is drawn at: the
/// next one up, so even a little shows. 0 for no fluid.
pub fn fluid_step(amount: u16) -> u16 {
    (amount.min(FULL_FLUID) as u32 * FLUID_STEPS as u32).div_ceil(FULL_FLUID as u32) as u16
}

/// The height fluid is drawn at, as one of the [`FLUID_STEPS`], given the
/// one it was drawn at before: that one still, while `amount` is within
/// half a step of it, and otherwise the amount's own [`fluid_step`]. Any
/// fluid at all is drawn at least a step high. Flow
/// nudges amounts by a few thousandths as it levels off, and without this,
/// an amount at the edge between two heights would make the surface
/// blink between them.
fn sticky_step(amount: u16, before: u16) -> u16 {
    let step = fluid_step(amount);
    let size = FULL_FLUID as f32 / FLUID_STEPS as f32;
    let (low, high) = ((before as f32 - 1.5) * size, (before as f32 + 0.5) * size);
    if step != before && (amount as f32) > low && (amount as f32) <= high {
        before
    } else {
        step
    }
}

/// How high one voxel's own fluid is, in blocks above its bottom: the
/// height it's drawn at, one of the [`FLUID_STEPS`] (see
/// [`Chunk::fluid_step_at`]). The surface you see, swim at and see the fog
/// start at blends this with the voxels around it: see
/// [`FluidTop`](super::FluidTop).
pub(super) fn fluid_surface(step: u16) -> f32 {
    step as f32 / FLUID_STEPS as f32
}

/// The amount of fluid in a voxel of block `state`, with `partial` the
/// amount its chunk keeps for it (see [`Chunk`]): 0 if it isn't a fluid.
pub fn fluid_amount(content: &Content, state: BlockState, partial: Option<u16>) -> u16 {
    if content.state(state).is_fluid() {
        partial.unwrap_or(FULL_FLUID)
    } else {
        0
    }
}

/// The axis an axis-aligned vector points along: 0, 1 or 2 for X, Y or Z.
pub fn axis_of(direction: IVec3) -> usize {
    if direction.x != 0 {
        0
    } else if direction.y != 0 {
        1
    } else {
        2
    }
}

/// The offsets of the chunks whose meshes show the voxel at a chunk-local
/// position: zero (its own chunk), and one for every chunk across the faces,
/// edges and corners it lies on, since a voxel shades the corners of faces
/// next to it.
pub fn touching_offsets(local: IVec3) -> impl Iterator<Item = IVec3> {
    let last = CHUNK_SIZE as i32 - 1;
    let edge = local.map(|v| match v {
        0 => -1,
        v if v == last => 1,
        _ => 0,
    });
    // One bit per axis: step across that axis's edge or not. Steps along an
    // axis the voxel isn't on an edge of would repeat an offset.
    (0..8)
        .filter(move |bits| (0..3).all(|axis| bits & (1 << axis) == 0 || edge[axis] != 0))
        .map(move |bits| {
            IVec3::new(
                if bits & 1 != 0 { edge.x } else { 0 },
                if bits & 2 != 0 { edge.y } else { 0 },
                if bits & 4 != 0 { edge.z } else { 0 },
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::block;

    /// A voxel keeps the height it's drawn at while its amount wobbles
    /// within half a step of it, and moves once the amount is clearly at
    /// another.
    #[test]
    fn drawn_heights_are_sticky() {
        // A sixteenth is 62.5: step 8 is drawn for 438 to 500.
        assert_eq!(sticky_step(500, 8), 8);
        assert_eq!(sticky_step(501, 8), 8, "a little over");
        assert_eq!(sticky_step(531, 8), 8);
        assert_eq!(sticky_step(532, 8), 9, "half a step over");
        assert_eq!(sticky_step(407, 8), 8, "a little under");
        assert_eq!(sticky_step(406, 8), 7, "half a step under");
        assert_eq!(sticky_step(200, 8), 4, "far off");
        assert_eq!(sticky_step(1, 1), 1, "a sliver shows");
        assert_eq!(sticky_step(1, 2), 1);

        let pos = IVec3::new(1, 2, 3);
        let mut chunk = Chunk::default();
        chunk.set_at(pos, block("water"));
        assert!(chunk.set_fluid_at(pos, 500), "from full");
        assert_eq!(chunk.fluid_step_at(pos), 8);
        for amount in [501, 499, 520, 470, 500] {
            assert!(!chunk.set_fluid_at(pos, amount), "{amount}");
            assert_eq!(chunk.partial_at(pos), Some(amount));
        }
        assert!(chunk.set_fluid_at(pos, 560));
        assert_eq!(chunk.fluid_step_at(pos), 9);
        assert!(chunk.set_fluid_at(pos, FULL_FLUID));
        assert_eq!(chunk.fluid_step_at(pos), FLUID_STEPS);
    }
}
