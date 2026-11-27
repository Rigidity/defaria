//! 16×16×16 voxel chunks. A chunk holds a [`BlockState`] per voxel; what
//! each state looks like comes from the mods (see `content`), and the
//! terrain generator (`worldgen`) decides what goes in new ones.

use bevy::{platform::collections::HashMap, prelude::*};

use crate::{
    block_entity::BlockEntityData,
    content::{BlockState, FULL_FLUID},
    light::ChunkLight,
    world::ChunkArray,
};

/// Voxels along each axis of a chunk.
pub const CHUNK_SIZE: usize = 16;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// A cube of voxels. Voxel `(x, y, z)` occupies the unit cube from
/// `(x, y, z)` to `(x + 1, y + 1, z + 1)` in the chunk's local space.
///
/// A voxel holds a block and, in a layer of its own, a fluid: which one
/// (a block with `fluid` set) and how much, from 1 to [`FULL_FLUID`] (see
/// [`VoxelFluid`]). The block is air in a voxel of nothing but water; a
/// slab in a pond shares its voxel with the water around it, up to the
/// slab's `fluid_room`. Setting a voxel's block leaves its fluid alone;
/// what fits is [`Voxels`](super::Voxels)' business.
///
/// The blocks and the fluid are each kept in a [`ChunkArray`], so a chunk
/// of one block throughout, or with no fluid, or full of one fluid (the
/// sea), holds just that one value.
///
/// A chunk that isn't spawned also holds its voxels' block entities (see
/// `block_entity`), by voxel index. Once it's spawned they're entities
/// under it instead, and are written back into a copy of it when it's
/// saved.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[reflect(Component)]
#[require(ChunkLight)]
pub struct Chunk {
    voxels: ChunkArray<BlockState>,
    /// Each voxel's fluid: the fluid's block (air for none), and its
    /// amount in the low [`AMOUNT_BITS`] bits with the height it's drawn at
    /// above them (see [`sticky_step`]).
    fluid: ChunkArray<(BlockState, u16)>,
    /// Block entities, by voxel index, while the chunk isn't spawned. Not
    /// sent to clients, which don't keep them.
    #[serde(skip)]
    block_entities: HashMap<u16, BlockEntityData>,
}

/// The fluid in a voxel: which fluid, by its block's state, and how much,
/// from 1 to [`FULL_FLUID`].
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoxelFluid {
    pub kind: BlockState,
    pub amount: u16,
}

impl VoxelFluid {
    /// A full voxel of `kind`.
    pub const fn full(kind: BlockState) -> Self {
        Self {
            kind,
            amount: FULL_FLUID,
        }
    }

    /// `amount` of `kind`, or nothing if that's none.
    pub fn of(kind: BlockState, amount: u16) -> Option<Self> {
        (amount > 0).then_some(Self { kind, amount })
    }
}

/// The bits of a [`Chunk`]'s kept fluid that hold the amount: enough for
/// [`FULL_FLUID`].
const AMOUNT_BITS: u32 = 10;
const AMOUNT_MASK: u16 = (1 << AMOUNT_BITS) - 1;
const _: () = assert!(FULL_FLUID <= AMOUNT_MASK && FLUID_STEPS < 1 << (16 - AMOUNT_BITS));

/// No fluid, as a [`Chunk`] keeps it.
const NO_FLUID: (BlockState, u16) = (BlockState::AIR, 0);

impl Default for Chunk {
    fn default() -> Self {
        Self::filled(BlockState::AIR)
    }
}

impl Chunk {
    /// A chunk with these voxels, in [`Chunk::index_of`] order, and no
    /// fluid. `None` unless there are exactly [`CHUNK_VOLUME`].
    pub fn from_voxels(voxels: Vec<BlockState>) -> Option<Self> {
        (voxels.len() == CHUNK_VOLUME).then(|| Self {
            voxels: ChunkArray::from_vec(voxels),
            fluid: ChunkArray::Uniform(NO_FLUID),
            block_entities: HashMap::new(),
        })
    }

    /// A chunk of one block throughout, with no fluid.
    pub fn filled(state: BlockState) -> Self {
        Self {
            voxels: ChunkArray::Uniform(state),
            fluid: ChunkArray::Uniform(NO_FLUID),
            block_entities: HashMap::new(),
        }
    }

