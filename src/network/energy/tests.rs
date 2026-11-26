//! The energy solver: [`balance`] on its own, and whole networks of the
//! base game's generator, wires, lamps and batteries over chunks spawned by
//! hand.

use std::time::Duration;

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::{
    block_entity::{BlockEntities, BlockEntityPlugin},
    connections::ConnectionsPlugin,
    content::{base, block},
    machine::MachineCorePlugin,
    network::{NetworkClock, NetworkCorePlugin},
    testing::{chunk_app, insert_chunk, unload_chunk},
    world::{Chunk, Voxels},
};

fn producer(offer: u64) -> Member {
    Member {
        role: Role::Producer,
        offer,
        demand: 0,
        need: 0,
    }
}

/// A consumer that needs all it asks for.
fn consumer(demand: u64) -> Member {
    consumer_needing(demand, demand)
}

fn consumer_needing(demand: u64, need: u64) -> Member {
    Member {
        role: Role::Consumer,
        offer: 0,
        demand,
        need,
    }
}

fn storage(offer: u64, demand: u64) -> Member {
    Member {
        role: Role::Storage,
        offer,
        demand,
        need: 0,
    }
}

#[test]
fn sharing_out_is_exact_and_within_caps() {
    assert_eq!(share_out(10, &[5, 5]), [5, 5]);
    assert_eq!(share_out(3, &[10, 20]), [1, 2]);
    assert_eq!(share_out(5, &[1, 1, 1, 1, 1, 1]), [1, 1, 1, 1, 1, 0]);
    assert_eq!(share_out(0, &[3, 4]), [0, 0]);
    assert_eq!(share_out(7, &[0, 7]), [0, 7]);
    let mut seed = 1u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed >> 33
    };
    for _ in 0..2000 {
        let caps: Vec<u64> = (0..1 + next() % 8).map(|_| next() % 50).collect();
        let sum: u64 = caps.iter().sum();
        let total = if sum == 0 { 0 } else { next() % (sum + 1) };
        let out = share_out(total, &caps);
        assert_eq!(out.iter().sum::<u64>(), total, "{total} {caps:?}");
        assert!(
            out.iter().zip(&caps).all(|(o, c)| o <= c),
            "{out:?} {caps:?}"
        );
    }
}

/// More supply than demand: every consumer gets what it asked for, and
/// producers give only that.
#[test]
fn supply_over_demand_serves_everyone() {
    let flow = balance(&[producer(100), consumer(10), consumer(30)], 1000);
    assert_eq!(flow.taken, [0, 10, 30]);
    assert_eq!(flow.given, [40, 0, 0]);
    assert_eq!((flow.produced, flow.consumed), (40, 40));
}

/// Less supply than demand: consumers are served whole, the smallest
/// first, until it runs out.
#[test]
fn demand_over_supply_serves_the_smallest_first() {
    let flow = balance(&[producer(30), consumer(40), consumer(20)], 1000);
    assert_eq!(flow.taken, [0, 10, 20]);
    assert_eq!(flow.consumed, 30);
    // Many small ones and a big one, short: the small ones run, whole.
    let mut members = vec![producer(20), consumer(25)];
    members.extend([consumer(1); 14]);
    let flow = balance(&members, 1000);
    assert_eq!(flow.taken[2..], [1; 14]);
    assert_eq!(flow.taken[1], 6);
    assert_eq!(serve_smallest_first(7, &[5, 3, 3]), [1, 3, 3]);
    assert_eq!(serve_smallest_first(0, &[5, 3]), [0, 0]);
    // Two producers give in proportion to what they offered.
    let flow = balance(&[producer(10), producer(30), consumer(20)], 1000);
    assert_eq!(flow.given, [5, 15, 0]);
}

