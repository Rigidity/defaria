//! Flowing fluid: water and lava fall and spread through the loaded world.
//!
//! A fluid is a block with `fluid` set (see `docs/modding.md`). A voxel
//! holds an amount of one, up to [`FULL_FLUID`], beside its block (see
//! [`Chunk`](crate::world::Chunk)): all of its cell for air, some for a
//! block that leaves room for it (`fluid_room`, a slab's half), none for a
//! full block. Flow only moves those amounts between voxels, never
//! touching blocks: fluid is never made, and only destroyed when a block
//! is placed into it and nothing around has room for what no longer fits
//! (see [`fluid_displaced`]). How it moves, and which voxels step when, is
//! in [`flow`]; this plugin runs that on the loaded chunks, in
//! [`FixedUpdate`].
//!
//! Fluid is woken (made active, so it steps) where it might move:
//!
//! - around every voxel set to another block, by anything (placing, or
//!   flow itself);
//! - around each voxel fluid a block took the place of was pushed into;
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
//! Placing a block into fluid pushes what no longer fits aside: half a
//! block for a slab, all of it for stone. Flow takes it on its next tick,
//! before it steps, putting it in voxels with room for it and spreading
//! it from there over the steps after (see [`fluid_displaced`]):
//! sideways first, the nearest on its level reached through the same
//! fluid, then above, then below. The sideways search looks at no more
//! than [`flow::PUSH_REACH`] voxels, so a block placed into a body of
//! fluid costs the same however big it is, and placing many blocks at
//! once costs no more than one each. What none of them has room for (a
//! block placed at the bottom of a full, sealed shaft) is deleted, and
//! counted.

pub mod flow;
pub mod reservoir;

use bevy::{ecs::system::SystemParam, prelude::*};

use crate::{
    content::{BlockState, Content, Fluid},
    fly_camera::EyeView,
    fog::{self, FogSystems},
    streaming::{BeforeChunkSave, ChunksSaving, SaveOnExit},
    world::{
        CHUNK_SIZE, Chunk, ChunkCoord, ChunkMap, FluidChanged, FluidTop, VoxelChanged, VoxelFluid,
        Voxels, fluid_surface,
    },
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
            .add_systems(
                FixedUpdate,
                (touch_reservoirs, run_flow).chain().in_set(FlowSystems),
            )
            .add_systems(Update, flush_reservoirs_before_save.in_set(BeforeChunkSave))
            .add_systems(
                SaveOnExit,
                flush_reservoirs_before_save.in_set(BeforeChunkSave),
            )
            .add_systems(PostUpdate, fog_inside_fluid.after(FogSystems));
    }
}

/// Where flow runs, in `FixedUpdate`. Systems there that change fluid
/// other than through flow (pumps, outlets, pipes spilling) run before it,
/// so reservoirs hear of their changes before they're next levelled.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowSystems;

/// How much flow may do.
#[derive(Resource, Reflect, Debug, Clone)]
#[reflect(Resource)]
pub struct FluidSettings {
    /// The most voxels that step in one fixed tick. Past it, flow slows
    /// down rather than taking longer each frame.
    pub cells_per_tick: usize,
    /// The most columns one reservoir (a body of fluid kept at one level,
    /// see [`reservoir`]) has. 0 turns reservoirs off.
    pub reservoir_columns: usize,
    /// How many columns reservoirs may level in one fixed tick, all told:
    /// past it, the rest wait for the next tick, so a big flood's
    /// levelling is spread over frames. At least one reservoir is levelled
    /// a tick, however big.
    pub reservoir_work: usize,
}

impl Default for FluidSettings {
    fn default() -> Self {
        Self {
            cells_per_tick: 4096,
            reservoir_columns: 16384,
            reservoir_work: 12288,
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
    /// Fluid that blocks placed into it, or another fluid poured over it,
    /// pushed aside since the start, in thousandths of a block.
    pub pushed_aside: u64,
    /// Fluid that blocks placed into it, or another fluid poured over it,
    /// deleted since the start, where nothing around had room for it, in
    /// thousandths of a block.
    pub deleted: u64,
    /// When flow last stepped anything, in seconds of fixed time since the
    /// start: once that stops going up, the fluid has settled.
    pub last_moved: f32,
    /// Reservoirs now, and how many columns they have.
    pub reservoirs: usize,
    pub reservoir_columns: usize,
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
        VoxelCells {
            voxels: &self.voxels,
            content: &self.content,
        }
        .cell(pos)
    }
}

/// The loaded world as cells for flow, read through [`Voxels`] borrowed
/// from elsewhere, such as a pump's system.
pub struct VoxelCells<'a, 'w, 's> {
    pub voxels: &'a Voxels<'w, 's>,
    pub content: &'a Content,
}