    /// The fluid in the voxel at a chunk-local position, if any.
    pub fn fluid_at(&self, pos: IVec3) -> Option<VoxelFluid> {
        let (kind, kept) = self.fluid.get(Self::index_of(pos));
        (!kind.is_air()).then_some(VoxelFluid {
            kind,
            amount: kept & AMOUNT_MASK,
        })
    }

    /// Which of the [`FLUID_STEPS`] heights the fluid in the voxel at a
    /// chunk-local position is drawn at, up the room its block leaves it
    /// (see [`sticky_step`] and [`fluid_surface`]): 0 for none.
    pub fn fluid_step_at(&self, pos: IVec3) -> u16 {
        match self.fluid.get(Self::index_of(pos)) {
            (kind, kept) if !kind.is_air() => kept >> AMOUNT_BITS,
            _ => 0,
        }
    }

    /// The amount of fluid in the voxel at a chunk-local position: 0 if
    /// there's none.
    pub fn fluid_amount_at(&self, pos: IVec3) -> u16 {
        self.fluid_at(pos).map_or(0, |fluid| fluid.amount)
    }

    /// Sets the fluid in the voxel at a chunk-local position, where its
    /// block leaves `room` for fluid (see `fluid_room`), and returns
    /// whether it's drawn differently: another fluid, or another height
    /// (see [`sticky_step`]). More or less of the fluid it held sticks to
    /// the height it was drawn at.
    pub fn set_fluid_at(&mut self, pos: IVec3, fluid: Option<VoxelFluid>, room: u16) -> bool {
        match (self.fluid_at(pos), fluid) {
            (Some(before), Some(after)) if before.kind == after.kind => {
                let step = sticky_step(filled(after.amount, room), self.fluid_step_at(pos));
                self.store_fluid(pos, fluid, step)
            }
            _ => self.start_fluid_at(pos, fluid, room),
        }
    }

    /// [`Chunk::set_fluid_at`] for fluid that's just arrived in the voxel,
    /// or whose room just changed, with no height drawn before to stick
    /// to: it's drawn at its amount's own height (see [`fluid_step`]).
    pub fn start_fluid_at(&mut self, pos: IVec3, fluid: Option<VoxelFluid>, room: u16) -> bool {
        let step = fluid.map_or(0, |fluid| fluid_step(filled(fluid.amount, room)));
        self.store_fluid(pos, fluid, step)
    }

    /// Fills the voxel at a chunk-local position with `kind`, with room for
    /// a whole block of it.
    pub fn fill_fluid_at(&mut self, pos: IVec3, kind: BlockState) {
        self.start_fluid_at(pos, Some(VoxelFluid::full(kind)), FULL_FLUID);
    }