/// What consumers need to run is served before buffers are topped up:
/// two furnaces each running a 25 J-a-tick recipe, each asking for 50 to
/// fill its buffer, on 50 a tick both run, rather than the first filling
/// up while the second waits. What's left over tops up in proportion.
#[test]
fn needs_come_before_top_ups() {
    let furnace = consumer_needing(50, 25);
    let flow = balance(&[producer(50), furnace, furnace], 1000);
    assert_eq!(flow.taken, [0, 25, 25]);
    let flow = balance(&[producer(62), furnace, consumer_needing(30, 10)], 1000);
    assert_eq!(flow.taken, [0, 25 + 15, 10 + 12]);
    // Short of even the needs: served whole, the smallest first.
    let flow = balance(&[producer(30), furnace, consumer_needing(50, 10)], 1000);
    assert_eq!(flow.taken, [0, 20, 10]);
    // Needing nothing, consumers share what there is by their room.
    let idle = consumer_needing(40, 0);
    let flow = balance(&[producer(30), idle, consumer_needing(20, 0)], 1000);
    assert_eq!(flow.taken, [0, 20, 10]);
}

/// Surplus charges storage; a shortage draws on it; the limit caps both.
#[test]
fn storage_takes_surplus_and_covers_shortage() {
    let flow = balance(&[producer(100), consumer(30), storage(500, 50)], 1000);
    assert_eq!(flow.taken, [0, 30, 50]);
    assert_eq!(flow.given, [80, 0, 0]);
    assert_eq!((flow.charged, flow.discharged), (50, 0));

    let flow = balance(&[producer(10), consumer(30), storage(500, 50)], 1000);
    assert_eq!(flow.taken, [0, 30, 0]);
    assert_eq!(flow.given, [10, 0, 20]);
    assert_eq!((flow.charged, flow.discharged), (0, 20));

    // A throughput of 25: consumers first, then what's left charges.
    let flow = balance(&[producer(100), consumer(20), storage(0, 50)], 25);
    assert_eq!(flow.taken, [0, 20, 5]);
    assert_eq!(flow.given, [25, 0, 0]);
    let flow = balance(&[producer(100), consumer(40), consumer(10)], 25);
    assert_eq!(flow.taken, [0, 15, 10]);
}

/// Whatever the members, everything given is taken, nobody gives more
/// than it offered or takes more than it asked, and storage doesn't
/// both charge and discharge.
#[test]
fn balancing_never_makes_or_loses_energy() {
    let mut seed = 7u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed >> 33
    };
    for _ in 0..5000 {
        let members: Vec<Member> = (0..next() % 7)
            .map(|_| match next() % 3 {
                0 => producer(next() % 100),
                1 => {
                    let demand = next() % 100;
                    consumer_needing(demand, next() % (demand + 1))
                }
                _ => storage(next() % 100, next() % 100),
            })
            .collect();
        let limit = next() % 300;
        let flow = balance(&members, limit);
        let given: u64 = flow.given.iter().sum();
        let taken: u64 = flow.taken.iter().sum();
        assert_eq!(given, taken);
        assert_eq!(given, flow.produced + flow.discharged);
        assert_eq!(taken, flow.consumed + flow.charged);
        assert!(given <= limit);
        assert!(flow.charged == 0 || flow.discharged == 0);
        let needed: u64 = members
            .iter()
            .filter(|m| m.role == Role::Consumer)
            .map(|m| m.need)
            .sum();
        for (i, m) in members.iter().enumerate() {
            assert!(flow.given[i] <= m.offer && flow.taken[i] <= m.demand);
            // Every need is met before any buffer is topped up.
            if m.role == Role::Consumer && flow.consumed >= needed {
                assert!(flow.taken[i] >= m.need, "{members:?} {flow:?}");
            }
        }
    }
}

// Whole networks.

fn app() -> App {
    let mut app = chunk_app(base().clone(), Duration::from_millis(10));
    app.add_plugins((
        ConnectionsPlugin,
        NetworkCorePlugin,
        BlockEntityPlugin,
        MachineCorePlugin,
        EnergyPlugin,
    ));
    insert_chunk(&mut app, IVec3::ZERO, Chunk::default());
    app.update();
    app
}

fn set(app: &mut App, voxels: &[(IVec3, BlockState)]) {
    let voxels = voxels.to_vec();
    app.world_mut()
        .run_system_once(move |mut world: Voxels| {
            for &(pos, state) in &voxels {
                assert!(world.set(pos, state), "{pos} isn't loaded");
            }
        })
        .unwrap();
    app.update();
    app.update();
}

fn line(from: IVec3, to: IVec3, state: BlockState) -> Vec<(IVec3, BlockState)> {
    let step = (to - from).signum();
    let mut pos = from;
    let mut out = vec![(pos, state)];
    while pos != to {
        pos += step;
        out.push((pos, state));
    }
    out
}

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}

