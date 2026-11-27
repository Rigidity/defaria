use super::*;
use crate::fluid::flow::{
    Simulation,
    tests::{Grid, WATER, fluids, water},
};

/// A pond `size` square and `depth` deep, full, with closed sides.
fn pond(size: i32, depth: i32) -> Grid {
    let mut grid = Grid::default();
    for x in 0..size {
        for z in 0..size {
            for y in 0..depth {
                grid.cells.insert(
                    IVec3::new(x, y, z),
                    Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID),
                );
            }
            // Room above it.
            grid.cells.insert(IVec3::new(x, depth, z), Cell::OPEN);
        }
    }
    grid
}

/// A 12 by 12 pond, 3 deep, with a channel 40 long and 2 wide, as deep,
/// opening from its side.
fn pond_and_channel() -> Grid {
    let mut grid = pond(12, 3);
    grid.open(IVec3::new(12, 0, 5), IVec3::new(51, 3, 6));
    grid
}

fn sim(reservoirs: bool) -> Simulation {
    let mut sim = Simulation::default();
    sim.reservoirs.max_columns = if reservoirs { 4096 } else { 0 };
    sim
}

/// What's in the grid and pooled in reservoirs.
fn held(grid: &Grid, sim: &Simulation) -> i64 {
    grid.total(WATER) as i64 + sim.reservoirs.pooled_total()
}

/// Ticks until nothing is active, up to `limit` ticks; how many it took.
fn run(
    grid: &mut Grid,
    sim: &mut Simulation,
    limit: usize,
    mut each: impl FnMut(&Grid, &Simulation),
) -> usize {
    for tick in 0..limit {
        sim.tick(grid, fluids, 0.02, usize::MAX);
        each(grid, sim);
        if sim.active() == 0 && sim.reservoirs.pooled_total() == 0 {
            return tick;
        }
    }
    limit
}

fn open_gate(grid: &Grid, sim: &mut Simulation) {
    for z in 5..=6 {
        for y in 0..3 {
            sim.wake_around(grid, IVec3::new(12, y, z));
        }
    }
}

#[test]
fn a_draining_pond_stays_flat_and_keeps_its_fluid() {
    let mut grid = pond_and_channel();
    let mut sim = sim(true);
    let start = held(&grid, &sim);
    open_gate(&grid, &mut sim);
    let mut formed = false;
    let mut pond_levels = Vec::new();
    let ticks = run(&mut grid, &mut sim, 4000, |grid, sim| {
        // The pond's surface stays flat as it drains.
        let top: Vec<u16> = (0..12)
            .flat_map(|x| (0..12).map(move |z| (x, z)))
            .map(|(x, z)| {
                (0..3)
                    .map(|y| grid.amount(IVec3::new(x, y, z)) as i64)
                    .sum::<i64>() as u16
            })
            .collect();
        // From the first levelling on: one is found at the end of a tick and
        // levelled in the next.
        if formed {
            pond_levels.push(*top.iter().max().unwrap() - *top.iter().min().unwrap());
        }
        formed |= !sim.reservoirs.is_empty();
        assert_eq!(held(grid, sim), start, "fluid made or lost");
    });
    assert!(formed, "no reservoir formed");
    // Once it's quiet, what was pooled is written out.
    assert!(ticks < 4000, "never settled");
    assert_eq!(sim.reservoirs.pooled_total(), 0);
    let worst = pond_levels.iter().max().unwrap();
    assert!(
        *worst <= 2,
        "the pond's surface was {worst} units out of flat"
    );
    assert_eq!(held(&grid, &sim), start);
}

#[test]
fn reservoirs_fill_a_channel_sooner() {
    let far = IVec3::new(51, 0, 5);
    let fill_time = |reservoirs: bool| {
        let mut grid = pond_and_channel();
        let mut sim = sim(reservoirs);
        open_gate(&grid, &mut sim);
        let mut filled = None;
        let mut tick = 0;
        run(&mut grid, &mut sim, 20000, |grid, _| {
            tick += 1;
            // Half a block deep at the far end of the channel.
            if filled.is_none() && grid.amount(far) >= FULL_FLUID / 2 {
                filled = Some(tick);
            }
        });
        filled.unwrap_or(usize::MAX)
    };
    let (with, without) = (fill_time(true), fill_time(false));
    assert!(
        with * 10 < without,
        "with reservoirs {with} ticks, without {without}"
    );
}

