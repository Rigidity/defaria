use std::{path::Path, time::Duration};

use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

use super::{solver::Step, *};
use crate::{
    block_entity::{BlockEntities, BlockEntityPlugin},
    connections::ConnectionsPlugin,
    content::{BlockState, Content, Direction, FULL_FLUID, PortRole, base},
    fluid::FluidPlugin,
    machine::{MachinePlugin, Tank, Tanks},
    network::{
        NetworkChanged, NetworkClock, NetworkCorePlugin, NetworkId, NetworkMap, NetworkState,
        Networks, Share, TICKS_PER_SECOND, energy::EnergyPlugin,
    },
    save::tests::temp_folder,
    streaming::{ChunkLoader, StreamingSettings},
    testing::{HOME, settle, test_content, world_app},
    world::Voxels,
};

const WATER: &str = "defaria:water";
const LAVA: &str = "defaria:lava";

fn water(amount: u64) -> FluidAmount {
    FluidAmount {
        fluid: WATER.into(),
        amount,
    }
}

fn tank(capacity: u64) -> Tank {
    Tank {
        name: "fluid".into(),
        fluid: None,
        amount: 0,
        capacity,
        accepts: Vec::new(),
    }
}

fn share(conduits: u32, capacity: f64) -> Share {
    Share { conduits, capacity }
}

/// Joining networks adds what they hold; one holding two fluids takes
/// neither in.
#[test]
fn networks_hold_one_fluid_and_merge_by_adding() {
    let mut a = FluidNet::default();
    a.add(WATER, 300);
    let mut b = FluidNet::default();
    b.add(WATER, 200);
    a.merge(b);
    assert_eq!(a.held, [water(500)]);
    assert!(a.takes(WATER) && !a.takes(LAVA));
    let mut c = FluidNet::default();
    c.add(LAVA, 100);
    a.merge(c);
    assert!(a.is_mixed());
    assert!(!a.takes(WATER) && !a.takes(LAVA), "mixed takes nothing");
    assert_eq!(a.total(), 600);
    assert_eq!(a.take(LAVA, 1000), 100);
    assert_eq!(a.fluid(), Some(WATER), "one fluid again once drained");
}

/// A split shares each fluid between the parts by volume, keeping every
/// unit; the conduits lost get nothing (theirs was spilled or saved).
#[test]
fn a_split_shares_fluid_by_volume() {
    let mut net = FluidNet::default();
    net.add(WATER, 1001);
    let parts = net.split(&[share(3, 1.5), share(1, 0.5)], share(2, 1.0));
    assert_eq!(parts[0].amount(WATER), 751);
    assert_eq!(parts[1].amount(WATER), 250);
    // A pipe's share, taken out exactly.
    let mut net = FluidNet::default();
    net.add(WATER, 1000);
    let shares = net.take_shares(&[0.5, 0.5], 1.0);
    assert_eq!(shares, [vec![water(250)], vec![water(250)]]);
    assert_eq!(net.amount(WATER), 500);
}

/// One step of a network with these ports and tanks, its throughput
/// `budget` a step.
fn step(
    net: &mut FluidNet,
    capacity: u64,
    budget: u64,
    ports: &[(PortRole, usize)],
    tanks: &mut [Tank],
    carries: &dyn Fn(&str) -> bool,
) {
    let mut step = Step {
        net,
        capacity,
        budget_in: budget,
        budget_out: budget,
        carries,
    };
    let mut refs: Vec<&mut Tank> = tanks.iter_mut().collect();
    step.run(ports, &mut refs);
}

/// A pump's tank fills the network, an outlet's drains it, and a store
/// takes what's left, then gives it back when an outlet wants it, all
/// within the pipes' throughput and room.
#[test]
fn the_solver_moves_fluid_by_role() {
    let any = |_: &str| true;
    let mut net = FluidNet::default();
    let mut tanks = [tank(1000), tank(16000)];
    tanks[0].fill(WATER, 1000);
    let ports = [(PortRole::Output, 0), (PortRole::Store, 1)];
    step(&mut net, 500, 100, &ports, &mut tanks, &any);
    assert_eq!(tanks[0].amount, 900, "throughput-limited");
    assert_eq!(tanks[1].amount, 100, "the store takes what's in the pipes");
    assert_eq!(net.total(), 0);
    // Room: a network of 50 takes only 50 in.
    step(&mut net, 50, 1000, &ports[..1], &mut tanks, &any);
    assert_eq!((tanks[0].amount, net.total()), (850, 50));
    // An outlet empties the network, then the store.
    let mut tanks = [tanks[1].clone(), tank(1000)];
    let ports = [(PortRole::Store, 0), (PortRole::Input, 1)];
    step(&mut net, 500, 120, &ports, &mut tanks, &any);
    assert_eq!(
        tanks[1].amount, 120,
        "the pipes' 50, then 70 from the store"
    );
    assert_eq!((tanks[0].amount, net.total()), (0, 30));
}

/// A network holding water takes no lava, and one whose pipes can't take
/// a fluid's heat doesn't take it in.
#[test]
fn a_network_refuses_a_second_fluid_and_heat() {
    let any = |_: &str| true;
    let mut net = FluidNet::default();
    net.add(WATER, 100);
    let mut tanks = [tank(1000)];
    tanks[0].fill(LAVA, 1000);
    let ports = [(PortRole::Output, 0)];
    step(&mut net, 5000, 1000, &ports, &mut tanks, &any);
    assert_eq!((tanks[0].amount, net.total()), (1000, 100));
    // Copper melts below lava's heat; iron doesn't.
    let content = base();
    let lava = solver::temperature(content, LAVA).unwrap();
    let melts = |metal: &str| {
        let pipe = content
            .block_state("defaria:pipe", &[("metal", &format!("defaria:{metal}"))])
            .unwrap();
        content.state(pipe).stat("max_temperature").unwrap()
    };
    assert!(melts("copper") < lava && melts("iron") > lava);
    assert!(solver::temperature(content, WATER).unwrap() < melts("copper"));
    let copper = |fluid: &str| solver::temperature(base(), fluid).unwrap() <= melts("copper");
    let mut empty = FluidNet::default();
    step(&mut empty, 5000, 1000, &ports, &mut tanks, &copper);
    assert_eq!((tanks[0].amount, empty.total()), (1000, 0));
}

