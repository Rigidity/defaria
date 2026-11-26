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
//! - a side with the same fluid, if it's at least a `ripple` lower (where an
//!   eighth of the difference rounds down to nothing, a single unit, if
//!   that can't overshoot);
//! - a side with room below it, however thin the cell: it runs off an
//!   edge. Half of what it holds goes over its edges in a step, straight
//!   into the cell below the edge, since what goes over falls away. (Into
//!   the edge itself, it would fill and empty the edge step after step.)
//!
//! What spreads is worked out from where the fluid is once it has fallen,
//! and then moved all at once (and whatever spreads onto room falls into it
//! straight away), so the order cells are visited in doesn't
//! matter and flow has no favourite direction. An eighth is small enough
//! that what a cell gives never drops it below a neighbour it gives to,
//! even giving to four at once, so levels drift toward each other rather
//! than swinging back and forth. Falling lowers fluid and spreading evens it out, so a closed
//! scene always settles. The `film` and `ripple` say how flat it settles: a
//! puddle stays put once it's thinner than a film, and neighbours stop
//! flowing once they're less than a ripple apart, or so close that a unit
//! trickling between them could overshoot (a few units at most).
//!
//! A cell has room for a whole block of fluid if it's air, and for some
//! if its block leaves room (a slab, see `fluid_room`). Levels are
//! compared by how high the fluid stands: its amount on top of the part of
//! the cell the block takes, so a pond around a slab is level when the
//! slab's half of its cell is full.
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

/// The smallest `ripple` a fluid can have: a unit flowing between two cells
/// a unit apart would only swap them.
pub const MIN_RIPPLE: u16 = 2;

/// What a cell holds, as flow sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// No fluid, and room for this much, from 1 to [`FULL_FLUID`]: all of
    /// it for air, some for a block that leaves room.
    Open(u16),
    /// Fluid, by its block; how much, from 1; and the room the cell has
    /// for it, as for [`Cell::Open`].
    Fluid(BlockState, u16, u16),
    /// No room for fluid: a block.
    Closed,
}

#[cfg(test)]
impl Cell {
    /// Air: room for a whole block of fluid.
    pub const OPEN: Self = Self::Open(FULL_FLUID);
}

/// How high `amount` of fluid stands in a cell with `room` for it, in
/// [`FULL_FLUID`] units from the cell's bottom: on top of the part of the
/// cell its block takes.
fn level(amount: u16, room: u16) -> u16 {
    amount + FULL_FLUID.saturating_sub(room)
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

/// Cells in the same layer as a cell whose spreading can depend on it
/// without touching it: whether a unit trickles between two cells counts
/// the cells around the one it would go to.
const NEAR_SIDES: [IVec3; 8] = [
    IVec3::new(2, 0, 0),
    IVec3::new(-2, 0, 0),
    IVec3::new(0, 0, 2),
    IVec3::new(0, 0, -2),
    IVec3::new(1, 0, 1),
    IVec3::new(1, 0, -1),
    IVec3::new(-1, 0, 1),
    IVec3::new(-1, 0, -1),
];

/// The cells whose fluid might move when a cell changes: the cell, the six
/// around it, the four diagonally above it, whose fluid can run off an edge
/// into it, and the cells two steps away in its layer (see
/// [`NEAR_SIDES`]).
pub fn affected_by(pos: IVec3) -> impl Iterator<Item = IVec3> {
    std::iter::once(pos)
        .chain(AROUND.map(|offset| pos + offset))
        .chain(SIDES.map(|side| pos + IVec3::Y + side))
        .chain(NEAR_SIDES.map(|offset| pos + offset))
}

/// How much more of `kind` a cell has room for, if any: all its room if
/// it's open, what's left if it's `kind` and not full.
fn room(kind: BlockState, cell: Option<Cell>) -> Option<u16> {
    match cell {
        Some(Cell::Open(room)) => Some(room),
        Some(Cell::Fluid(k, amount, room)) if k == kind && amount < room => Some(room - amount),
        _ => None,
    }
}

/// A cell with `room` holding `amount` of `kind`: open if that's none.
fn fill(kind: BlockState, amount: u16, room: u16) -> Cell {
    if amount == 0 {
        Cell::Open(room)
    } else {
        Cell::Fluid(kind, amount, room)
    }
}

/// How many sides of the cell at `at`, with `kind` standing at `here`
/// (see [`level`]), hold `kind` at least a ripple `higher` or lower: the
/// cells it takes from, or gives to.
fn differing(
    cells: &impl CellView,
    kind: BlockState,
    fluid: &Fluid,
    at: IVec3,
    here: u16,
    higher: bool,
) -> u16 {
    SIDES
        .iter()
        .filter(|&&side| match cells.cell(at + side) {
            Some(Cell::Fluid(k, amount, room)) if k == kind => {
                let there = level(amount, room);
                if higher {
                    there >= here.saturating_add(fluid.ripple)
                } else {
                    here >= there.saturating_add(fluid.ripple)
                }
            }
            _ => false,
        })
        .count() as u16
}

/// Fluid to move from one cell to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transfer {
    from: IVec3,
    to: IVec3,
    amount: u16,
    /// The edge it goes over, for fluid running off one into the cell
    /// below it, which must still be open when it moves.
    over: Option<IVec3>,
}