#[test]
fn a_pump_lowers_the_whole_surface() {
    let mut grid = pond(8, 2);
    let mut sim = sim(true);
    let start = grid.total(WATER);
    let drawn = sim
        .reservoirs
        .draw(&grid, &water(), IVec3::new(0, 0, 0), 8 * FULL_FLUID as u64);
    assert_eq!(drawn, Some(8 * FULL_FLUID as u64));
    let drawn = drawn.unwrap();
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    // 8 blocks off 64 surface voxels, an eighth of a block each.
    for x in 0..8 {
        for z in 0..8 {
            assert_eq!(grid.amount(IVec3::new(x, 1, z)), FULL_FLUID - 125);
            assert_eq!(grid.amount(IVec3::new(x, 0, z)), FULL_FLUID);
        }
    }
    assert_eq!(grid.total(WATER), start - drawn);
}

#[test]
fn a_pump_drains_a_pond_layer_by_layer() {
    let mut grid = pond(8, 3);
    let mut sim = sim(true);
    let start = grid.total(WATER);
    let mut drawn = 0;
    for _ in 0..400 {
        let got = sim
            .reservoirs
            .draw(&grid, &water(), IVec3::new(3, 0, 3), 1500)
            .unwrap_or(0);
        drawn += got;
        sim.tick(&mut grid, fluids, 0.02, usize::MAX);
        assert_eq!(held(&grid, &sim), (start - drawn) as i64);
        // The top layer empties before the one under it is drawn from.
        for x in 0..8 {
            for z in 0..8 {
                let column: Vec<u16> = (0..3).map(|y| grid.amount(IVec3::new(x, y, z))).collect();
                assert!(
                    column
                        .windows(2)
                        .all(|pair| pair[1] == 0 || pair[0] == FULL_FLUID),
                    "{column:?} at {x}, {z}"
                );
            }
        }
    }
    // All but a thin film, which flows as before.
    let film = water().film as u64;
    assert!(drawn >= start - 64 * film * 2, "drew {drawn} of {start}");
}

#[test]
fn a_member_changed_from_outside_is_levelled_with_the_rest() {
    let mut grid = pond(8, 2);
    let mut sim = sim(true);
    let start = grid.total(WATER);
    let drawn = sim
        .reservoirs
        .draw(&grid, &water(), IVec3::new(2, 1, 2), 500);
    assert_eq!(drawn, Some(500));
    let drawn = 500;
    assert_eq!(sim.reservoirs.len(), 1);
    // Something other than flow takes some out of a surface voxel.
    grid.set(IVec3::new(5, 1, 5), Cell::Fluid(WATER, 900, FULL_FLUID));
    sim.reservoirs.touch(IVec3::new(5, 1, 5));
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    // It's levelled with the rest, and nothing is made or lost.
    assert_eq!(held(&grid, &sim), (start - drawn - 100) as i64);
    let levels: HashSet<u16> = (0..8)
        .flat_map(|x| (0..8).map(move |z| (x, z)))
        .map(|(x, z)| grid.amount(IVec3::new(x, 1, z)))
        .collect();
    assert!(levels.len() <= 2, "{levels:?}");
}

#[test]
fn small_puddles_make_no_reservoir() {
    let mut grid = pond(3, 1);
    let mut sim = sim(true);
    let drawn = sim
        .reservoirs
        .draw(&grid, &water(), IVec3::new(1, 0, 1), 500);
    assert_eq!(drawn, None);
    assert!(sim.reservoirs.is_empty());
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    assert_eq!(grid.total(WATER), 9 * FULL_FLUID as u64);
}

#[test]
fn flushing_puts_everything_back_exactly() {
    let mut grid = pond(8, 2);
    let mut sim = sim(true);
    let start = grid.total(WATER);
    // Not a whole number of units a voxel.
    let drawn = sim
        .reservoirs
        .draw(&grid, &water(), IVec3::new(0, 0, 0), 333)
        .unwrap();
    sim.flush_reservoirs(&mut grid);
    assert_eq!(sim.reservoirs.pooled_total(), 0);
    assert_eq!(grid.total(WATER), start - drawn);
}