/// A second's network ticks add up to the rate exactly, in fluid units.
#[test]
fn rates_add_up_over_ticks() {
    let total = |blocks_per_second: f64| -> u64 {
        let mut clock = NetworkClock::default();
        (0..TICKS_PER_SECOND)
            .map(|_| {
                clock.ticks += 1;
                solver::units_this_tick(&clock, blocks_per_second)
            })
            .sum()
    };
    assert_eq!(total(1.0), FULL_FLUID as u64);
    assert_eq!(total(0.37), 370);
    assert_eq!(total(0.0), 0);
}

/// A tank's state shows how full it is and what it holds.
#[test]
fn tanks_show_their_level() {
    let content = base();
    let empty = content.default_state("defaria:tank").unwrap();
    let mut held = tank(16000);
    assert_eq!(gauge::showing(content, empty, &held), empty);
    held.fill(LAVA, 1);
    let shown = gauge::showing(content, empty, &held);
    assert_eq!(
        shown,
        content
            .block_state("defaria:tank", &[("level", "1"), ("fluid", "lava")])
            .unwrap()
    );
    held.fill(LAVA, 16000);
    let full = gauge::showing(content, shown, &held);
    assert!(
        content
            .state(full)
            .values
            .contains(&("level".into(), "4".into()))
    );
    assert!(
        content.machine(full).is_some(),
        "every level is the machine"
    );
}

// The game: terrain, streaming, block entities, networks, machines, flow
// and fluid networks, on a world of a few chunks.

fn app(folder: Option<&Path>) -> App {
    app_with(base().clone(), folder)
}

/// `content` with nothing evaporating: these tests count every drop the
/// pipes and outlets move, and pits poured from outlets are open to the
/// sky. Evaporation is tested on its own (`fluid::evaporation`).
fn without_evaporation(mut content: Content) -> Content {
    for state in &mut content.states {
        if let Some(fluid) = &mut state.fluid {
            fluid.evaporation = 0.0;
        }
    }
    content
}

fn app_with(content: Content, folder: Option<&Path>) -> App {
    world_app(without_evaporation(content))
        .resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )))
        .plugins((
            BlockEntityPlugin,
            ConnectionsPlugin,
            NetworkCorePlugin,
            MachinePlugin,
            EnergyPlugin,
            FluidPlugin,
            FluidNetworkPlugin,
        ))
        .resource(StreamingSettings {
            radius: 2,
            above: 1,
            below: 1,
            unload_margin: 0,
            ..default()
        })
        .folder(folder)
        .build()
}

fn state(id: &str, values: &[(&str, &str)]) -> BlockState {
    base().block_state(id, values).unwrap()
}

fn block(id: &str) -> BlockState {
    base().default_state(id).unwrap()
}

/// Sets voxels, in order, and lets a frame or two pass.
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

/// Fills a box (inclusive) with `state`.
fn fill(app: &mut App, from: IVec3, to: IVec3, state: BlockState) {
    let mut voxels = Vec::new();
    for x in from.x..=to.x {
        for y in from.y..=to.y {
            for z in from.z..=to.z {
                voxels.push((IVec3::new(x, y, z), state));
            }
        }
    }
    set(app, &voxels);
}

fn run(app: &mut App, seconds: f32) {
    for _ in 0..(seconds / 0.05) as usize {
        app.update();
    }
}

fn tanks_at(app: &App, pos: IVec3) -> Tanks {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world().get::<Tanks>(entity).unwrap().clone()
}

fn edit_tank(app: &mut App, pos: IVec3, change: impl FnOnce(&mut Tank)) {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    change(&mut app.world_mut().get_mut::<Tanks>(entity).unwrap().0[0]);
    app.update();
}

/// How much water there is in the world in a box around the test's
/// builds, in the pipes and in every machine's tanks.
fn totals(app: &mut App) -> (u64, u64, u64) {
    let world = app
        .world_mut()
        .run_system_once(|voxels: Voxels| {
            let mut total = 0;
            for x in -12..=12 {
                for y in 44..=63 {
                    for z in -12..=12 {
                        total += voxels.fluid_amount(IVec3::new(x, y, z)) as u64;
                    }
                }
            }
            total
        })
        .unwrap();
    let pipes = held_in_pipes(app.world_mut())
        .get(WATER)
        .copied()
        .unwrap_or_default();
    let mut query = app.world_mut().query::<&Tanks>();
    let tanks = query
        .iter(app.world())
        .map(|tanks| tanks.total(WATER))
        .sum();
    (world, pipes, tanks)
}

fn sum((world, pipes, tanks): (u64, u64, u64)) -> u64 {
    world + pipes + tanks
}

const Y: i32 = 52;

