//! Reservoirs: a connected body of one fluid kept at one shared level, so
//! fluid leaving it (or reaching it) lowers (or raises) the whole surface
//! at once, and fluid poured into one end of it is level across all of it
//! straight away, rather than diffusing a voxel at a time.
//!
//! A reservoir is made of **columns**: in each `(x, z)` it covers, an
//! unbroken run of its fluid from a **bottom** cell resting on something
//! solid up to a top cell. Columns join sideways where their runs overlap.
//! One is found from a cell that's giving fluid away (or that a pump draws
//! from), by walking the columns joined to it, up to
//! [`Reservoirs::max_columns`] of them. Fluid that's falling (with room under
//! its run) is never part of one.
//!
//! Whatever flow would take out of or put into a **member** (a cell in one
//! of its columns) is **pooled** instead: the member stays as it was and the
//! reservoir's `balance` goes down or up by as much. After each step of its
//! fluid, the reservoir is **levelled**: everything it holds, with the
//! balance, is shared out over its columns so that its surface is flat,
//! filling each column from its bottom up to the same height. Columns on
//! shallow ground empty when the level drops below them, and the level can
//! rise a cell above a column's top. Cells that fluid spreads onto beside a
//! column join the reservoir, so a front pouring out of one is as high as
//! the reservoir right behind it.
//!
//! Levelling only writes the cells whose amounts change, as flow writes
//! them, and wakes only the cells along the reservoir's edge, so a calm lake
//! costs nothing and a draining one costs a write a column a step.
//!
//! A reservoir is dropped (its balance paid out first) when a chunk it's in
//! unloads, or when it's left holding nothing. The balance is fluid like
//! any other: it's counted in the totals ([`Reservoirs::pooled`]), and paid
//! out before chunks are saved.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use bevy::prelude::*;

use super::flow::{Cell, CellView, Cells};
use crate::{
    content::{BlockState, FULL_FLUID, Fluid},
    world::CHUNK_SIZE,
};

/// The fewest columns a reservoir has: smaller bodies flow as before.
pub const MIN_COLUMNS: usize = 16;

/// The most columns a reservoir takes in from around its rim each time it's
/// levelled (see `grow_from_rim`).
const RIM_GROWTH: usize = 1024;

/// The most columns a reservoir starts with. It can grow past this, up to
/// [`Reservoirs::max_columns`], as fluid beside it joins it, a column at a
/// time; walking a whole sea at once would take too long for one tick.
pub const START_COLUMNS: usize = 1024;

/// How many units a column a reservoir's balance comes to before the whole
/// reservoir is levelled again, and how far a column can be from the level
/// before levelling writes it. A quarter of a drawn step: the surface is
/// drawn flat, and a big lake is written over every so often rather than
/// every step.
pub const LAZY: i64 = FULL / 64;

/// How long a column that was looked at and didn't make a reservoir waits
/// before it's looked at again, in seconds.
const RETRY_SECS: f32 = 2.0;

/// How tall a column's run can be.
const MAX_DEPTH: i32 = 256;

/// The four sides, as `(x, z)`.
const SIDES: [IVec2; 4] = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y];

const FULL: i64 = FULL_FLUID as i64;

/// One column of a reservoir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Column {
    /// Its bottom cell, which rests on something solid.
    bottom: i32,
    /// Its top cell holding fluid; `bottom - 1` while it holds none.
    top: i32,
    /// How much it holds, as last read or written.
    amount: i64,
    /// Whether the cell above its top is open, so the level can rise into
    /// it.
    room_above: bool,
    /// Something may have changed it: read it again before levelling.
    stale: bool,
    /// It's on the reservoir's edge: a side has no column of the reservoir.
    rim: bool,
    /// Every cell under its top is full, as levelling leaves it.
    even: bool,
}

impl Column {
    /// How much it can hold: its run, and a cell above it if that's open.
    fn capacity(&self) -> i64 {
        (self.top - self.bottom + 1 + self.room_above as i32) as i64 * FULL
    }

    /// How much it holds at `level`, in units up from the bottom of y 0.
    fn at_level(&self, level: i64) -> i64 {
        (level - self.bottom as i64 * FULL).clamp(0, self.capacity())
    }

