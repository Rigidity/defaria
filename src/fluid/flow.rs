//! How fluid moves, on any grid of [`Cells`], and the [`Simulation`] that
//! decides which cells move when.
//!
//! Flow goes in steps, and a step never makes or destroys fluid. In a
//! step, each active cell first **falls**: as much as the cell below has
//! room for, up to the fluid's `fall`. Cells fall bottom up, so a falling
//! column moves down a cell a step without breaking up. Then each cell
//! that can't fall any further **spreads** to its four sides, giving each
//! lower side an eighth of the difference between them ([`SHARE`]):
//!
//! - an open side, if the cell holds at least a `film` of fluid;
//! - a side with the same fluid, if it's at least a `ripple` lower;
//! - an open side with room below it, however thin the cell: it runs off an
//!   edge. Half of what it holds goes over its edges in a step, since what
//!   goes over falls away.
//!
//! What spreads is worked out from where the fluid is once it has fallen,
//! and then moved all at once, so the order cells are visited in doesn't
//! matter and flow has no favourite direction. An eighth is small enough
//! that a cell never ends up below a neighbour it gave to, even giving to
//! four at once and with its neighbours giving too: fluid only ever moves
//! downhill, so levels fall or rise smoothly and never swing back and
//! forth. Falling lowers fluid and spreading evens it out, so a closed
//! scene always settles. The `film` and `ripple` say how flat it settles: a
//! puddle stays put once it's thinner than a film, and a surface counts as
//! level once its cells are less than a ripple apart.
//!
//! Different fluids that touch don't flow into each other: they stop, like
//! fluid against a wall, through [`Cells::meet`].

use std::collections::{BTreeMap, HashMap, HashSet};

use bevy::prelude::*;

use crate::content::{BlockState, FULL_FLUID, Fluid};

/// A cell gives each lower side this fraction of the difference between
/// them, at most: one over this. Eight is the most that keeps fluid moving
/// only downhill when a cell gives to four sides at once.
pub const SHARE: u16 = 8;

/// What a cell holds, as flow sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Room for a full block of fluid: air.
    Open,
    /// Fluid, by its block, and how much, from 1 to [`FULL_FLUID`].
    Fluid(BlockState, u16),
    /// No room for fluid: a block.
    Closed,
}

/// A grid of cells, to look at.
pub trait CellView {
    /// The cell at `pos`; `None` where the grid isn't loaded, which flow
    /// treats as closed, without changing what's there.
    fn cell(&self, pos: IVec3) -> Option<Cell>;
}

/// A grid of cells that fluid flows through.
pub trait Cells: CellView {
    /// Sets a cell to [`Cell::Open`] or [`Cell::Fluid`].
    fn set(&mut self, pos: IVec3, cell: Cell);

    /// Fluid at `from` touches a different fluid at `to`. Neither flows
    /// into the other; this is where they'd react (#85). For now nothing
    /// happens.
    fn meet(&mut self, _from: IVec3, _to: IVec3) {}
}

/// The four sides fluid spreads to.
const SIDES: [IVec3; 4] = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z];

/// The six cells next to one.
pub const AROUND: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// The cells whose fluid might move when a cell changes: the cell, the six
/// around it, and the four diagonally above it, whose fluid can run off an
/// edge into it from the side.
pub fn affected_by(pos: IVec3) -> impl Iterator<Item = IVec3> {
    std::iter::once(pos)
        .chain(AROUND.map(|offset| pos + offset))
        .chain(SIDES.map(|side| pos + IVec3::Y + side))
}

/// How much more of `kind` a cell has room for, if any: everything if it's
/// open, what's left if it's `kind` and not full.
fn room(kind: BlockState, cell: Option<Cell>) -> Option<u16> {
    match cell {
        Some(Cell::Open) => Some(FULL_FLUID),
        Some(Cell::Fluid(k, amount)) if k == kind && amount < FULL_FLUID => {
            Some(FULL_FLUID - amount)
        }
        _ => None,
    }
}

/// A cell holding `amount` of `kind`: open if that's none.
fn fill(kind: BlockState, amount: u16) -> Cell {
    if amount == 0 {
        Cell::Open
    } else {
        Cell::Fluid(kind, amount)
    }
}