/// A stone basin holding a 3×3 pond at `Y`, x and z 1 to 3, with a pump
/// in its west wall facing in, a pipe up from the pump's back and a tank
/// on top. Returns the pump's and tank's positions.
fn pond_and_pump(app: &mut App) -> (IVec3, IVec3) {
    let stone = block("defaria:stone");
    // The air above the basin is clear, whatever the terrain made.
    fill(
        app,
        IVec3::new(-3, Y - 1, -1),
        IVec3::new(5, Y + 4, 5),
        BlockState::AIR,
    );
    fill(app, IVec3::new(0, Y - 1, 0), IVec3::new(4, Y, 4), stone);
    fill(
        app,
        IVec3::new(1, Y, 1),
        IVec3::new(3, Y, 3),
        block("defaria:water"),
    );
    let pump = IVec3::new(0, Y, 2);
    let east = state("defaria:pump", &[("facing", "east")]);
    assert_eq!(world::front(base(), east), Some(Direction::East));
    let tank = IVec3::new(-1, Y + 2, 2);
    set(
        app,
        &[
            (pump, east),
            (IVec3::new(-1, Y, 2), block("defaria:pipe")),
            (IVec3::new(-1, Y + 1, 2), block("defaria:pipe")),
            (tank, block("defaria:tank")),
        ],
    );
    (pump, tank)
}

/// A pump draws a pond dry into a tank, and the water in the world, the
/// pipes and the tank adds up to the pond's all the while.
#[test]
fn a_pump_draws_a_pond_into_a_tank() {
    let mut app = app(None);
    let (_, tank) = pond_and_pump(&mut app);
    let start = sum(totals(&mut app));
    assert_eq!(start, 9 * FULL_FLUID as u64);
    let pipe = IVec3::new(-1, Y, 2);
    let id = app.world().resource::<Networks>().at(pipe).unwrap();
    let ports = app.world().resource::<crate::machine::MachinePorts>();
    let mut roles: Vec<PortRole> = ports.on(id).iter().map(|port| port.role).collect();
    roles.sort_by_key(|role| role.name());
    assert_eq!(roles, [PortRole::Output, PortRole::Store]);
    for _ in 0..30 {
        run(&mut app, 0.5);
        let now = totals(&mut app);
        assert_eq!(sum(now), start, "{now:?}");
    }
    let (world, pipes, tanks) = totals(&mut app);
    assert_eq!((world, pipes), (0, 0), "pumped dry");
    assert_eq!(tanks, start);
    assert_eq!(tanks_at(&app, tank).total(WATER), start);
    // The tank shows it's over half full.
    let shown = app
        .world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(tank).unwrap())
        .unwrap();
    assert_eq!(
        shown,
        state("defaria:tank", &[("level", "3"), ("fluid", "water")])
    );
}

/// An outlet pours a tank out into a pit, all of it, and the network
/// refuses lava while it holds water.
#[test]
fn an_outlet_pours_a_tank_into_a_pit() {
    let mut app = app(None);
    let stone = block("defaria:stone");
    fill(
        &mut app,
        IVec3::new(-3, Y - 1, -3),
        IVec3::new(6, Y + 4, 6),
        BlockState::AIR,
    );
    // A pit 3×3, two deep, at x and z 1 to 3.
    fill(
        &mut app,
        IVec3::new(0, Y - 2, 0),
        IVec3::new(4, Y, 4),
        stone,
    );
    fill(
        &mut app,
        IVec3::new(1, Y - 1, 1),
        IVec3::new(3, Y, 3),
        BlockState::AIR,
    );
    let outlet = IVec3::new(0, Y, 2);
    let tank = IVec3::new(-1, Y + 2, 2);
    set(
        &mut app,
        &[
            (outlet, state("defaria:outlet", &[("facing", "east")])),
            (IVec3::new(-1, Y, 2), block("defaria:pipe")),
            (IVec3::new(-1, Y + 1, 2), block("defaria:pipe")),
            (tank, block("defaria:tank")),
        ],
    );
    edit_tank(&mut app, tank, |tank| {
        assert_eq!(tank.fill(WATER, 3500), 3500);
    });
    let start = sum(totals(&mut app));
    assert_eq!(start, 3500);
    for _ in 0..16 {
        run(&mut app, 0.5);
        let now = totals(&mut app);
        assert_eq!(sum(now), start, "{now:?}");
    }
    let (world, pipes, tanks) = totals(&mut app);
    assert_eq!((world, pipes, tanks), (start, 0, 0));
    // All in the pit.
    let pit = app
        .world_mut()
        .run_system_once(|voxels: Voxels| {
            let mut total = 0;
            for x in 1..=3 {
                for z in 1..=3 {
                    for y in Y - 1..=Y {
                        total += voxels.fluid_amount(IVec3::new(x, y, z)) as u64;
                    }
                }
            }
            total
        })
        .unwrap();
    assert_eq!(pit, start);
}

/// Breaking a pipe spills its share where it was, and splits the rest
/// between the two halves by volume.
#[test]
fn breaking_a_pipe_spills_its_share_and_splits_the_rest() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-2, Y - 1, -2),
        IVec3::new(8, Y + 2, 2),
        BlockState::AIR,
    );
    // A stone shelf under the line, so the spill stays put and is easy to
    // count.
    fill(
        &mut app,
        IVec3::new(-1, Y - 1, -1),
        IVec3::new(7, Y - 1, 1),
        block("defaria:stone"),
    );
    fill(
        &mut app,
        IVec3::new(0, Y, 0),
        IVec3::new(5, Y, 0),
        block("defaria:pipe"),
    );
    let id = app
        .world()
        .resource::<Networks>()
        .at(IVec3::new(0, Y, 0))
        .unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<FluidNet>>()
        .get_mut(id)
        .add(WATER, 2400);
    // Break the second pipe: 1 pipe one side, 4 the other.
    set(&mut app, &[(IVec3::new(1, Y, 0), BlockState::AIR)]);
    let networks = app.world().resource::<Networks>();
    let (a, b) = (
        networks.at(IVec3::new(0, Y, 0)).unwrap(),
        networks.at(IVec3::new(2, Y, 0)).unwrap(),
    );
    assert_ne!(a, b);
    let nets = app.world().resource::<NetworkMap<FluidNet>>();
    assert_eq!(nets.get(a).unwrap().amount(WATER), 400);
    assert_eq!(nets.get(b).unwrap().amount(WATER), 1600);
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.spilled, stats.lost), (400, 0));
    let (world, pipes, _) = totals(&mut app);
    assert_eq!((world, pipes), (400, 2000));
}