    /// The cells to wake when its level changes, if it's on the rim: its
    /// top, where it meets what's beside the reservoir, and the cells
    /// beside that. (Not everything around it, as an edit wakes: a big
    /// reservoir's rim is long, and it's levelled often.)
    fn rim_cells(&self, x: i32, z: i32) -> impl Iterator<Item = IVec3> + use<> {
        let top = IVec3::new(x, self.top.max(self.bottom), z);
        let cells =
            std::iter::once(top).chain(SIDES.map(|side| top + IVec3::new(side.x, 0, side.y)));
        cells
            .filter(move |_| self.rim)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

/// Whether the column at `at` is on the rim: a side has no column of the
/// reservoir.
fn is_rim(columns: &BTreeMap<(i32, i32), Column>, at: IVec2) -> bool {
    SIDES
        .iter()
        .any(|&side| !columns.contains_key(&(at.x + side.x, at.y + side.y)))
}

/// One body of fluid kept at a shared level.
#[derive(Debug, Clone)]
pub struct Reservoir {
    /// Its fluid block.
    pub kind: BlockState,
    /// By `(x, z)`, in order, so runs come out the same every time.
    columns: BTreeMap<(i32, i32), Column>,
    /// Fluid pooled and not yet written into the world: taken out if
    /// negative, put in if positive.
    balance: i64,
    /// What its columns hold, as last read or written.
    held: i64,
    /// The level its columns were last filled to, in units up from the
    /// bottom of y 0; `None` until it's first levelled.
    level: Option<i64>,
    /// Columns joined or changed since, to read again and fill to the level.
    dirty: Vec<(i32, i32)>,
    /// A chunk it's in is unloading: pay out and drop it.
    dropped: bool,
    /// Levelling emptied a column since it was last checked for still
    /// being one body.
    emptied: bool,
    /// Its fluid's `film`: spread thinner than this on average, it's
    /// dropped and flows as before.
    film: u16,
}

impl Reservoir {
    pub fn columns(&self) -> usize {
        self.columns.len()
    }

    /// Fluid pooled and not yet written into the world.
    pub fn balance(&self) -> i64 {
        self.balance
    }

    /// Everything it holds, written and pooled.
    fn total(&self) -> i64 {
        self.held + self.balance
    }

    /// Whether its columns holding fluid are all joined, side to side, to
    /// one another. (Two that both hold fluid overlap, since both are
    /// filled to its level.)
    fn is_one_body(&self) -> bool {
        let wet: HashSet<(i32, i32)> = self
            .columns
            .iter()
            .filter(|(_, column)| column.amount > 0)
            .map(|(&key, _)| key)
            .collect();
        let Some(&start) = wet.iter().next() else {
            return true;
        };
        let mut seen = HashSet::from([start]);
        let mut queue = vec![start];
        while let Some((x, z)) = queue.pop() {
            for side in SIDES {
                let next = (x + side.x, z + side.y);
                if wet.contains(&next) && seen.insert(next) {
                    queue.push(next);
                }
            }
        }
        seen.len() == wet.len()
    }

    /// Whether the cell at `pos` is in one of its columns.
    fn holds(&self, pos: IVec3) -> bool {
        self.columns
            .get(&(pos.x, pos.z))
            .is_some_and(|c| (c.bottom..=c.top).contains(&pos.y))
    }
}

/// Every reservoir, and what flow needs to find more.
#[derive(Debug, Default)]
pub struct Reservoirs {
    /// The most columns one reservoir has; none form at 0.
    pub max_columns: usize,
    /// By id, in order, so runs come out the same every time.
    list: BTreeMap<u32, Reservoir>,
    next_id: u32,
    /// Which reservoir has a column at each `(x, z)`.
    owners: HashMap<IVec2, u32>,
    /// Cells that took fluid and aren't members, to join to a reservoir
    /// beside them.
    joining: Vec<(BlockState, IVec3)>,
    /// Reservoirs found beside each other, to merge (see
    /// `note_neighbours`).
    touching: Vec<(u32, u32)>,
    /// Columns looked at without making a reservoir, and when they can be
    /// looked at again.
    tried: HashMap<IVec2, f32>,
    clock: f32,
    /// Fluid reservoirs couldn't put back when they were dropped, by fluid
    /// block, which should never happen; counted in [`Self::pooled`] so the
    /// totals show it if it does.
    stranded: BTreeMap<BlockState, i64>,
}

fn amount_of(kind: BlockState, cell: Option<Cell>) -> Option<u16> {
    match cell {
        // Only whole voxels: one a block shares with fluid (a waterlogged
        // slab) ends a run.
        Some(Cell::Fluid(k, amount, FULL_FLUID)) if k == kind => Some(amount),
        _ => None,
    }
}

/// The run of `kind` through `pos`, as a column, read from `cells`: `None`
/// if `pos` isn't `kind`, or if the run has room under it (it's falling) or
/// isn't loaded under it.
fn run_at(cells: &impl CellView, kind: BlockState, pos: IVec3) -> Option<Column> {
    amount_of(kind, cells.cell(pos))?;
    let (x, z) = (pos.x, pos.z);
    let at = |y: i32| IVec3::new(x, y, z);
    let mut bottom = pos.y;
    while amount_of(kind, cells.cell(at(bottom - 1))).is_some() {
        bottom -= 1;
        if pos.y - bottom > MAX_DEPTH {
            return None;
        }
    }
    if !matches!(cells.cell(at(bottom - 1)), Some(Cell::Closed)) {
        return None;
    }
    let mut column = Column {
        bottom,
        top: bottom - 1,
        amount: 0,
        room_above: false,
        stale: false,
        rim: false,
        even: false,
    };
    read(cells, kind, x, z, &mut column);
    Some(column)
}

/// Reads a column's top and amount from the cells, from its bottom up.
fn read(cells: &impl CellView, kind: BlockState, x: i32, z: i32, column: &mut Column) {
    let at = |y: i32| IVec3::new(x, y, z);
    let mut y = column.bottom;
    let mut amount = 0;
    let mut even = true;
    let mut last = FULL_FLUID;
    while y - column.bottom < MAX_DEPTH {
        let Some(here) = amount_of(kind, cells.cell(at(y))) else {
            break;
        };
        even &= last == FULL_FLUID;
        last = here;
        amount += here as i64;
        y += 1;
    }
    column.even = even;
    column.top = y - 1;
    column.amount = amount;
    column.room_above = matches!(cells.cell(at(y)), Some(Cell::Open(FULL_FLUID)));
    column.stale = false;
}

impl Reservoirs {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Reservoir> {
        self.list.values()
    }

    /// The fluids that have reservoirs.
    pub fn kinds(&self) -> BTreeSet<BlockState> {
        self.list.values().map(|r| r.kind).collect()
    }

    /// All the fluid [`Self::pooled`], whatever it is.
    pub fn pooled_total(&self) -> i64 {
        self.pooled().values().sum()
    }

    /// The fluid pooled in reservoirs and not yet in the world, by fluid
    /// block, with any they couldn't put back.
    pub fn pooled(&self) -> BTreeMap<BlockState, i64> {
        let mut pooled = self.stranded.clone();
        for reservoir in self.list.values() {
            *pooled.entry(reservoir.kind).or_default() += reservoir.balance;
        }
        pooled
    }

    /// The reservoir of `kind` the cell at `pos` is a member of.
    pub fn member(&self, pos: IVec3, kind: BlockState) -> Option<u32> {
        let id = *self.owners.get(&IVec2::new(pos.x, pos.z))?;
        let reservoir = &self.list[&id];
        (reservoir.kind == kind && !reservoir.dropped && reservoir.holds(pos)).then_some(id)
    }

    /// Flow is about to change the cell at `pos` from `was` to `now`.
    /// Returns whether a reservoir took the change, in which case flow
    /// leaves the cell as it is.
    pub fn pool(&mut self, pos: IVec3, was: Cell, now: Cell) -> bool {
        if self.max_columns == 0 {
            return false;
        }
        let (kind, before, after) = match (was, now) {
            (Cell::Fluid(kind, before, _), Cell::Fluid(k, after, _)) if k == kind => {
                (kind, before, after)
            }
            (Cell::Fluid(kind, before, _), Cell::Open(_)) => (kind, before, 0),
            (Cell::Open(_), Cell::Fluid(kind, after, _)) => (kind, 0, after),
            _ => return false,
        };
        let delta = after as i64 - before as i64;
        if delta == 0 {
            return false;
        }
        if before > 0
            && let Some(id) = self.member(pos, kind)
        {
            let reservoir = self.list.get_mut(&id).expect("member");
            reservoir.balance += delta;
            if reservoir.columns[&(pos.x, pos.z)].rim {
                self.note_neighbours(id, kind, pos);
            }
            return true;
        }
        if delta > 0 {
            self.joining.push((kind, pos));
        }
        false
    }

    /// Notes another reservoir of `kind` with a column beside the member of
    /// reservoir `id` at `pos`, at its height, to merge the two: fluid
    /// moving between them would be pooled on both sides and wake nothing,
    /// so they'd stop at different levels.
    fn note_neighbours(&mut self, id: u32, kind: BlockState, pos: IVec3) {
        for side in SIDES {
            let next = IVec3::new(pos.x + side.x, pos.y, pos.z + side.y);
            if let Some(other) = self.member(next, kind)
                && other != id
            {
                self.touching.push((id.min(other), id.max(other)));
            }
        }
    }

    /// Merges the reservoirs noted touching, the smaller into the bigger,
    /// as long as the two fit in one. If they don't, it moves fluid from
    /// the higher one's balance to the lower one's, as much as brings them
    /// to one level, and both are levelled.
    fn merge_touching(&mut self) {
        let mut touching = std::mem::take(&mut self.touching);
        touching.sort_unstable();
        touching.dedup();
        for (a, b) in touching {
            let (Some(first), Some(second)) = (self.list.get(&a), self.list.get(&b)) else {
                continue;
            };
            if first.dropped || second.dropped {
                continue;
            }
            if first.columns.len() + second.columns.len() > self.max_columns {
                let (Some(high), Some(low)) = (first.level, second.level) else {
                    continue;
                };
                // Levels closer than levelling writes are left be.
                if (high - low).abs() < LAZY {
                    continue;
                }
                // Over two bodies of n and m columns, a difference d in
                // level evens out when d·n·m/(n+m) moves across.
                let (n, m) = (first.columns.len() as i128, second.columns.len() as i128);
                let moved = ((high - low) as i128 * n * m / (n + m)) as i64;
                let moved = moved.clamp(-second.total(), first.total());
                for (id, change) in [(a, -moved), (b, moved)] {
                    let reservoir = self.list.get_mut(&id).expect("listed");
                    reservoir.balance += change;
                    reservoir.level = None;
                }
                continue;
            }
            let (into, from) = if first.columns.len() >= second.columns.len() {
                (a, b)
            } else {
                (b, a)
            };
            let from = self.list.remove(&from).expect("listed");
            let into_id = into;
            let into = self.list.get_mut(&into_id).expect("listed");
            into.balance += from.balance;
            into.held += from.held;
            into.dirty.extend(from.dirty);
            into.level = None;
            into.emptied |= from.emptied;
            let keys: Vec<(i32, i32)> = from.columns.keys().copied().collect();
            into.columns.extend(from.columns);
            for &(x, z) in &keys {
                self.owners.insert(IVec2::new(x, z), into_id);
            }
            let into = self.list.get_mut(&into_id).expect("listed");
            for (x, z) in keys {
                mark_rims(into, IVec2::new(x, z));
            }
        }
    }

    /// Draws up to `want` of the fluid at `pos` out of its reservoir, for
    /// a pump, finding the reservoir first if it's in none. Returns how much
    /// it drew; `None` if `pos` is in no reservoir.
    pub fn draw(
        &mut self,
        cells: &impl CellView,
        fluid: &Fluid,
        pos: IVec3,
        want: u64,
    ) -> Option<u64> {
        if self.max_columns == 0 {
            return None;
        }
        let Some(Cell::Fluid(kind, _, _)) = cells.cell(pos) else {
            return None;
        };
        let id = match self.member(pos, kind) {
            Some(id) => id,
            // Another reservoir has this column, at another height: the
            // pump draws as it would without one.
            None if self.owners.contains_key(&IVec2::new(pos.x, pos.z)) => return None,
            // Not one that was looked at a moment ago and wasn't one.
            None if self
                .tried
                .get(&IVec2::new(pos.x, pos.z))
                .is_some_and(|&until| until > self.clock) =>
            {
                return None;
            }
            None => self.detect(cells, kind, fluid.film, pos)?,
        };
        let reservoir = self.list.get_mut(&id).expect("listed");
        let drawn = (want as i64).min(reservoir.total()).max(0);
        reservoir.balance -= drawn;
        Some(drawn as u64)
    }

    /// Something other than levelling may have changed the voxel at `pos`:
    /// its column, if a reservoir has one there, is read again before the
    /// reservoir is next levelled.
    pub fn touch(&mut self, pos: IVec3) {
        let Some(&id) = self.owners.get(&IVec2::new(pos.x, pos.z)) else {
            return;
        };
        let reservoir = self.list.get_mut(&id).expect("listed");
        if let Some(column) = reservoir.columns.get_mut(&(pos.x, pos.z))
            && !column.stale
        {
            column.stale = true;
            reservoir.dirty.push((pos.x, pos.z));
        }
    }

    /// A block was put at `pos`, which may cut a reservoir in two: every
    /// reservoir with a column there or beside it, reaching its height, is
    /// dropped, and forms again (as one or two) the next time it's needed.
    pub fn walled(&mut self, pos: IVec3) {
        let here = IVec2::new(pos.x, pos.z);
        for spot in std::iter::once(here).chain(SIDES.map(|side| here + side)) {
            let Some(&id) = self.owners.get(&spot) else {
                continue;
            };
            let reservoir = self.list.get_mut(&id).expect("listed");
            let column = &reservoir.columns[&(spot.x, spot.y)];
            if (column.bottom - 1..=column.top + 1).contains(&pos.y) {
                reservoir.dropped = true;
            }
        }
    }

    /// Chunk `coord` is unloading: drop every reservoir with a column in it
    /// or against it.
    pub fn unloading(&mut self, coord: IVec3) {
        let size = CHUNK_SIZE as i32;
        let (min, max) = (coord * size, coord * size + (size - 1));
        for x in min.x - 1..=max.x + 1 {
            for z in min.z - 1..=max.z + 1 {
                let Some(&id) = self.owners.get(&IVec2::new(x, z)) else {
                    continue;
                };
                let reservoir = self.list.get_mut(&id).expect("listed");
                let column = &reservoir.columns[&(x, z)];
                if column.bottom <= max.y + 1 && column.top >= min.y - 1 {
                    reservoir.dropped = true;
                }
            }
        }
    }

    /// Pays out and forgets the reservoirs marked to be dropped, and no
    /// others, as when chunks unload without the rest being saved.
    pub fn drop_marked(&mut self, cells: &mut impl Cells, wake: &mut Vec<IVec3>) {
        let ids: Vec<u32> = self
            .list
            .iter()
            .filter(|(_, r)| r.dropped)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.drop_reservoir(cells, id, wake);
        }
    }

    /// Joins the cells that took fluid to reservoirs beside them, then
    /// levels the reservoirs of `kind`, writing the world through `cells`,
    /// and drops those that need it. With `flush`, levels them all, so
    /// every balance is paid out. Pushes the cells to wake onto `wake`. Only call it when no
    /// step of `kind` is under way, since it writes the cells a step reads.
    pub fn settle(
        &mut self,
        cells: &mut impl Cells,
        kind: BlockState,
        flush: bool,
        wake: &mut Vec<IVec3>,
    ) {
        self.join(&*cells, kind);
        self.merge_touching();
        let ids: Vec<u32> = self
            .list
            .iter()
            .filter(|(_, r)| r.kind == kind)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let reservoir = self.list.get_mut(&id).expect("listed");
            if reservoir.dropped || self.max_columns == 0 {
                self.drop_reservoir(cells, id, wake);
                continue;
            }
            refresh(cells, reservoir, wake);
            let due = reservoir.balance.abs() >= LAZY * reservoir.columns.len() as i64;
            if reservoir.level.is_none() || due || (flush && reservoir.balance != 0) {
                level(cells, reservoir, flush, wake);
                self.grow_from_rim(&*cells, id);
            }
            // The level dropping below where parts of it joined, such as
            // over a sill, leaves two bodies: drop it, and each forms again
            // on its own.
            let reservoir = self.list.get_mut(&id).expect("listed");
            if std::mem::take(&mut reservoir.emptied) && !reservoir.is_one_body() {
                self.drop_reservoir(cells, id, wake);
                continue;
            }
            // Columns that emptied, on ground the level is below, stay: the
            // level can rise over them again. One spread thinner than a film
            // goes, and its last flows as before.
            let reservoir = &self.list[&id];
            if reservoir.total() < reservoir.film as i64 * reservoir.columns.len() as i64 {
                self.drop_reservoir(cells, id, wake);
            }
        }
    }