/// Moves the fluid at `pos` into the cell below, as much as fits, up to
/// its `fall`.
fn fall(cells: &mut impl Cells, fluid: &Fluid, pos: IVec3) {
    let Some(Cell::Fluid(kind, amount)) = cells.cell(pos) else {
        return;
    };
    let below = pos - IVec3::Y;
    let under = cells.cell(below);
    if let Some(Cell::Fluid(other, _)) = under
        && other != kind
    {
        cells.meet(pos, below);
    }
    let Some(space) = room(kind, under) else {
        return;
    };
    let falls = amount.min(space).min(fluid.fall);
    let there = match under {
        Some(Cell::Fluid(_, there)) => there,
        _ => 0,
    };
    cells.set(below, Cell::Fluid(kind, there + falls));
    cells.set(pos, fill(kind, amount - falls));
}

/// Fluid to move from one cell to another, at the end of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transfer {
    from: IVec3,
    to: IVec3,
    amount: u16,
}

/// Works out what the fluid at `pos` gives to its sides, as the module
/// docs describe, without moving any.
fn plan_spread(cells: &mut impl Cells, fluid: &Fluid, pos: IVec3, transfers: &mut Vec<Transfer>) {
    let Some(Cell::Fluid(kind, amount)) = cells.cell(pos) else {
        return;
    };
    // Still falling.
    if room(kind, cells.cell(pos - IVec3::Y)).is_some() {
        return;
    }
    // Sides it runs off an edge into: open or holding the same fluid, with
    // room below them, so whatever goes there falls on.
    let falling = SIDES.map(|side| {
        let next = pos + side;
        room(kind, cells.cell(next)).is_some() && room(kind, cells.cell(next - IVec3::Y)).is_some()
    });
    let edges = falling.iter().filter(|&&falling| falling).count() as u16;
    let mut given = 0;
    for (side, falling) in SIDES.into_iter().zip(falling) {
        let next = pos + side;
        let gives = match cells.cell(next) {
            // Half of it goes over its edges in a step, however thin: what
            // goes over falls away, so it can't come back.
            Some(Cell::Open) if falling => (amount / (2 * edges)).max(1),
            Some(Cell::Fluid(k, there)) if k == kind && falling => {
                amount.saturating_sub(there) / (2 * edges)
            }
            Some(Cell::Open) => {
                if amount >= fluid.film {
                    amount / SHARE
                } else {
                    0
                }
            }
            Some(Cell::Fluid(k, there)) if k == kind => {
                let difference = amount.saturating_sub(there);
                if difference >= fluid.ripple {
                    difference / SHARE
                } else {
                    0
                }
            }
            Some(Cell::Fluid(..)) => {
                cells.meet(pos, next);
                0
            }
            _ => 0,
        };
        let gives = gives.min(amount - given);
        if gives > 0 {
            transfers.push(Transfer {
                from: pos,
                to: next,
                amount: gives,
            });
            given += gives;
        }
    }
}

/// Whether the fluid at `pos` would move if it stepped now: it has room
/// below it, or it would spread. False where there's no fluid. This is
/// flow's own rule, so waking only the fluid this says might move loses
/// nothing.
pub fn might_move(cells: &impl CellView, fluid: &Fluid, pos: IVec3) -> bool {
    let Some(Cell::Fluid(kind, _)) = cells.cell(pos) else {
        return false;
    };
    if room(kind, cells.cell(pos - IVec3::Y)).is_some() {
        return true;
    }
    let mut transfers = Vec::new();
    plan_spread(&mut Peek(cells), fluid, pos, &mut transfers);
    !transfers.is_empty()
}

/// Cells only looked at: flow can't change them, and fluids meeting does
/// nothing.
struct Peek<'a, C>(&'a C);

impl<C: CellView> CellView for Peek<'_, C> {
    fn cell(&self, pos: IVec3) -> Option<Cell> {
        self.0.cell(pos)
    }
}

impl<C: CellView> Cells for Peek<'_, C> {
    fn set(&mut self, _pos: IVec3, _cell: Cell) {
        unreachable!("planning only looks");
    }
}

