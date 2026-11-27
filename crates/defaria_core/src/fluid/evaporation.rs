//! Evaporation: a film of fluid (thinner than its `film`, so it no longer
//! spreads) under open sky slowly disappears, at its fluid's
//! `evaporation` rate (see `docs/modding.md`). Water evaporates; lava
//! doesn't. What evaporates is a counted loss ([`FluidStats::evaporated`]),
//! not a water cycle: it doesn't come back as rain.
//!
//! Films are found as fluid changes and as chunks load, and kept in a
//! queue ([`Films`], counted in [`FluidStats::films`]). Each tick visits
//! a share of it, so every film is visited about once every
//! [`VISIT_SECONDS`], and loses what it would have in that time: never
//! every film every tick, however many there are. A film that stops being one (refilled, gone, unloaded) leaves the
//! queue when it's next visited; one under a roof stays in it, in case
//! the roof goes.
//!
//! Open sky means the voxel above the film has full sky light: sky light
//! only stays at full strength straight down from the sky, through
//! nothing that dims it.

use std::collections::VecDeque;

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use super::{FluidStats, FluidView, WorldCells, flow::Cell, flow::CellView};
use crate::{
    content::{BlockState, Content, FULL_FLUID, Fluid, MAX_LIGHT},
    light::{ChunkLight, SKY},
    world::{Chunk, ChunkCoord, ChunkMap, FluidChanged, VoxelChanged, VoxelFluid},
};

/// About how often each film is visited, in seconds.
pub const VISIT_SECONDS: f32 = 2.0;

/// How many times faster than their fluid's `evaporation` rate films dry
/// up: 1 in the game. Headless scripts raise it (`evaporation <speed>`) to
/// watch puddles dry in seconds rather than minutes.
#[derive(Resource, Reflect, Debug, Clone, Copy, PartialEq)]
#[reflect(Resource)]
pub struct EvaporationSpeed(pub f32);

impl Default for EvaporationSpeed {
    fn default() -> Self {
        Self(1.0)
    }
}

/// The films that might evaporate, in the order they're visited.
#[derive(Resource, Debug, Default)]
pub struct Films {
    queue: VecDeque<IVec3>,
    listed: HashSet<IVec3>,
    /// Visits owed to the queue, carried over between ticks.
    due: f32,
    /// What each fluid's visits lost short of a whole unit, carried over to
    /// the next visit, so a rate that isn't a whole number of units a
    /// visit still comes out right on average.
    owed: HashMap<BlockState, f32>,
}

impl Films {
    /// Puts `pos` at the back of the queue, unless it's already in it.
    pub fn note(&mut self, pos: IVec3) {
        if self.listed.insert(pos) {
            self.queue.push_back(pos);
        }
    }

    /// How many films are queued.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether no films are queued.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Takes the films due a visit after `seconds` more: the share that
    /// visits each about once every [`VISIT_SECONDS`].
    fn take_due(&mut self, seconds: f32) -> Vec<IVec3> {
        let count = self.queue.len();
        self.due = (self.due + count as f32 * seconds / VISIT_SECONDS).min(count as f32);
        let visits = self.due as usize;
        self.due -= visits as f32;
        let due: Vec<IVec3> = self.queue.drain(..visits).collect();
        for pos in &due {
            self.listed.remove(pos);
        }
        due
    }

    /// How many whole units a visit to a film of `kind` takes, `speed`
    /// times its fluid's rate, carrying what's short of a unit over to the
    /// next visit to one.
    fn lose(&mut self, kind: BlockState, fluid: &Fluid, speed: f32) -> u16 {
        let owed = self.owed.entry(kind).or_default();
        *owed += per_visit(fluid) * speed;
        let whole = owed.floor();
        *owed -= whole;
        whole as u16
    }
}

/// The fluid at `pos`, if it's a film of one that evaporates.
fn film<'a>(
    cells: &impl CellView,
    content: &'a Content,
    pos: IVec3,
) -> Option<(BlockState, u16, &'a Fluid)> {
    let Some(Cell::Fluid(kind, amount, _)) = cells.cell(pos) else {
        return None;
    };
    let fluid = evaporating(content, kind)?;
    (amount < fluid.film).then_some((kind, amount, fluid))
}