    /// Joins each cell that took fluid this step to a reservoir of its kind
    /// with a column beside it at its height, as a column of its own, or
    /// marks its own column to be read again.
    fn join(&mut self, cells: &impl CellView, kind: BlockState) {
        let mut joining = std::mem::take(&mut self.joining);
        self.joining = joining.extract_if(.., |&mut (k, _)| k != kind).collect();
        joining.sort_unstable_by_key(|(_, pos)| (pos.y, pos.x, pos.z));
        joining.dedup();
        for (_, pos) in joining {
            if self.owners.contains_key(&IVec2::new(pos.x, pos.z)) {
                self.touch(pos);
            } else if let Some(id) = self.beside(kind, pos) {
                self.adopt(cells, id, kind, pos);
            }
        }
    }

    /// A reservoir of `kind` with a column beside `pos` whose run reaches
    /// its height (or the cell above its top), with room for another column.
    fn beside(&self, kind: BlockState, pos: IVec3) -> Option<u32> {
        let here = IVec2::new(pos.x, pos.z);
        SIDES.iter().find_map(|&side| {
            let id = *self.owners.get(&(here + side))?;
            let reservoir = &self.list[&id];
            let column = reservoir.columns.get(&(pos.x + side.x, pos.z + side.y))?;
            (reservoir.kind == kind
                && !reservoir.dropped
                && reservoir.columns.len() < self.max_columns
                && (column.bottom..=column.top + 1).contains(&pos.y))
            .then_some(id)
        })
    }

