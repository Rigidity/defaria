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
//! Placing a block into fluid deletes the fluid. Each time, flow calls
//! [`fluid_displaced`] with what was there before it steps, which is where
//! pushing it aside instead would go (#83).

pub mod flow;

use bevy::{ecs::system::SystemParam, prelude::*};

use crate::{
    content::{BlockState, Content, FULL_FLUID, Fluid},
    fly_camera::EyeView,
    fog::{self, FogSystems},
    terrain::{ChunkCoord, ChunkMap, FluidChanged, VoxelChanged, Voxels},
    voxel::{CHUNK_SIZE, Chunk, fluid_amount, fluid_surface},
};
use flow::{AROUND, Cell, CellView, Cells, Simulation};

pub struct FluidPlugin;

impl Plugin for FluidPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FluidSettings>()
            .init_resource::<FluidStats>()
            .init_resource::<FluidSim>()
            .add_systems(
                Update,
                (note_displaced_fluid, wake_around_edits, wake_loaded_chunks).chain(),
            )
            .add_systems(FixedUpdate, run_flow)
            .add_systems(PostUpdate, fog_inside_fluid.after(FogSystems));
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
    /// Fluid deleted by placing blocks into it since the start, in
    /// thousandths of a block.
    pub displaced: u64,
}

/// The flow simulation over the loaded world.
#[derive(Resource, Debug, Default)]
pub struct FluidSim {
    pub sim: Simulation,
    /// Fluid that blocks were placed into, by where, which fluid and how
    /// much, waiting for [`fluid_displaced`].
    displaced: Vec<(IVec3, BlockState, u16)>,
}

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
    /// the voxel's bottom, as it's drawn (see [`fluid_surface`]).
    pub fn surface(&self, pos: IVec3) -> Option<(BlockState, f32)> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        let kind = chunk.at(local);
        if !self.content.state(kind).is_fluid() {
            return None;
        }
        let covered =
            matches!(self.cell(pos + IVec3::Y), Some(Cell::Fluid(above, _)) if above == kind);
        Some((kind, fluid_surface(chunk.fluid_step_at(local), covered)))
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
                // Setting fluid over itself would fill it.
                if self.voxels.get(pos) == Some(kind) {
                    self.voxels.flow_fluid(pos, amount);
                } else {
                    self.voxels.flow_into(pos, kind, amount);
                }
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
pub(crate) fn run_flow(
    time: Res<Time>,
    settings: Res<FluidSettings>,
    mut sim: ResMut<FluidSim>,
    mut stats: ResMut<FluidStats>,
    content: Res<Content>,
    mut cells: WorldCells,
) {
    for (pos, fluid, amount) in std::mem::take(&mut sim.displaced) {
        fluid_displaced(&mut cells, pos, fluid, amount);
        stats.displaced += amount as u64;
    }
    let start = std::time::Instant::now();
    let report = sim.sim.tick(
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

/// Wakes the fluid that might move because a voxel was set to another
/// block, or its amount of fluid was set by something other than flow: see
/// [`flow::affected_by`].
fn wake_around_edits(
    mut changes: MessageReader<VoxelChanged>,
    mut fluid_changes: MessageReader<FluidChanged>,
    mut sim: ResMut<FluidSim>,
    cells: FluidView,
) {
    let set = fluid_changes
        .read()
        .filter(|change| !change.by_flow)
        .map(|change| change.pos);
    for pos in changes.read().map(|change| change.pos).chain(set) {
        sim.sim.wake_around(&cells, pos);
    }
}

/// Notes the fluid each block (or other fluid) was put in place of, for
/// [`run_flow`] to pass to [`fluid_displaced`]. Whether a block took fluid's place is read from the
/// edit itself, not the voxel now, so fluid that had already flowed away
/// isn't counted.
fn note_displaced_fluid(
    mut changes: MessageReader<VoxelChanged>,
    content: Res<Content>,
    mut sim: ResMut<FluidSim>,
) {
    for change in changes.read() {
        let amount = fluid_amount(&content, change.before, change.partial_before);
        // Anything but air (fluid emptying) or the same fluid (filling it)
        // took the fluid's place: a block, or another fluid.
        if amount > 0 && !change.after.is_air() && change.after != change.before {
            sim.displaced.push((change.pos, change.before, amount));
        }
    }
}

/// A block was placed at `pos`, where there was `amount` of the fluid
/// `fluid`. For now that fluid is deleted: it's already gone, so this does
/// nothing. Pushing it into the cells around instead (#83) goes here, with
/// `cells` to put it in.
fn fluid_displaced(_cells: &mut impl Cells, _pos: IVec3, _fluid: BlockState, _amount: u16) {}

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
        fog.falloff = fog::falloff_between(0.0, fluid.view_distance);
    }
}