/// Which cells step when.
///
/// Only **active** cells step: ones that changed, or whose neighbours did,
/// since the last step (see [`affected_by`]). Settled fluid isn't active, so
/// it costs nothing, however much of it there is. Each fluid steps at its
/// own `rate`.
///
/// A step's changes are kept aside until the step is done and then made
/// all at once, so the world (and what's drawn) only ever shows whole
/// steps. That lets a step be spread evenly over the ticks until the next
/// one, rather than done all in one, so its cost doesn't land on a single
/// frame. And [`Simulation::tick`] does at most a budget of work: a step
/// that doesn't fit carries on in the next tick, so a big flood slows down
/// rather than taking longer each frame. If a cell a step changes is
/// changed by something else before the step is done, or unloads, the step
/// is dropped and its cells step again.
#[derive(Debug, Default)]
pub struct Simulation {
    /// By fluid block, in order so they share the budget the same way every
    /// time.
    queues: BTreeMap<BlockState, Queue>,
}

#[derive(Debug, Default)]
struct Queue {
    /// Active cells, for the next step.
    next: HashSet<IVec3>,
    /// The step under way, if one is.
    step: Option<Step>,
    /// Seconds toward the next step.
    clock: f32,
    /// How many cells to visit each tick, to finish the step in time for
    /// the next one.
    per_tick: usize,
}

/// A step under way.
#[derive(Debug, Default)]
struct Step {
    /// Its cells, bottom up.
    cells: Vec<IVec3>,
    /// Visits so far: each cell is visited twice, to fall and then to plan
    /// its spreading.
    visited: usize,
    /// What the step has changed so far, kept aside: each cell as it was
    /// and as it is now.
    changes: HashMap<IVec3, (Option<Cell>, Cell)>,
    /// What spreads, once everything has fallen.
    transfers: Vec<Transfer>,
    /// Fluids that met, to tell the world about.
    meetings: Vec<(IVec3, IVec3)>,
}

impl Step {
    fn visits(&self) -> usize {
        self.cells.len() * 2
    }
}

/// The world as a step under way sees it: its changes so far on top of
/// the world.
struct Layer<'a, C> {
    world: &'a C,
    step: &'a mut Step,
}

impl<C: CellView> CellView for Layer<'_, C> {
    fn cell(&self, pos: IVec3) -> Option<Cell> {
        match self.step.changes.get(&pos) {
            Some(&(_, now)) => Some(now),
            None => self.world.cell(pos),
        }
    }
}

impl<C: CellView> Cells for Layer<'_, C> {
    fn set(&mut self, pos: IVec3, cell: Cell) {
        let was = self.world.cell(pos);
        self.step
            .changes
            .entry(pos)
            .and_modify(|(_, now)| *now = cell)
            .or_insert((was, cell));
    }

    fn meet(&mut self, from: IVec3, to: IVec3) {
        self.step.meetings.push((from, to));
    }
}

/// How many cells a [`Simulation::tick`] visited, and how many are active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    pub stepped: usize,
    pub active: usize,
}

impl Simulation {
    /// Makes the fluid at `pos` active, if there is any.
    pub fn wake(&mut self, cells: &impl CellView, pos: IVec3) {
        if let Some(Cell::Fluid(kind, _)) = cells.cell(pos) {
            self.queues.entry(kind).or_default().next.insert(pos);
        }
    }

    /// Makes the fluid that might move because the cell at `pos` changed
    /// active: see [`affected_by`].
    pub fn wake_around(&mut self, cells: &impl CellView, pos: IVec3) {
        for at in affected_by(pos) {
            self.wake(cells, at);
        }
    }

    /// Cells active now or in a step under way.
    pub fn active(&self) -> usize {
        self.queues
            .values()
            .map(|queue| queue.next.len() + queue.step.as_ref().map_or(0, |s| s.cells.len()))
            .sum()
    }