/// What of the fluid at `pos` falls into the cell below: as much as fits,
/// up to its `fall`. Pushes onto `meetings` if a different fluid is below.
fn fall(
    cells: &impl CellView,
    fluid: &Fluid,
    pos: IVec3,
    meetings: &mut Vec<(IVec3, IVec3)>,
) -> Option<Transfer> {
    let Some(Cell::Fluid(kind, amount, _)) = cells.cell(pos) else {
        return None;
    };
    let below = pos - IVec3::Y;
    let under = cells.cell(below);
    if let Some(Cell::Fluid(other, ..)) = under
        && other != kind
    {
        meetings.push((pos, below));
    }
    let space = room(kind, under)?;
    Some(Transfer {
        from: pos,
        to: below,
        amount: amount.min(space).min(fluid.fall),
        over: None,
    })
}

/// Works out what the fluid at `pos` gives to its sides, as the module
/// docs describe, without moving any. Pushes onto `meetings` for each side
/// holding a different fluid.
fn plan_spread(
    cells: &impl CellView,
    fluid: &Fluid,
    pos: IVec3,
    transfers: &mut Vec<Transfer>,
    meetings: &mut Vec<(IVec3, IVec3)>,
) {
    let Some(Cell::Fluid(kind, amount, own_room)) = cells.cell(pos) else {
        return;
    };
    let here = level(amount, own_room);
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
    let lower = std::cell::OnceCell::new();
    let mut given = 0;
    for (side, falling) in SIDES.into_iter().zip(falling) {
        let next = pos + side;
        let gives = match cells.cell(next) {
            // Half of it goes over its edges in a step, however thin: what
            // goes over falls away, so it can't come back.
            Some(Cell::Open(_)) if falling => (amount / (2 * edges)).max(1),
            Some(Cell::Fluid(k, there, room)) if k == kind && falling => {
                here.saturating_sub(level(there, room)) / (2 * edges)
            }
            // Onto the floor of an open cell, if it's below the fluid.
            Some(Cell::Open(room)) => {
                let floor = level(0, room);
                if amount >= fluid.film && here > floor {
                    (here - floor) / SHARE
                } else {
                    0
                }
            }
            Some(Cell::Fluid(k, there, room)) if k == kind => {
                let there = level(there, room);
                let difference = here.saturating_sub(there);
                if difference < fluid.ripple {
                    0
                } else if difference >= SHARE {
                    difference / SHARE
                } else if difference
                    >= *lower.get_or_init(|| differing(cells, kind, fluid, pos, here, false))
                        + differing(cells, kind, fluid, next, there, true)
                {
                    // Too close for an eighth to be anything: a unit
                    // trickles, if every unit this cell gives and that
                    // one takes still can't put it above this one. Wide
                    // pools settle flat this way rather than in a slope.
                    1
                } else {
                    0
                }
            }
            Some(Cell::Fluid(..)) => {
                meetings.push((pos, next));
                0
            }
            _ => 0,
        };
        let gives = gives.min(amount - given);
        if gives > 0 {
            transfers.push(Transfer {
                from: pos,
                // What goes over an edge goes straight down past it, so the
                // lip doesn't fill and empty step after step.
                to: if falling { next - IVec3::Y } else { next },
                amount: gives,
                over: falling.then_some(next),
            });
            given += gives;
        }
    }
}

/// The most voxels [`push_aside`] looks through sideways for room, so a
/// block placed into a body of fluid costs the same however big it is.
pub const PUSH_REACH: usize = 64;

/// Pushes `amount` of `kind` that a block took the place of at `pos` into
/// cells with room for it, for flow to spread over the steps after.
/// Sideways first: the cells on `pos`'s level, nearest first, reached
/// from it through cells of the same fluid, so a block dropped into a
/// pond raises the pond around it rather than piling water on top. Cells
/// the same distance away share what's pushed (those with the least room
/// first, so the rest share what they can't take), and the search stops
/// after [`PUSH_REACH`] cells. Only what none of those has room for goes
/// into the cell above, as a stone dropped in a full glass pushes the
/// water over the rim, then into the cell below. It's outside any step,
/// the one place flow loses fluid: returns what had nowhere to go, which
/// is lost, and the cells it went into.
pub fn push_aside(
    cells: &mut impl Cells,
    pos: IVec3,
    kind: BlockState,
    amount: u16,
) -> (u16, Vec<IVec3>) {
    let mut push = Push {
        kind,
        left: amount,
        into: Vec::new(),
    };
    for ring in sideways(cells, pos, kind) {
        for (i, &(_, at)) in ring.iter().enumerate() {
            let share = push.left.div_ceil((ring.len() - i) as u16);
            push.give(cells, at, share);
        }
        if push.left == 0 {
            break;
        }
    }
    push.give(cells, pos + IVec3::Y, push.left);
    push.give(cells, pos - IVec3::Y, push.left);
    (push.left, push.into)
}