    /// Adds the columns of its fluid beside reservoir `id`'s rim at its
    /// surface, up to [`RIM_GROWTH`] at a time: when its level drops, the
    /// fluid around it joins it rather than standing above it until flow
    /// brings it down, so a lake bigger than a reservoir doesn't show a
    /// step where the reservoir ends.
    fn grow_from_rim(&mut self, cells: &impl CellView, id: u32) {
        let reservoir = &self.list[&id];
        let kind = reservoir.kind;
        let mut budget = self
            .max_columns
            .saturating_sub(reservoir.columns.len())
            .min(RIM_GROWTH);
        // Outward from the rim, a column at a time, through the fluid at
        // the height of the column it's reached from.
        let mut frontier: VecDeque<(IVec2, i32)> = reservoir
            .columns
            .iter()
            .filter(|(_, column)| column.rim && column.amount > 0)
            .map(|(&(x, z), column)| (IVec2::new(x, z), column.top))
            .collect();
        while let Some((at, top)) = frontier.pop_front() {
            for side in SIDES {
                if budget == 0 {
                    return;
                }
                let next = at + side;
                if let Some(&other) = self.owners.get(&next) {
                    let pos = IVec3::new(next.x, top, next.y);
                    if other != id && self.member(pos, kind) == Some(other) {
                        self.touching.push((id.min(other), id.max(other)));
                    }
                    continue;
                }
                let Some(pos) = (top - 1..=top)
                    .rev()
                    .map(|y| IVec3::new(next.x, y, next.y))
                    .find(|&pos| amount_of(kind, cells.cell(pos)).is_some())
                else {
                    continue;
                };
                self.adopt(cells, id, kind, pos);
                if let Some(column) = self.list[&id].columns.get(&(next.x, next.y)) {
                    budget -= 1;
                    frontier.push_back((next, column.top));
                }
            }
        }
    }