/// The fluid `kind` is, if it evaporates.
fn evaporating(content: &Content, kind: BlockState) -> Option<&Fluid> {
    content
        .state(kind)
        .fluid
        .as_ref()
        .filter(|fluid| fluid.evaporation > 0.0)
}

/// How much of `fluid` evaporates from a film in a visit, in units.
fn per_visit(fluid: &Fluid) -> f32 {
    fluid.evaporation * FULL_FLUID as f32 * VISIT_SECONDS
}

/// Queues the films where fluid changed, by flow or anything else, and in
/// chunks that have just loaded.
pub(super) fn note_films(
    mut changes: MessageReader<VoxelChanged>,
    mut fluid_changes: MessageReader<FluidChanged>,
    added: Query<(&ChunkCoord, &Chunk), Added<Chunk>>,
    mut films: ResMut<Films>,
    cells: FluidView,
) {
    let content = &cells.content;
    let changed = changes.read().map(|change| change.pos);
    for pos in changed.chain(fluid_changes.read().map(|change| change.pos)) {
        if film(&cells, content, pos).is_some() {
            films.note(pos);
        }
    }
    // Read from the chunk's fluid, without looking each voxel up.
    for (coord, chunk) in &added {
        let origin = coord.origin();
        for (index, fluid) in chunk.fluids() {
            if evaporating(content, fluid.kind).is_some_and(|f| fluid.amount < f.film) {
                films.note(origin + Chunk::position_of(index));
            }
        }
    }
}

