//! Flowing fluid: water and lava fall and spread through the loaded world.
//!
//! A fluid is a block with `fluid` set (see `docs/modding.md`). A voxel of
//! one also holds an amount of it, up to [`FULL_FLUID`] (see
//! [`Chunk`](crate::voxel::Chunk)), and flow only moves those amounts
//! between voxels: fluid is never made, and only destroyed when a block is
//! placed into it. How it moves, and which voxels step when, is in [`flow`];
//! this plugin runs that on the loaded chunks, in [`FixedUpdate`].
//!
//! Fluid is woken (made active, so it steps) where it might move:
//!
//! - around every voxel set to another block, by anything (placing, or
//!   flow itself);
//! - in a chunk as it loads, where its fluid isn't settled: a voxel that
//!   isn't full, or that has room beside or below it. The same goes for
//!   the fluid along the faces of the chunks next to it, which couldn't
//!   flow into it while it wasn't loaded. Fluid never flows into or out of a
//!   chunk that isn't loaded; it waits at the edge.
//!
//! Generated seas and ponds are full voxels on solid ground, so they load
//! settled and stay put until something disturbs them.
//!
//! Placing a block into fluid deletes the fluid, in [`fluid_displaced`],
//! which is where pushing it aside instead would go (#83).

pub mod flow;

use bevy::{ecs::system::SystemParam, prelude::*};

use crate::{
    content::{BlockState, Content, FULL_FLUID},
    terrain::{ChunkCoord, ChunkMap, VoxelChanged, Voxels},
    voxel::{CHUNK_SIZE, Chunk, fluid_amount},
};
use flow::{AROUND, Cell, CellView, Cells, Simulation};

pub struct FluidPlugin;

impl Plugin for FluidPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FluidSettings>()
            .init_resource::<FluidStats>()
            .init_resource::<FluidSim>()
            .add_systems(Update, (wake_around_edits, wake_loaded_chunks).chain())
            .add_systems(FixedUpdate, run_flow);
    }
}

/// How much flow may do.
#[derive(Resource, Reflect, Debug, Clone)]
#[reflect(Resource)]
pub struct FluidSettings {
    /// The most voxels that step in one fixed tick. Past it, flow slows
    /// down rather than taking longer each frame.
    pub cells_per_tick: usize,
}

impl Default for FluidSettings {
    fn default() -> Self {
        Self {
            cells_per_tick: 16_384,
        }
    }
}

/// What flow did in the last fixed tick.
#[derive(Resource, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Resource)]
pub struct FluidStats {
    /// Voxels that stepped.
    pub stepped: usize,
    /// Voxels active: waiting to step.
    pub active: usize,
}

/// The flow simulation over the loaded world.
#[derive(Resource, Debug, Default)]
pub struct FluidSim(pub Simulation);

/// The loaded world as cells for flow.
#[derive(SystemParam)]
pub struct WorldCells<'w, 's> {
    voxels: Voxels<'w, 's>,
    content: Res<'w, Content>,
}

impl CellView for WorldCells<'_, '_> {
    fn cell(&self, pos: IVec3) -> Option<Cell> {
        let (state, partial) = self.voxels.get_with_fluid(pos)?;
        Some(cell_of(&self.content, state, partial))
    }
}

/// The loaded world as cells, to look at.
#[derive(SystemParam)]
pub struct WorldView<'w, 's> {
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, &'static Chunk>,
    content: Res<'w, Content>,
}

impl CellView for WorldView<'_, '_> {
    fn cell(&self, pos: IVec3) -> Option<Cell> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some(cell_of(
            &self.content,
            chunk.at(local),
            chunk.partial_at(local),
        ))
    }
}

impl Cells for WorldCells<'_, '_> {
    fn set(&mut self, pos: IVec3, cell: Cell) {
        match cell {
            Cell::Open => {
                self.voxels.set(pos, BlockState::AIR);
            }
            Cell::Fluid(kind, amount) => {
                self.voxels.set(pos, kind);
                self.voxels.set_fluid(pos, amount);
            }
            Cell::Closed => unreachable!("flow only opens and fills cells"),
        }
    }
}

