//! Reading and writing voxels by world position, across every loaded
//! chunk.

use bevy::{ecs::system::SystemParam, prelude::*};

use super::{
    Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated, VoxelChanged, VoxelFluid,
};
use crate::content::{BlockState, Content, FULL_FLUID};

/// How many voxels around a chunk its drawing reads on each side: two, as
/// a fluid face's corner averages the flow of the voxels around it (see
/// `meshing::scroll`), which depends on the voxels around those. A voxel's
/// change redraws the chunks this close to it.
pub const MARGIN: i32 = 2;

/// Reads and writes voxels by world position, across every chunk.
#[derive(SystemParam)]
pub struct Voxels<'w, 's> {
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, &'static mut Chunk>,
    changes: MessageWriter<'w, VoxelChanged>,
    fluid_changes: MessageWriter<'w, FluidChanged>,
    updates: MessageWriter<'w, PropertiesUpdated>,
    content: Res<'w, Content>,
}

impl Voxels<'_, '_> {
    /// The block at a world position; `None` outside the world or in a
    /// chunk that isn't loaded.
    pub fn get(&self, pos: IVec3) -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some(chunk.at(local))
    }

    /// The block at a world position and the fluid in it; `None` outside
    /// the loaded chunks.
    pub fn get_with_fluid(&self, pos: IVec3) -> Option<(BlockState, Option<VoxelFluid>)> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some((chunk.at(local), chunk.fluid_at(local)))
    }

    /// How much more of the fluid `kind` the voxel at a world position has
    /// room for: what its block leaves (see `fluid_room`), less the fluid
    /// already there, and none if another fluid is there. `None` outside
    /// the loaded chunks.
    pub fn room_for(&self, pos: IVec3, kind: BlockState) -> Option<u16> {
        let (block, fluid) = self.get_with_fluid(pos)?;
        let room = self.content.state(block).fluid_room;
        Some(match fluid {
            None => room,
            Some(fluid) if fluid.kind == kind => room.saturating_sub(fluid.amount),
            Some(_) => 0,
        })
    }

    /// Sets the block at a world position, and returns whether it's there
    /// to set: there's nothing outside the world or the loaded chunks. A
    /// voxel on a chunk's edge also shows, hides or shades faces of the
    /// chunks across that edge, so they are marked changed too and
    /// remeshed with it. A change sends [`VoxelChanged`].
    ///
    /// The voxel keeps as much of its fluid as the new block leaves room
    /// for (see `fluid_room`): all of it for air, half a block for a slab,
    /// none for stone. What doesn't fit is displaced
    /// ([`VoxelChanged::displaced`]), and flow pushes it aside (see
    /// `fluid`).
    ///
    /// Setting a voxel to a fluid (a block with `fluid` set) fills it with
    /// that fluid instead: as much as its block leaves room for, or a
    /// whole block in place of a block that leaves none.
    pub fn set(&mut self, pos: IVec3, state: BlockState) -> bool {
        if self.content.state(state).is_fluid() {
            return self.fill(pos, state);
        }
        let (coord, local) = ChunkCoord::split(pos);
        let Some(mut chunk) = self
            .map
            .get(coord)
            .and_then(|entity| self.chunks.get_mut(entity).ok())
        else {
            return false;
        };
        let before = chunk.at(local);
        if before == state {
            return true;
        }
        let fluid_before = chunk.fluid_at(local);
        let room = self.content.state(state).fluid_room;
        let fluid_after =
            fluid_before.and_then(|fluid| VoxelFluid::of(fluid.kind, fluid.amount.min(room)));
        chunk.set_at(local, state);
        if fluid_before.is_some() {
            // Drawn afresh, up the new block's room.
            chunk.start_fluid_at(local, fluid_after, room);
        }
        self.touched(pos);
        self.changes.write(VoxelChanged {
            pos,
            before,
            after: state,
            fluid_before,
            fluid_after,
        });
        true
    }

    /// Fills the voxel at a world position with the fluid `kind`, as
    /// [`Voxels::set`] does with a fluid.
    fn fill(&mut self, pos: IVec3, kind: BlockState) -> bool {
        let Some(block) = self.get(pos) else {
            return false;
        };
        let room = match self.content.state(block).fluid_room {
            0 => {
                self.set(pos, BlockState::AIR);
                FULL_FLUID
            }
            room => room,
        };
        self.set_fluid(pos, VoxelFluid::of(kind, room))
    }

    /// Adds up to `amount` of the fluid `kind` to the voxel at a world
    /// position, as much as it has room for (see [`Voxels::room_for`]),
    /// and returns how much it added.
    pub fn pour(&mut self, pos: IVec3, kind: BlockState, amount: u16) -> u16 {
        let added = amount.min(self.room_for(pos, kind).unwrap_or(0));
        if added == 0 {
            return 0;
        }
        let there = self.fluid_amount(pos);
        self.set_fluid(pos, VoxelFluid::of(kind, there + added));
        added
    }

    /// Takes up to `amount` of the fluid in the voxel at a world position,
    /// and returns what it took.
    pub fn drain(&mut self, pos: IVec3, amount: u16) -> Option<VoxelFluid> {
        let (_, fluid) = self.get_with_fluid(pos)?;
        let fluid = fluid?;
        let taken = amount.min(fluid.amount);
        self.set_fluid(pos, VoxelFluid::of(fluid.kind, fluid.amount - taken));
        VoxelFluid::of(fluid.kind, taken)
    }

    /// How much fluid the voxel at a world position holds: 0 for none, or
    /// outside the loaded chunks.
    pub fn fluid_amount(&self, pos: IVec3) -> u16 {
        self.get_with_fluid(pos)
            .and_then(|(_, fluid)| fluid)
            .map_or(0, |fluid| fluid.amount)
    }

    /// Sets the voxel at a world position to `state`, another state of the
    /// block it holds, as the engine does when properties it derives from
    /// the world change. It isn't an edit: any fluid it holds is kept, and
    /// it sends [`PropertiesUpdated`] rather than [`VoxelChanged`]. The
    /// chunks that show it are remeshed, once a frame however many voxels
    /// change. Returns whether it changed.
    pub fn update(&mut self, pos: IVec3, state: BlockState) -> bool {
        let (coord, local) = ChunkCoord::split(pos);
        let Some(mut chunk) = self
            .map
            .get(coord)
            .and_then(|entity| self.chunks.get_mut(entity).ok())
        else {
            return false;
        };
        let before = chunk.at(local);
        if before == state {
            return false;
        }
        chunk.bypass_change_detection().set_at(local, state);
        self.touched(pos);
        self.updates.write(PropertiesUpdated {
            pos,
            before,
            after: state,
        });
        true
    }

    /// Sets the fluid in the voxel at a world position, leaving its block
    /// alone, and returns whether it's loaded. It should fit the room the
    /// block leaves (see [`Voxels::room_for`]). The chunks that show it are
    /// remeshed only if it's drawn differently (see
    /// [`Chunk::set_fluid_at`]). Sends [`FluidChanged`] if it changed.
    pub fn set_fluid(&mut self, pos: IVec3, fluid: Option<VoxelFluid>) -> bool {
        self.change_fluid(pos, fluid, false)
    }

    /// [`Voxels::set_fluid`], for flow moving fluid.
    pub fn flow_fluid(&mut self, pos: IVec3, fluid: Option<VoxelFluid>) -> bool {
        self.change_fluid(pos, fluid, true)
    }

    fn change_fluid(&mut self, pos: IVec3, fluid: Option<VoxelFluid>, by_flow: bool) -> bool {
        let (coord, local) = ChunkCoord::split(pos);
        let Some(mut chunk) = self
            .map
            .get(coord)
            .and_then(|entity| self.chunks.get_mut(entity).ok())
        else {
            return false;
        };
        let before = chunk.fluid_at(local);
        if before == fluid {
            return true;
        }
        debug_assert!(
            fluid
                .is_none_or(|fluid| fluid.amount <= self.content.state(chunk.at(local)).fluid_room),
            "{fluid:?} doesn't fit at {pos}"
        );
        let room = self.content.state(chunk.at(local)).fluid_room;
        let chunk = chunk.bypass_change_detection();
        if chunk.set_fluid_at(local, fluid, room) {
            // A fluid voxel decides the flow on faces further away than
            // its own neighbours' (see `meshing::scroll`).
            self.touched_within(pos, MARGIN);
        }
        self.fluid_changes.write(FluidChanged {
            pos,
            by_flow,
            before,
            after: fluid,
        });
        true
    }

    /// Marks the chunks that show the voxel at `pos` changed, so they're
    /// remeshed: a voxel on a chunk's edge also shows, hides or shades
    /// faces of the chunks across that edge.
    fn touched(&mut self, pos: IVec3) {
        self.touched_within(pos, 1);
    }

    /// [`Voxels::touched`], for a voxel whose change reaches `reach`
    /// voxels into the chunks around it.
    fn touched_within(&mut self, pos: IVec3, reach: i32) {
        for touching in ChunkCoord::within(pos, reach) {
            if let Some(mut chunk) = self
                .map
                .get(touching)
                .and_then(|entity| self.chunks.get_mut(entity).ok())
            {
                chunk.set_changed();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{
        content::{FULL_FLUID, base, block},
        testing::{ChunkMapPlugin, insert_chunk},
    };

    /// The bare systems [`Voxels`] needs, and one chunk with `chunk` in it
    /// at the origin.
    fn voxels_app(chunk: Chunk) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, ChunkMapPlugin))
            .insert_resource(base().clone());
        let entity = insert_chunk(&mut app, IVec3::ZERO, chunk);
        (app, entity)
    }

    /// Setting a voxel's fluid remeshes the chunks that show it only when
    /// its drawn height changes, and across a chunk edge, its neighbour's
    /// too.
    #[test]
    fn fluid_changes_remesh_only_when_the_height_changes() {
        let edge = IVec3::new(15, 5, 5);
        let mut chunk = Chunk::default();
        chunk.fill_fluid_at(edge, block("water"));
        let (mut app, _) = voxels_app(chunk);
        insert_chunk(&mut app, IVec3::X, Chunk::default());
        let changed = app.world_mut().register_system(
            |chunks: Query<(&ChunkCoord, Ref<Chunk>)>| -> Vec<IVec3> {
                let mut changed: Vec<IVec3> = chunks
                    .iter()
                    .filter(|(_, chunk)| chunk.is_changed())
                    .map(|(coord, _)| coord.0)
                    .collect();
                changed.sort_by_key(|c| c.x);
                changed
            },
        );
        let set_fluid = |app: &mut App, amount: u16| {
            let fluid = VoxelFluid::of(block("water"), amount);
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| voxels.set_fluid(edge, fluid))
                .unwrap();
            app.world_mut().run_system(changed).unwrap()
        };
        set_fluid(&mut app, FULL_FLUID);
        // 500 and 490 are both drawn half a block high.
        assert_eq!(set_fluid(&mut app, 500), [IVec3::ZERO, IVec3::X]);
        assert_eq!(set_fluid(&mut app, 490), Vec::<IVec3>::new());
        assert_eq!(set_fluid(&mut app, 400), [IVec3::ZERO, IVec3::X]);
    }

    /// A voxel keeps as much of its fluid as each block set in it leaves
    /// room for: all for air, half for a slab, none for stone, and says
    /// how much was displaced. Setting it to a fluid fills its room, or
    /// clears a block that leaves none.
    #[test]
    fn blocks_keep_the_fluid_they_leave_room_for() {
        let pos = IVec3::new(3, 4, 5);
        let mut chunk = Chunk::default();
        chunk.set_fluid_at(pos, VoxelFluid::of(block("water"), 250), FULL_FLUID);
        let (mut app, entity) = voxels_app(chunk);
        let set = |app: &mut App, state: BlockState| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| voxels.set(pos, state))
                .unwrap();
            let changes: Vec<VoxelChanged> = app
                .world_mut()
                .resource_mut::<Messages<VoxelChanged>>()
                .drain()
                .collect();
            let fluid_changes: Vec<FluidChanged> = app
                .world_mut()
                .resource_mut::<Messages<FluidChanged>>()
                .drain()
                .collect();
            let chunk = app.world().get::<Chunk>(entity).unwrap();
            (
                chunk.at(pos),
                chunk.fluid_amount_at(pos),
                changes,
                fluid_changes,
            )
        };
        let water = |amount| VoxelFluid::of(block("water"), amount);

        // Filling the water is a change of fluid, not of block.
        let (air, amount, changes, fluid) = set(&mut app, block("water"));
        assert_eq!(
            (air, amount, changes.len()),
            (BlockState::AIR, FULL_FLUID, 0)
        );
        assert_eq!((fluid[0].before, fluid[0].after), (water(250), water(1000)));

        let (slab, amount, changes, _) = set(&mut app, block("stone_slab"));
        assert_eq!((slab, amount), (block("stone_slab"), 500));
        assert_eq!(changes[0].displaced(), water(500));

        // Filling a slab fills its half.
        let (_, amount, _, _) = set(&mut app, block("water"));
        assert_eq!(amount, 500);

        let (stone, amount, changes, _) = set(&mut app, block("stone"));
        assert_eq!((stone, amount), (block("stone"), 0));
        assert_eq!(changes[0].displaced(), water(500));

        // Water poured where stone is takes its place.
        let (air, amount, changes, _) = set(&mut app, block("water"));
        assert_eq!(
            (air, amount, changes.len()),
            (BlockState::AIR, FULL_FLUID, 1)
        );
        assert_eq!(changes[0].displaced(), None);
        // A plant takes up no room: it keeps all of the water.
        let (_, amount, changes, _) = set(&mut app, block("tuft"));
        assert_eq!(amount, FULL_FLUID);
        assert_eq!(changes[0].displaced(), None);
        let (_, amount, changes, _) = set(&mut app, BlockState::AIR);
        assert_eq!(
            (amount, changes[0].displaced()),
            (FULL_FLUID, None),
            "breaking keeps it"
        );
    }
}
