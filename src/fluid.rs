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
//! With the eyes inside a fluid, the fog is the fluid's color and short,
//! as the fluid's `view` says (see `docs/modding.md`).
//!
//! Placing a block into fluid deletes the fluid, in [`fluid_displaced`],
//! which is where pushing it aside instead would go (#83).

pub mod flow;

use bevy::{ecs::system::SystemParam, pbr::FogFalloff, prelude::*};

use crate::{
    content::{BlockState, Content, FULL_FLUID, Fluid},
    fly_camera::EyeView,
    fog::apply_fog,
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
            .add_systems(FixedUpdate, run_flow)
            .add_systems(PostUpdate, fog_inside_fluid.after(apply_fog));
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
            cells_per_tick: 4096,
        }
    }
}

/// What flow has done, for measuring it.
#[derive(Resource, Reflect, Debug, Clone, Default, PartialEq)]
#[reflect(Resource)]
pub struct FluidStats {
    /// Voxels stepped since the start.
    pub stepped: u64,
    /// Voxels active now: waiting to step.
    pub active: usize,
    /// The most voxels stepped in one fixed tick since the start.
    pub most_in_a_tick: usize,
    /// Wall-clock milliseconds spent stepping since the start.
    pub millis: f64,
    /// The most wall-clock milliseconds one fixed tick took.
    pub worst_millis: f64,
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

/// The fluid in the loaded world, to look at: as cells for flow, and where
/// its surface is, for swimming and the view from inside it.
#[derive(SystemParam)]
pub struct FluidView<'w, 's> {
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, &'static Chunk>,
    content: Res<'w, Content>,
}

impl CellView for FluidView<'_, '_> {
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

impl FluidView<'_, '_> {
    /// The fluid in the voxel at `pos`, and how high its surface is above
    /// the voxel's bottom: the whole voxel if the same fluid is above it,
    /// and otherwise as high as its amount fills it.
    pub fn surface(&self, pos: IVec3) -> Option<(BlockState, f32)> {
        let Some(Cell::Fluid(kind, amount)) = self.cell(pos) else {
            return None;
        };
        let full =
            matches!(self.cell(pos + IVec3::Y), Some(Cell::Fluid(above, _)) if above == kind);
        let height = if full {
            1.0
        } else {
            amount as f32 / FULL_FLUID as f32
        };
        Some((kind, height))
    }

    /// The fluid `point` is in, below its surface.
    pub fn fluid_at(&self, point: Vec3) -> Option<BlockState> {
        let pos = point.floor().as_ivec3();
        let (kind, height) = self.surface(pos)?;
        (point.y < pos.y as f32 + height).then_some(kind)
    }

    /// How a fluid flows and feels.
    pub fn fluid(&self, kind: BlockState) -> Option<&Fluid> {
        self.content.state(kind).fluid.as_ref()
    }

    /// How much of the height from `bottom` to `top`, up the column of
    /// voxels at `(x, z)`, is under fluid, as a fraction from 0 to 1, and the
    /// fluid most of that is. `None` if none of it is.
    pub fn immersion(&self, x: f64, z: f64, bottom: f64, top: f64) -> Option<(BlockState, f64)> {
        let mut depths: Vec<(BlockState, f64)> = Vec::new();
        for y in bottom.floor() as i32..=top.floor() as i32 {
            let pos = IVec3::new(x.floor() as i32, y, z.floor() as i32);
            let Some((kind, height)) = self.surface(pos) else {
                continue;
            };
            let under = (y as f64 + height as f64).min(top) - (y as f64).max(bottom);
            if under <= 0.0 {
                continue;
            }
            match depths.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, depth)) => *depth += under,
                None => depths.push((kind, under)),
            }
        }
        let total: f64 = depths.iter().map(|(_, depth)| depth).sum();
        let (kind, _) = depths.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        Some((kind, (total / (top - bottom)).min(1.0)))
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
    let start = std::time::Instant::now();
    let report = sim.0.tick(
        &mut cells,
        |state| content.state(state).fluid.as_ref(),
        time.delta_secs(),
        settings.cells_per_tick,
    );
    if report.stepped > 0 || stats.active != report.active {
        let millis = start.elapsed().as_secs_f64() * 1000.0;
        stats.stepped += report.stepped as u64;
        stats.active = report.active;
        stats.most_in_a_tick = stats.most_in_a_tick.max(report.stepped);
        stats.millis += millis;
        stats.worst_millis = stats.worst_millis.max(millis);
    }
}