#[test]
fn random_scenes_keep_their_fluid_with_reservoirs_and_edits() {
    let mut seed = 0x5E5E_A41A_u64;
    let mut random = move |below: u32| (crate::perlin::splitmix64(&mut seed) % below as u64) as u32;
    for scene in 0..60 {
        let mut grid = Grid::default();
        for x in 0..16 {
            for y in 0..6 {
                for z in 0..16 {
                    if random(6) > 0 {
                        grid.cells.insert(IVec3::new(x, y, z), Cell::OPEN);
                    }
                }
            }
        }
        // A floor, and some full water to make ponds of.
        for x in 0..16 {
            for z in 0..16 {
                grid.cells.remove(&IVec3::new(x, -1, z));
                if random(2) == 0 && grid.cells.contains_key(&IVec3::new(x, 0, z)) {
                    grid.cells.insert(
                        IVec3::new(x, 0, z),
                        Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID),
                    );
                }
            }
        }
        let mut open: Vec<IVec3> = grid.cells.keys().copied().collect();
        open.sort_by_key(|p| (p.x, p.y, p.z));
        let mut sim = sim(true);
        for &pos in &open {
            sim.wake(&grid, pos);
        }
        for _ in 0..random(40) + 1 {
            let pos = open[random(open.len() as u32) as usize];
            if grid.cell(pos) == Some(Cell::OPEN) {
                let amount = random(FULL_FLUID as u32) as u16 + 1;
                grid.set(pos, Cell::Fluid(WATER, amount, FULL_FLUID));
                sim.wake_around(&grid, pos);
            }
        }
        let start = held(&grid, &sim);
        let seconds = [0.004, 1.0 / 60.0, 0.05][random(3) as usize];
        let budget = [7, 50, usize::MAX][random(3) as usize];
        let mut drawn = 0;
        for tick in 0..3000 {
            if random(20) == 0 {
                let pos = open[random(open.len() as u32) as usize];
                drawn += sim.reservoirs.draw(&grid, &water(), pos, 400).unwrap_or(0) as i64;
            }
            // Something other than flow changes a cell: a block is put in
            // it or taken out, or its fluid is set. Half the time the
            // reservoirs hear of it before they next level, half the time
            // they don't, as when it's changed between ticks.
            if random(30) == 0 {
                let pos = open[random(open.len() as u32) as usize];
                let cell = match random(3) {
                    0 => Cell::Closed,
                    1 => Cell::OPEN,
                    _ => Cell::Fluid(WATER, random(FULL_FLUID as u32) as u16 + 1, FULL_FLUID),
                };
                drawn += grid.amount(pos) as i64;
                grid.cells.insert(pos, cell);
                drawn -= grid.amount(pos) as i64;
                if random(2) == 0 {
                    sim.reservoirs.touch(pos);
                }
                sim.wake_around(&grid, pos);
            }
            sim.tick(&mut grid, fluids, seconds, budget);
            assert_eq!(
                held(&grid, &sim),
                start - drawn,
                "scene {scene}, tick {tick}"
            );
        }
        sim.reservoirs.max_columns = 0;
        sim.flush_reservoirs(&mut grid);
        assert_eq!(grid.total(WATER) as i64, start - drawn, "scene {scene}");
    }
}

/// A change from outside that a reservoir hasn't heard of yet isn't
/// written over when it levels: its column is read again first, and
/// nothing is made or lost.
#[test]
fn levelling_doesnt_write_over_a_change_it_hasnt_heard_of() {
    let mut grid = pond(12, 3);
    let mut sim = sim(true);
    let start = grid.total(WATER) as i64;
    sim.reservoirs
        .draw(&grid, &water(), IVec3::new(0, 0, 0), 100)
        .unwrap();
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    // A block put in a surface voxel, deleting its fluid, unheard of.
    let gone = grid.amount(IVec3::new(5, 2, 5)) as i64;
    grid.cells.insert(IVec3::new(5, 2, 5), Cell::Closed);
    // Enough drawn that the whole pond is levelled.
    sim.reservoirs
        .draw(&grid, &water(), IVec3::new(0, 0, 0), 3000)
        .unwrap();
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    sim.reservoirs.touch(IVec3::new(5, 2, 5));
    for _ in 0..50 {
        sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    }
    sim.flush_reservoirs(&mut grid);
    assert_eq!(held(&grid, &sim), start - 3100 - gone);
    assert_eq!(grid.cell(IVec3::new(5, 2, 5)), Some(Cell::Closed));
}