/// A pipe broken before it's given back what it was loaded with (its
/// network hasn't had a tick since its chunk loaded) spills that too.
#[test]
fn a_pipe_broken_before_it_gives_back_its_fluid_spills_it() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-1, Y - 1, -1),
        IVec3::new(1, Y + 1, 1),
        BlockState::AIR,
    );
    set(
        &mut app,
        &[(IVec3::new(0, Y - 1, 0), block("defaria:stone"))],
    );
    let pos = IVec3::new(0, Y, 0);
    set(&mut app, &[(pos, block("defaria:pipe"))]);
    // As a chunk loading would leave it, and no fixed step to reclaim it.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world_mut()
        .entity_mut(entity)
        .insert(PipeFluid(vec![water(300)]));
    app.world_mut()
        .resource_mut::<pipes::Reclaim>()
        .0
        .insert(entity);
    assert_eq!(held_in_pipes(app.world_mut()).get(WATER), Some(&300));
    set(&mut app, &[(pos, BlockState::AIR)]);
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.spilled, stats.lost), (300, 0));
    let (world, pipes, _) = totals(&mut app);
    assert_eq!((world, pipes), (300, 0));
}

/// Two pipes of one network broken in the same frame each spill their
/// own share: nothing is left in a network with no pipes.
#[test]
fn pipes_broken_together_each_spill_their_share() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-2, Y - 1, -2),
        IVec3::new(4, Y + 2, 2),
        BlockState::AIR,
    );
    fill(
        &mut app,
        IVec3::new(-1, Y - 1, -1),
        IVec3::new(3, Y - 1, 1),
        block("defaria:stone"),
    );
    fill(
        &mut app,
        IVec3::new(0, Y, 0),
        IVec3::new(1, Y, 0),
        block("defaria:pipe"),
    );
    let id = app
        .world()
        .resource::<Networks>()
        .at(IVec3::new(0, Y, 0))
        .unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<FluidNet>>()
        .get_mut(id)
        .add(WATER, 800);
    set(
        &mut app,
        &[
            (IVec3::new(0, Y, 0), BlockState::AIR),
            (IVec3::new(1, Y, 0), BlockState::AIR),
        ],
    );
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.spilled, stats.lost), (800, 0));
    let (world, pipes, _) = totals(&mut app);
    assert_eq!((world, pipes), (800, 0));
}

/// A network that ends with fluid still in it (its pipes replaced by
/// fluid conduits that didn't join it) has nowhere to put it: it's
/// counted as lost, not dropped unseen.
#[test]
fn a_network_ending_with_fluid_in_it_counts_it_as_lost() {
    let mut app = App::new();
    app.add_message::<NetworkChanged>()
        .init_resource::<NetworkMap<FluidNet>>()
        .init_resource::<FluidNetworkStats>();
    let (ended, split) = (NetworkId(1), NetworkId(2));
    let mut nets = app.world_mut().resource_mut::<NetworkMap<FluidNet>>();
    nets.get_mut(ended).add(WATER, 400);
    nets.get_mut(split).add(WATER, 300);
    let share = Share {
        conduits: 1,
        capacity: 1.0,
    };
    app.world_mut().write_message(NetworkChanged::Split {
        from: ended,
        parts: Vec::new(),
        removed: share,
    });
    app.world_mut().write_message(NetworkChanged::Split {
        from: split,
        parts: vec![(split, share), (NetworkId(3), share)],
        removed: share,
    });
    app.world_mut()
        .run_system_once(pipes::count_ended_networks)
        .unwrap();
    assert_eq!(app.world().resource::<FluidNetworkStats>().lost, 400);
}

/// The base game and a hose: a fluid conduit of its own group, so it
/// doesn't join pipes.
fn with_hose() -> Content {
    test_content(&[(
        "blocks/hose.json5",
        r#"{
            model: "defaria:cube", textures: { all: "defaria:machine_side" },
            properties: { connect: { connections: true } },
            conduit: { network: "defaria:fluid", group: "hose", capacity: 1, throughput: 1 },
        }"#,
    )])
}

/// A one-pipe network with water in it, its pipe replaced by a hose that
/// doesn't join it, ends: what it held is counted as lost, which needs
/// the count to run before the network states follow the split.
#[test]
fn a_pipe_replaced_by_a_hose_counts_its_water_as_lost() {
    let content = with_hose();
    let hose = content.default_state("test:hose").unwrap();
    let mut app = app_with(content, None);
    let pos = IVec3::new(0, Y, 0);
    set(&mut app, &[(pos, block("defaria:pipe"))]);
    let id = app.world().resource::<Networks>().at(pos).unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<FluidNet>>()
        .get_mut(id)
        .add(WATER, 300);
    set(&mut app, &[(pos, hose)]);
    assert_ne!(app.world().resource::<Networks>().at(pos), Some(id));
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.lost, stats.spilled), (300, 0));
    assert_eq!(held_in_pipes(app.world_mut()).get(WATER), None);
}