    /// Makes the run of `kind` through `pos` a column of reservoir `id`, if
    /// it rests on something and no reservoir has its column yet.
    fn adopt(&mut self, cells: &impl CellView, id: u32, kind: BlockState, pos: IVec3) {
        if self.owners.contains_key(&IVec2::new(pos.x, pos.z)) {
            return;
        }
        let Some(column) = run_at(cells, kind, pos) else {
            return;
        };
        let reservoir = self.list.get_mut(&id).expect("listed");
        reservoir.held += column.amount;
        reservoir.columns.insert((pos.x, pos.z), column);
        reservoir.dirty.push((pos.x, pos.z));
        self.owners.insert(IVec2::new(pos.x, pos.z), id);
        mark_rims(reservoir, IVec2::new(pos.x, pos.z));
    }

    /// Pays reservoir `id`'s balance out in full and forgets it.
    fn drop_reservoir(&mut self, cells: &mut impl Cells, id: u32, wake: &mut Vec<IVec3>) {
        let mut reservoir = self.list.remove(&id).expect("listed");
        for &(x, z) in reservoir.columns.keys() {
            self.owners.remove(&IVec2::new(x, z));
        }
        // A column found changed when it's written is read again and
        // written the next time round.
        for _ in 0..3 {
            refresh(cells, &mut reservoir, wake);
            if reservoir.balance == 0 {
                break;
            }
            level(cells, &mut reservoir, true, wake);
        }
        if reservoir.balance != 0 {
            *self.stranded.entry(reservoir.kind).or_default() += reservoir.balance;
            warn!(
                "a reservoir of {:?} was dropped with {} units it couldn't put back",
                reservoir.kind, reservoir.balance
            );
        }
        for (&(x, z), column) in &reservoir.columns {
            wake.extend(column.rim_cells(x, z));
        }
    }