/// A reservoir bigger than it starts out takes in the fluid around its rim
/// as its level goes down, so the whole body goes down together.
#[test]
fn a_reservoir_grows_over_a_body_too_big_to_start_with() {
    let mut grid = pond(40, 3);
    let mut sim = sim(true);
    let start = held(&grid, &sim);
    let mut drawn = 0;
    for _ in 0..100 {
        drawn += sim
            .reservoirs
            .draw(&grid, &water(), IVec3::new(0, 0, 0), 20_000)
            .unwrap_or(0) as i64;
        sim.tick(&mut grid, fluids, 0.02, usize::MAX);
        assert_eq!(held(&grid, &sim), start - drawn);
    }
    assert!(drawn > 0);
    let columns: usize = sim.reservoirs.iter().map(|r| r.columns()).sum();
    assert_eq!(columns, 1600, "{} reservoirs", sim.reservoirs.len());
    let totals: Vec<i64> = (0..40)
        .flat_map(|x| (0..40).map(move |z| (x, z)))
        .map(|(x, z)| {
            (0..3)
                .map(|y| grid.amount(IVec3::new(x, y, z)) as i64)
                .sum()
        })
        .collect();
    let (low, high) = (totals.iter().min().unwrap(), totals.iter().max().unwrap());
    assert!(high - low <= 2 * LAZY, "columns from {low} to {high}");
}

/// A reservoir reaching into a chunk that unloads is dropped, and what it
/// pooled is put in the world first.
#[test]
fn a_reservoir_in_an_unloading_chunk_is_dropped_and_paid_out() {
    // Across the border of chunks x 0 and 1.
    let mut grid = pond(20, 2);
    let mut sim = sim(true);
    let start = grid.total(WATER) as i64;
    let drawn = sim
        .reservoirs
        .draw(&grid, &water(), IVec3::new(2, 1, 2), 500)
        .unwrap() as i64;
    assert_eq!(drawn, 500);
    // Nothing happens for a chunk it doesn't reach.
    sim.reservoirs.unloading(IVec3::new(3, 0, 0));
    sim.drop_marked_reservoirs(&mut grid);
    assert_eq!(sim.reservoirs.len(), 1);
    sim.reservoirs.unloading(IVec3::new(1, 0, 0));
    sim.drop_marked_reservoirs(&mut grid);
    assert!(sim.reservoirs.is_empty());
    assert_eq!(sim.reservoirs.pooled_total(), 0);
    assert_eq!(grid.total(WATER) as i64, start - drawn);
}

/// Two reservoirs that come to touch, at different levels, merge and end
/// level, as one body of water would.
#[test]
fn touching_reservoirs_end_level() {
    touching_reservoirs(4096, 1);
}

/// Two reservoirs that touch but are too big to merge still end level:
/// fluid moves between their balances.
#[test]
fn touching_reservoirs_too_big_to_merge_end_level() {
    touching_reservoirs(200, 2);
}