/// A pipe still waiting to give back what it was loaded with, replaced by
/// a hose: its network takes it back first, so it's counted with the rest
/// when the network ends, rather than disappearing.
#[test]
fn a_waiting_pipe_replaced_by_a_hose_counts_its_water() {
    let content = with_hose();
    let hose = content.default_state("test:hose").unwrap();
    let mut app = app_with(content, None);
    let pos = IVec3::new(0, Y, 0);
    set(&mut app, &[(pos, block("defaria:pipe"))]);
    // As a chunk loading would leave it, and no fixed step to reclaim it.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world_mut()
        .entity_mut(entity)
        .insert(PipeFluid(vec![water(300)]));
    app.world_mut()
        .resource_mut::<pipes::Reclaim>()
        .0
        .insert(entity);
    set(&mut app, &[(pos, hose)]);
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.lost, stats.spilled), (300, 0));
    assert_eq!(held_in_pipes(app.world_mut()).get(WATER), None);
}

/// What pipes hold goes with their chunk when it unloads, and comes back
/// when it loads; on exit it's saved in them, and back in the network
/// after a restart.
#[test]
fn pipes_keep_their_fluid_through_unloading_and_restarts() {
    let folder = temp_folder("fluid-networks");
    // A line of pipes across a chunk border, well away from home's chunk.
    let line = |x: i32| IVec3::new(x, Y, 24);
    {
        let mut app = app(Some(&folder));
        fill(&mut app, line(10), line(21), block("defaria:pipe"));
        let id = app.world().resource::<Networks>().at(line(10)).unwrap();
        app.world_mut()
            .resource_mut::<NetworkMap<FluidNet>>()
            .get_mut(id)
            .add(WATER, 5999);
        // Walk away so the line's chunks unload, then come back.
        let loader = app
            .world_mut()
            .query_filtered::<Entity, With<ChunkLoader>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Transform>(loader)
            .unwrap()
            .translation = HOME - Vec3::new(0.0, 0.0, 200.0);
        settle(&mut app);
        assert!(app.world().resource::<Networks>().at(line(10)).is_none());
        let away = held_in_pipes(app.world_mut()).get(WATER).copied();
        assert_eq!(away, None, "nothing left in the loaded networks");
        app.world_mut()
            .get_mut::<Transform>(loader)
            .unwrap()
            .translation = HOME;
        settle(&mut app);
        run(&mut app, 0.2);
        let id = app.world().resource::<Networks>().at(line(10)).unwrap();
        assert_eq!(
            app.world().resource::<Networks>().at(line(21)),
            Some(id),
            "one network again"
        );
        let nets = app.world().resource::<NetworkMap<FluidNet>>();
        assert_eq!(nets.get(id).unwrap().amount(WATER), 5999);
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    let mut app = app(Some(&folder));
    run(&mut app, 0.2);
    let id = app.world().resource::<Networks>().at(line(10)).unwrap();
    let nets = app.world().resource::<NetworkMap<FluidNet>>();
    assert_eq!(nets.get(id).unwrap().amount(WATER), 5999);
    assert_eq!(held_in_pipes(app.world_mut()).get(WATER), Some(&5999));
}

/// A tank of lava piped to the combustion generator's left side (its fuel
/// input, #115) fills the generator's `fuel` tank, and no lava is made or
/// lost.
#[test]
fn pipes_fill_the_combustion_generators_fuel_tank() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-2, Y - 1, -2),
        IVec3::new(2, Y + 3, 2),
        BlockState::AIR,
    );
    // Facing north, so its left (west, −X) takes fluid.
    let generator = IVec3::new(0, Y, 0);
    let tank = IVec3::new(-1, Y + 1, 0);
    set(
        &mut app,
        &[
            (generator, block("defaria:combustion_generator")),
            (IVec3::new(-1, Y, 0), block("defaria:pipe")),
            (tank, block("defaria:tank")),
        ],
    );
    let fuel = |app: &App| {
        let tanks = tanks_at(app, generator);
        tanks.get("fuel").map(|tank| tank.amount).unwrap()
    };
    edit_tank(&mut app, tank, |held| {
        held.fill(LAVA, 6 * FULL_FLUID as u64);
    });
    run(&mut app, 5.0);
    let capacity = 4 * FULL_FLUID as u64;
    assert_eq!(fuel(&app), capacity, "the generator's fuel tank is full");
    assert_eq!(
        tanks_at(&app, tank).total(LAVA) + held_lava(&mut app) + capacity,
        6 * FULL_FLUID as u64,
        "no lava made or lost"
    );
}

/// Lava piped into an empty combustion generator starts burning on the
/// network tick its fuel tank first gets some: fluid moves before recipe
/// machines run.
#[test]
fn a_piped_generator_starts_on_the_tick_the_lava_arrives() {
    let mut app = app_with(base().clone(), None);
    app.add_plugins(crate::recipe::RecipePlugin);
    fill(
        &mut app,
        IVec3::new(-2, Y - 1, -2),
        IVec3::new(2, Y + 3, 2),
        BlockState::AIR,
    );
    let generator = IVec3::new(0, Y, 0);
    let tank = IVec3::new(-1, Y + 1, 0);
    set(
        &mut app,
        &[
            (generator, block("defaria:combustion_generator")),
            (IVec3::new(-1, Y, 0), block("defaria:pipe")),
            (tank, block("defaria:tank")),
        ],
    );
    let entity = app
        .world()
        .resource::<BlockEntities>()
        .get(generator)
        .unwrap();
    edit_tank(&mut app, tank, |held| {
        held.fill(LAVA, FULL_FLUID as u64);
    });
    // Short frames, so no frame runs two network ticks.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        10,
    )));
    // The recipe burns a tenth of a block.
    let burns = FULL_FLUID as u64 / 10;
    for _ in 0..400 {
        app.update();
        let fuel = tanks_at(&app, generator).get("fuel").unwrap().amount;
        let progress = app.world().get::<crate::recipe::RecipeProgress>(entity);
        if progress.unwrap().recipe.is_some() {
            return;
        }
        assert!(
            fuel < burns,
            "{fuel} lava in the tank, and no recipe started"
        );
    }
    panic!("the generator never started");
}

