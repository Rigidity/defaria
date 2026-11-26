//! How fluid moves, on any grid of [`Cells`]: one cell's step ([`step`]),
//! and the [`Simulation`] that decides which cells step when.
//!
//! A step moves fluid and never makes or destroys any. A cell first falls:
//! as much as the cell below has room for, up to the fluid's `fall`. If the
//! cell below can still take more, that's all: it's still falling. If not,
//! the cell spreads to its four sides, sharing out toward an even level
//! with each side that's lower:
//!
//! - an open side, if the cell holds at least a `film` of fluid, or if the
//!   side has room below it, so fluid runs off an edge however thin;
//! - a side with the same fluid, if it's at least a `ripple` lower.
//!
//! Sharing only moves fluid from higher cells to lower ones and never past
//! the level, and falling only moves it down, so every step either lowers
//! fluid or evens it out, and a closed scene always settles: it stops
//! changing after a bounded number of steps. The `film` and `ripple` say
//! how flat it settles: a puddle stays put once it's thinner than a film,
//! and a surface counts as level once its cells are less than a ripple
//! apart.
//!
//! Different fluids that touch don't flow into each other: they stop, like
//! fluid against a wall, through [`Cells::meet`].

use std::collections::{BTreeMap, HashSet};

use bevy::prelude::*;

use crate::content::{BlockState, FULL_FLUID, Fluid};

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

/// The four sides fluid spreads to, in the order it tries them.
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

/// Moves the fluid at `pos` one step, as the module docs describe, with
/// `fluid` its flow. Every cell it changes, `pos` included, is pushed onto
/// `changed`; nothing is pushed if nothing moved.
pub fn step(cells: &mut impl Cells, fluid: &Fluid, pos: IVec3, changed: &mut Vec<IVec3>) {
    let Some(Cell::Fluid(kind, mut amount)) = cells.cell(pos) else {
        return;
    };
    // Room for more of this fluid: how much a cell has, if it isn't full.
    let room = |cell: Option<Cell>| match cell {
        Some(Cell::Open) => Some(0),
        Some(Cell::Fluid(k, a)) if k == kind && a < FULL_FLUID => Some(a),
        _ => None,
    };

    let below = pos - IVec3::Y;
    let under = cells.cell(below);
    if let Some(Cell::Fluid(other, _)) = under
        && other != kind
    {
        cells.meet(pos, below);
    }
    if let Some(there) = room(under) {
        let falls = amount.min(FULL_FLUID - there).min(fluid.fall);
        amount -= falls;
        cells.set(below, Cell::Fluid(kind, there + falls));
        cells.set(pos, fill(kind, amount));
        changed.extend([pos, below]);
        if amount == 0 || there + falls < FULL_FLUID {
            return;
        }
    }

    // The sides it could spread to, with what they hold, and whether each
    // has room below it.
    let mut sides: Vec<(IVec3, u16, bool)> = Vec::with_capacity(SIDES.len());
    for side in SIDES {
        let next = pos + side;
        match cells.cell(next) {
            Some(Cell::Open) => {
                let drips = room(cells.cell(next - IVec3::Y)).is_some();
                if amount >= fluid.film || drips {
                    sides.push((next, 0, drips));
                }
            }
            Some(Cell::Fluid(k, there)) if k == kind => {
                if amount >= there.saturating_add(fluid.ripple) {
                    sides.push((next, there, false));
                }
            }
            Some(Cell::Fluid(..)) => cells.meet(pos, next),
            _ => {}
        }
    }
    // Lowest first (stable, so ties keep the order of `SIDES`). Take sides
    // while they're below the level of what's taken so far: those are the
    // ones the level settles above.
    sides.sort_by_key(|&(_, there, _)| there);
    let mut total = amount as u32;
    let mut count = 1;
    for &(_, there, _) in &sides {
        if there as u32 * count >= total {
            break;
        }
        total += there as u32;
        count += 1;
    }
    let taken = &sides[..count as usize - 1];
    let level = (total / count) as u16;
    if level == 0 {
        // Too little to share: it runs off an edge whole, if there's one.
        if let Some(&(next, _, _)) = taken.iter().find(|(_, _, drips)| *drips) {
            cells.set(next, Cell::Fluid(kind, amount));
            cells.set(pos, Cell::Open);
            changed.extend([pos, next]);
        }
        return;
    }
    let mut moved = false;
    for &(next, there, _) in taken {
        if level > there {
            cells.set(next, Cell::Fluid(kind, level));
            changed.push(next);
            moved = true;
        }
    }
    if moved {
        let kept = level + (total % count) as u16;
        cells.set(pos, Cell::Fluid(kind, kept));
        changed.push(pos);
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

/// Which cells step when.
///
/// Only **active** cells step: ones that changed, or whose neighbours did,
/// since the last step. Settled fluid isn't active, so it costs nothing,
/// however much of it there is. Each fluid steps at its own `rate`. In a
/// step, its active cells each step once, in a fixed order that reverses
/// every step, so flow has no favourite direction and runs come out the
/// same every time. A cell that fluid moved into earlier in the same step
/// waits for the next one, so fluid moves at most a cell a step.
///
/// A step is spread evenly over the ticks until the next one, rather than
/// done all in one, so its cost doesn't land on a single frame. And
/// [`Simulation::tick`] steps at most a budget of cells: a step that doesn't
/// fit carries on in the next tick, so a big flood slows down rather than
/// taking longer each frame.
#[derive(Debug, Default)]
pub struct Simulation {
    /// By fluid block, in order so they share the budget the same way every
    /// time.
    queues: BTreeMap<BlockState, Queue>,
    /// Cells of other fluids woken during a tick, queued after it.
    pending_wakes: Vec<(BlockState, IVec3)>,
}

#[derive(Debug, Default)]
struct Queue {
    /// Active cells, for the next step.
    next: HashSet<IVec3>,
    /// The step under way: cells still to step, last first.
    current: Vec<IVec3>,
    /// Cells changed in the step under way, which wait for the next step.
    changed: HashSet<IVec3>,
    /// Seconds toward the next step.
    clock: f32,
    /// Which way the step under way goes through the cells.
    reversed: bool,
    /// How many of the step's cells to step each tick, to finish it in
    /// time for the next one.
    per_tick: usize,
}

/// How many cells a [`Simulation::tick`] stepped, and how many are active.
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

    /// Makes the fluid at `pos` and the six cells around it active.
    pub fn wake_around(&mut self, cells: &impl CellView, pos: IVec3) {
        self.wake(cells, pos);
        for offset in AROUND {
            self.wake(cells, pos + offset);
        }
    }

    /// Cells active now or in the step under way.
    pub fn active(&self) -> usize {
        self.queues
            .values()
            .map(|queue| queue.next.len() + queue.current.len())
            .sum()
    }

    /// Runs `seconds` of flow, stepping at most `budget` cells, with
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
                if queue.current.is_empty() {
                    if queue.clock < interval || queue.next.is_empty() {
                        break;
                    }
                    queue.clock -= interval;
                    queue.start_step();
                    let share = queue.current.len() as f32 * (seconds / interval).min(1.0);
                    queue.per_tick = (share.ceil() as usize).max(1);
                    left = queue.per_tick;
                }
                if left == 0 {
                    break;
                }
                let Some(pos) = queue.current.pop() else {
                    continue;
                };
                if queue.changed.contains(&pos) {
                    queue.next.insert(pos);
                    continue;
                }
                left -= 1;
                stepped += 1;
                step(cells, flow, pos, &mut changed);
                for &cell in &changed {
                    queue.changed.insert(cell);
                }
                // Everything around what moved may move next.
                for cell in changed.drain(..) {
                    for at in std::iter::once(cell).chain(AROUND.map(|offset| cell + offset)) {
                        if let Some(Cell::Fluid(k, _)) = cells.cell(at) {
                            if k == kind {
                                queue.next.insert(at);
                            } else {
                                self.pending_wakes.push((k, at));
                            }
                        }
                    }
                }
            }
        }
        for (kind, pos) in std::mem::take(&mut self.pending_wakes) {
            self.queues.entry(kind).or_default().next.insert(pos);
        }
        self.queues
            .retain(|_, queue| !queue.next.is_empty() || !queue.current.is_empty());
        TickReport {
            stepped,
            active: self.active(),
        }
    }
}