/// Visits the share of the films that's due: one under open sky loses
/// what evaporated since its last visit, and is gone once it's all gone.
pub(super) fn evaporate(
    time: Res<Time>,
    speed: Res<EvaporationSpeed>,
    mut films: ResMut<Films>,
    mut stats: ResMut<FluidStats>,
    mut cells: WorldCells,
    map: Res<ChunkMap>,
    lights: Query<&ChunkLight>,
) {
    let open_sky = |pos: IVec3| {
        let (coord, local) = ChunkCoord::split(pos + IVec3::Y);
        map.get(coord)
            .and_then(|entity| lights.get(entity).ok())
            .is_some_and(|light| light.at(local).get(SKY) == MAX_LIGHT)
    };
    for pos in films.take_due(time.delta_secs()) {
        let Some((kind, amount, fluid)) = film(&cells, &cells.content, pos) else {
            continue;
        };
        if open_sky(pos) {
            let lost = films.lose(kind, fluid, speed.0).min(amount);
            stats.evaporated += lost as u64;
            if lost == amount {
                // Gone, leaving the voxel's block as it was (a slab's room
                // dries too): that wakes the fluid around, which may flow
                // in.
                cells.voxels.set_fluid(pos, None);
                continue;
            }
            // Thinner, it wakes nothing: the films around it would only
            // trickle into it a unit at a time while they all dry up, and
            // anything deep enough to spread into it already has.
            let thinner = VoxelFluid {
                kind,
                amount: amount - lost,
            };
            cells.voxels.flow_fluid(pos, Some(thinner));
        }
        films.note(pos);
    }
    stats.films = films.len();
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{
        content::{base, block},
        fluid::{FluidSettings, FluidSim, run_flow, wake_around_edits, wake_loaded_chunks},
        light::Light,
        testing::{chunk_app, insert_chunk},
    };

    /// Flow and evaporation alone, on chunks spawned by the test.
    fn app() -> App {
        let mut app = chunk_app(base().clone(), Duration::from_millis(50));
        app.init_resource::<FluidSettings>()
            .init_resource::<FluidStats>()
            .init_resource::<FluidSim>()
            .init_resource::<Films>()
            .init_resource::<EvaporationSpeed>()
            .add_systems(
                Update,
                (wake_around_edits, wake_loaded_chunks, note_films).chain(),
            )
            .add_systems(FixedUpdate, (run_flow, evaporate).chain());
        app
    }

    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds / 0.05) as usize {
            app.update();
        }
    }

    /// A stone floor at y 0 with air above it, lit by the full sky except
    /// in the columns `shaded` names, under a roof.
    fn floor(films: &[(IVec3, BlockState, u16)], shaded: &[IVec3]) -> (Chunk, ChunkLight) {
        let mut chunk = Chunk::filled(BlockState::AIR);
        let mut light = ChunkLight::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set(x, 0, z, block("stone"));
                for y in 1..16 {
                    light.set_at(
                        IVec3::new(x as i32, y, z as i32),
                        Light::new(MAX_LIGHT, [0; 3]),
                    );
                }
            }
        }
        for &column in shaded {
            chunk.set(column.x as usize, 3, column.z as usize, block("stone"));
            for y in 1..3 {
                light.set_at(column.with_y(y), Light::new(MAX_LIGHT - 1, [0; 3]));
            }
        }
        for &(pos, kind, amount) in films {
            chunk.start_fluid_at(pos, Some(VoxelFluid { kind, amount }), FULL_FLUID);
        }
        (chunk, light)
    }

    fn amount(app: &App, pos: IVec3) -> u16 {
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(IVec3::ZERO))
            .unwrap();
        let chunk = app.world().get::<Chunk>(entity).unwrap();
        chunk.fluid_amount_at(pos)
    }

    /// Under open sky, a film of water evaporates a little each visit
    /// until it's gone, and all of it is counted; one under a roof, and a
    /// film of lava, stay as they are.
    #[test]
    fn open_water_films_evaporate() {
        let mut app = app();
        let water = block("water");
        let open = IVec3::new(3, 1, 3);
        let roofed = IVec3::new(9, 1, 9);
        let lava = IVec3::new(12, 1, 3);
        let (chunk, light) = floor(
            &[
                (open, water, 40),
                (roofed, water, 40),
                (lava, block("lava"), 100),
            ],
            &[roofed],
        );
        insert_chunk(&mut app, IVec3::ZERO, (chunk, light));
        let fluid = base().state(water).fluid.clone().unwrap();
        assert!(40 < fluid.film, "a film");
        run(&mut app, 1.0);
        assert_eq!(
            app.world().resource::<Films>().len(),
            2,
            "the two water films"
        );
        let stepped = app.world().resource::<FluidStats>().stepped;
        run(&mut app, VISIT_SECONDS * 2.0);
        let partly = amount(&app, open);
        assert!(0 < partly && partly < 40, "{partly}");
        // Thinning a film wakes nothing for flow to step.
        assert_eq!(app.world().resource::<FluidStats>().stepped, stepped);
        assert_eq!(
            app.world().resource::<FluidStats>().evaporated,
            (40 - partly) as u64
        );
        // Long enough for the rest to go.
        let seconds = 40.0 / per_visit(&fluid) * VISIT_SECONDS + 2.0 * VISIT_SECONDS;
        run(&mut app, seconds);
        assert_eq!(amount(&app, open), 0);
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(IVec3::ZERO))
            .unwrap();
        assert_eq!(
            app.world().get::<Chunk>(entity).unwrap().fluid_at(open),
            None
        );
        assert_eq!(amount(&app, roofed), 40, "under a roof");
        assert_eq!(amount(&app, lava), 100, "lava doesn't evaporate");
        assert_eq!(app.world().resource::<FluidStats>().evaporated, 40);
        assert_eq!(app.world().resource::<Films>().len(), 1, "the roofed film");
    }

    /// A block of water poured on open ground spreads into films and dries
    /// up entirely, the voxel it was poured in too: that one becomes a film
    /// by draining, which only changes its amount. Sped up, as a script's
    /// `evaporation` does, so it takes seconds rather than minutes.
    #[test]
    fn a_poured_puddle_dries_up() {
        let mut app = app();
        app.insert_resource(EvaporationSpeed(20.0));
        let (chunk, light) = floor(&[], &[]);
        insert_chunk(&mut app, IVec3::ZERO, (chunk, light));
        run(&mut app, 0.5);
        let pos = IVec3::new(8, 1, 8);
        app.world_mut()
            .run_system_once(move |mut voxels: crate::world::Voxels| {
                voxels.pour(pos, block("water"), FULL_FLUID);
            })
            .unwrap();
        run(&mut app, 24.0);
        let total: u64 = (0..16)
            .flat_map(|x| (0..16).map(move |z| IVec3::new(x, 1, z)))
            .map(|pos| amount(&app, pos) as u64)
            .sum();
        assert_eq!(total, 0, "dried up");
        assert_eq!(
            app.world().resource::<FluidStats>().evaporated,
            FULL_FLUID as u64
        );
    }

    /// A rate that isn't a whole number of units a visit still comes out
    /// right on average: a fifth of a unit a visit loses one unit every
    /// fifth visit.
    #[test]
    fn slow_rates_carry_over_between_visits() {
        let mut films = Films::default();
        let fluid = Fluid {
            evaporation: 0.2 / (FULL_FLUID as f32 * VISIT_SECONDS),
            ..base().state(block("water")).fluid.clone().unwrap()
        };
        let lost: Vec<u16> = (0..10)
            .map(|_| films.lose(block("water"), &fluid, 1.0))
            .collect();
        assert_eq!(lost.iter().sum::<u16>(), 2, "{lost:?}");
    }

    /// A faster speed loses that many times as much a visit.
    #[test]
    fn speed_multiplies_what_a_visit_loses() {
        let fluid = Fluid {
            evaporation: 2.0 / (FULL_FLUID as f32 * VISIT_SECONDS),
            ..base().state(block("water")).fluid.clone().unwrap()
        };
        let mut films = Films::default();
        assert_eq!(films.lose(block("water"), &fluid, 1.0), 2);
        assert_eq!(films.lose(block("water"), &fluid, 15.0), 30);
    }

    /// A film in a slab's room dries like any other, and the slab stays.
    #[test]
    fn a_film_in_a_slab_dries_and_leaves_the_slab() {
        let mut app = app();
        let pos = IVec3::new(5, 1, 5);
        let (mut chunk, light) = floor(&[], &[]);
        let slab = block("stone_slab");
        let room = base().state(slab).fluid_room;
        chunk.set_at(pos, slab);
        let film = VoxelFluid {
            kind: block("water"),
            amount: 20,
        };
        chunk.start_fluid_at(pos, Some(film), room);
        insert_chunk(&mut app, IVec3::ZERO, (chunk, light));
        run(&mut app, VISIT_SECONDS * 48.0);
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(IVec3::ZERO))
            .unwrap();
        let chunk = app.world().get::<Chunk>(entity).unwrap();
        assert_eq!(chunk.fluid_at(pos), None, "dried");
        assert_eq!(chunk.at(pos), slab, "the slab stays");
        assert_eq!(app.world().resource::<FluidStats>().evaporated, 20);
    }

    /// Water deeper than a film doesn't evaporate, even under open sky.
    #[test]
    fn only_films_evaporate() {
        let mut app = app();
        let pos = IVec3::new(3, 1, 3);
        let mut chunk = floor(&[], &[]);
        // A full voxel in a one-voxel hole, so it can't spread.
        chunk.0.set_to_fluid(IVec3::new(3, 1, 3), block("water"));
        for (x, z) in [(2, 3), (4, 3), (3, 2), (3, 4)] {
            chunk.0.set(x, 1, z, block("stone"));
        }
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        run(&mut app, VISIT_SECONDS * 5.0);
        assert_eq!(amount(&app, pos), FULL_FLUID);
        assert_eq!(app.world().resource::<FluidStats>().evaporated, 0);
        assert_eq!(app.world().resource::<Films>().len(), 0);
    }

    /// Each tick visits only its share of the films: a thousand films are
    /// visited over [`VISIT_SECONDS`], a few each tick, each once.
    #[test]
    fn films_are_visited_a_share_at_a_time() {
        let mut films = Films::default();
        for x in 0..1000 {
            films.note(IVec3::new(x, 0, 0));
        }
        films.note(IVec3::ZERO);
        assert_eq!(films.len(), 1000, "no film twice");
        let tick = 1.0 / 64.0;
        let mut seen = HashSet::new();
        let mut most = 0;
        for _ in 0..(VISIT_SECONDS / tick) as usize {
            let due = films.take_due(tick);
            most = most.max(due.len());
            for pos in due {
                assert!(seen.insert(pos), "{pos} twice");
                // Still a film: back of the queue.
                films.note(pos);
            }
        }
        assert!((990..=1000).contains(&seen.len()), "{}", seen.len());
        assert!(most <= 8, "{most} in one tick");
    }
}
