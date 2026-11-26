//! Reading and writing voxels by world position, across every loaded
//! chunk.

use bevy::{ecs::system::SystemParam, prelude::*};

use super::{Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated, VoxelChanged};
use crate::content::{BlockState, FULL_FLUID};

/// Reads and writes voxels by world position, across every chunk.
#[derive(SystemParam)]
pub struct Voxels<'w, 's> {
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, &'static mut Chunk>,
    changes: MessageWriter<'w, VoxelChanged>,
    fluid_changes: MessageWriter<'w, FluidChanged>,
    updates: MessageWriter<'w, PropertiesUpdated>,
}

impl Voxels<'_, '_> {
    /// The voxel at a world position; `None` outside the world or in a
    /// chunk that isn't loaded.
    pub fn get(&self, pos: IVec3) -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some(chunk.at(local))
    }

    /// Sets the voxel at a world position, and returns whether it's there to
    /// set: there's nothing outside the world or the loaded chunks. A voxel on a chunk's edge also
    /// shows, hides or shades faces of the chunks across that edge, so they
    /// are marked changed too and remeshed with it. Setting a voxel to the
    /// fluid it holds fills it. A change sends [`VoxelChanged`].
    pub fn set(&mut self, pos: IVec3, state: BlockState) -> bool {
        let (coord, local) = ChunkCoord::split(pos);
        let Some(mut chunk) = self
            .map
            .get(coord)
            .and_then(|entity| self.chunks.get_mut(entity).ok())
        else {
            return false;
        };
        let before = chunk.at(local);
        let partial_before = chunk.partial_at(local);
        if before == state {
            // Setting fluid fills it, if it wasn't full.
            if partial_before.is_none() {
                return true;
            }
            chunk.set_fluid_at(local, FULL_FLUID);
        } else {
            chunk.set_at(local, state);
        }
        self.touched(pos);
        self.changes.write(VoxelChanged {
            pos,
            before,
            after: state,
            partial_before,
        });
        true
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
        chunk.bypass_change_detection().update_at(local, state);
        self.touched(pos);
        self.updates.write(PropertiesUpdated {
            pos,
            before,
            after: state,
        });
        true
    }

    /// The block at a world position and the amount of fluid its chunk
    /// keeps for it (see [`Chunk::partial_at`]); `None` outside the loaded
    /// chunks.
    pub fn get_with_fluid(&self, pos: IVec3) -> Option<(BlockState, Option<u16>)> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some((chunk.at(local), chunk.partial_at(local)))
    }

    /// Sets the amount of fluid in the fluid voxel at a world position,
    /// from 1 to [`FULL_FLUID`], and returns whether it's loaded. The chunks
    /// that show it are remeshed only if its drawn height changes (see
    /// [`Chunk::set_fluid_at`]). Sends [`FluidChanged`] if the amount changed.
    pub fn set_fluid(&mut self, pos: IVec3, amount: u16) -> bool {
        self.change_fluid(pos, amount, false, false)
    }

    /// [`Voxels::set_fluid`], for flow moving fluid.
    pub fn flow_fluid(&mut self, pos: IVec3, amount: u16) -> bool {
        self.change_fluid(pos, amount, true, false)
    }

    /// Sets the voxel at a world position to `amount` of the fluid `state`,
    /// for flow moving fluid into it (see [`Chunk::start_fluid_at`]).
    pub fn flow_into(&mut self, pos: IVec3, state: BlockState, amount: u16) -> bool {
        self.set(pos, state) && self.change_fluid(pos, amount, true, true)
    }

    fn change_fluid(&mut self, pos: IVec3, amount: u16, by_flow: bool, fresh: bool) -> bool {
        let (coord, local) = ChunkCoord::split(pos);
        let Some(mut chunk) = self
            .map
            .get(coord)
            .and_then(|entity| self.chunks.get_mut(entity).ok())
        else {
            return false;
        };
        let before = chunk.partial_at(local).unwrap_or(FULL_FLUID);
        if before == amount {
            return true;
        }
        let chunk = chunk.bypass_change_detection();
        let redraw = if fresh {
            chunk.start_fluid_at(local, amount)
        } else {
            chunk.set_fluid_at(local, amount)
        };
        if redraw {
            self.touched(pos);
        }
        self.fluid_changes.write(FluidChanged { pos, by_flow });
        true
    }

    /// Marks the chunks that show the voxel at `pos` changed, so they're
    /// remeshed: a voxel on a chunk's edge also shows, hides or shades
    /// faces of the chunks across that edge.
    fn touched(&mut self, pos: IVec3) {
        for touching in ChunkCoord::touching(pos) {
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
        content::{base, block},
        world::WorldPlugin,
        worldgen::{
            Terrain, TerrainBlocks, TerrainSettings,
            testing::{PATCH_SIZE, spawn_patch},
        },
    };

    #[test]
    fn voxels_are_read_and_written_by_world_position() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins(WorldPlugin);
        spawn_patch(app.world_mut());
        let terrain = Terrain::new(
            TerrainSettings::default(),
            TerrainBlocks::from_content(base()),
        );
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                for pos in [IVec3::new(5, 3, 70), IVec3::new(127, 0, 0)] {
                    assert_eq!(voxels.get(pos), Some(terrain.voxel(pos)));
                }
                // Outside the loaded chunks there's nothing to read or write.
                for pos in [IVec3::new(-1, 0, 0), IVec3::new(0, PATCH_SIZE.y, 0)] {
                    assert_eq!(voxels.get(pos), None);
                    assert!(!voxels.set(pos, block("stone")));
                }
                let pos = IVec3::new(20, 40, 33);
                assert!(voxels.set(pos, block("planks")));
                assert_eq!(voxels.get(pos), Some(block("planks")));
            })
            .unwrap();
    }

    /// Setting a voxel's fluid remeshes the chunks that show it only when
    /// its drawn height changes, and across a chunk edge, its neighbour's
    /// too.
    #[test]
    fn fluid_changes_remesh_only_when_the_height_changes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<PropertiesUpdated>()
            .add_message::<FluidChanged>();
        let edge = IVec3::new(15, 5, 5);
        for coord in [IVec3::ZERO, IVec3::X] {
            let mut chunk = Chunk::default();
            if coord == IVec3::ZERO {
                chunk.set_at(edge, block("water"));
            }
            let entity = app.world_mut().spawn((ChunkCoord(coord), chunk)).id();
            app.world_mut()
                .resource_mut::<ChunkMap>()
                .insert(ChunkCoord(coord), entity);
        }
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
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| voxels.set_fluid(edge, amount))
                .unwrap();
            app.world_mut().run_system(changed).unwrap()
        };
        set_fluid(&mut app, FULL_FLUID);
        // 500 and 490 are both drawn half a block high.
        assert_eq!(set_fluid(&mut app, 500), [IVec3::ZERO, IVec3::X]);
        assert_eq!(set_fluid(&mut app, 490), Vec::<IVec3>::new());
        assert_eq!(set_fluid(&mut app, 400), [IVec3::ZERO, IVec3::X]);
    }

    /// Setting a voxel to the fluid it holds fills it, and says so; setting
    /// a full one changes nothing.
    #[test]
    fn setting_fluid_over_itself_fills_it() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<PropertiesUpdated>()
            .add_message::<FluidChanged>();
        let pos = IVec3::new(3, 4, 5);
        let mut chunk = Chunk::default();
        chunk.set_at(pos, block("water"));
        chunk.set_fluid_at(pos, 250);
        let entity = app.world_mut().spawn((ChunkCoord(IVec3::ZERO), chunk)).id();
        app.world_mut()
            .resource_mut::<ChunkMap>()
            .insert(ChunkCoord(IVec3::ZERO), entity);
        let set = |app: &mut App| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| voxels.set(pos, block("water")))
                .unwrap();
            let mut changes = app.world_mut().resource_mut::<Messages<VoxelChanged>>();
            changes.drain().collect::<Vec<_>>()
        };
        let changes = set(&mut app);
        assert_eq!(
            app.world().get::<Chunk>(entity).unwrap().partial_at(pos),
            None
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].partial_before, Some(250));
        assert!(set(&mut app).is_empty(), "already full");
    }
}