impl CellView for VoxelCells<'_, '_, '_> {
    fn cell(&self, pos: IVec3) -> Option<Cell> {
        let (state, fluid) = self.voxels.get_with_fluid(pos)?;
        Some(cell_of(self.content, state, fluid))
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
            chunk.fluid_at(local),
        ))
    }
}

impl FluidView<'_, '_> {
    /// The fluid in the voxel at `pos` and how high it's drawn there (see
    /// [`fluid_surface`]), if it's loaded and holds any.
    fn voxel(&self, pos: IVec3) -> Option<(BlockState, f32)> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        let fluid = chunk.fluid_at(local)?;
        let room = self.content.state(chunk.at(local)).fluid_room;
        Some((fluid.kind, fluid_surface(chunk.fluid_step_at(local), room)))
    }

    /// The fluid in the voxel at `pos`, and its surface, as it's drawn
    /// (see [`FluidTop`]).
    pub fn surface(&self, pos: IVec3) -> Option<(BlockState, FluidTop)> {
        let (kind, _) = self.voxel(pos)?;
        Some((kind, FluidTop::of(kind, pos, |at| self.voxel(at))))
    }

    /// The fluid `point` is in, below its surface.
    pub fn fluid_at(&self, point: Vec3) -> Option<BlockState> {
        let pos = point.floor().as_ivec3();
        let (kind, top) = self.surface(pos)?;
        let height = top.at(point.xz() - pos.xz().as_vec2());
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
            let Some((kind, surface)) = self.surface(pos) else {
                continue;
            };
            let height = surface.at(Vec2::new((x - x.floor()) as f32, (z - z.floor()) as f32));
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
        let fluid = match cell {
            Cell::Open(_) => None,
            Cell::Fluid(kind, amount, _) => Some(VoxelFluid { kind, amount }),
            Cell::Closed => unreachable!("flow only opens and fills cells"),
        };
        self.voxels.flow_fluid(pos, fluid);
    }
}

/// What a voxel of block `state` holding `fluid` is to flow: its room is
/// the block's `fluid_room`, all of the voxel for air and none for a full
/// block. This decides where fluid can go.
pub fn cell_of(content: &Content, state: BlockState, fluid: Option<VoxelFluid>) -> Cell {
    let room = content.state(state).fluid_room;
    match fluid {
        Some(fluid) => Cell::Fluid(fluid.kind, fluid.amount, room),
        None if room > 0 => Cell::Open(room),
        None => Cell::Closed,
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
        let deleted = fluid_displaced(&mut cells, &mut sim.sim, pos, fluid, amount);
        stats.pushed_aside += (amount - deleted) as u64;
        stats.deleted += deleted as u64;
    }
    sim.sim.reservoirs.max_columns = settings.reservoir_columns;
    sim.sim.reservoirs.work = settings.reservoir_work;
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
        if report.stepped > 0 {
            stats.last_moved = time.elapsed_secs();
        }
    }
    stats.reservoirs = sim.sim.reservoirs.len();
    stats.reservoir_columns = sim.sim.reservoirs.iter().map(|r| r.columns()).sum();
}