/// The cells with room for `kind` on `pos`'s level, by how many steps
/// sideways they are from it, each step through a cell of `kind` (a cell
/// with room but none of it ends the way), at most [`PUSH_REACH`] cells
/// looked at; each ring with its room, least first.
fn sideways(cells: &impl CellView, pos: IVec3, kind: BlockState) -> Vec<Vec<(u16, IVec3)>> {
    let mut seen = HashSet::from([pos]);
    let mut frontier = vec![pos];
    let mut looked = 0;
    let mut rings = Vec::new();
    while !frontier.is_empty() && looked < PUSH_REACH {
        let mut ring = Vec::new();
        let mut next = Vec::new();
        'ring: for &at in &frontier {
            for side in SIDES {
                if looked == PUSH_REACH {
                    break 'ring;
                }
                let cell_pos = at + side;
                if !seen.insert(cell_pos) {
                    continue;
                }
                looked += 1;
                let cell = cells.cell(cell_pos);
                if let Some(room) = room(kind, cell) {
                    ring.push((room, cell_pos));
                }
                if matches!(cell, Some(Cell::Fluid(k, ..)) if k == kind) {
                    next.push(cell_pos);
                }
            }
        }
        ring.sort_unstable_by_key(|&(room, at)| (room, at.x, at.z));
        rings.push(ring);
        frontier = next;
    }
    rings
}

/// Fluid being pushed aside: see [`push_aside`].
struct Push {
    kind: BlockState,
    /// What's still to go.
    left: u16,
    /// The cells it's gone into.
    into: Vec<IVec3>,
}

impl Push {
    /// Puts up to `most` of what's left in the cell at `at`, as much as it
    /// has room for.
    fn give(&mut self, cells: &mut impl Cells, at: IVec3, most: u16) {
        let cell = cells.cell(at);
        let Some(room) = room(self.kind, cell) else {
            return;
        };
        let given = most.min(room).min(self.left);
        if given == 0 {
            return;
        }
        let cell = match cell {
            Some(Cell::Fluid(_, there, room)) => Cell::Fluid(self.kind, there + given, room),
            Some(Cell::Open(room)) => Cell::Fluid(self.kind, given, room),
            _ => unreachable!("only cells with room"),
        };
        cells.set(at, cell);
        self.left -= given;
        self.into.push(at);
    }
}

/// Whether the fluid at `pos` would move if it stepped now: it has room
/// below it, or it would spread. False where there's no fluid. This is
/// flow's own rule, so waking only the fluid this says might move loses
/// nothing.
pub fn might_move(cells: &impl CellView, fluid: &Fluid, pos: IVec3) -> bool {
    let Some(Cell::Fluid(kind, ..)) = cells.cell(pos) else {
        return false;
    };
    if room(kind, cells.cell(pos - IVec3::Y)).is_some() {
        return true;
    }
    let mut transfers = Vec::new();
    plan_spread(cells, fluid, pos, &mut transfers, &mut Vec::new());
    !transfers.is_empty()
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
    /// By fluid block, in order, so runs come out the same every time.
    queues: BTreeMap<BlockState, Queue>,
    /// Which fluid goes first in the next tick. It turns over every tick,
    /// so a flood of one fluid can't use up the budget every time before
    /// the others get any.
    first: usize,
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
    /// What it has changed so far, kept aside.
    changes: Changes,
    /// What spreads, once everything has fallen.
    transfers: Vec<Transfer>,
    /// Fluids that met, to tell the world about.
    meetings: Vec<(IVec3, IVec3)>,
    /// Cells fluid fell into this step, which it doesn't fall on from.
    fell_into: HashSet<IVec3>,
}

impl Step {
    fn visits(&self) -> usize {
        self.cells.len() * 2
    }
}

/// What a step has changed so far, kept aside: each cell as it was in the
/// world and as it is now.
type Changes = HashMap<IVec3, (Option<Cell>, Cell)>;

/// The world as a step under way sees it: its changes so far on top of
/// the world.
struct Layer<'a, C> {
    world: &'a C,
    changes: &'a Changes,
}

impl<C: CellView> CellView for Layer<'_, C> {
    fn cell(&self, pos: IVec3) -> Option<Cell> {
        match self.changes.get(&pos) {
            Some(&(_, now)) => Some(now),
            None => self.world.cell(pos),
        }
    }
}