    /// Runs `seconds` of flow, visiting at most `budget` cells, with
    /// `fluid` giving each fluid block's flow.
    pub fn tick<'a>(
        &mut self,
        cells: &mut impl Cells,
        fluid: impl Fn(BlockState) -> Option<&'a Fluid>,
        seconds: f32,
        budget: usize,
    ) -> TickReport {
        let mut stepped = 0;
        let mut changed = Vec::new();
        let kinds: Vec<BlockState> = self.queues.keys().copied().collect();
        for kind in kinds {
            let Some(flow) = fluid(kind) else {
                // Not a fluid any more.
                self.queues.remove(&kind);
                continue;
            };
            let interval = 1.0 / flow.rate;
            let queue = self.queues.get_mut(&kind).expect("listed");
            queue.clock = (queue.clock + seconds).min(interval * 2.0);
            let mut left = queue.per_tick;
            while stepped < budget {
                if queue.step.is_none() {
                    if queue.clock < interval || queue.next.is_empty() {
                        break;
                    }
                    queue.clock -= interval;
                    let step = queue.start_step();
                    let share = step.visits() as f32 * (seconds / interval).min(1.0);
                    queue.per_tick = (share.ceil() as usize).max(1);
                    left = queue.per_tick;
                }
                if left == 0 {
                    break;
                }
                let step = queue.step.as_mut().expect("started");
                let count = step.cells.len();
                let visit = step.visited;
                let pos = step.cells[visit % count];
                let mut layer = Layer {
                    world: &*cells,
                    step,
                };
                if visit < count {
                    fall(&mut layer, flow, pos);
                } else {
                    let mut transfers = std::mem::take(&mut layer.step.transfers);
                    plan_spread(&mut layer, flow, pos, &mut transfers);
                    layer.step.transfers = transfers;
                }
                step.visited += 1;
                left -= 1;
                stepped += 1;
                if step.visited == step.visits() {
                    let step = queue.step.take().expect("started");
                    finish_step(cells, step, &mut queue.next, &mut changed);
                }
            }
        }
        // Everything that might move because of what moved.
        for cell in changed.drain(..) {
            self.wake_around(cells, cell);
        }
        self.queues
            .retain(|_, queue| !queue.next.is_empty() || queue.step.is_some());
        TickReport {
            stepped,
            active: self.active(),
        }
    }
}

impl Queue {
    /// Starts a step over every active cell.
    fn start_step(&mut self) -> &Step {
        let mut cells: Vec<IVec3> = self.next.drain().collect();
        cells.sort_unstable_by_key(|pos| (pos.y, pos.x, pos.z));
        self.step.insert(Step { cells, ..default() })
    }
}