fn held_lava(app: &mut App) -> u64 {
    held_in_pipes(app.world_mut())
        .get(LAVA)
        .copied()
        .unwrap_or_default()
}

/// Moves the chunk loader (the player) to `to`.
fn walk_to(app: &mut App, to: Vec3) {
    let loader = app
        .world_mut()
        .query_filtered::<Entity, With<ChunkLoader>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Transform>(loader)
        .unwrap()
        .translation = to;
}

/// The water in the network the pipe at `pos` is on.
fn water_on(app: &App, pos: IVec3) -> u64 {
    let id = app.world().resource::<Networks>().at(pos).unwrap();
    app.world()
        .resource::<NetworkMap<FluidNet>>()
        .get(id)
        .map_or(0, |net| net.amount(WATER))
}

/// The pipes' fluid comes through an autosave that falls on the frame
/// their chunk unloads, whichever frame that is.
#[test]
fn pipes_keep_their_fluid_when_an_autosave_falls_on_an_unload() {
    for offset in 0..4 {
        let folder = temp_folder("fluid-autosave-unload");
        let line = |x: i32| IVec3::new(x, Y, 24);
        let mut app = app(Some(&folder));
        fill(&mut app, line(10), line(21), block("defaria:pipe"));
        let id = app.world().resource::<Networks>().at(line(10)).unwrap();
        app.world_mut()
            .resource_mut::<NetworkMap<FluidNet>>()
            .get_mut(id)
            .add(WATER, 5999);
        // Autosaves every frame, while walking away and back.
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .autosave_seconds = 0.0;
        for _ in 0..offset {
            std::thread::sleep(Duration::from_millis(30));
            app.update();
        }
        std::thread::sleep(Duration::from_millis(30));
        walk_to(&mut app, HOME - Vec3::new(0.0, 0.0, 200.0));
        settle(&mut app);
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .autosave_seconds = 1000.0;
        walk_to(&mut app, HOME);
        settle(&mut app);
        run(&mut app, 0.2);
        assert_eq!(water_on(&app, line(10)), 5999, "offset {offset}");
    }
}

/// The game exiting on the frame half a network's chunks unload saves
/// the loaded half's share in full: nothing is lost over a restart.
#[test]
fn pipes_keep_their_fluid_when_the_game_exits_as_chunks_unload() {
    let folder = temp_folder("fluid-exit-unload");
    let line = |x: i32| IVec3::new(x, Y, 24);
    {
        let mut app = app(Some(&folder));
        fill(&mut app, line(10), line(21), block("defaria:pipe"));
        let id = app.world().resource::<Networks>().at(line(10)).unwrap();
        app.world_mut()
            .resource_mut::<NetworkMap<FluidNet>>()
            .get_mut(id)
            .add(WATER, 5999);
        // A chunk west: the line's east chunk unloads, its west one stays,
        // on the frame the game exits (the frame after the move, once the
        // loader's transform is propagated).
        walk_to(&mut app, HOME - Vec3::new(16.0, 0.0, 0.0));
        app.update();
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert!(
            app.world().resource::<Networks>().at(line(10)).is_some()
                && !app
                    .world()
                    .resource::<crate::world::ChunkMap>()
                    .get(crate::world::ChunkCoord(IVec3::new(1, 3, 1)))
                    .is_some(),
            "the east chunk unloaded, the west one didn't"
        );
    }
    let mut app = app(Some(&folder));
    run(&mut app, 0.2);
    assert_eq!(water_on(&app, line(10)), 5999);
}

/// A network whose pipes' chunk has just unloaded, before the networks
/// see it, doesn't run: it counts the room of pipes that are gone.
#[test]
fn a_network_waits_while_its_chunk_unloads() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-4, Y - 1, 0),
        IVec3::new(3, Y + 1, 4),
        BlockState::AIR,
    );
    // A pump in chunk 0, its pipe running west into chunk −1.
    let pump = IVec3::new(1, Y, 2);
    set(
        &mut app,
        &[
            (pump, state("defaria:pump", &[("facing", "east")])),
            (IVec3::new(0, Y, 2), block("defaria:pipe")),
            (IVec3::new(-1, Y, 2), block("defaria:pipe")),
            (IVec3::new(-2, Y, 2), block("defaria:pipe")),
        ],
    );
    edit_tank(&mut app, pump, |tank| {
        tank.fill(WATER, 1000);
    });
    let pipe = IVec3::new(0, Y, 2);
    let held = |app: &mut App| (tanks_at(app, pump).total(WATER), water_on(app, pipe));
    let step = |app: &mut App| {
        app.world_mut()
            .run_system_once(solver::run_networks)
            .unwrap();
    };
    let before = held(&mut app);
    step(&mut app);
    assert_ne!(held(&mut app), before, "it runs while it's whole");
    // The west chunk is gone from the map, as unloading does.
    let west = crate::world::ChunkCoord(IVec3::new(-1, 3, 0));
    let entity = app
        .world_mut()
        .resource_mut::<crate::world::ChunkMap>()
        .remove(west)
        .unwrap();
    let before = held(&mut app);
    step(&mut app);
    assert_eq!(held(&mut app), before);
    app.world_mut()
        .resource_mut::<crate::world::ChunkMap>()
        .insert(west, entity);
}