/// Runs frames until `n` more network ticks have run.
fn ticks(app: &mut App, n: u64) {
    let until = app.world().resource::<NetworkClock>().ticks + n;
    while app.world().resource::<NetworkClock>().ticks < until {
        app.update();
    }
    // Let the active states reach the voxels.
    app.update();
}

fn buffer(app: &App, pos: IVec3) -> EnergyBuffer {
    let entity = app
        .world()
        .resource::<BlockEntities>()
        .get(pos)
        .unwrap_or_else(|| panic!("no block entity at {pos}"));
    *app.world().get::<EnergyBuffer>(entity).unwrap()
}

fn charge(app: &mut App, pos: IVec3, stored: u64) {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world_mut()
        .get_mut::<EnergyBuffer>(entity)
        .unwrap()
        .stored = stored;
}

fn lit(app: &App, pos: IVec3) -> bool {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world().get::<MachineActive>(entity).unwrap().0
}

/// Every buffer's energy, generated energy taken away and used energy
/// added back: what should never change.
fn balance_sheet(app: &mut App) -> i128 {
    let stored: u64 = app
        .world_mut()
        .query::<&EnergyBuffer>()
        .iter(app.world())
        .map(|b| b.stored)
        .sum();
    let report = app.world().resource::<EnergyReport>();
    stored as i128 - report.generated as i128 + report.used as i128
}

fn wire() -> BlockState {
    block("wire")
}

fn lamp() -> BlockState {
    block("powered_lamp")
}

fn report(app: &App, pos: IVec3) -> NetworkEnergy {
    let id = app.world().resource::<Networks>().at(pos).unwrap();
    app.world().resource::<EnergyReport>().networks[&id].clone()
}

const GENERATOR: IVec3 = IVec3::new(2, 4, 4);
const BATTERY: IVec3 = IVec3::new(4, 4, 5);

/// A generator (facing north, so its right is east), a copper wire east
/// along z = 4 to x = `end`, lamps above it at `lamps`, and a battery
/// (facing north) joined to it at x = 4.
fn build(app: &mut App, end: i32, lamps: &[i32]) {
    let mut voxels = vec![
        (GENERATOR, block("creative_generator")),
        (BATTERY, block("battery")),
    ];
    voxels.extend(line(v(3, 4, 4), v(end, 4, 4), wire()));
    voxels.extend(lamps.iter().map(|&x| (v(x, 5, 4), lamp())));
    set(app, &voxels);
}

/// A generator over a copper wire lights every lamp and charges the
/// battery with the rest of what the wire carries.
#[test]
fn a_generator_lights_lamps_and_charges_a_battery() {
    let mut app = app();
    build(&mut app, 10, &[5, 8]);
    let before = balance_sheet(&mut app);
    ticks(&mut app, 40);
    assert!(lit(&app, v(5, 5, 4)) && lit(&app, v(8, 5, 4)));
    let state = app
        .world_mut()
        .run_system_once(|voxels: Voxels| voxels.get(v(5, 5, 4)).unwrap())
        .unwrap();
    assert_eq!(state, block_lit());
    let energy = report(&app, v(6, 4, 4));
    // Copper carries 1 kW: 40 W to the lamps, the rest to the battery.
    assert_eq!(energy.limit, 1000);
    assert_eq!(energy.produced(), 1000);
    assert_eq!(energy.consumed(), 40);
    assert!(
        buffer(&app, BATTERY).stored >= 1800,
        "{:?}",
        buffer(&app, BATTERY)
    );
    assert_eq!(energy.stored, buffer(&app, BATTERY).stored);
    assert_eq!(balance_sheet(&mut app), before);
}

fn block_lit() -> BlockState {
    base()
        .block_state("defaria:powered_lamp", &[("lit", "true")])
        .unwrap()
}