    /// Joins the cells a step is about to take fluid out of to a reservoir
    /// beside them, or looks for a new one from them (one at most), before
    /// the step moves anything, so what it takes is pooled and the body
    /// stays flat.
    pub fn find_new(
        &mut self,
        cells: &impl CellView,
        fluid: &Fluid,
        changes: &[(IVec3, (Option<Cell>, Cell))],
    ) {
        if self.max_columns == 0 {
            self.joining.clear();
            return;
        }
        let clock = self.clock;
        self.tried.retain(|_, until| *until > clock);
        let losing = changes
            .iter()
            .filter_map(|&(pos, (was, now))| match (was, now) {
                (Some(Cell::Fluid(kind, before, _)), Cell::Fluid(k, after, _))
                    if k == kind && after < before =>
                {
                    Some((kind, pos))
                }
                (Some(Cell::Fluid(kind, _, _)), Cell::Open(_)) => Some((kind, pos)),
                _ => None,
            });
        let mut detected = false;
        for (kind, pos) in losing {
            if self.member(pos, kind).is_some() {
                continue;
            }
            let column = IVec2::new(pos.x, pos.z);
            if self.owners.contains_key(&column) {
                continue;
            }
            // Fluid beside a reservoir joins it rather than starting another.
            if let Some(id) = self.beside(kind, pos) {
                self.adopt(cells, id, kind, pos);
                continue;
            }
            if detected || self.tried.contains_key(&column) {
                continue;
            }
            // One walk a step, whether it finds one or not.
            self.detect(cells, kind, fluid.film, pos);
            detected = true;
        }
    }