/// Wakes the fluid around every voxel set to another block, and passes
/// fluid a block was placed into to [`fluid_displaced`].
fn wake_around_edits(
    mut changes: MessageReader<VoxelChanged>,
    mut sim: ResMut<FluidSim>,
    cells: FluidView,
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

/// With the eyes inside a fluid, the fog is the fluid's: its color, from
/// right at the eyes out to its `view.distance`, in place of the fog at the
/// edge of the world.
fn fog_inside_fluid(
    fluids: FluidView,
    mut views: Query<(&GlobalTransform, &mut DistanceFog), With<EyeView>>,
) {
    for (transform, mut fog) in &mut views {
        let inside = fluids.fluid_at(transform.translation());
        let Some(fluid) = inside.and_then(|kind| fluids.fluid(kind)) else {
            continue;
        };
        fog.color = fluid.view_color;
        fog.falloff = FogFalloff::Atmospheric {
            extinction: Vec3::splat(fluid.view_distance),
            inscattering: Vec3::ZERO,
        };
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
    cells: FluidView,
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::{
        content::{base, block},
        terrain::FluidChanged,
    };

    /// The flow systems alone, on chunks spawned by the test.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                50,
            )))
            .insert_resource(base().clone())
            .init_resource::<ChunkMap>()
            .init_resource::<FluidSettings>()
            .init_resource::<FluidStats>()
            .init_resource::<FluidSim>()
            .add_message::<VoxelChanged>()
            .add_message::<FluidChanged>()
            .add_systems(Update, (wake_around_edits, wake_loaded_chunks).chain())
            .add_systems(FixedUpdate, run_flow);
        app
    }

    fn spawn(app: &mut App, coord: IVec3, chunk: Chunk) {
        let coord = ChunkCoord(coord);
        let entity = app.world_mut().spawn((coord, chunk)).id();
        app.world_mut()
            .resource_mut::<ChunkMap>()
            .insert(coord, entity);
    }

    fn amount(app: &App, pos: IVec3) -> u16 {
        let (coord, local) = ChunkCoord::split(pos);
        let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
        let chunk = app.world().get::<Chunk>(entity).unwrap();
        chunk.fluid_at(local, app.world().resource::<Content>())
    }

    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds / 0.05) as usize {
            app.update();
        }
    }

    /// Fluid against the face of a chunk that isn't loaded waits there,
    /// and flows into it once it loads.
    #[test]
    fn fluid_waits_for_the_next_chunk_to_load() {
        let mut app = app();
        // Stone, but for a tunnel along X at y 1, z 8: water fills the end
        // of the first chunk's, against its +X face.
        let tunnel = || {
            let mut chunk = Chunk::filled(block("stone"));
            for x in 0..16 {
                chunk.set(x, 1, 8, BlockState::AIR);
            }
            chunk
        };
        let mut chunk = Chunk::filled(block("stone"));
        chunk.set(15, 1, 8, block("water"));
        spawn(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 2.0);
        let near = IVec3::new(15, 1, 8);
        assert_eq!(amount(&app, near), FULL_FLUID, "nowhere to go");
        assert_eq!(app.world().resource::<FluidStats>().active, 0, "settled");

        spawn(&mut app, IVec3::X, tunnel());
        run(&mut app, 3.0);
        let along: Vec<u16> = (15..32)
            .map(|x| amount(&app, IVec3::new(x, 1, 8)))
            .collect();
        assert!(along[1] > 0, "flowed into the new chunk: {along:?}");
        assert_eq!(along.iter().map(|&a| a as u32).sum::<u32>(), 1000);
    }

    /// A chunk full of settled water, like the sea's, loads with nothing
    /// active.
    #[test]
    fn settled_water_loads_asleep() {
        let mut app = app();
        let mut sea = Chunk::filled(block("water"));
        for x in 0..16 {
            for z in 0..16 {
                sea.set(x, 0, z, block("stone"));
            }
        }
        // Walls all round, as the sea's neighbours would be.
        for i in 0..16 {
            for y in 0..16 {
                sea.set(i, y, 0, block("stone"));
                sea.set(i, y, 15, block("stone"));
                sea.set(0, y, i, block("stone"));
                sea.set(15, y, i, block("stone"));
            }
        }
        spawn(&mut app, IVec3::ZERO, sea);
        run(&mut app, 0.5);
        let stats = app.world().resource::<FluidStats>();
        assert_eq!((stats.active, stats.stepped), (0, 0));
    }

    /// With the eyes under a fluid's surface, the fog is the fluid's; above
    /// it, or in a voxel's empty top, it's left alone.
    #[test]
    fn the_fog_inside_fluid_is_the_fluids() {
        let mut app = app();
        app.add_systems(Update, fog_inside_fluid);
        let mut chunk = Chunk::default();
        chunk.set(4, 4, 4, block("water"));
        chunk.set_fluid_at(IVec3::splat(4), FULL_FLUID / 2);
        spawn(&mut app, IVec3::ZERO, chunk);
        let fog_at = |app: &mut App, y: f32| {
            let eye = app
                .world_mut()
                .spawn((
                    EyeView,
                    GlobalTransform::from_translation(Vec3::new(4.5, y, 4.5)),
                    DistanceFog::default(),
                ))
                .id();
            app.update();
            let fog = app.world().get::<DistanceFog>(eye).unwrap().color;
            app.world_mut().despawn(eye);
            fog
        };
        let water = base().state(block("water")).fluid.clone().unwrap();
        assert_eq!(fog_at(&mut app, 4.25), water.view_color);
        assert_eq!(fog_at(&mut app, 4.75), DistanceFog::default().color);
        assert_eq!(fog_at(&mut app, 3.5), DistanceFog::default().color);
    }
}