/// An iron wire carries 170 W, less than ten lamps' 200 W: the network
/// consumes exactly what the wire carries.
#[test]
fn a_thin_wire_limits_the_whole_network() {
    let mut app = app();
    let iron = base()
        .block_state("defaria:wire", &[("metal", "defaria:iron")])
        .unwrap();
    let mut voxels = vec![(GENERATOR, block("creative_generator"))];
    voxels.extend(line(v(3, 4, 4), v(12, 4, 4), wire()));
    voxels.push((v(7, 4, 4), iron));
    voxels.extend((3..13).map(|x| (v(x, 5, 4), lamp())));
    set(&mut app, &voxels);
    ticks(&mut app, 40);
    let energy = report(&app, v(3, 4, 4));
    assert_eq!(energy.limit, 170);
    assert_eq!(energy.consumed(), 170);
    // Short of power, the lamps it can light stay lit, tick after tick
    // (8 J a tick, 160 W), and the last ones wait, dark.
    for _ in 0..10 {
        ticks(&mut app, 1);
        for x in 3..11 {
            assert!(lit(&app, v(x, 5, 4)), "lamp {x}");
        }
        assert!(!lit(&app, v(12, 5, 4)));
    }
}

/// A lamp that holds enough for its tick isn't served first: with a
/// battery giving one lamp's tick, the empty lamp gets it and the other
/// runs on what it holds, whichever of them that is.
#[test]
fn a_machine_that_holds_its_tick_is_served_last() {
    for (full, empty) in [(v(5, 5, 4), v(8, 5, 4)), (v(8, 5, 4), v(5, 5, 4))] {
        let mut app = app();
        build(&mut app, 10, &[5, 8]);
        set(&mut app, &[(GENERATOR, BlockState::AIR)]);
        charge(&mut app, empty, 0);
        charge(&mut app, full, 10);
        charge(&mut app, BATTERY, 0);
        for tick in 0..5 {
            // A lamp's tick: 1 J.
            charge(&mut app, BATTERY, 1);
            ticks(&mut app, 1);
            assert!(lit(&app, full) && lit(&app, empty), "tick {tick}");
            assert_eq!(buffer(&app, empty).stored, 0);
            assert_eq!(buffer(&app, full).stored, 9 - tick);
        }
    }
}

/// With the generator gone, the battery lights the lamps until it runs
/// out, then they go dark.
#[test]
fn a_battery_discharges_into_lamps_then_runs_out() {
    let mut app = app();
    build(&mut app, 10, &[5, 8]);
    set(&mut app, &[(GENERATOR, BlockState::AIR)]);
    charge(&mut app, BATTERY, 100);
    let before = balance_sheet(&mut app);
    ticks(&mut app, 20);
    assert!(lit(&app, v(5, 5, 4)) && lit(&app, v(8, 5, 4)));
    // Two lamps at 20 W each, for a second.
    assert_eq!(buffer(&app, BATTERY).stored, 60);
    assert_eq!(report(&app, v(6, 4, 4)).consumed(), 40);
    ticks(&mut app, 40);
    assert_eq!(buffer(&app, BATTERY).stored, 0);
    assert!(!lit(&app, v(5, 5, 4)) && !lit(&app, v(8, 5, 4)));
    assert_eq!(balance_sheet(&mut app), before);
}

/// Breaking the wire leaves the far lamps unpowered and dark, while the
/// near one stays lit.
#[test]
fn a_split_leaves_one_half_unpowered() {
    let mut app = app();
    build(&mut app, 12, &[5, 9, 11]);
    ticks(&mut app, 10);
    assert!(lit(&app, v(11, 5, 4)));
    set(&mut app, &[(v(7, 4, 4), BlockState::AIR)]);
    ticks(&mut app, 20);
    assert!(lit(&app, v(5, 5, 4)));
    assert!(!lit(&app, v(9, 5, 4)) && !lit(&app, v(11, 5, 4)));
    assert_eq!(report(&app, v(10, 4, 4)).produced(), 0);
    // Mending it powers them again.
    set(&mut app, &[(v(7, 4, 4), wire())]);
    ticks(&mut app, 5);
    assert!(lit(&app, v(9, 5, 4)) && lit(&app, v(11, 5, 4)));
}