    /// Advances the clock that retries go by.
    pub fn advance(&mut self, seconds: f32) {
        self.clock += seconds;
    }

    /// Finds the reservoir the cell at `seed` is in, if it makes one, and
    /// adds it. Columns it looked at without making one wait a while.
    fn detect(
        &mut self,
        cells: &impl CellView,
        kind: BlockState,
        film: u16,
        seed: IVec3,
    ) -> Option<u32> {
        let start = IVec2::new(seed.x, seed.z);
        let mut columns = BTreeMap::new();
        let mut seen = HashSet::from([start]);
        let mut queue = VecDeque::new();
        if let Some(column) = run_at(cells, kind, seed) {
            columns.insert((seed.x, seed.z), column);
            queue.push_back((start, column));
        }
        while let Some((at, column)) = queue.pop_front() {
            for side in SIDES {
                if columns.len() >= self.max_columns.min(START_COLUMNS) {
                    break;
                }
                let next = at + side;
                if self.owners.contains_key(&next) || seen.contains(&next) {
                    continue;
                }
                // A run beside this one, overlapping it.
                let found = (column.bottom..=column.top).rev().find(|&y| {
                    amount_of(kind, cells.cell(IVec3::new(next.x, y, next.y))).is_some()
                });
                let Some(y) = found else {
                    continue;
                };
                seen.insert(next);
                if let Some(run) = run_at(cells, kind, IVec3::new(next.x, y, next.y)) {
                    columns.insert((next.x, next.y), run);
                    queue.push_back((next, run));
                }
            }
        }
        let held: i64 = columns.values().map(|c: &Column| c.amount).sum();
        if columns.len() < MIN_COLUMNS || held < film as i64 * columns.len() as i64 * 2 {
            let until = self.clock + RETRY_SECS;
            for column in seen {
                self.tried.insert(column, until);
            }
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut reservoir = Reservoir {
            kind,
            columns,
            balance: 0,
            held,
            level: None,
            dirty: Vec::new(),
            dropped: false,
            emptied: false,
            film,
        };
        let keys: Vec<IVec2> = reservoir
            .columns
            .keys()
            .map(|&(x, z)| IVec2::new(x, z))
            .collect();
        for &key in &keys {
            self.owners.insert(key, id);
            let rim = is_rim(&reservoir.columns, key);
            reservoir
                .columns
                .get_mut(&(key.x, key.y))
                .expect("listed")
                .rim = rim;
        }
        self.list.insert(id, reservoir);
        Some(id)
    }
}

/// Works out whether the column at `at` and those beside it are on the
/// reservoir's rim, after the column at `at` joined.
fn mark_rims(reservoir: &mut Reservoir, at: IVec2) {
    for spot in std::iter::once(at).chain(SIDES.map(|side| at + side)) {
        let rim = is_rim(&reservoir.columns, spot);
        if let Some(column) = reservoir.columns.get_mut(&(spot.x, spot.y)) {
            column.rim = rim;
        }
    }
}

/// Reads the columns that changed again, and fills them to the
/// reservoir's level from its balance. Pushes the cells along its rim that
/// changed onto `wake`.
fn refresh(cells: &mut impl Cells, reservoir: &mut Reservoir, wake: &mut Vec<IVec3>) {
    let kind = reservoir.kind;
    let mut dirty = std::mem::take(&mut reservoir.dirty);
    dirty.sort_unstable();
    dirty.dedup();
    for (x, z) in dirty {
        let Some(column) = reservoir.columns.get_mut(&(x, z)) else {
            continue;
        };
        if column.stale {
            // What changed from outside is the world's, not the balance's.
            let before = column.amount;
            read(&*cells, kind, x, z, column);
            reservoir.held += column.amount - before;
        }
        let Some(level) = reservoir.level else {
            continue;
        };
        let want = column.at_level(level);
        if want != column.amount {
            let before = column.amount;
            if !write(cells, kind, (x, z), column, want) {
                column.stale = true;
                reservoir.dirty.push((x, z));
                continue;
            }
            let moved = column.amount - before;
            reservoir.emptied |= before > 0 && column.amount == 0;
            reservoir.held += moved;
            reservoir.balance -= moved;
            wake.extend(column.rim_cells(x, z));
        }
    }
}

/// Shares everything a reservoir holds, with its balance, over its
/// columns so its surface is flat, and writes the columns that are a
/// [`LAZY`] or more off the new level (or that empty or start to fill);
/// what the others are off by stays in the balance. With `flush`, writes
/// every column, and the units that don't divide evenly go one a column,
/// so only what the columns have no room for stays in the balance. Pushes
/// the cells along its rim that changed onto `wake`.
fn level(cells: &mut impl Cells, reservoir: &mut Reservoir, flush: bool, wake: &mut Vec<IVec3>) {
    let kind = reservoir.kind;
    let total = reservoir.total();
    if total < 0 {
        // Nothing takes more than a reservoir holds, so this can't happen.
        return;
    }
    // The highest level whose columns hold no more than the total, looked
    // for in a flat list, which is much quicker to go over many times.
    let spans: Vec<(i64, i64)> = reservoir
        .columns
        .values()
        .map(|c| (c.bottom as i64 * FULL, c.capacity()))
        .collect();
    let held_at = |level: i64| -> i64 {
        spans
            .iter()
            .map(|&(bottom, capacity)| (level - bottom).clamp(0, capacity))
            .sum()
    };
    let mut low = spans.iter().map(|&(bottom, _)| bottom).min().unwrap_or(0);
    let mut high = spans
        .iter()
        .map(|&(bottom, capacity)| bottom + capacity)
        .max()
        .unwrap_or(0);
    while low < high {
        let mid = low + (high - low + 1) / 2;
        if held_at(mid) <= total {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let level = low;
    let mut extra = if flush { total - held_at(level) } else { 0 };
    reservoir.level = Some(level);
    for (&(x, z), column) in reservoir.columns.iter_mut() {
        let mut want = column.at_level(level);
        // The units left over go to columns the level is inside, so a
        // column it's below doesn't get a unit.
        if extra > 0 && column.at_level(level + 1) > want {
            want += 1;
            extra -= 1;
        }
        let off = (want - column.amount).abs();
        if off == 0 || !(flush || off >= LAZY || want == 0 || column.amount == 0) {
            continue;
        }
        let before = column.amount;
        if !write(cells, kind, (x, z), column, want) {
            column.stale = true;
            reservoir.dirty.push((x, z));
            continue;
        }
        reservoir.emptied |= before > 0 && column.amount == 0;
        reservoir.held += column.amount - before;
        wake.extend(column.rim_cells(x, z));
    }
    reservoir.balance = total - reservoir.held;
}

/// Writes `want` into a column, filling it from the bottom up, and sets its
/// amount and top to what it holds after. Returns false, writing nothing,
/// if the cells aren't what the column remembers.
fn write(
    cells: &mut impl Cells,
    kind: BlockState,
    (x, z): (i32, i32),
    column: &mut Column,
    want: i64,
) -> bool {
    let at = |y: i32| IVec3::new(x, y, z);
    let old_top = column.top;
    let new_top = column.bottom + ((want + FULL - 1) / FULL) as i32 - 1;
    // Cells under both tops are full before and after, so only the cells
    // from the lower top up change.
    let from = if column.even {
        column.bottom.max(old_top.min(new_top))
    } else {
        column.bottom
    };
    let below = (from - column.bottom) as i64 * FULL;
    // First check the cells are as the column remembers them: something
    // other than flow may have changed them since, which it hears of a
    // tick later. If they aren't, write nothing: it's read again first.
    let mut found = below;
    for y in from..=old_top.max(new_top) {
        let here = cells.cell(at(y));
        match here {
            Some(Cell::Fluid(k, amount, FULL_FLUID)) if k == kind && y <= old_top => {
                found += amount as i64
            }
            Some(Cell::Open(FULL_FLUID)) if y > old_top => {}
            _ => return false,
        }
    }
    if found != column.amount {
        return false;
    }
    let mut amount = below;
    for y in from..=old_top.max(new_top) {
        let pos = at(y);
        let here = cells.cell(pos);
        let cell = (want - (y - column.bottom) as i64 * FULL).clamp(0, FULL) as u16;
        let new = if cell == 0 {
            Cell::Open(FULL_FLUID)
        } else {
            Cell::Fluid(kind, cell, FULL_FLUID)
        };
        if here != Some(new) {
            cells.set(pos, new);
        }
        amount += cell as i64;
    }
    column.amount = amount;
    column.even = true;
    column.top = column.bottom + ((amount + FULL - 1) / FULL) as i32 - 1;
    column.room_above = matches!(cells.cell(at(column.top + 1)), Some(Cell::Open(FULL_FLUID)));
    true
}

#[cfg(test)]
mod tests;