/// Tells reservoirs about every voxel something set since, or whose amount
/// of fluid something other than flow set, so the columns they're in are
/// read again before they're next levelled (see [`reservoir`]).
fn touch_reservoirs(
    mut changes: MessageReader<VoxelChanged>,
    mut fluid_changes: MessageReader<FluidChanged>,
    mut sim: ResMut<FluidSim>,
    content: Res<Content>,
) {
    let reservoirs = &mut sim.sim.reservoirs;
    for change in changes.read() {
        // A block that leaves less room for fluid than the one before can
        // cut a body of it in two.
        let room = |state: BlockState| content.state(state).fluid_room;
        if room(change.after) < room(change.before) {
            reservoirs.walled(change.pos);
        }
        reservoirs.touch(change.pos);
    }
    for change in fluid_changes.read().filter(|change| !change.by_flow) {
        reservoirs.touch(change.pos);
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

/// Before chunks are saved: drops the reservoirs that reach into a chunk
/// that's unloading, paying their balances out into the world, and when
/// the loaded chunks are saved too, pays every balance out, so the saved
/// chunks hold all their fluid.
fn flush_reservoirs_before_save(
    saving: Option<Res<ChunksSaving>>,
    mut sim: ResMut<FluidSim>,
    mut cells: WorldCells,
) {
    let Some(saving) = saving else {
        return;
    };
    for &coord in &saving.unloading {
        sim.sim.reservoirs.unloading(coord);
    }
    if saving.loaded {
        sim.sim.flush_reservoirs(&mut cells);
    } else if !saving.unloading.is_empty() {
        sim.sim.drop_marked_reservoirs(&mut cells);
    }
}

/// Notes the fluid each block (or other fluid) was put in place of, for
/// [`run_flow`] to pass to [`fluid_displaced`]: what a new block leaves no
/// room for, and fluid that another fluid was poured over. It's read from
/// the edit itself, not the voxel now, so fluid that had already flowed
/// away isn't counted.
fn note_displaced_fluid(
    mut changes: MessageReader<VoxelChanged>,
    mut fluid_changes: MessageReader<FluidChanged>,
    mut sim: ResMut<FluidSim>,
) {
    let edits = changes
        .read()
        .map(|change| (change.pos, change.displaced()));
    let replaced = fluid_changes.read().map(|change| {
        let replaced = match (change.before, change.after) {
            (Some(before), Some(after)) if !change.by_flow && before.kind != after.kind => {
                Some(before)
            }
            _ => None,
        };
        (change.pos, replaced)
    });
    for (pos, displaced) in edits.chain(replaced) {
        if let Some(fluid) = displaced {
            sim.displaced.push((pos, fluid.kind, fluid.amount));
        }
    }
}

/// A block was placed at `pos`, taking the place of `amount` of the fluid
/// `fluid`, which is gone from there already. Pushes it into the voxels
/// around with room for it (see [`flow::push_aside`]), and wakes the fluid
/// around where it went, to spread it on. Returns what none of them had
/// room for, which is deleted.
fn fluid_displaced(
    cells: &mut WorldCells,
    sim: &mut Simulation,
    pos: IVec3,
    fluid: BlockState,
    amount: u16,
) -> u16 {
    let (deleted, into) = flow::push_aside(cells, pos, fluid, amount);
    for at in into {
        // Written as flow writes, so reservoirs must be told.
        sim.reservoirs.touch(at);
        sim.wake_around(cells, at);
    }
    deleted
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
        let Some(kind) = chunk.fluid_at(local).map(|fluid| fluid.kind) else {
            return;
        };
        let Some(fluid) = content.state(kind).fluid.as_ref() else {
            return;
        };
        let pos = origin + local;
        if flow::might_move(&cells, fluid, pos) {
            sim.sim.wake(&cells, pos);
        }
    };
    for (coord, chunk) in &added {
        let origin = coord.origin();
        for (index, _) in chunk.fluids() {
            wake(chunk, origin, Chunk::position_of(index));
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
        content::{FULL_FLUID, base, block},
        testing::{chunk_app, insert_chunk},
        world::FLUID_STEPS,
    };

    /// The flow systems alone, on chunks spawned by the test.
    fn app() -> App {
        let mut app = chunk_app(base().clone(), Duration::from_millis(50));
        app.init_resource::<FluidSettings>()
            .init_resource::<FluidStats>()
            .init_resource::<FluidSim>()
            .add_systems(Update, (wake_around_edits, wake_loaded_chunks).chain())
            .add_systems(FixedUpdate, (touch_reservoirs, run_flow).chain());
        app
    }

    fn amount(app: &App, pos: IVec3) -> u16 {
        let (coord, local) = ChunkCoord::split(pos);
        let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
        let chunk = app.world().get::<Chunk>(entity).unwrap();
        chunk.fluid_amount_at(local)
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
        chunk.set_to_fluid(IVec3::new(15, 1, 8), block("water"));
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 2.0);
        let near = IVec3::new(15, 1, 8);
        assert_eq!(amount(&app, near), FULL_FLUID, "nowhere to go");
        assert_eq!(app.world().resource::<FluidStats>().active, 0, "settled");

        insert_chunk(&mut app, IVec3::X, tunnel());
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
        // A floor, and walls all round, as the sea's neighbours would be.
        let mut sea = Chunk::default();
        for (x, y, z) in Chunk::positions() {
            if y == 0 || x == 0 || x == 15 || z == 0 || z == 15 {
                sea.set(x, y, z, block("stone"));
            } else {
                sea.fill_fluid_at(IVec3::new(x as i32, y as i32, z as i32), block("water"));
            }
        }
        insert_chunk(&mut app, IVec3::ZERO, sea);
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
        let half = VoxelFluid::of(block("water"), FULL_FLUID / 2);
        chunk.set_fluid_at(IVec3::splat(4), half, FULL_FLUID);
        insert_chunk(&mut app, IVec3::ZERO, chunk);
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
        chunk.set_to_fluid(IVec3::new(0, 1, 1), block("water"));
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        app.update();
        assert!(app.world().resource::<FluidSim>().sim.active() > 0);
        run(&mut app, 2.0);
        assert!(amount(&app, IVec3::new(3, 1, 1)) > 0, "it spread");
    }

    /// Water flows into the room a slab and a tuft leave in their voxels,
    /// beside it and below it, never more than that room, and keeps all of
    /// itself. The blocks stay as they are.
    #[test]
    fn water_fills_the_room_blocks_leave() {
        let mut app = app();
        // A sealed trench along X at y 1: water, a slab, a tuft, and a
        // slab under the water's end.
        let mut chunk = Chunk::filled(block("stone"));
        for x in 1..=4 {
            chunk.set(x, 1, 1, BlockState::AIR);
        }
        chunk.set_to_fluid(IVec3::new(1, 1, 1), block("water"));
        chunk.set_to_fluid(IVec3::new(2, 1, 1), block("water"));
        chunk.set(3, 1, 1, block("stone_slab"));
        chunk.set(4, 1, 1, block("tuft"));
        chunk.set(1, 0, 1, block("stone_slab"));
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 6.0);
        let at = |x, y| amount(&app, IVec3::new(x, y, 1));
        assert_eq!(at(1, 0), 500, "the slab below fills");
        assert!(at(3, 1) > 0 && at(3, 1) <= 500, "{}", at(3, 1));
        assert!(at(4, 1) > 0 && at(4, 1) <= 900, "{}", at(4, 1));
        let total: u16 = (1..=4).map(|x| at(x, 1)).sum::<u16>() + at(1, 0);
        assert_eq!(total, 2 * FULL_FLUID);
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(IVec3::ZERO));
        let chunk = app.world().get::<Chunk>(entity.unwrap()).unwrap();
        assert_eq!(chunk.at(IVec3::new(3, 1, 1)), block("stone_slab"));
        assert_eq!(chunk.at(IVec3::new(4, 1, 1)), block("tuft"));
    }

    /// A sealed pond of water, `size` wide and `depth` deep, full, in a
    /// chunk of stone, with `air` blocks of air above it inside its rim.
    fn pond(size: i32, depth: i32, air: i32) -> Chunk {
        let mut chunk = Chunk::filled(block("stone"));
        for x in 1..=size {
            for z in 1..=size {
                for y in 1..=depth {
                    chunk.set_to_fluid(IVec3::new(x, y, z), block("water"));
                }
                for y in depth + 1..=depth + air {
                    chunk.set_at(IVec3::new(x, y, z), BlockState::AIR);
                }
            }
        }
        chunk
    }

    /// Places `state` at each of `at` in one frame, as an edit.
    fn place(app: &mut App, at: Vec<IVec3>, state: BlockState) {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                for &pos in &at {
                    voxels.set(pos, state);
                }
            })
            .unwrap();
    }

    fn total(app: &App, size: i32, height: i32) -> u32 {
        let mut total = 0;
        for x in 1..=size {
            for z in 1..=size {
                for y in 1..=height {
                    total += amount(app, IVec3::new(x, y, z)) as u32;
                }
            }
        }
        total
    }

    /// A block placed into a pond's surface pushes the water it takes the
    /// place of up onto the surface, and it spreads over the pond, raising
    /// it: none is lost. A slab placed there pushes only the half it takes.
    #[test]
    fn a_block_in_a_ponds_surface_raises_it() {
        let mut app = app();
        app.add_systems(Update, note_displaced_fluid.before(wake_around_edits));
        insert_chunk(&mut app, IVec3::ZERO, pond(6, 2, 3));
        run(&mut app, 0.5);
        let before = total(&app, 6, 5);
        assert_eq!(before, 72_000);
        place(&mut app, vec![IVec3::new(3, 2, 3)], block("stone"));
        place(&mut app, vec![IVec3::new(5, 2, 5)], block("stone_slab"));
        run(&mut app, 8.0);
        let stats = app.world().resource::<FluidStats>().clone();
        assert_eq!((stats.pushed_aside, stats.deleted), (1500, 0));
        assert_eq!(total(&app, 6, 5), before, "none lost");
        // Spread over the pond: a thin layer above the old surface, over
        // many more voxels than the two it came up into.
        let spread = (1..=6)
            .flat_map(|x| (1..=6).map(move |z| IVec3::new(x, 3, z)))
            .filter(|&pos| amount(&app, pos) > 0)
            .count();
        assert!(spread > 10, "{spread}");
        assert_eq!(stats.active, 0, "settled");
    }

    /// A block dropped into a pond whose top layer isn't full pushes its
    /// water sideways through the pond, raising that layer, rather than up
    /// onto it: nothing stands above the old surface, and none is lost.
    #[test]
    fn a_block_in_a_pond_that_isnt_full_raises_it_sideways() {
        let mut app = app();
        app.add_systems(Update, note_displaced_fluid.before(wake_around_edits));
        let mut chunk = pond(6, 2, 3);
        let water = VoxelFluid::of(block("water"), 400);
        for x in 1..=6 {
            for z in 1..=6 {
                chunk.set_fluid_at(IVec3::new(x, 2, z), water, FULL_FLUID);
            }
        }
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 0.5);
        let before = total(&app, 6, 5);
        place(&mut app, vec![IVec3::new(3, 2, 3)], block("stone"));
        run(&mut app, 4.0);
        let stats = app.world().resource::<FluidStats>().clone();
        assert_eq!((stats.pushed_aside, stats.deleted), (400, 0));
        assert_eq!(total(&app, 6, 5), before, "none lost");
        let above: u32 = (1..=6)
            .flat_map(|x| (1..=6).map(move |z| IVec3::new(x, 3, z)))
            .map(|pos| amount(&app, pos) as u32)
            .sum();
        assert_eq!(above, 0, "nothing piled on top");
    }

    /// A block placed where nothing around has room for the fluid it takes
    /// the place of (deep in a full, sealed pond) deletes it and counts it.
    /// Placing many at once, they wake only the fluid around each, never
    /// the whole pond, however big it is.
    #[test]
    fn blocks_deep_in_a_full_pond_delete_and_wake_little() {
        let mut app = app();
        app.add_systems(Update, note_displaced_fluid.before(wake_around_edits));
        insert_chunk(&mut app, IVec3::ZERO, pond(14, 14, 0));
        run(&mut app, 0.5);
        assert_eq!(app.world().resource::<FluidStats>().active, 0);
        let cube: Vec<IVec3> = (6..9)
            .flat_map(|x| (6..9).flat_map(move |y| (6..9).map(move |z| IVec3::new(x, y, z))))
            .collect();
        let placed = cube.len() as u64;
        place(&mut app, cube, block("stone"));
        app.update();
        let woken = app.world().resource::<FluidSim>().sim.active();
        assert!(woken < 300, "{woken} woken by {placed} blocks");
        run(&mut app, 2.0);
        let stats = app.world().resource::<FluidStats>().clone();
        assert_eq!((stats.pushed_aside, stats.deleted), (0, placed * 1000));
        assert!(stats.stepped < 1000, "{} stepped", stats.stepped);
        assert_eq!(total(&app, 14, 14), (14 * 14 * 14 - placed as u32) * 1000);
    }

    /// Blocks placed across a wide, full pond's surface push their water
    /// up, and wake only the fluid around each block and where its water
    /// went, not the pond.
    #[test]
    fn pushing_aside_wakes_little() {
        let mut app = app();
        app.add_systems(Update, note_displaced_fluid.before(wake_around_edits));
        insert_chunk(&mut app, IVec3::ZERO, pond(14, 13, 2));
        run(&mut app, 0.5);
        let row: Vec<IVec3> = (1..=14).map(|x| IVec3::new(x, 13, 7)).collect();
        place(&mut app, row, block("stone"));
        // The edit's frame, then the fixed tick that pushes the water.
        app.update();
        app.update();
        let stats = app.world().resource::<FluidStats>().clone();
        assert_eq!((stats.pushed_aside, stats.deleted), (14_000, 0));
        let woken = app.world().resource::<FluidSim>().sim.active();
        assert!(woken < 600, "{woken} woken in a pond of {}", 14 * 14 * 13);
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
                    insert_chunk(&mut app, IVec3::new(x, y, z), chunk);
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
        // With what reservoirs hold and haven't put back yet.
        let pooled = app
            .world()
            .resource::<FluidSim>()
            .sim
            .reservoirs
            .pooled_total();
        assert_eq!(total as i64 + pooled, poured as i64);
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
        app.insert_resource(FluidSettings {
            cells_per_tick: 7,
            ..default()
        });
        let mut chunk = Chunk::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set(x, 0, z, block("stone"));
            }
        }
        for x in 4..12 {
            chunk.set_to_fluid(IVec3::new(x, 3, 8), block("water"));
        }
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 2.0);
        let stats = app.world().resource::<FluidStats>();
        assert!(stats.stepped > 0);
        assert_eq!(stats.most_in_a_tick, 7);
        // When flow last moved anything, which stops going up once it's
        // settled.
        let moved = stats.last_moved;
        assert!(moved > 0.0);
        run(&mut app, 30.0);
        let moved = app.world().resource::<FluidStats>().last_moved;
        run(&mut app, 1.0);
        assert_eq!(
            app.world().resource::<FluidStats>().last_moved,
            moved,
            "settled"
        );
    }

    /// Fluid that a block or another fluid takes the place of is passed to
    /// `fluid_displaced`, and deleted and counted where nothing it can
    /// reach has room for it (here, a trench of water sealed in stone);
    /// fluid emptied to air or topped up isn't.
    #[test]
    fn fluid_replaced_by_anything_else_is_displaced() {
        let mut app = app();
        app.add_systems(Update, note_displaced_fluid.before(wake_around_edits));
        let mut chunk = Chunk::filled(block("stone"));
        for x in 0..4 {
            chunk.set_to_fluid(IVec3::new(x, 1, 1), block("water"));
        }
        chunk.set_fluid_at(
            IVec3::new(3, 1, 1),
            VoxelFluid::of(block("water"), 300),
            FULL_FLUID,
        );
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        // How much was there before each edit, and how much it displaced.
        let set = |app: &mut App, x: i32, state: BlockState| {
            let pos = IVec3::new(x, 1, 1);
            let displaced = |app: &App| {
                let stats = app.world().resource::<FluidStats>();
                stats.pushed_aside + stats.deleted
            };
            let (before, was) = (amount(app, pos), displaced(app));
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| {
                    voxels.set(pos, state);
                })
                .unwrap();
            run(app, 0.1);
            (before as u64, displaced(app) - was)
        };
        let (before, displaced) = set(&mut app, 0, block("stone"));
        assert_eq!((before, displaced), (1000, 1000));
        // Along the trench to its part-full end, through the water: what
        // that had room for went there, and the rest had nowhere to go.
        let deleted = app.world().resource::<FluidStats>().deleted;
        assert_eq!(deleted, 300, "the end had room for 700");
        let (before, displaced) = set(&mut app, 1, block("lava"));
        assert!(before > 0);
        assert_eq!(displaced, before, "the lava took the water's place");
        assert_eq!(set(&mut app, 2, BlockState::AIR).1, 0, "emptied");
        assert_eq!(set(&mut app, 3, block("water")).1, 0, "topped up");
    }

    /// A block put in a reservoir drops it, since it may cut it in two; a
    /// block put above it, out of its reach, doesn't.
    #[test]
    fn a_block_put_in_a_reservoir_drops_it() {
        let mut app = app();
        // An 8 by 8 pond, one deep, in a stone basin, with air above.
        let mut chunk = Chunk::filled(block("stone"));
        for x in 2..10 {
            for z in 2..10 {
                chunk.set_to_fluid(IVec3::new(x as i32, 1, z as i32), block("water"));
                for y in 2..16 {
                    chunk.set(x, y, z, BlockState::AIR);
                }
            }
        }
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        run(&mut app, 0.2);
        let found = app
            .world_mut()
            .run_system_once(
                |voxels: Voxels, content: Res<Content>, mut sim: ResMut<FluidSim>| {
                    let cells = VoxelCells {
                        voxels: &voxels,
                        content: &content,
                    };
                    let water = content.state(block("water")).fluid.clone().unwrap();
                    sim.sim
                        .reservoirs
                        .draw(&cells, &water, IVec3::new(5, 1, 5), 0)
                },
            )
            .unwrap();
        assert_eq!(found, Some(0));
        let reservoirs = |app: &App| app.world().resource::<FluidSim>().sim.reservoirs.len();
        assert_eq!(reservoirs(&app), 1);
        let put = |app: &mut App, pos: IVec3| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| {
                    voxels.set(pos, block("stone"));
                })
                .unwrap();
            run(app, 0.2);
        };
        put(&mut app, IVec3::new(5, 6, 5));
        assert_eq!(reservoirs(&app), 1, "a block well above it");
        put(&mut app, IVec3::new(5, 1, 5));
        assert_eq!(reservoirs(&app), 0, "a block in it");
    }

    /// Swimming and the fog find the surface where it's drawn: sloping
    /// from a full voxel down to a quarter-full one beside it, and full
    /// height beside a falling column.
    #[test]
    fn the_surface_you_swim_at_slopes_as_drawn() {
        let mut app = app();
        // A channel along X in stone: full water, a quarter, then air,
        // with air above them all.
        let mut chunk = Chunk::filled(block("stone"));
        let (full, quarter) = (IVec3::new(4, 4, 4), IVec3::new(5, 4, 4));
        for x in 4..7 {
            for y in 4..7 {
                chunk.set_at(IVec3::new(x, y, 4), BlockState::AIR);
            }
        }
        chunk.set_to_fluid(full, block("water"));
        chunk.set_to_fluid(quarter, block("water"));
        let water = VoxelFluid::of(block("water"), FULL_FLUID / 4);
        chunk.set_fluid_at(quarter, water, FULL_FLUID);
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        let at = |app: &mut App, point: Vec3| {
            app.world_mut()
                .run_system_once(move |fluids: FluidView| fluids.fluid_at(point))
                .unwrap()
        };
        let water = Some(block("water"));
        let q = quarter.as_vec3();
        // Beside the full voxel the quarter's surface is 0.625 high; at its
        // far side, 0.25.
        assert_eq!(at(&mut app, q + Vec3::new(0.02, 0.6, 0.5)), water);
        assert_eq!(at(&mut app, q + Vec3::new(0.98, 0.3, 0.5)), None);
        assert_eq!(at(&mut app, q + Vec3::new(0.98, 0.2, 0.5)), water);
        let top = app
            .world_mut()
            .run_system_once(move |fluids: FluidView| fluids.surface(quarter).unwrap().1)
            .unwrap();
        assert_eq!(top.corner(0, 0), 0.625);
        assert_eq!(top.corner(1, 1), 0.25);
        // Swimming measures the same slope: how much of a block-high body
        // standing on the floor is under water.
        let depth = |app: &mut App, x: f64| {
            app.world_mut()
                .run_system_once(move |fluids: FluidView| {
                    fluids.immersion(x, 4.5, 4.0, 5.0).map(|(_, depth)| depth)
                })
                .unwrap()
                .unwrap()
        };
        assert!((depth(&mut app, 5.02) - (0.625 - 0.375 * 0.02)).abs() < 1e-4);
        assert!((depth(&mut app, 5.98) - (0.625 - 0.375 * 0.98)).abs() < 1e-4);

        // Water falling into the quarter's other side raises that edge to
        // a full block.
        let (beside, above) = (IVec3::new(6, 4, 4), IVec3::new(6, 5, 4));
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                voxels.set(beside, block("water"));
                voxels.set(above, block("water"));
            })
            .unwrap();
        let top = app
            .world_mut()
            .run_system_once(move |fluids: FluidView| fluids.surface(quarter).unwrap().1)
            .unwrap();
        assert_eq!(top.corner(1, 0), 1.0);
        assert_eq!(at(&mut app, q + Vec3::new(0.98, 0.9, 0.5)), water);
    }

    /// A pond draining through a gap in its side goes down steadily as it's
    /// drawn: no voxel's drawn height, nor any corner of its sloped top,
    /// ever goes back up, so nothing blinks.
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
            chunk.set_to_fluid(pos, block("water"));
        }
        for x in 3..9 {
            for y in 2..4 {
                chunk.set(x, y, 9, BlockState::AIR);
            }
        }
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        let steps = |app: &App| -> Vec<u16> {
            let entity = app
                .world()
                .resource::<ChunkMap>()
                .get(ChunkCoord(IVec3::ZERO))
                .unwrap();
            let chunk = app.world().get::<Chunk>(entity).unwrap();
            // As drawn: none, full under the same fluid, or its height.
            pond.iter()
                .map(|&pos| match chunk.fluid_at(pos) {
                    None => 0,
                    Some(_) if chunk.fluid_at(pos + IVec3::Y).is_some() => FLUID_STEPS,
                    Some(_) => chunk.fluid_step_at(pos),
                })
                .collect()
        };
        // The drawn corners of every pond voxel's top, by corner, each with
        // which of the four voxels around it are water, and which have
        // water above them.
        let corners = |app: &mut App| -> Vec<Option<(f32, u8)>> {
            let pond = pond.clone();
            app.world_mut()
                .run_system_once(move |fluids: FluidView| {
                    let water =
                        |pos: IVec3| fluids.voxel(pos).is_some_and(|(b, _)| b == block("water"));
                    pond.iter()
                        .flat_map(|&pos| {
                            let top = fluids.surface(pos).map(|(_, top)| top);
                            [(0, 0), (0, 1), (1, 0), (1, 1)].map(|(x, z)| {
                                let top = top?;
                                let mut around = 0;
                                for (bit, (dx, dz)) in
                                    [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().enumerate()
                                {
                                    let at = pos + IVec3::new(x + dx - 1, 0, z + dz - 1);
                                    around |= (water(at) as u8) << bit;
                                    around |= (water(at + IVec3::Y) as u8) << (bit + 4);
                                }
                                Some((top.corner(x as usize, z as usize), around))
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        let mut drawn = corners(&mut app);
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
            // Nor does any corner of the drawn surface, while the same
            // voxels around it hold water: averaging heights that don't rise
            // never raises a corner. (One that empties stops counting,
            // which can.) Corners beside the gap are left
            // out: the water pouring out through it sloshes by more than a
            // step, so its own height goes back and forth.
            let now = corners(&mut app);
            for (i, (before, after)) in drawn.iter().zip(&now).enumerate() {
                let (pos, corner) = (pond[i / 4], i % 4);
                if let (Some((before, around)), Some((after, around_now))) = (before, after)
                    && around == around_now
                    && pos.z + (corner % 2) as i32 <= 8
                {
                    assert!(
                        after <= before,
                        "frame {frame}: corner {corner} of {pos} rose from {before} to {after}",
                    );
                }
            }
            drawn = now;
        }
        assert!(last.iter().sum::<u16>() < 72 * FLUID_STEPS, "it drained");
    }
}