    /// Makes the voxel at a chunk-local position a full voxel of the fluid
    /// `kind`: air, with the fluid in it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_to_fluid(&mut self, pos: IVec3, kind: BlockState) {
        self.set_at(pos, BlockState::AIR);
        self.fill_fluid_at(pos, kind);
    }

    /// Sets the voxel at a chunk-local position to `state` of the base
    /// game, as tests build voxels from a block each: a full voxel of a
    /// fluid (see [`split_fluid`]), anything else with no fluid.
    #[cfg(any(test, feature = "test-support"))]
    pub fn put(&mut self, pos: IVec3, state: BlockState) {
        let (block, fluid) = split_fluid(crate::content::base(), state);
        self.set_at(pos, block);
        self.start_fluid_at(pos, fluid, FULL_FLUID);
    }

    /// Stores `fluid`, drawn at `step`, and returns whether it's drawn
    /// differently than before.
    fn store_fluid(&mut self, pos: IVec3, fluid: Option<VoxelFluid>, step: u16) -> bool {
        let kind = |fluid: Option<VoxelFluid>| fluid.map(|fluid| fluid.kind);
        let before = (kind(self.fluid_at(pos)), self.fluid_step_at(pos));
        let kept = match fluid {
            None => NO_FLUID,
            Some(fluid) => {
                debug_assert!(
                    (1..=FULL_FLUID).contains(&fluid.amount),
                    "fluid amount {}",
                    fluid.amount
                );
                (fluid.kind, fluid.amount | step << AMOUNT_BITS)
            }
        };
        self.fluid.set(Self::index_of(pos), kept);
        before != (kind(fluid), step)
    }

    /// The fluid in the voxel at an index from [`Chunk::index_of`], by
    /// its block: air for none.
    pub fn fluid_kind_at_index(&self, index: usize) -> BlockState {
        self.fluid.get(index).0
    }

    /// The fluid in the voxel at an index from [`Chunk::index_of`] and how
    /// much of it there is: air and 0 for none.
    pub fn fluid_at_index(&self, index: usize) -> (BlockState, u16) {
        let (kind, kept) = self.fluid.get(index);
        if kind.is_air() {
            (kind, 0)
        } else {
            (kind, kept & AMOUNT_MASK)
        }
    }

    /// Every voxel with fluid in it, by index.
    pub fn fluids(&self) -> impl Iterator<Item = (usize, VoxelFluid)> + '_ {
        self.fluid
            .iter()
            .enumerate()
            .filter(|(_, (kind, _))| !kind.is_air())
            .map(|(index, &(kind, kept))| {
                let amount = kept & AMOUNT_MASK;
                (index, VoxelFluid { kind, amount })
            })
    }

    /// The voxels' fluid, as kept (see [`ChunkArray`]).
    pub fn fluid_layer(&self) -> &ChunkArray<(BlockState, u16)> {
        &self.fluid
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

    /// Drops the voxels, or their fluid, where they're all one value
    /// (see [`ChunkArray`]).
    pub fn compact(&mut self) -> bool {
        let blocks = self.voxels.compact();
        self.fluid.compact() | blocks
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        self.voxels.get(Self::index(x, y, z))
    }

    /// Sets a voxel's block, leaving its fluid alone.
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) {
        self.voxels.set(Self::index(x, y, z), state);
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

    pub fn positions() -> impl Iterator<Item = (usize, usize, usize)> {
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

/// How full `amount` of fluid makes the `room` a voxel's block leaves it
/// (see `fluid_room`), in [`FULL_FLUID`] units: as much as a voxel of air
/// would be with that share of a block in it.
pub fn filled(amount: u16, room: u16) -> u16 {
    let room = room.clamp(1, FULL_FLUID) as u32;
    (amount as u32 * FULL_FLUID as u32 / room).min(FULL_FLUID as u32) as u16
}

/// How high one voxel's own fluid is, in blocks above its bottom: the
/// height it's drawn at, one of the [`FLUID_STEPS`] up the `room` its
/// block leaves it (see `fluid_room` and [`Chunk::fluid_step_at`]), on top
/// of the part of the voxel the block takes. A slab's half full of water
/// reaches the top of its voxel, so a pond's surface runs on unbroken over
/// it. The surface you see, swim at and see the fog start at blends this
/// with the voxels around it: see [`FluidTop`](super::FluidTop).
pub fn fluid_surface(step: u16, room: u16) -> f32 {
    let room = room.min(FULL_FLUID) as f32 / FULL_FLUID as f32;
    1.0 - room + room * step as f32 / FLUID_STEPS as f32
}

/// A block of the base game as a voxel's block and fluid, for tests that
/// build voxels from a block each: a fluid goes in the voxel's fluid,
/// full, with air for its block.
#[cfg(any(test, feature = "test-support"))]
pub fn split_fluid(
    content: &crate::content::Content,
    state: BlockState,
) -> (BlockState, Option<VoxelFluid>) {
    if content.state(state).is_fluid() {
        (BlockState::AIR, Some(VoxelFluid::full(state)))
    } else {
        (state, None)
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
    offsets_within(local, 1)
}

/// The offsets of the chunks within `reach` voxels of the voxel at chunk
/// position `local`, its own (zero) among them: [`touching_offsets`] for
/// what can see further, such as the flow on fluid faces, which a voxel
/// two in from an edge decides for the chunk across it (see
/// `meshing::scroll`).
pub fn offsets_within(local: IVec3, reach: i32) -> impl Iterator<Item = IVec3> {
    let size = CHUNK_SIZE as i32;
    let edge = local.map(|v| {
        if v < reach {
            -1
        } else if v >= size - reach {
            1
        } else {
            0
        }
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
        let water = |amount| VoxelFluid::of(block("water"), amount);
        chunk.fill_fluid_at(pos, block("water"));
        assert!(chunk.set_fluid_at(pos, water(500), FULL_FLUID), "from full");
        assert_eq!(chunk.fluid_step_at(pos), 8);
        for amount in [501, 499, 520, 470, 500] {
            assert!(
                !chunk.set_fluid_at(pos, water(amount), FULL_FLUID),
                "{amount}"
            );
            assert_eq!(chunk.fluid_amount_at(pos), amount);
        }
        assert!(chunk.set_fluid_at(pos, water(560), FULL_FLUID));
        assert_eq!(chunk.fluid_step_at(pos), 9);
        assert!(chunk.set_fluid_at(pos, water(FULL_FLUID), FULL_FLUID));
        assert_eq!(chunk.fluid_step_at(pos), FLUID_STEPS);
        // Another fluid, or none, is drawn differently.
        let lava = VoxelFluid::of(block("lava"), FULL_FLUID);
        assert!(chunk.set_fluid_at(pos, lava, FULL_FLUID));
        assert!(chunk.set_fluid_at(pos, None, FULL_FLUID));
        assert_eq!((chunk.fluid_at(pos), chunk.fluid_step_at(pos)), (None, 0));
    }

    /// Heights are steps up the room a block leaves: water rising in a
    /// tuft's voxel, as flow fills it, ends drawn at the top of the voxel
    /// once it's full, or nearly, whatever height it rose from; a slab's
    /// half full is drawn to its top too.
    #[test]
    fn heights_are_steps_up_a_blocks_room() {
        let pos = IVec3::new(1, 2, 3);
        let mut chunk = Chunk::default();
        let water = |amount| VoxelFluid::of(block("water"), amount);
        chunk.start_fluid_at(pos, water(700), 900);
        let step = chunk.fluid_step_at(pos);
        for amount in [760, 813, 860, 894] {
            chunk.set_fluid_at(pos, water(amount), 900);
        }
        assert!(chunk.fluid_step_at(pos) > step);
        assert_eq!(fluid_surface(chunk.fluid_step_at(pos), 900), 1.0);
        assert_eq!(fluid_surface(FLUID_STEPS, 500), 1.0);
        assert_eq!(fluid_surface(FLUID_STEPS / 2, 500), 0.75);
        assert_eq!(fluid_surface(FLUID_STEPS / 2, FULL_FLUID), 0.5);
    }

    /// A chunk with no fluid, or full of one fluid, keeps its fluid as one
    /// value once compacted.
    #[test]
    fn uniform_fluid_takes_no_room() {
        let mut chunk = Chunk::default();
        assert!(chunk.fluid_layer().uniform().is_some());
        chunk.fill_fluid_at(IVec3::ONE, block("water"));
        assert_eq!(chunk.fluid_layer().uniform(), None);
        chunk.set_fluid_at(IVec3::ONE, None, FULL_FLUID);
        assert!(chunk.compact());
        assert_eq!(chunk.fluid_layer().heap_bytes(), 0);
    }

    /// A voxel reaches the chunks across the edges it's within `reach` of:
    /// with a reach of 2, the second voxel in from an edge does too, and
    /// one in the middle reaches only its own chunk.
    #[test]
    fn offsets_within_a_reach() {
        let sorted = |local: IVec3, reach| {
            let mut offsets: Vec<IVec3> = offsets_within(local, reach).collect();
            offsets.sort_by_key(|o| o.to_array());
            offsets
        };
        let last = CHUNK_SIZE as i32 - 1;
        assert_eq!(sorted(IVec3::new(last - 1, 5, 5), 1), [IVec3::ZERO]);
        assert_eq!(
            sorted(IVec3::new(last - 1, 5, 5), 2),
            [IVec3::ZERO, IVec3::X]
        );
        assert_eq!(
            sorted(IVec3::new(1, 1, 8), 2),
            [
                IVec3::new(-1, -1, 0),
                IVec3::NEG_X,
                IVec3::NEG_Y,
                IVec3::ZERO
            ]
        );
        assert_eq!(sorted(IVec3::splat(8), 2), [IVec3::ZERO]);
        // A reach of 1 is touching.
        for local in [IVec3::ZERO, IVec3::new(last, 3, 0), IVec3::splat(4)] {
            assert_eq!(sorted(local, 1), {
                let mut t: Vec<IVec3> = touching_offsets(local).collect();
                t.sort_by_key(|o| o.to_array());
                t
            });
        }
    }
}