/// A pipe broken just after part of its network's chunks unloaded (their
/// pipes' shares gone with them, before the networks see it) spills its
/// share of what the loaded pipes hold, not counting the gone ones.
#[test]
fn a_pipe_broken_as_its_networks_chunk_unloads_spills_its_share() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-4, Y - 1, 0),
        IVec3::new(3, Y + 1, 4),
        BlockState::AIR,
    );
    // Two pipes in chunk 0, two in chunk −1.
    fill(
        &mut app,
        IVec3::new(-2, Y, 2),
        IVec3::new(1, Y, 2),
        block("defaria:pipe"),
    );
    let pipe = IVec3::new(1, Y, 2);
    let id = app.world().resource::<Networks>().at(pipe).unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<FluidNet>>()
        .get_mut(id)
        .add(WATER, 400);
    let west = crate::world::ChunkCoord(IVec3::new(-1, 3, 0));
    app.world_mut()
        .resource_mut::<crate::world::ChunkMap>()
        .remove(west)
        .unwrap();
    // Broken, and spilled before the networks see either.
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| assert!(voxels.set(pipe, BlockState::AIR)))
        .unwrap();
    app.world_mut()
        .run_system_once(pipes::spill_broken_pipes)
        .unwrap();
    app.world_mut().run_system_once(pipes::pour_spills).unwrap();
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.spilled, stats.lost), (200, 0));
}

/// A network's limits follow its pipes changing in place (another metal,
/// as a script's `set_state` makes), and go with the network.
#[test]
fn fluid_limits_follow_the_networks() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-4, Y - 1, 0),
        IVec3::new(3, Y + 1, 4),
        BlockState::AIR,
    );
    let pump = IVec3::new(1, Y, 2);
    let pipes = [IVec3::new(0, Y, 2), IVec3::new(-1, Y, 2)];
    set(
        &mut app,
        &[
            (pump, state("defaria:pump", &[("facing", "east")])),
            (pipes[0], block("defaria:pipe")),
            (pipes[1], block("defaria:pipe")),
        ],
    );
    edit_tank(&mut app, pump, |tank| {
        tank.fill(WATER, 1000);
    });
    run(&mut app, 0.2);
    let id = app.world().resource::<Networks>().at(pipes[0]).unwrap();
    let throughput = |metal: &str| {
        let pipe = state("defaria:pipe", &[("metal", metal)]);
        base().conduit(pipe).unwrap().throughput as f64
    };
    let known = |app: &App| {
        app.world()
            .resource::<solver::FluidLimits>()
            .known(id)
            .map(|limits| limits.throughput)
    };
    assert_eq!(known(&app), Some(throughput("defaria:iron")));
    // Copper, keeping the sides they join.
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            for pos in pipes {
                let now = voxels.get(pos).unwrap();
                let mut values = base().state(now).values.clone();
                for (name, value) in &mut values {
                    if name == "metal" {
                        *value = "defaria:copper".into();
                    }
                }
                let values: Vec<(&str, &str)> = values
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();
                let copper = base().block_state("defaria:pipe", &values).unwrap();
                assert!(voxels.update(pos, copper));
            }
        })
        .unwrap();
    run(&mut app, 0.2);
    assert_eq!(app.world().resource::<Networks>().at(pipes[0]), Some(id));
    assert_eq!(known(&app), Some(throughput("defaria:copper")));
    // Broken: the network and its limits go.
    set(
        &mut app,
        &[(pipes[0], BlockState::AIR), (pipes[1], BlockState::AIR)],
    );
    run(&mut app, 0.2);
    assert_eq!(known(&app), None);
}

/// The base game and two test pipes alike but for how hot a fluid they
/// carry.
fn with_warm_and_cool_pipes() -> Content {
    let pipe = |max: u32| {
        format!(
            r#"{{
                model: "defaria:cube", textures: {{ all: "defaria:machine_side" }},
                properties: {{ connect: {{ connections: true }} }},
                stats: {{ max_temperature: {max} }},
                conduit: {{ network: "defaria:fluid", capacity: 0.5, throughput: 1 }},
            }}"#
        )
    };
    let (warm, cool) = (pipe(2000), pipe(300));
    test_content(&[("blocks/warm.json5", &warm), ("blocks/cool.json5", &cool)])
}

/// Pipes changed for ones with the same throughput but a lower
/// `max_temperature` lower the network's limit.
#[test]
fn fluid_limits_follow_a_change_of_max_temperature() {
    let content = with_warm_and_cool_pipes();
    let warm = content.default_state("test:warm").unwrap();
    let cool = content.default_state("test:cool").unwrap();
    let mut app = app_with(content, None);
    fill(
        &mut app,
        IVec3::new(-4, Y - 1, 0),
        IVec3::new(3, Y + 1, 4),
        BlockState::AIR,
    );
    let pump = IVec3::new(1, Y, 2);
    let pipes = [IVec3::new(0, Y, 2), IVec3::new(-1, Y, 2)];
    set(
        &mut app,
        &[
            (pump, state("defaria:pump", &[("facing", "east")])),
            (pipes[0], warm),
            (pipes[1], warm),
        ],
    );
    edit_tank(&mut app, pump, |tank| {
        tank.fill(WATER, 1000);
    });
    run(&mut app, 0.2);
    let id = app.world().resource::<Networks>().at(pipes[0]).unwrap();
    let known = |app: &App| {
        app.world()
            .resource::<solver::FluidLimits>()
            .known(id)
            .map(|limits| (limits.throughput, limits.max_temperature))
    };
    assert_eq!(known(&app), Some((1.0, Some(2000.0))));
    set(&mut app, &[(pipes[0], cool), (pipes[1], cool)]);
    run(&mut app, 0.2);
    assert_eq!(app.world().resource::<Networks>().at(pipes[0]), Some(id));
    assert_eq!(known(&app), Some((1.0, Some(300.0))));
}