/// Wakes the fluid that might move in each chunk that's just loaded, and
/// along the faces of the loaded chunks next to it, which couldn't flow
/// into it while it wasn't loaded.
fn wake_loaded_chunks(
    added: Query<(&ChunkCoord, &Chunk), Added<Chunk>>,
    mut sim: ResMut<FluidSim>,
    cells: FluidView,
) {
    let content = &cells.content;
    let mut wake = |chunk: &Chunk, origin: IVec3, local: IVec3| {
        let state = chunk.at(local);
        let Some(fluid) = content.state(state).fluid.as_ref() else {
            return;
        };
        let pos = origin + local;
        if flow::might_move(&cells, fluid, pos) {
            sim.sim.wake(&cells, pos);
        }
    };
    for (coord, chunk) in &added {
        let origin = coord.origin();
        if chunk
            .voxels()
            .iter()
            .any(|&state| content.state(state).is_fluid())
        {
            for index in 0..chunk.voxels().len() {
                wake(chunk, origin, Chunk::position_of(index));
            }
        }
        // The layer of each neighbour against this chunk.
        let last = CHUNK_SIZE as i32 - 1;
        for offset in AROUND {
            let next = ChunkCoord(coord.0 + offset);
            let Some(neighbour) = cells.map.get(next).and_then(|e| cells.chunks.get(e).ok()) else {
                continue;
            };
            for a in 0..CHUNK_SIZE as i32 {
                for b in 0..CHUNK_SIZE as i32 {
                    // This chunk's face toward the neighbour, seen from the
                    // neighbour: the opposite face.
                    let local = face_voxel(-offset, a, b, last);
                    wake(neighbour, next.origin(), local);
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

    use super::*;
    use crate::{
        content::{base, block},
        terrain::FluidChanged,
        voxel::FLUID_STEPS,
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
            .add_message::<crate::terrain::PropertiesUpdated>()
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
        chunk.fluid_amount_at(local, app.world().resource::<Content>())
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
            let fog = app.world().get::<DistanceFog>(eye).unwrap().clone();
            app.world_mut().despawn(eye);
            (fog.color, fog.falloff)
        };
        let water = base().state(block("water")).fluid.clone().unwrap();
        assert_eq!(fog_at(&mut app, 4.25).0, water.view_color);
        assert_eq!(
            format!("{:?}", fog_at(&mut app, 4.25).1),
            format!("{:?}", fog::falloff_between(0.0, water.view_distance))
        );
        assert_eq!(fog_at(&mut app, 4.75).0, DistanceFog::default().color);
        assert_eq!(fog_at(&mut app, 3.5).0, DistanceFog::default().color);
    }

    /// A chunk that loads with fluid in mid-flow, as saved during a flood,
    /// carries on flowing: its own unsettled fluid wakes.
    #[test]
    fn fluid_saved_mid_flow_flows_on_when_it_loads() {
        let mut app = app();
        let mut chunk = Chunk::filled(block("stone"));
        for x in 0..4 {
            chunk.set(x, 1, 1, BlockState::AIR);
        }
        chunk.set(0, 1, 1, block("water"));
        spawn(&mut app, IVec3::ZERO, chunk);
        app.update();
        assert!(app.world().resource::<FluidSim>().sim.active() > 0);
        run(&mut app, 2.0);
        assert!(amount(&app, IVec3::new(3, 1, 1)) > 0, "it spread");
    }

    /// Water falling and spreading across chunk borders keeps all of itself.
    #[test]
    fn falling_and_spreading_across_chunks_loses_nothing() {
        let mut app = app();
        // Eight chunks around the origin: a floor at y -8, open above.
        for x in -1..=0 {
            for y in -1..=0 {
                for z in -1..=0 {
                    let mut chunk = Chunk::default();
                    if y == -1 {
                        for i in 0..16 {
                            for k in 0..16 {
                                for j in 0..8 {
                                    chunk.set(i, j, k, block("stone"));
                                }
                            }
                        }
                    }
                    spawn(&mut app, IVec3::new(x, y, z), chunk);
                }
            }
        }
        // A tower of water on the corner where all eight meet.
        let mut poured = 0;
        for y in 0..6 {
            for (x, z) in [(-1, -1), (0, 0), (-1, 0), (0, -1)] {
                let pos = IVec3::new(x, y, z);
                app.world_mut()
                    .run_system_once(move |mut voxels: Voxels| {
                        voxels.set(pos, block("water"));
                    })
                    .unwrap();
                poured += FULL_FLUID as u32;
            }
        }
        run(&mut app, 4.0);
        let mut total = 0;
        for x in -16..16 {
            for y in -8..16 {
                for z in -16..16 {
                    total += amount(&app, IVec3::new(x, y, z)) as u32;
                }
            }
        }
        assert_eq!(total, poured);
        for corner in [
            IVec3::new(-3, -8, -3),
            IVec3::new(2, -8, 2),
            IVec3::new(-3, -8, 2),
        ] {
            assert!(
                amount(&app, corner) > 0,
                "it spread into each chunk: {corner}"
            );
        }
    }

    /// Flow steps at most `cells_per_tick` voxels a tick, however many
    /// are active.
    #[test]
    fn flow_keeps_to_its_budget() {
        let mut app = app();
        app.insert_resource(FluidSettings { cells_per_tick: 7 });
        let mut chunk = Chunk::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set(x, 0, z, block("stone"));
            }
        }
        for x in 4..12 {
            chunk.set(x, 3, 8, block("water"));
        }
        spawn(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 2.0);
        let stats = app.world().resource::<FluidStats>();
        assert!(stats.stepped > 0);
        assert_eq!(stats.most_in_a_tick, 7);
    }

    /// Fluid that a block or another fluid takes the place of is passed to
    /// `fluid_displaced` and counted; fluid emptied to air or topped up isn't.
    #[test]
    fn fluid_replaced_by_anything_else_is_displaced() {
        let mut app = app();
        app.add_systems(Update, note_displaced_fluid.before(wake_around_edits));
        let mut chunk = Chunk::filled(block("stone"));
        for x in 0..4 {
            chunk.set(x, 1, 1, block("water"));
        }
        chunk.set_fluid_at(IVec3::new(3, 1, 1), 300);
        spawn(&mut app, IVec3::ZERO, chunk);
        let set = |app: &mut App, x: i32, state: BlockState| {
            let pos = IVec3::new(x, 1, 1);
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| {
                    voxels.set(pos, state);
                })
                .unwrap();
            run(app, 0.1);
            app.world().resource::<FluidStats>().displaced
        };
        assert_eq!(set(&mut app, 0, block("stone")), 1000);
        assert_eq!(set(&mut app, 1, block("lava")), 2000);
        assert_eq!(set(&mut app, 2, BlockState::AIR), 2000);
        assert_eq!(set(&mut app, 3, block("water")), 2000, "topped up");
    }

    /// A pond draining through a gap in its side goes down steadily as it's
    /// drawn: no voxel's drawn height ever goes back up, so nothing blinks.
    #[test]
    fn a_pond_draining_through_its_side_never_blinks() {
        let mut app = app();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )));
        // A floor at y 0; a 6×6 basin two deep, its south wall open at the
        // water, onto the floor a block lower outside.
        let mut chunk = Chunk::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set(x, 0, z, block("stone"));
            }
        }
        for x in 2..10 {
            for z in 2..10 {
                for y in 1..4 {
                    chunk.set(x, y, z, block("stone"));
                }
            }
        }
        let pond: Vec<IVec3> = (3..9)
            .flat_map(|x| (3..9).flat_map(move |z| [IVec3::new(x, 2, z), IVec3::new(x, 3, z)]))
            .collect();
        for &pos in &pond {
            chunk.set_at(pos, block("water"));
        }
        for x in 3..9 {
            for y in 2..4 {
                chunk.set(x, y, 9, BlockState::AIR);
            }
        }
        spawn(&mut app, IVec3::ZERO, chunk);
        let steps = |app: &App| -> Vec<u16> {
            let entity = app
                .world()
                .resource::<ChunkMap>()
                .get(ChunkCoord(IVec3::ZERO))
                .unwrap();
            let chunk = app.world().get::<Chunk>(entity).unwrap();
            // As drawn: none, full under the same fluid, or its height.
            pond.iter()
                .map(|&pos| {
                    if chunk.at(pos).is_air() {
                        0
                    } else if chunk.at(pos + IVec3::Y) == chunk.at(pos) {
                        FLUID_STEPS
                    } else {
                        chunk.fluid_step_at(pos)
                    }
                })
                .collect()
        };
        let mut last = steps(&app);
        for frame in 0..900 {
            app.update();
            let now = steps(&app);
            for (i, (&before, &after)) in last.iter().zip(&now).enumerate() {
                assert!(
                    after <= before,
                    "frame {frame}: {} rose from {before} to {after}",
                    pond[i]
                );
            }
            last = now;
        }
        assert!(last.iter().sum::<u16>() < 72 * FLUID_STEPS, "it drained");
    }
}