/// Two ponds, 3 deep and 1 deep, each a reservoir, then the wall between
/// them goes; they end with `left` reservoirs, level.
fn touching_reservoirs(max_columns: usize, left: usize) {
    // A pond 3 deep and one 1 deep, a wall between them.
    let mut grid = pond(12, 3);
    for x in 13..25 {
        for z in 0..12 {
            grid.cells.insert(
                IVec3::new(x, 0, z),
                Cell::Fluid(WATER, FULL_FLUID, FULL_FLUID),
            );
            for y in 1..4 {
                grid.cells.insert(IVec3::new(x, y, z), Cell::OPEN);
            }
        }
    }
    let mut sim = sim(true);
    sim.reservoirs.max_columns = max_columns;
    let start = held(&grid, &sim);
    for seed in [IVec3::new(0, 0, 0), IVec3::new(20, 0, 0)] {
        sim.reservoirs.draw(&grid, &water(), seed, 0).unwrap();
    }
    assert_eq!(sim.reservoirs.len(), 2);
    // The wall goes.
    for y in 0..4 {
        for z in 0..12 {
            grid.cells.insert(IVec3::new(12, y, z), Cell::OPEN);
            sim.wake_around(&grid, IVec3::new(12, y, z));
        }
    }
    let ticks = run(&mut grid, &mut sim, 4000, |grid, sim| {
        assert_eq!(held(grid, sim), start);
    });
    assert!(ticks < 4000, "never settled");
    assert_eq!(sim.reservoirs.len(), left);
    let totals: Vec<i64> = (0..25)
        .flat_map(|x| (0..12).map(move |z| (x, z)))
        .map(|(x, z)| {
            (0..4)
                .map(|y| grid.amount(IVec3::new(x, y, z)) as i64)
                .sum()
        })
        .collect();
    let (low, high) = (totals.iter().min().unwrap(), totals.iter().max().unwrap());
    assert!(high - low <= 2 * LAZY, "columns from {low} to {high}");
}

/// A wall built across a reservoir cuts it in two: the water on either
/// side keeps its own level, and drawing from one side leaves the other.
#[test]
fn a_wall_cuts_a_reservoir_in_two() {
    let mut grid = pond_and_channel();
    let mut sim = sim(true);
    open_gate(&grid, &mut sim);
    let ticks = run(&mut grid, &mut sim, 4000, |_, _| {});
    assert!(ticks < 4000, "never settled");
    assert_eq!(sim.reservoirs.len(), 1);
    // A wall where the gate was.
    for y in 0..4 {
        for z in 5..=6 {
            let pos = IVec3::new(12, y, z);
            grid.cells.insert(pos, Cell::Closed);
            sim.reservoirs.walled(pos);
            sim.reservoirs.touch(pos);
            sim.wake_around(&grid, pos);
        }
    }
    let channel = |grid: &Grid| -> u64 {
        (13..52)
            .flat_map(|x| (5..=6).flat_map(move |z| (0..4).map(move |y| IVec3::new(x, y, z))))
            .map(|pos| grid.amount(pos) as u64)
            .sum()
    };
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    let before = channel(&grid);
    for _ in 0..50 {
        sim.reservoirs
            .draw(&grid, &water(), IVec3::new(0, 0, 0), 5000);
        sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    }
    assert_eq!(channel(&grid), before, "drawn through the wall");
}

/// Paying out before a save reads a column again and writes it if it
/// was changed from outside unheard of, so nothing is left pooled.
#[test]
fn flushing_retries_a_column_changed_unheard_of() {
    let mut grid = pond(12, 3);
    let mut sim = sim(true);
    let start = grid.total(WATER) as i64;
    sim.reservoirs
        .draw(&grid, &water(), IVec3::new(0, 0, 0), 100)
        .unwrap();
    sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    let gone = grid.amount(IVec3::new(5, 2, 5)) as i64;
    grid.cells.insert(IVec3::new(5, 2, 5), Cell::Closed);
    sim.reservoirs
        .draw(&grid, &water(), IVec3::new(0, 0, 0), 3000)
        .unwrap();
    sim.flush_reservoirs(&mut grid);
    assert_eq!(sim.reservoirs.pooled_total(), 0);
    assert_eq!(grid.total(WATER) as i64, start - 3100 - gone);
}

/// A pump that found no reservoir looks again once the wait is over, even
/// on still fluid.
#[test]
fn a_pump_looks_again_after_a_while() {
    let mut grid = pond(3, 1);
    let mut sim = sim(true);
    assert_eq!(
        sim.reservoirs
            .draw(&grid, &water(), IVec3::new(1, 0, 1), 500),
        None
    );
    // The pond is made bigger, without flow moving anything.
    grid = pond(8, 1);
    for _ in 0..200 {
        sim.tick(&mut grid, fluids, 0.02, usize::MAX);
    }
    assert_eq!(
        sim.reservoirs
            .draw(&grid, &water(), IVec3::new(1, 0, 1), 500),
        Some(500)
    );
}