/// What a voxel of block `state` is to flow, with `partial` the amount of
/// fluid its chunk keeps for it. This decides where fluid can go: only into
/// air, for now. Blocks that share their voxel with fluid (#75) would have
/// room for some.
pub fn cell_of(content: &Content, state: BlockState, partial: Option<u16>) -> Cell {
    if state.is_air() {
        Cell::Open
    } else if content.state(state).is_fluid() {
        Cell::Fluid(state, partial.unwrap_or(FULL_FLUID))
    } else {
        Cell::Closed
    }
}

/// Steps the fluid, at most [`FluidSettings::cells_per_tick`] voxels a tick.
fn run_flow(
    time: Res<Time>,
    settings: Res<FluidSettings>,
    mut sim: ResMut<FluidSim>,
    mut stats: ResMut<FluidStats>,
    content: Res<Content>,
    mut cells: WorldCells,
) {
    let report = sim.0.tick(
        &mut cells,
        |state| content.state(state).fluid.as_ref(),
        time.delta_secs(),
        settings.cells_per_tick,
    );
    stats.set_if_neq(FluidStats {
        stepped: report.stepped,
        active: report.active,
    });
}

/// Wakes the fluid around every voxel set to another block, and passes
/// fluid a block was placed into to [`fluid_displaced`].
fn wake_around_edits(
    mut changes: MessageReader<VoxelChanged>,
    mut sim: ResMut<FluidSim>,
    cells: WorldView,
) {
    for change in changes.read() {
        let before = fluid_amount(&cells.content, change.before, change.partial_before);
        if before > 0
            && let Some(Cell::Closed) = cells.cell(change.pos)
        {
            fluid_displaced(change.pos, change.before, before);
        }
        sim.0.wake_around(&cells, change.pos);
    }
}

/// A block was placed at `pos`, where there was `amount` of the fluid
/// `fluid`. For now that fluid is deleted: it's already gone, so this does
/// nothing. Pushing it into the voxels around instead (#83) would go here.
fn fluid_displaced(_pos: IVec3, _fluid: BlockState, _amount: u16) {}

/// Wakes the fluid that isn't settled in each chunk that's just loaded, and
/// along the faces of the loaded chunks next to it.
fn wake_loaded_chunks(
    added: Query<(&ChunkCoord, &Chunk), Added<Chunk>>,
    mut sim: ResMut<FluidSim>,
    cells: WorldView,
) {
    for (coord, chunk) in &added {
        let origin = coord.origin();
        let has_fluid = chunk
            .voxels()
            .iter()
            .any(|&state| cells.content.state(state).is_fluid());
        if has_fluid {
            for index in 0..chunk.voxels().len() {
                let pos = origin + Chunk::position_of(index);
                if unsettled(&cells, pos) {
                    sim.0.wake(&cells, pos);
                }
            }
        }
        // The layer of each neighbour against this chunk.
        let last = CHUNK_SIZE as i32 - 1;
        for offset in AROUND {
            for a in 0..CHUNK_SIZE as i32 {
                for b in 0..CHUNK_SIZE as i32 {
                    let local = face_voxel(offset, a, b, last);
                    let pos = origin + local + offset;
                    if unsettled(&cells, pos) {
                        sim.0.wake(&cells, pos);
                    }
                }
            }
        }
    }
}

/// The voxel at `(a, b)` on a chunk's face toward `offset`, chunk-local.
fn face_voxel(offset: IVec3, a: i32, b: i32, last: i32) -> IVec3 {
    let edge = |v: i32| if v > 0 { last } else { 0 };
    match offset {
        IVec3 { x: 1 | -1, .. } => IVec3::new(edge(offset.x), a, b),
        IVec3 { y: 1 | -1, .. } => IVec3::new(a, edge(offset.y), b),
        _ => IVec3::new(a, b, edge(offset.z)),
    }
}

/// Whether the fluid at `pos` might move: it isn't full, or it has room
/// below it or beside it. False where there's no fluid.
fn unsettled(cells: &impl CellView, pos: IVec3) -> bool {
    let Some(Cell::Fluid(kind, amount)) = cells.cell(pos) else {
        return false;
    };
    if amount < FULL_FLUID {
        return true;
    }
    let has_room = |at: IVec3| match cells.cell(at) {
        Some(Cell::Open) => true,
        Some(Cell::Fluid(k, a)) => k == kind && a < FULL_FLUID,
        _ => false,
    };
    has_room(pos - IVec3::Y)
        || [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
            .into_iter()
            .any(|side| has_room(pos + side))
}