/// A network reaching into a chunk that unloads is frozen: its battery
/// keeps its charge, though its lamps could use it, until the chunk loads
/// again.
#[test]
fn a_frozen_network_keeps_its_batterys_charge() {
    let mut app = app();
    insert_chunk(&mut app, IVec3::X, Chunk::default());
    app.update();
    let mut voxels = vec![(BATTERY, block("battery"))];
    voxels.extend(line(v(3, 4, 4), v(20, 4, 4), wire()));
    voxels.extend([(v(6, 5, 4), lamp()), (v(18, 5, 4), lamp())]);
    set(&mut app, &voxels);
    charge(&mut app, BATTERY, 1000);
    ticks(&mut app, 2);
    let saved = unload_chunk(&mut app, IVec3::X);
    let frozen = buffer(&app, BATTERY).stored;
    assert!(
        app.world()
            .resource::<Networks>()
            .get(app.world().resource::<Networks>().at(v(5, 4, 4)).unwrap())
            .unwrap()
            .is_frozen()
    );
    ticks(&mut app, 60);
    assert_eq!(buffer(&app, BATTERY).stored, frozen);
    assert!(report(&app, v(5, 4, 4)).frozen);
    // Its lamp used up what it had, and is dark.
    assert!(!lit(&app, v(6, 5, 4)));

    insert_chunk(&mut app, IVec3::X, saved);
    app.update();
    ticks(&mut app, 20);
    assert!(buffer(&app, BATTERY).stored < frozen);
    assert!(lit(&app, v(6, 5, 4)));
}

/// Energy enters only at the generator and leaves only at lamps, tick
/// after tick, as the wire is broken and mended, merging and splitting the
/// network.
#[test]
fn energy_is_conserved_through_merges_and_splits() {
    let mut app = app();
    build(&mut app, 14, &[5, 7, 9, 11, 13]);
    // A second battery at the far end, facing north, on the wire.
    set(&mut app, &[(v(14, 4, 5), block("battery"))]);
    charge(&mut app, v(14, 4, 5), 500);
    let before = balance_sheet(&mut app);
    let mut seed = 3u64;
    for round in 0..40 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = 3 + (seed >> 33) as i32 % 12;
        let state = if round % 2 == 0 {
            BlockState::AIR
        } else {
            wire()
        };
        set(&mut app, &[(v(x, 4, 4), state)]);
        ticks(&mut app, 1 + (seed >> 40) % 5);
        assert_eq!(balance_sheet(&mut app), before, "round {round}");
    }
    assert!(app.world().resource::<EnergyReport>().generated > 0);
    assert!(app.world().resource::<EnergyReport>().used > 0);
}

/// A battery joined to two networks keeps to its rate over both: two
/// generators, each on its own wire to one of its sockets, charge it at
/// 1 kW in all, not 1 kW each.
#[test]
fn a_machine_on_two_networks_keeps_its_rates() {
    let mut app = app();
    let battery = v(8, 4, 8);
    let mut voxels = vec![
        (battery, block("battery")),
        (v(2, 4, 7), block("creative_generator")),
        (v(2, 4, 9), block("creative_generator")),
    ];
    voxels.extend(line(v(3, 4, 7), v(8, 4, 7), wire()));
    voxels.extend(line(v(3, 4, 9), v(8, 4, 9), wire()));
    set(&mut app, &voxels);
    let networks = app.world().resource::<Networks>();
    assert_ne!(networks.at(v(8, 4, 7)), networks.at(v(8, 4, 9)));
    ticks(&mut app, 1);
    let start = buffer(&app, battery).stored;
    ticks(&mut app, 20);
    assert_eq!(buffer(&app, battery).stored - start, 1000);
}

/// A network the solver hasn't run yet, as one just split off, still has
/// an energy line, with nothing moved; and one split off a powered line
/// says so from the frame it splits.
#[test]
fn fresh_networks_describe_nothing_moved() {
    assert_eq!(
        describe(&EnergyReport::default(), NetworkId(7)),
        "energy: stored 0 of 0 J, produced 0 J and consumed 0 J in the last second, throughput 0 W"
    );
    let mut app = app();
    build(&mut app, 12, &[5, 11]);
    ticks(&mut app, 10);
    set(&mut app, &[(v(8, 4, 4), BlockState::AIR)]);
    let far = app.world().resource::<Networks>().at(v(10, 4, 4)).unwrap();
    let line = describe(app.world().resource::<EnergyReport>(), far);
    assert!(line.starts_with("energy: stored 0 of 0 J"), "{line}");
}