impl Queue {
    /// Starts a step over every active cell, in the opposite order to the
    /// last one.
    fn start_step(&mut self) {
        self.changed.clear();
        self.current.extend(self.next.drain());
        // Popped from the end: bottom up, then by X and Z, or the reverse.
        self.current
            .sort_unstable_by_key(|pos| std::cmp::Reverse((pos.y, pos.x, pos.z)));
        if self.reversed {
            self.current.reverse();
        }
        self.reversed = !self.reversed;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use bevy::platform::collections::HashMap;

    use super::*;

    pub(crate) const WATER: BlockState = BlockState(1);
    const LAVA: BlockState = BlockState(2);

    pub(crate) fn water() -> Fluid {
        Fluid {
            rate: 10.0,
            fall: FULL_FLUID,
            film: 62,
            ripple: 5,
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

        grid.set(IVec3::ZERO, Cell::Fluid(WATER, 510));
        sim.wake_around(&grid, IVec3::ZERO);
        settle(&mut grid, &mut sim, 10);
        assert_eq!(
            (grid.amount(IVec3::ZERO), grid.amount(IVec3::X)),
            (505, 505)
        );
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
        // 12 blocks over the pit's 16 cells: three quarters deep, level to
        // within a ripple or two.
        let pit: Vec<u16> = (8..12)
            .flat_map(|x| (0..4).map(move |z| IVec3::new(x, 0, z)))
            .map(|pos| grid.amount(pos))
            .collect();
        let (low, high) = (pit.iter().min().unwrap(), pit.iter().max().unwrap());
        assert!(high - low <= 2 * water().ripple, "{pit:?}");
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
        settle(&mut grid, &mut sim, 50);
        assert_eq!(grid.amount(IVec3::ZERO) + grid.amount(IVec3::X), FULL_FLUID);

        grid.unloaded.clear();
        sim.wake(&grid, IVec3::X);
        settle(&mut grid, &mut sim, 50);
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
        let first = stepped.iter().position(|&n| n > 0).unwrap();
        assert!((9..=10).contains(&first), "waits for the first step");
        assert_eq!(stepped[first..first + 10], [1; 10], "{stepped:?}");
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