/// A lone pipe replaced by a pipe of another metal keeps what it holds:
/// it's the same network, not one emptied and a new one.
#[test]
fn a_pipe_changed_to_another_metal_keeps_its_fluid() {
    let mut app = app(None);
    fill(
        &mut app,
        IVec3::new(-1, Y - 1, -1),
        IVec3::new(1, Y + 1, 1),
        BlockState::AIR,
    );
    let pipe = IVec3::new(0, Y, 0);
    set(&mut app, &[(pipe, block("defaria:pipe"))]);
    let id = app.world().resource::<Networks>().at(pipe).unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<FluidNet>>()
        .get_mut(id)
        .add(WATER, 300);
    set(
        &mut app,
        &[(pipe, state("defaria:pipe", &[("metal", "defaria:copper")]))],
    );
    assert_eq!(app.world().resource::<Networks>().at(pipe), Some(id));
    assert_eq!(water_on(&app, pipe), 300);
    let stats = app.world().resource::<FluidNetworkStats>();
    assert_eq!((stats.spilled, stats.lost), (0, 0));
}

/// The water in the world in a box, inclusive.
fn water_in(app: &mut App, from: IVec3, to: IVec3) -> u64 {
    app.world_mut()
        .run_system_once(move |voxels: Voxels, content: Res<Content>| {
            let mut total = 0;
            for x in from.x..=to.x {
                for y in from.y..=to.y {
                    for z in from.z..=to.z {
                        let (_, fluid) = voxels.get_with_fluid(IVec3::new(x, y, z)).unwrap();
                        if let Some(fluid) = fluid
                            && content.block_of(fluid.kind).id == WATER
                        {
                            total += fluid.amount as u64;
                        }
                    }
                }
            }
            total
        })
        .unwrap()
}

/// Pipes broken (one to three at once) and placed at random, the chunks they're in unloading
/// (half or all of them, on autosave frames or not) and the game
/// restarting, never make or lose water: what's in the world, in the
/// pipes and lost (counted) adds up to what there was.
#[test]
fn random_edits_unloads_saves_and_restarts_keep_every_drop() {
    let folder = temp_folder("fluid-fuzz");
    // A stone basin across the border of chunks x 0 and 1, south of home,
    // with a line of pipes along it.
    let (low, high) = (IVec3::new(7, Y - 1, 21), IVec3::new(24, Y + 3, 27));
    let line = |x: i32| IVec3::new(x, Y, 24);
    let mut app = app(Some(&folder));
    let stone = block("defaria:stone");
    fill(&mut app, low, high, BlockState::AIR);
    fill(&mut app, low, IVec3::new(high.x, Y - 1, high.z), stone);
    fill(&mut app, low, IVec3::new(low.x, Y + 1, high.z), stone);
    fill(
        &mut app,
        IVec3::new(high.x, Y - 1, low.z),
        IVec3::new(high.x, Y + 1, high.z),
        stone,
    );
    fill(&mut app, low, IVec3::new(high.x, Y + 1, low.z), stone);
    fill(
        &mut app,
        IVec3::new(low.x, Y - 1, high.z),
        IVec3::new(high.x, Y + 1, high.z),
        stone,
    );
    fill(&mut app, line(10), line(21), block("defaria:pipe"));
    let id = app.world().resource::<Networks>().at(line(10)).unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<FluidNet>>()
        .get_mut(id)
        .add(WATER, 5999);
    let start = 5999;
    let mut lost_before = 0;
    let check = |app: &mut App, lost_before: u64, step: usize, what: &str| {
        let world = water_in(app, low, high);
        let pipes = held_in_pipes(app.world_mut())
            .get(WATER)
            .copied()
            .unwrap_or_default();
        let lost = lost_before + app.world().resource::<FluidNetworkStats>().lost;
        assert_eq!(
            world + pipes + lost,
            start,
            "step {step} ({what}): world {world}, pipes {pipes}, lost {lost}"
        );
    };
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let away = |app: &mut App, to: Vec3, autosave: bool| {
        if autosave {
            app.world_mut()
                .resource_mut::<StreamingSettings>()
                .autosave_seconds = 0.0;
        }
        walk_to(app, to);
        settle(app);
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .autosave_seconds = 1000.0;
        walk_to(app, HOME);
        settle(app);
    };
    for step in 0..40 {
        let what = match next(7) {
            0 => {
                // One to three pipes, broken in the same frame.
                let mut broken = Vec::new();
                for _ in 0..=next(3) {
                    let pos = line(10 + next(12) as i32);
                    if app.world().resource::<Networks>().at(pos).is_some() {
                        broken.push((pos, BlockState::AIR));
                    }
                }
                set(&mut app, &broken);
                "break"
            }
            1 => {
                let pos = line(10 + next(12) as i32);
                // Into dry air: a pipe placed into spilt water would
                // delete it.
                let here = app
                    .world_mut()
                    .run_system_once(move |voxels: Voxels| voxels.get_with_fluid(pos).unwrap())
                    .unwrap();
                if here == (BlockState::AIR, None) {
                    set(&mut app, &[(pos, block("defaria:pipe"))]);
                }
                "place"
            }
            2 => {
                away(&mut app, HOME - Vec3::new(16.0, 0.0, 0.0), next(2) == 0);
                "half unloads"
            }
            3 => {
                away(&mut app, HOME - Vec3::new(0.0, 0.0, 200.0), next(2) == 0);
                "all unload"
            }
            4 => {
                // The game exits, sometimes as half the chunks unload.
                if next(2) == 0 {
                    walk_to(&mut app, HOME - Vec3::new(16.0, 0.0, 0.0));
                    app.update();
                }
                app.world_mut().write_message(AppExit::Success);
                app.update();
                lost_before += app.world().resource::<FluidNetworkStats>().lost;
                drop(app);
                app = self::app(Some(&folder));
                "restart"
            }
            _ => {
                run(&mut app, 0.3);
                "run"
            }
        };
        run(&mut app, 0.1);
        check(&mut app, lost_before, step, what);
    }
}