/// Spreads what `step` planned, then makes all its changes in the world,
/// pushing each cell it touched onto `changed`. If the world has changed under
/// the step since it began, it's dropped instead, and its cells go back in
/// `next` to step again.
fn finish_step(
    cells: &mut impl Cells,
    mut step: Step,
    next: &mut HashSet<IVec3>,
    changed: &mut Vec<IVec3>,
) {
    let transfers = std::mem::take(&mut step.transfers);
    let meetings = std::mem::take(&mut step.meetings);
    let mut layer = Layer {
        world: &*cells,
        step: &mut step,
    };
    for Transfer { from, to, amount } in transfers {
        let (Some(Cell::Fluid(kind, has)), there) = (layer.cell(from), layer.cell(to)) else {
            continue;
        };
        let space = room(kind, there).unwrap_or(0);
        let amount = amount.min(has).min(space);
        if amount == 0 {
            continue;
        }
        let there = match there {
            Some(Cell::Fluid(_, there)) => there,
            _ => 0,
        };
        layer.set(to, Cell::Fluid(kind, there + amount));
        layer.set(from, fill(kind, has - amount));
    }
    let stale = step
        .changes
        .iter()
        .any(|(&pos, &(was, _))| was.is_none() || cells.cell(pos) != was);
    if stale {
        next.extend(step.cells);
        return;
    }
    for (pos, (was, now)) in step.changes {
        if was != Some(now) {
            cells.set(pos, now);
        }
        // Even fluid that ended as it began moved: it fell and was refilled,
        // say, and will again.
        changed.push(pos);
    }
    for (from, to) in meetings {
        cells.meet(from, to);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use bevy::platform::collections::HashMap;

    use super::*;
    use crate::voxel::fluid_step;

    pub(crate) const WATER: BlockState = BlockState(1);
    const LAVA: BlockState = BlockState(2);

    pub(crate) fn water() -> Fluid {
        Fluid {
            rate: 10.0,
            fall: FULL_FLUID,
            film: 62,
            ripple: 10,
            move_speed: 0.5,
            view_color: Color::BLACK,
            view_distance: 10.0,
        }
    }

    fn lava() -> Fluid {
        Fluid {
            rate: 2.0,
            fall: 500,
            film: 250,
            ripple: 50,
            ..water()
        }
    }

    /// Cells in a map: anything not in it is closed, and anything in
    /// `unloaded` isn't loaded.
    #[derive(Default, Clone)]
    pub(crate) struct Grid {
        pub cells: HashMap<IVec3, Cell>,
        pub unloaded: HashSet<IVec3>,
        pub meetings: usize,
    }

    impl CellView for Grid {
        fn cell(&self, pos: IVec3) -> Option<Cell> {
            if self.unloaded.contains(&pos) {
                return None;
            }
            Some(self.cells.get(&pos).copied().unwrap_or(Cell::Closed))
        }
    }

    impl Cells for Grid {
        fn set(&mut self, pos: IVec3, cell: Cell) {
            assert!(
                !self.unloaded.contains(&pos),
                "set {pos}, which isn't loaded"
            );
            assert_ne!(
                self.cell(pos),
                Some(Cell::Closed),
                "set {pos}, which is closed"
            );
            if let Cell::Fluid(_, amount) = cell {
                assert!((1..=FULL_FLUID).contains(&amount), "{amount} at {pos}");
            }
            self.cells.insert(pos, cell);
        }

        fn meet(&mut self, _from: IVec3, _to: IVec3) {
            self.meetings += 1;
        }
    }

    impl Grid {
        /// Opens every cell in the box from `min` to `max`, both included.
        pub fn open(&mut self, min: IVec3, max: IVec3) {
            for x in min.x..=max.x {
                for y in min.y..=max.y {
                    for z in min.z..=max.z {
                        self.cells.insert(IVec3::new(x, y, z), Cell::Open);
                    }
                }
            }
        }

        fn total(&self, kind: BlockState) -> u64 {
            self.cells
                .values()
                .map(|cell| match cell {
                    Cell::Fluid(k, amount) if *k == kind => *amount as u64,
                    _ => 0,
                })
                .sum()
        }

        fn amount(&self, pos: IVec3) -> u16 {
            match self.cells.get(&pos) {
                Some(Cell::Fluid(_, amount)) => *amount,
                _ => 0,
            }
        }
    }

    fn fluids(state: BlockState) -> Option<&'static Fluid> {
        static WATER_FLOW: std::sync::LazyLock<Fluid> = std::sync::LazyLock::new(water);
        static LAVA_FLOW: std::sync::LazyLock<Fluid> = std::sync::LazyLock::new(lava);
        match state {
            WATER => Some(&WATER_FLOW),
            LAVA => Some(&LAVA_FLOW),
            _ => None,
        }
    }

    /// Pours `amount` at `pos` and wakes it.
    fn pour(grid: &mut Grid, sim: &mut Simulation, pos: IVec3, kind: BlockState, amount: u16) {
        grid.set(pos, Cell::Fluid(kind, amount));
        sim.wake_around(grid, pos);
    }

    /// Ticks until nothing's active, checking that each fluid's total never
    /// changes. Returns how many ticks it took. Fails if it doesn't settle
    /// within `limit`.
    fn settle(grid: &mut Grid, sim: &mut Simulation, limit: usize) -> usize {
        let totals = [WATER, LAVA].map(|kind| grid.total(kind));
        for ticks in 0..limit {
            if sim.active() == 0 {
                return ticks;
            }
            sim.tick(grid, fluids, 0.1, usize::MAX);
            assert_eq!([WATER, LAVA].map(|kind| grid.total(kind)), totals);
        }
        panic!("still {} active after {limit} ticks", sim.active());
    }

    /// Fluid falls to the floor, a cell a step, and keeps all of itself.
    #[test]
    fn fluid_falls_to_the_floor() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(0, 5, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::new(0, 5, 0), WATER, 700);
        sim.tick(&mut grid, fluids, 0.1, usize::MAX);
        assert_eq!(grid.amount(IVec3::new(0, 4, 0)), 700, "one cell in a step");
        assert_eq!(grid.cell(IVec3::new(0, 5, 0)), Some(Cell::Open));
        settle(&mut grid, &mut sim, 100);
        assert_eq!(grid.amount(IVec3::ZERO), 700);
        assert_eq!(grid.total(WATER), 700);
    }

    /// A cell falls at most `fall` a step.
    #[test]
    fn falling_is_limited_per_step() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(0, 1, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::Y, LAVA, FULL_FLUID);
        sim.tick(&mut grid, fluids, 0.5, usize::MAX);
        assert_eq!(grid.amount(IVec3::ZERO), 500);
        assert_eq!(grid.amount(IVec3::Y), 500);
    }

    /// A full block on a wide floor spreads out until it's thinner than a
    /// film, and then stops, with every drop still there.
    #[test]
    fn a_puddle_spreads_thin_and_stops() {
        let mut grid = Grid::default();
        grid.open(IVec3::new(-20, 0, -20), IVec3::new(20, 0, 20));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, WATER, FULL_FLUID);
        settle(&mut grid, &mut sim, 500);
        let wet: Vec<u16> = grid
            .cells
            .values()
            .filter_map(|cell| match cell {
                Cell::Fluid(_, amount) => Some(*amount),
                _ => None,
            })
            .collect();
        assert!(wet.len() >= 8, "spread over {} cells", wet.len());
        assert!(wet.iter().all(|&a| a < 2 * water().film), "{wet:?}");
        assert_eq!(grid.total(WATER), FULL_FLUID as u64);
    }

    /// A film thinner than `film` stays put on dry ground, but still runs
    /// off an edge.
    #[test]
    fn thin_films_stay_unless_theres_an_edge() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(2, 0, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::new(1, 0, 0), WATER, 40);
        settle(&mut grid, &mut sim, 10);
        assert_eq!(grid.amount(IVec3::new(1, 0, 0)), 40, "too thin to spread");

        // Open a drop beside it.
        grid.open(IVec3::new(3, -1, 0), IVec3::new(3, 0, 0));
        grid.cells.insert(IVec3::new(2, -1, 0), Cell::Open);
        sim.wake_around(&grid, IVec3::new(1, 0, 0));
        settle(&mut grid, &mut sim, 50);
        assert_eq!(grid.amount(IVec3::new(1, 0, 0)), 0, "it ran off the edge");
        assert_eq!(grid.total(WATER), 40);
    }

    /// A film too thin to spread at a ledge runs off it once room opens
    /// diagonally below it: digging out under the edge wakes it.
    #[test]
    fn films_run_off_a_ledge_dug_out_later() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::X);
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, LAVA, 200);
        settle(&mut grid, &mut sim, 50);
        assert_eq!(grid.amount(IVec3::ZERO), 200, "too thin to spread");

        let hole = IVec3::new(1, -1, 0);
        grid.cells.insert(hole, Cell::Open);
        sim.wake_around(&grid, hole);
        settle(&mut grid, &mut sim, 500);
        assert_eq!(grid.amount(hole), 200, "it all ran into the hole");
    }

    /// Cells less than a ripple apart are level, so they don't flow.
    #[test]
    fn ripples_dont_flow() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(1, 0, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, WATER, 504);
        pour(&mut grid, &mut sim, IVec3::X, WATER, 500);
        settle(&mut grid, &mut sim, 10);
        assert_eq!(grid.amount(IVec3::ZERO), 504);

        // Ten apart, they even out until they're less than a ripple apart.
        grid.set(IVec3::ZERO, Cell::Fluid(WATER, 510));
        sim.wake_around(&grid, IVec3::ZERO);
        settle(&mut grid, &mut sim, 10);
        let (a, b) = (grid.amount(IVec3::ZERO), grid.amount(IVec3::X));
        assert_eq!(a + b, 1010);
        assert!(a > b && a - b < water().ripple, "{a} {b}");
    }

    /// Water poured on a slope runs down into the pit at its foot and
    /// fills it to a level surface, losing nothing on the way.
    #[test]
    fn water_runs_down_a_slope_and_fills_a_pit_level() {
        let mut grid = Grid::default();
        // A staircase down in +X, a step every two cells, into a 4×3×4 pit.
        for x in 0..8 {
            let floor = 8 - x / 2;
            grid.open(IVec3::new(x, floor, 0), IVec3::new(x, 12, 3));
        }
        grid.open(IVec3::new(8, 0, 0), IVec3::new(11, 12, 3));
        let mut sim = Simulation::default();
        for z in 0..4 {
            pour(&mut grid, &mut sim, IVec3::new(0, 9, z), WATER, FULL_FLUID);
            pour(&mut grid, &mut sim, IVec3::new(1, 9, z), WATER, FULL_FLUID);
            pour(&mut grid, &mut sim, IVec3::new(0, 8, z), WATER, FULL_FLUID);
        }
        settle(&mut grid, &mut sim, 2000);
        assert_eq!(grid.total(WATER), 12 * FULL_FLUID as u64);
        // 12 blocks over the pit's 16 cells: three quarters deep, and level:
        // less than a ripple between neighbours, so less than three across
        // the pit's four.
        let pit: Vec<u16> = (8..12)
            .flat_map(|x| (0..4).map(move |z| IVec3::new(x, 0, z)))
            .map(|pos| grid.amount(pos))
            .collect();
        let (low, high) = (pit.iter().min().unwrap(), pit.iter().max().unwrap());
        assert!(high - low < 3 * water().ripple, "{pit:?}");
        assert!(*low > 600, "{pit:?}");
        // What's left on the stairs is thin.
        for x in 0..8 {
            for y in 0..13 {
                for z in 0..4 {
                    assert!(grid.amount(IVec3::new(x, y, z)) < 2 * water().film);
                }
            }
        }
    }

    /// Fluid moves at most a cell a step, either way, so it spreads evenly.
    #[test]
    fn fluid_moves_a_cell_a_step() {
        let mut grid = Grid::default();
        grid.open(IVec3::new(-5, 0, 0), IVec3::new(5, 0, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, WATER, FULL_FLUID);
        sim.tick(&mut grid, fluids, 0.1, usize::MAX);
        let wet = |grid: &Grid| {
            (-5..=5)
                .filter(|&x| grid.amount(IVec3::new(x, 0, 0)) > 0)
                .collect::<Vec<_>>()
        };
        assert_eq!(wet(&grid), [-1, 0, 1]);
        sim.tick(&mut grid, fluids, 0.1, usize::MAX);
        assert_eq!(wet(&grid), [-2, -1, 0, 1, 2]);
    }

    /// Water and lava that touch stay apart, each keeping all of itself.
    #[test]
    fn different_fluids_stop_where_they_meet() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(9, 1, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::new(0, 1, 0), WATER, FULL_FLUID);
        pour(&mut grid, &mut sim, IVec3::new(9, 1, 0), LAVA, FULL_FLUID);
        pour(&mut grid, &mut sim, IVec3::new(4, 0, 0), LAVA, FULL_FLUID);
        settle(&mut grid, &mut sim, 500);
        assert!(grid.meetings > 0);
        for x in 0..10 {
            for y in 0..2 {
                let pos = IVec3::new(x, y, 0);
                assert!(matches!(grid.cell(pos), Some(Cell::Open | Cell::Fluid(..))));
            }
        }
        assert_eq!(grid.total(WATER), FULL_FLUID as u64);
        assert_eq!(grid.total(LAVA), 2 * FULL_FLUID as u64);
        // Settled, both stay settled.
        assert_eq!(sim.active(), 0);
    }

    /// Fluid waits at the edge of what's loaded, and flows on once the
    /// rest is woken.
    #[test]
    fn fluid_waits_at_unloaded_cells() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(3, 0, 0));
        grid.unloaded
            .extend([IVec3::new(2, 0, 0), IVec3::new(3, 0, 0)]);
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, WATER, FULL_FLUID);
        settle(&mut grid, &mut sim, 500);
        assert_eq!(grid.amount(IVec3::ZERO) + grid.amount(IVec3::X), FULL_FLUID);

        grid.unloaded.clear();
        sim.wake(&grid, IVec3::X);
        settle(&mut grid, &mut sim, 500);
        assert!(grid.amount(IVec3::new(3, 0, 0)) > 0);
        assert_eq!(grid.total(WATER), FULL_FLUID as u64);
    }

    /// A step is shared out over the ticks before the next one, not done
    /// in the first.
    #[test]
    fn steps_are_spread_over_the_ticks_between_them() {
        let mut grid = Grid::default();
        grid.open(IVec3::new(0, 0, 0), IVec3::new(9, 1, 0));
        let mut sim = Simulation::default();
        for x in 0..10 {
            pour(&mut grid, &mut sim, IVec3::new(x, 1, 0), WATER, FULL_FLUID);
        }
        // Water steps ten times a second; ticks of a hundredth of a second
        // do a tenth of a step each.
        let stepped: Vec<usize> = (0..21)
            .map(|_| sim.tick(&mut grid, fluids, 0.01, usize::MAX).stepped)
            .collect();
        // Each cell is visited twice in a step: to fall, then to spread.
        let first = stepped.iter().position(|&n| n > 0).unwrap();
        assert!((9..=10).contains(&first), "waits for the first step");
        assert_eq!(stepped[first..first + 10], [2; 10], "{stepped:?}");
    }

    /// A pond draining through a hole in its floor goes down steadily: no
    /// voxel ever gains more than a ripple, and its drawn height almost
    /// never rises, so the surface doesn't flicker as it empties. (A voxel
    /// that gets too thin to spread onto dry ground can gain a little from
    /// its neighbours before they level off; once in a while that crosses
    /// into the next sixteenth.)
    #[test]
    fn a_draining_pond_goes_down_steadily() {
        let mut grid = Grid::default();
        grid.open(IVec3::new(0, 0, 0), IVec3::new(5, 0, 5));
        // A drain in one corner, into a deep shaft.
        grid.open(IVec3::new(0, -40, 0), IVec3::new(0, -1, 0));
        let mut sim = Simulation::default();
        let pond: Vec<IVec3> = (1..6)
            .flat_map(|x| (0..6).map(move |z| IVec3::new(x, 0, z)))
            .collect();
        for &pos in &pond {
            pour(&mut grid, &mut sim, pos, WATER, 600);
        }
        let mut last: Vec<u16> = pond.iter().map(|&p| grid.amount(p)).collect();
        let mut drawn_rises = 0;
        for tick in 0..400 {
            sim.tick(&mut grid, fluids, 0.02, usize::MAX);
            let now: Vec<u16> = pond.iter().map(|&p| grid.amount(p)).collect();
            for (i, (&before, &after)) in last.iter().zip(&now).enumerate() {
                assert!(
                    after < before + water().ripple,
                    "tick {tick}: {} rose from {before} to {after}",
                    pond[i]
                );
                drawn_rises += (fluid_step(after) > fluid_step(before)) as usize;
            }
            last = now;
        }
        assert!(drawn_rises <= 2, "drawn heights rose {drawn_rises} times");
        assert!(last.iter().all(|&a| a < 600), "{last:?}");
    }

    /// With a budget, a step spreads over several ticks, but fluid still
    /// settles and keeps all of itself.
    #[test]
    fn a_budget_spreads_steps_over_ticks() {
        let mut grid = Grid::default();
        grid.open(IVec3::new(-6, 0, -6), IVec3::new(6, 3, 6));
        let mut sim = Simulation::default();
        for x in -2..=2 {
            pour(&mut grid, &mut sim, IVec3::new(x, 3, 0), WATER, FULL_FLUID);
        }
        let total = grid.total(WATER);
        let mut ticks = 0;
        while sim.active() > 0 {
            let report = sim.tick(&mut grid, fluids, 0.1, 3);
            assert!(report.stepped <= 3);
            assert_eq!(grid.total(WATER), total);
            ticks += 1;
            assert!(ticks < 20_000, "never settled");
        }
    }
}