/// Moves fluid as `transfer` says, in `changes` over `world`: as much as
/// there is, and as the cell it goes to has room for.
fn apply(world: &impl CellView, changes: &mut Changes, transfer: Transfer) {
    let Transfer {
        from,
        to,
        amount,
        over,
    } = transfer;
    let layer = Layer { world, changes };
    let (Some(Cell::Fluid(kind, has, from_room)), there) = (layer.cell(from), layer.cell(to))
    else {
        return;
    };
    // Something filled the edge since this was planned.
    if over.is_some_and(|edge| room(kind, layer.cell(edge)).is_none()) {
        return;
    }
    let amount = amount.min(has).min(room(kind, there).unwrap_or(0));
    if amount == 0 {
        return;
    }
    let (there, to_room) = match there {
        Some(Cell::Fluid(_, there, room)) => (there, room),
        Some(Cell::Open(room)) => (0, room),
        _ => return,
    };
    for (pos, cell) in [
        (to, Cell::Fluid(kind, there + amount, to_room)),
        (from, fill(kind, has - amount, from_room)),
    ] {
        let was = world.cell(pos);
        changes
            .entry(pos)
            .and_modify(|(_, now)| *now = cell)
            .or_insert((was, cell));
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
        if let Some(Cell::Fluid(kind, ..)) = cells.cell(pos) {
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
        let mut kinds: Vec<BlockState> = self.queues.keys().copied().collect();
        if !kinds.is_empty() {
            let first = self.first % kinds.len();
            kinds.rotate_left(first);
        }
        self.first = self.first.wrapping_add(1);
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
                let pos = step.cells[step.visited % count];
                let layer = Layer {
                    world: &*cells,
                    changes: &step.changes,
                };
                if step.visited < count {
                    if let Some(falls) = fall(&layer, flow, pos, &mut step.meetings) {
                        step.fell_into.insert(falls.to);
                        apply(&*cells, &mut step.changes, falls);
                    }
                } else {
                    plan_spread(&layer, flow, pos, &mut step.transfers, &mut step.meetings);
                }
                step.visited += 1;
                left -= 1;
                stepped += 1;
                if step.visited == step.visits() {
                    let step = queue.step.take().expect("started");
                    finish_step(cells, flow, step, &mut queue.next, &mut changed);
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
    fluid: &Fluid,
    mut step: Step,
    next: &mut HashSet<IVec3>,
    changed: &mut Vec<IVec3>,
) {
    let transfers = std::mem::take(&mut step.transfers);
    let mut landed: Vec<IVec3> = transfers.iter().map(|transfer| transfer.to).collect();
    for transfer in transfers {
        apply(&*cells, &mut step.changes, transfer);
    }
    // What spread onto fluid that's drained in the same step falls into it
    // now, bottom up, rather than showing on top for a step and then
    // dropping in.
    // Not where fluid already fell this step: it falls a cell a step.
    landed.retain(|pos| !step.fell_into.contains(pos));
    landed.sort_unstable_by_key(|pos| (pos.y, pos.x, pos.z));
    landed.dedup();
    for pos in landed {
        let layer = Layer {
            world: &*cells,
            changes: &step.changes,
        };
        if let Some(falls) = fall(&layer, fluid, pos, &mut step.meetings) {
            apply(&*cells, &mut step.changes, falls);
        }
    }
    let stale = step
        .changes
        .iter()
        .any(|(&pos, &(was, _))| cells.cell(pos) != was);
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
    for (from, to) in step.meetings {
        cells.meet(from, to);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use bevy::platform::collections::HashMap;

    use super::*;
    use crate::world::fluid_step;

    pub(crate) const WATER: BlockState = BlockState(1);
    const LAVA: BlockState = BlockState(2);

    pub(crate) fn water() -> Fluid {
        Fluid {
            rate: 10.0,
            fall: FULL_FLUID,
            film: 62,
            ripple: 2,
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
            if let Cell::Fluid(_, amount, room) = cell {
                assert!((1..=room).contains(&amount), "{amount} at {pos}");
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
                        self.cells.insert(IVec3::new(x, y, z), Cell::OPEN);
                    }
                }
            }
        }

        fn total(&self, kind: BlockState) -> u64 {
            self.cells
                .values()
                .map(|cell| match cell {
                    Cell::Fluid(k, amount, _) if *k == kind => *amount as u64,
                    _ => 0,
                })
                .sum()
        }

        fn amount(&self, pos: IVec3) -> u16 {
            match self.cells.get(&pos) {
                Some(Cell::Fluid(_, amount, _)) => *amount,
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
        grid.set(pos, Cell::Fluid(kind, amount, FULL_FLUID));
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
        assert_eq!(grid.cell(IVec3::new(0, 5, 0)), Some(Cell::OPEN));
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
                Cell::Fluid(_, amount, _) => Some(*amount),
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
        grid.cells.insert(IVec3::new(2, -1, 0), Cell::OPEN);
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
        grid.cells.insert(hole, Cell::OPEN);
        sim.wake_around(&grid, hole);
        settle(&mut grid, &mut sim, 500);
        assert_eq!(grid.amount(hole), 200, "it all ran into the hole");
    }

    /// A step shows only once it's done: until then the world is as it
    /// was. If something changes a cell the step changes before it's done,
    /// or it unloads, the step is dropped and done again, so the change
    /// isn't overwritten and no fluid is made or lost.
    #[test]
    fn steps_show_whole_and_start_over_when_the_world_changes() {
        for unload in [false, true] {
            let mut grid = Grid::default();
            grid.open(IVec3::ZERO, IVec3::new(9, 1, 0));
            let mut sim = Simulation::default();
            for x in 0..10 {
                pour(&mut grid, &mut sim, IVec3::new(x, 1, 0), WATER, FULL_FLUID);
            }
            let before = grid.cells.clone();
            // Ticks of a hundredth of a second: the first step starts and
            // is under way, a fifth done.
            for _ in 0..12 {
                sim.tick(&mut grid, fluids, 0.01, usize::MAX);
            }
            assert!(grid.cells == before, "nothing shows mid-step");
            // Under a falling cell, the step's target changes.
            let under = IVec3::new(3, 0, 0);
            if unload {
                grid.unloaded.insert(under);
            } else {
                grid.cells.insert(under, Cell::Closed);
            }
            sim.wake_around(&grid, under);
            for _ in 0..200 {
                sim.tick(&mut grid, fluids, 0.01, usize::MAX);
            }
            // (The test grid fails if flow writes over the closed cell.)
            assert_eq!(grid.total(WATER), 10 * FULL_FLUID as u64);
        }
    }

    /// Fluid running off an edge doesn't pass through a block put on the
    /// edge while the step was under way.
    #[test]
    fn fluid_doesnt_run_through_a_new_block_on_the_edge() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::X);
        let below = IVec3::new(1, -1, 0);
        grid.cells.insert(below, Cell::OPEN);
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, WATER, 400);
        // A second cell, on its own, that steps after it.
        grid.cells.insert(IVec3::new(5, 0, 0), Cell::OPEN);
        pour(&mut grid, &mut sim, IVec3::new(5, 0, 0), WATER, 400);
        // A visit a tick: both fall, then the first plans running off its
        // edge. The step isn't done until the second has planned too.
        let mut visits = 0;
        while visits < 3 {
            visits += sim.tick(&mut grid, fluids, 0.01, 1).stepped;
        }
        assert_eq!(grid.amount(below), 0, "not done yet");
        grid.cells.insert(IVec3::X, Cell::Closed);
        for _ in 0..100 {
            sim.tick(&mut grid, fluids, 0.01, usize::MAX);
        }
        assert_eq!(grid.amount(below), 0, "it went through the block");
        assert_eq!(grid.amount(IVec3::ZERO), 400);
    }

    /// Fluid falls at most one cell a step: what fell into a cell this
    /// step doesn't fall on again at its end, though fluid that spread in
    /// beside it can.
    #[test]
    fn fluid_falls_one_cell_a_step() {
        let mut grid = Grid::default();
        // Lava 1000 at A falls into B (400) below it, over C (1000, on the
        // floor), which spreads into the open cell beside it, making room;
        // D beside B spreads into it.
        let (a, b, c, d) = (
            IVec3::new(0, 2, 0),
            IVec3::Y,
            IVec3::ZERO,
            IVec3::new(-1, 1, 0),
        );
        for pos in [a, b, c, d, IVec3::X, IVec3::new(1, 1, 0)] {
            grid.cells.insert(pos, Cell::OPEN);
        }
        let mut sim = Simulation::default();
        for (pos, amount) in [(a, 1000), (b, 400), (c, 1000), (d, 1000)] {
            pour(&mut grid, &mut sim, pos, LAVA, amount);
        }
        sim.tick(&mut grid, fluids, 0.5, usize::MAX);
        assert_eq!(grid.amount(a), 500, "half fell");
        assert_eq!(
            grid.amount(c),
            875,
            "C gave an eighth, and took nothing from above"
        );
        assert_eq!(grid.total(LAVA), 3400);
    }

    /// Half of a cell goes over its edges in a step, shared between them,
    /// straight into the cells below the edges.
    #[test]
    fn half_goes_over_the_edges_in_a_step() {
        for edges in [1, 2] {
            let mut grid = Grid::default();
            grid.open(IVec3::ZERO, IVec3::X);
            grid.cells.insert(IVec3::new(1, -1, 0), Cell::OPEN);
            if edges == 2 {
                grid.open(IVec3::new(-1, -1, 0), IVec3::NEG_X);
            }
            let mut sim = Simulation::default();
            pour(&mut grid, &mut sim, IVec3::ZERO, LAVA, 400);
            sim.tick(&mut grid, fluids, 0.5, usize::MAX);
            assert_eq!(grid.amount(IVec3::ZERO), 200, "{edges} edges");
            assert_eq!(grid.amount(IVec3::X), 0, "the lip stays empty");
            assert_eq!(grid.amount(IVec3::new(1, -1, 0)), 200 / edges);
        }
    }

    /// Cells less than a ripple apart are level, so they don't flow; further
    /// apart, they even out.
    #[test]
    fn ripples_dont_flow() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(1, 0, 0));
        let mut sim = Simulation::default();
        pour(&mut grid, &mut sim, IVec3::ZERO, WATER, 501);
        pour(&mut grid, &mut sim, IVec3::X, WATER, 500);
        settle(&mut grid, &mut sim, 10);
        assert_eq!(grid.amount(IVec3::ZERO), 501);

        grid.set(IVec3::ZERO, Cell::Fluid(WATER, 530, FULL_FLUID));
        sim.wake_around(&grid, IVec3::ZERO);
        settle(&mut grid, &mut sim, 100);
        let (a, b) = (grid.amount(IVec3::ZERO), grid.amount(IVec3::X));
        assert_eq!(a + b, 1030);
        assert!(a.abs_diff(b) < water().ripple, "{a} {b}");
    }

    /// A long channel, half deep and half shallow, levels off flat: less
    /// than a drawn height from end to end, not a slope of a ripple a cell.
    #[test]
    fn a_long_channel_levels_off_flat() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(39, 0, 0));
        let mut sim = Simulation::default();
        for x in 0..40 {
            let amount = if x < 20 { 900 } else { 100 };
            pour(&mut grid, &mut sim, IVec3::new(x, 0, 0), WATER, amount);
        }
        settle(&mut grid, &mut sim, 20_000);
        let row: Vec<u16> = (0..40).map(|x| grid.amount(IVec3::new(x, 0, 0))).collect();
        let (low, high) = (row.iter().min().unwrap(), row.iter().max().unwrap());
        assert!(high - low < FULL_FLUID / 16, "{row:?}");
    }

    /// Random scenes of open and closed cells with water and lava poured
    /// in, at random tick lengths and budgets: no fluid is ever made or
    /// lost, every scene settles, and once settled, waking everything moves
    /// nothing.
    #[test]
    fn random_scenes_keep_their_fluid_and_settle() {
        let mut seed = 0x0DEF_A41A_u64;
        let mut random =
            move |below: u32| (crate::perlin::splitmix64(&mut seed) % below as u64) as u32;
        for _ in 0..300 {
            let mut grid = Grid::default();
            for x in 0..8 {
                for y in 0..6 {
                    for z in 0..8 {
                        // Some cells with room for only part of a
                        // block, as a slab or a tuft leaves.
                        let cell = match random(12) {
                            0..3 => continue,
                            3 => Cell::Open(500),
                            4 => Cell::Open(900),
                            _ => Cell::OPEN,
                        };
                        grid.cells.insert(IVec3::new(x, y, z), cell);
                    }
                }
            }
            let mut open: Vec<IVec3> = grid.cells.keys().copied().collect();
            open.sort_by_key(|p| (p.x, p.y, p.z));
            let mut sim = Simulation::default();
            for _ in 0..random(12) + 1 {
                let pos = open[random(open.len() as u32) as usize];
                let kind = if random(3) == 0 { LAVA } else { WATER };
                if grid.cell(pos) == Some(Cell::OPEN) {
                    let amount = random(FULL_FLUID as u32) as u16 + 1;
                    pour(&mut grid, &mut sim, pos, kind, amount);
                }
            }
            let totals = [WATER, LAVA].map(|kind| grid.total(kind));
            let seconds = [0.004, 1.0 / 60.0, 0.05, 0.25][random(4) as usize];
            let budget = [1, 7, 50, usize::MAX][random(4) as usize];
            let mut ticks = 0;
            while sim.active() > 0 {
                sim.tick(&mut grid, fluids, seconds, budget);
                assert_eq!([WATER, LAVA].map(|kind| grid.total(kind)), totals);
                ticks += 1;
                assert!(ticks < 1_000_000, "never settled");
            }
            let before = grid.cells.clone();
            for &pos in &open {
                sim.wake(&grid, pos);
            }
            for _ in 0..40 {
                sim.tick(&mut grid, fluids, 0.25, usize::MAX);
            }
            assert!(grid.cells == before, "settled, but moved when woken");
        }
    }

    /// A cell with room for half a block (a slab) between full ones fills
    /// from them, and the row levels off with the water in it standing as
    /// high as beside it: its amount on top of the slab's half.
    #[test]
    fn part_cells_fill_and_level_with_the_water_beside_them() {
        let mut grid = Grid::default();
        grid.open(IVec3::ZERO, IVec3::new(4, 0, 0));
        let slab = IVec3::new(2, 0, 0);
        grid.cells.insert(slab, Cell::Open(500));
        let mut sim = Simulation::default();
        for x in [0, 1, 3, 4] {
            pour(&mut grid, &mut sim, IVec3::new(x, 0, 0), WATER, FULL_FLUID);
        }
        settle(&mut grid, &mut sim, 10_000);
        // 4000 over room for 4500: each stands 900 high, the slab's 400
        // on top of its 500.
        let levels: Vec<u16> = (0..5)
            .map(|x| {
                let pos = IVec3::new(x, 0, 0);
                grid.amount(pos) + if pos == slab { 500 } else { 0 }
            })
            .collect();
        let (low, high) = (levels.iter().min().unwrap(), levels.iter().max().unwrap());
        assert!(high - low < FULL_FLUID / 16, "{levels:?}");
        assert!(grid.amount(slab) > 300, "{levels:?}");
    }

    /// Fluid pushed aside goes sideways before up, further than the cells
    /// beside it: through the same fluid on its level, to the nearest cells
    /// with room, and only up with what none within reach can take. Not
    /// through another fluid, nor past a cell with room and none of it.
    #[test]
    fn pushing_aside_looks_sideways_through_the_fluid_first() {
        let full = Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID);
        let push = |cells: &[(IVec3, Cell)], amount: u16| {
            let mut grid = Grid::default();
            grid.cells.extend(cells.iter().copied());
            let (left, _) = push_aside(&mut grid, IVec3::ZERO, WATER, amount);
            (grid, left)
        };
        // Through two full cells to one with room for 800, the rest up.
        let row = [
            (IVec3::X, full),
            (IVec3::new(2, 0, 0), full),
            (IVec3::new(3, 0, 0), Cell::Fluid(WATER, 200, FULL_FLUID)),
            (IVec3::Y, Cell::OPEN),
        ];
        let (grid, left) = push(&row, 1000);
        assert_eq!(left, 0);
        assert_eq!(grid.amount(IVec3::new(3, 0, 0)), 1000);
        assert_eq!(grid.amount(IVec3::Y), 200, "only what didn't fit goes up");
        // The nearest room first: beside it, before further along.
        let mut near = row.to_vec();
        near.push((IVec3::NEG_X, Cell::OPEN));
        let (grid, _) = push(&near, 500);
        assert_eq!(grid.amount(IVec3::NEG_X), 500);
        assert_eq!(grid.amount(IVec3::new(3, 0, 0)), 200);
        // Not through lava, nor through the dry cell beyond which there's
        // room: up instead.
        for between in [Cell::Fluid(LAVA, FULL_FLUID, FULL_FLUID), Cell::Open(1)] {
            let mut blocked = row.to_vec();
            blocked[0] = (IVec3::X, between);
            let (grid, _) = push(&blocked, 1000);
            assert_eq!(grid.amount(IVec3::new(3, 0, 0)), 200, "{between:?}");
        }
    }

    /// The search sideways looks at no more than [`PUSH_REACH`] cells, so
    /// room at the far end of a long channel of full water isn't found,
    /// and the fluid goes up; nearer, it is.
    #[test]
    fn pushing_aside_sideways_is_bounded() {
        let channel = |room_at: i32| {
            let mut grid = Grid::default();
            for x in 1..room_at {
                grid.cells.insert(
                    IVec3::new(x, 0, 0),
                    Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID),
                );
            }
            grid.cells.insert(IVec3::new(room_at, 0, 0), Cell::OPEN);
            grid.cells.insert(IVec3::Y, Cell::OPEN);
            push_aside(&mut grid, IVec3::ZERO, WATER, 1000);
            (
                grid.amount(IVec3::new(room_at, 0, 0)),
                grid.amount(IVec3::Y),
            )
        };
        assert_eq!(channel(10), (1000, 0));
        // Each step along looks at three cells: past the reach.
        const { assert!(3 * 40 > PUSH_REACH) };
        assert_eq!(channel(40), (0, 1000));
    }

    /// Fluid pushed aside is shared between the sides first (those with
    /// less room first, so the rest share what they can't take), then what
    /// they can't take goes into the cell above, then into the one below,
    /// as much as each has room for; what none has room for is what's
    /// left.
    #[test]
    fn pushing_aside_goes_to_the_sides_then_up_then_down() {
        let pos = IVec3::ZERO;
        let full = Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID);
        let push = |cells: &[(IVec3, Cell)], amount: u16| {
            let mut grid = Grid::default();
            grid.cells.extend(cells.iter().copied());
            let (left, mut into) = push_aside(&mut grid, pos, WATER, amount);
            into.sort_by_key(|p| (p.x, p.y, p.z));
            (grid, left, into)
        };
        // Beside it, in a trench that isn't full, rather than on top.
        let half = Cell::Fluid(WATER, 500, FULL_FLUID);
        let (grid, left, into) = push(&[(IVec3::Y, Cell::OPEN), (IVec3::X, half)], 400);
        assert_eq!((left, into), (0, vec![IVec3::X]));
        assert_eq!(grid.amount(IVec3::X), 900);
        // Shared between the sides: the one with room for only 100 takes
        // that, the others the rest evenly; none for another fluid's.
        let cells = [
            (IVec3::Y, Cell::OPEN),
            (IVec3::X, Cell::OPEN),
            (IVec3::NEG_X, Cell::Fluid(WATER, 900, FULL_FLUID)),
            (IVec3::Z, Cell::Open(500)),
            (IVec3::NEG_Z, Cell::Fluid(LAVA, 10, FULL_FLUID)),
            (IVec3::NEG_Y, Cell::OPEN),
        ];
        let (grid, left, _) = push(&cells, 1000);
        assert_eq!(left, 0);
        assert_eq!(grid.amount(IVec3::NEG_X), 1000, "its 100");
        assert_eq!((grid.amount(IVec3::X), grid.amount(IVec3::Z)), (450, 450));
        assert_eq!(grid.amount(IVec3::NEG_Z), 10, "no other fluid's");
        assert_eq!(
            grid.amount(IVec3::Y) + grid.amount(IVec3::NEG_Y),
            0,
            "not needed"
        );
        // The sides full, as in a brim-full pond: up over the rim.
        let (grid, left, into) = push(&[(IVec3::X, full), (IVec3::Y, Cell::OPEN)], 1000);
        assert_eq!((left, into), (0, vec![IVec3::Y]));
        assert_eq!(grid.amount(IVec3::Y), 1000);
        // Up before down, with both open: back up a one-wide shaft.
        let open = [(IVec3::Y, Cell::OPEN), (IVec3::NEG_Y, Cell::OPEN)];
        let (_, left, into) = push(&open, 1000);
        assert_eq!((left, into), (0, vec![IVec3::Y]));
        // Below last, then nothing: what's left is lost.
        let (grid, left, into) = push(&[(IVec3::Y, full), (IVec3::NEG_Y, Cell::Open(300))], 1000);
        assert_eq!((left, into), (700, vec![IVec3::NEG_Y]));
        assert_eq!(grid.amount(IVec3::NEG_Y), 300);
        let (_, left, into) = push(&[(IVec3::Y, full)], 1000);
        assert_eq!((left, into.len()), (1000, 0));
    }

    /// A pond full to the brim around a slab whose half is full is
    /// settled: nothing moves when it's all woken.
    #[test]
    fn a_full_pond_around_a_full_slab_is_settled() {
        let mut grid = Grid::default();
        let mut sim = Simulation::default();
        for x in 0..5 {
            let pos = IVec3::new(x, 0, 0);
            let cell = if x == 2 {
                Cell::Fluid(WATER, 500, 500)
            } else {
                Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID)
            };
            grid.cells.insert(pos, cell);
            sim.wake(&grid, pos);
        }
        let before = grid.cells.clone();
        assert_eq!(
            settle(&mut grid, &mut sim, 10),
            1,
            "one step, moving nothing"
        );
        assert!(grid.cells == before);
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
                assert!(matches!(grid.cell(pos), Some(Cell::OPEN | Cell::Fluid(..))));
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

    /// Fluids take turns going first, so a flood of one that uses up the
    /// whole budget doesn't stop the others.
    #[test]
    fn fluids_take_turns_with_the_budget() {
        let mut grid = Grid::default();
        // A wide water flood spreading over a floor and, apart from it, a
        // column of lava.
        grid.open(IVec3::new(0, 0, 0), IVec3::new(59, 0, 59));
        grid.open(IVec3::new(80, 0, 0), IVec3::new(80, 9, 0));
        let mut sim = Simulation::default();
        for x in 20..40 {
            for z in 20..40 {
                pour(&mut grid, &mut sim, IVec3::new(x, 0, z), WATER, FULL_FLUID);
            }
        }
        pour(&mut grid, &mut sim, IVec3::new(80, 9, 0), LAVA, FULL_FLUID);
        for _ in 0..180 {
            sim.tick(&mut grid, fluids, 1.0 / 60.0, 100);
        }
        assert!(sim.active() > 400, "the flood is still spreading");
        // Three seconds of lava at two steps a second falls five cells,
        // though the water's flood could use up much of every tick's budget.
        let lava_low: u16 = (0..6).map(|y| grid.amount(IVec3::new(80, y, 0))).sum();
        assert!(lava_low > 0, "the lava fell too slowly");
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
    /// voxel ever gains more than a few units, and its drawn height almost
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
                    after < before + SHARE,
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
