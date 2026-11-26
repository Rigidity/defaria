//! Networks over chunks spawned by hand, with the connection properties
//! set by the real `connections` plugin.

use std::{
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::Duration,
};

use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

use super::*;
use crate::{
    connections::ConnectionsPlugin,
    content::{base, block, test_mod},
    world::FluidChanged,
};

/// What a test solver keeps: units, divided by share, with what the
/// removed conduits took counted in `spilled`, or in the sink `S` when no
/// network is left to count it.
#[derive(Debug, Clone, PartialEq)]
struct Stored<S = Sinks> {
    units: u64,
    spilled: u64,
    sink: std::marker::PhantomData<S>,
}

impl<S> Default for Stored<S> {
    fn default() -> Self {
        Self {
            units: 0,
            spilled: 0,
            sink: std::marker::PhantomData,
        }
    }
}

/// Where a test's spills go when they have no network to stay with: one
/// per test that counts them, as tests run side by side.
trait Sink: Send + Sync + 'static {
    fn sink() -> &'static AtomicU64;
}

#[derive(Debug, Clone, PartialEq)]
struct Sinks;

impl Sink for Sinks {
    fn sink() -> &'static AtomicU64 {
        static SINK: AtomicU64 = AtomicU64::new(0);
        &SINK
    }
}

impl<S: Sink> NetworkState for Stored<S> {
    fn merge(&mut self, other: Self) {
        self.units += other.units;
        self.spilled += other.spilled;
    }

    fn split(self, parts: &[Share], removed: Share) -> Vec<Self> {
        let (amounts, gone) = divide_by_share(self.units, parts, removed);
        let mut out: Vec<Self> = amounts
            .into_iter()
            .map(|units| Stored { units, ..default() })
            .collect();
        match out.first_mut() {
            Some(first) => first.spilled = self.spilled + gone,
            None => {
                S::sink().fetch_add(self.spilled + gone, Relaxed);
            }
        }
        out
    }
}

fn app_with(content: Content) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(content)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .init_resource::<ChunkMap>()
        .add_message::<VoxelChanged>()
        .add_message::<FluidChanged>()
        .add_message::<PropertiesUpdated>()
        .add_plugins((ConnectionsPlugin, NetworkPlugin))
        .add_network_state::<Stored>();
    app
}

/// The base game, with one loaded chunk of air at the origin.
fn app() -> App {
    let mut app = app_with(base().clone());
    spawn(&mut app, IVec3::ZERO, Chunk::default());
    app.update();
    app
}

fn spawn(app: &mut App, coord: IVec3, chunk: Chunk) {
    let world = app.world_mut();
    let entity = world.spawn((ChunkCoord(coord), chunk)).id();
    world
        .resource_mut::<ChunkMap>()
        .insert(ChunkCoord(coord), entity);
}

fn unload(app: &mut App, coord: IVec3) {
    let entity = app
        .world_mut()
        .resource_mut::<ChunkMap>()
        .remove(ChunkCoord(coord))
        .unwrap();
    app.world_mut().despawn(entity);
    app.update();
}

/// Sets voxels, then runs a frame.
fn set(app: &mut App, voxels: &[(IVec3, BlockState)]) {
    let voxels = voxels.to_vec();
    app.world_mut()
        .run_system_once(move |mut world: crate::world::Voxels| {
            for &(pos, state) in &voxels {
                assert!(world.set(pos, state), "{pos} isn't loaded");
            }
        })
        .unwrap();
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

fn pipe() -> BlockState {
    block("pipe")
}

fn at(app: &App, pos: IVec3) -> Option<NetworkId> {
    app.world().resource::<Networks>().at(pos)
}

fn network(app: &App, pos: IVec3) -> Network {
    let networks = app.world().resource::<Networks>();
    networks
        .get(
            networks
                .at(pos)
                .unwrap_or_else(|| panic!("no network at {pos}")),
        )
        .unwrap()
        .clone()
}

fn count(app: &App) -> usize {
    app.world().resource::<Networks>().len()
}

/// Every network's units, plus everything spilled.
fn total(app: &App) -> u64 {
    let map = app.world().resource::<NetworkMap<Stored>>();
    map.iter()
        .map(|(_, stored)| stored.units + stored.spilled)
        .sum()
}

fn give(app: &mut App, pos: IVec3, units: u64) {
    let id = at(app, pos).unwrap();
    app.world_mut()
        .resource_mut::<NetworkMap<Stored>>()
        .get_mut(id)
        .units += units;
}

fn units(app: &App, pos: IVec3) -> u64 {
    let id = at(app, pos).unwrap();
    app.world()
        .resource::<NetworkMap<Stored>>()
        .get(id)
        .map_or(0, |stored| stored.units)
}

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}

#[test]
fn a_line_of_pipes_is_one_network() {
    let mut app = app();
    set(&mut app, &line(v(2, 4, 4), v(8, 4, 4), pipe()));
    assert_eq!(count(&app), 1);
    let network = network(&app, v(5, 4, 4));
    assert_eq!(network.conduits().len(), 7);
    assert_eq!(network.capacity(), 3.5);
    assert_eq!(network.chunks().collect::<Vec<_>>(), [IVec3::ZERO]);
    assert!(!network.is_frozen());
    assert!(network.ports().is_empty());
    assert_eq!(
        base().networks[network.kind()].id,
        "defaria:fluid",
        "pipes are on the fluid network"
    );
    // Air and stone aren't on any network.
    assert_eq!(at(&app, v(1, 4, 4)), None);
}

#[test]
fn a_pipe_between_two_networks_merges_them() {
    let mut app = app();
    set(&mut app, &line(v(1, 4, 4), v(4, 4, 4), pipe()));
    set(&mut app, &line(v(6, 4, 4), v(7, 4, 4), pipe()));
    assert_eq!(count(&app), 2);
    let (big, small) = (at(&app, v(1, 4, 4)).unwrap(), at(&app, v(7, 4, 4)).unwrap());
    give(&mut app, v(1, 4, 4), 400);
    give(&mut app, v(7, 4, 4), 200);

    set(&mut app, &[(v(5, 4, 4), pipe())]);
    assert_eq!(count(&app), 1);
    // The larger keeps its id, and holds what both did.
    assert_eq!(at(&app, v(7, 4, 4)), Some(big));
    assert_ne!(at(&app, v(7, 4, 4)), Some(small));
    assert_eq!(network(&app, v(5, 4, 4)).conduits().len(), 7);
    assert_eq!(units(&app, v(5, 4, 4)), 600);
}

#[test]
fn breaking_the_middle_splits_a_line() {
    let mut app = app();
    set(&mut app, &line(v(1, 4, 4), v(7, 4, 4), pipe()));
    let id = at(&app, v(1, 4, 4)).unwrap();
    give(&mut app, v(1, 4, 4), 700);

    set(&mut app, &[(v(4, 4, 4), BlockState::AIR)]);
    assert_eq!(count(&app), 2);
    let (west, east) = (at(&app, v(1, 4, 4)).unwrap(), at(&app, v(7, 4, 4)).unwrap());
    assert_ne!(west, east);
    // One half keeps the network.
    assert!(west == id || east == id);
    assert_eq!(network(&app, v(3, 4, 4)).conduits().len(), 3);
    assert_eq!(network(&app, v(5, 4, 4)).conduits().len(), 3);
    // Shared by capacity: 300 each, and 100 went with the broken pipe.
    assert_eq!(units(&app, v(1, 4, 4)), 300);
    assert_eq!(units(&app, v(7, 4, 4)), 300);
    assert_eq!(total(&app), 700);

    // Breaking the end of one half shrinks it without splitting.
    set(&mut app, &[(v(1, 4, 4), BlockState::AIR)]);
    assert_eq!(count(&app), 2);
    assert_eq!(network(&app, v(2, 4, 4)).conduits().len(), 2);
    assert_eq!(units(&app, v(2, 4, 4)), 200);
    assert_eq!(total(&app), 700);
}

#[test]
fn breaking_a_loop_once_doesnt_split_it() {
    let mut app = app();
    // A square ring, 4 by 4.
    let mut ring = line(v(2, 4, 2), v(5, 4, 2), pipe());
    ring.extend(line(v(5, 4, 3), v(5, 4, 5), pipe()));
    ring.extend(line(v(4, 4, 5), v(2, 4, 5), pipe()));
    ring.extend(line(v(2, 4, 4), v(2, 4, 3), pipe()));
    set(&mut app, &ring);
    assert_eq!(count(&app), 1);
    let id = at(&app, v(2, 4, 2)).unwrap();
    assert_eq!(network(&app, v(2, 4, 2)).conduits().len(), 12);

    set(&mut app, &[(v(3, 4, 2), BlockState::AIR)]);
    assert_eq!(count(&app), 1);
    assert_eq!(at(&app, v(2, 4, 2)), Some(id));
    assert_eq!(network(&app, v(4, 4, 2)).conduits().len(), 11);
    // The second break opens it into a line, still one network.
    set(&mut app, &[(v(5, 4, 4), BlockState::AIR)]);
    assert_eq!(count(&app), 2);
}

#[test]
fn breaking_a_cross_splits_it_four_ways() {
    let mut app = app();
    let mut cross = line(v(1, 4, 4), v(7, 4, 4), pipe());
    cross.extend(line(v(4, 4, 1), v(4, 4, 7), pipe()));
    set(&mut app, &cross);
    give(&mut app, v(4, 4, 4), 1300);
    set(&mut app, &[(v(4, 4, 4), BlockState::AIR)]);
    assert_eq!(count(&app), 4);
    for end in [v(1, 4, 4), v(7, 4, 4), v(4, 4, 1), v(4, 4, 7)] {
        assert_eq!(network(&app, end).conduits().len(), 3);
        assert_eq!(units(&app, end), 300);
    }
    assert_eq!(total(&app), 1300);
}

#[test]
fn breaking_every_pipe_leaves_nothing_but_what_spilled() {
    let mut app = app();
    set(&mut app, &line(v(1, 9, 9), v(3, 9, 9), pipe()));
    give(&mut app, v(1, 9, 9), 90);
    let before = Sinks::sink().load(Relaxed);
    set(&mut app, &line(v(1, 9, 9), v(3, 9, 9), BlockState::AIR));
    assert_eq!(count(&app), 0);
    let after = Sinks::sink().load(Relaxed);
    assert!(after - before >= 90);
}

#[test]
fn different_kinds_dont_join() {
    let mut app = app();
    let wire = block("wire");
    let tube = block("tube");
    set(
        &mut app,
        &[
            (v(1, 4, 4), pipe()),
            (v(2, 4, 4), pipe()),
            (v(3, 4, 4), wire),
            (v(4, 4, 4), wire),
            (v(5, 4, 4), tube),
            (v(6, 4, 4), tube),
            (v(7, 4, 4), block("stone")),
        ],
    );
    assert_eq!(count(&app), 3);
    let kind = |pos| base().networks[network(&app, pos).kind()].id.clone();
    assert_eq!(kind(v(1, 4, 4)), "defaria:fluid");
    assert_eq!(kind(v(3, 4, 4)), "defaria:electric");
    assert_eq!(kind(v(5, 4, 4)), "defaria:item");
    for pos in [v(1, 4, 4), v(3, 4, 4), v(5, 4, 4)] {
        assert_eq!(network(&app, pos).conduits().len(), 2);
    }
    // Pipes of different metals are one network.
    let copper = base()
        .block_state("defaria:pipe", &[("metal", "defaria:copper")])
        .unwrap();
    set(&mut app, &[(v(1, 5, 4), copper)]);
    assert_eq!(network(&app, v(1, 5, 4)).conduits().len(), 3);
}

/// A mod with a power network and cables in two colours, and one with no
/// colour.
fn cables() -> Content {
    let content = test_mod(&[
        (
            "networks/power.json5",
            r#"{ name: "Power", solver: "energy" }"#,
        ),
        (
            "blocks/cable.json5",
            r#"{
                model: "quad", textures: { all: "a" },
                properties: { color: ["red", "blue"], connect: { connections: true } },
                conduit: { network: "power", group: "{color}", capacity: 0, throughput: 10 },
            }"#,
        ),
        (
            "blocks/bare.json5",
            r#"{
                model: "quad", textures: { all: "a" },
                properties: { connect: { connections: true } },
                conduit: { network: "power", capacity: 0, throughput: 10 },
            }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    content
}

#[test]
fn groups_keep_cables_apart() {
    let content = cables();
    let red = content
        .block_state("test:cable", &[("color", "red")])
        .unwrap();
    let blue = content
        .block_state("test:cable", &[("color", "blue")])
        .unwrap();
    let bare = content.default_state("test:bare").unwrap();
    let mut app = app_with(content);
    spawn(&mut app, IVec3::ZERO, Chunk::default());
    app.update();
    // Red and blue side by side, a bare one across the end of both.
    let mut voxels = line(v(1, 4, 4), v(5, 4, 4), red);
    voxels.extend(line(v(1, 4, 5), v(5, 4, 5), blue));
    voxels.push((v(6, 4, 4), bare));
    voxels.push((v(6, 4, 5), bare));
    set(&mut app, &voxels);
    assert_eq!(count(&app), 3);
    assert_eq!(network(&app, v(1, 4, 4)).conduits().len(), 5);
    assert_eq!(network(&app, v(1, 4, 5)).conduits().len(), 5);
    assert_eq!(network(&app, v(6, 4, 4)).conduits().len(), 2);
    // Wires hold nothing; the network's capacity is 0.
    assert_eq!(network(&app, v(1, 4, 4)).capacity(), 0.0);
    // A red cable put down blue joins the blue ones, and only them.
    set(&mut app, &[(v(3, 4, 4), blue)]);
    assert_eq!(count(&app), 4);
    assert_eq!(network(&app, v(3, 4, 4)).conduits().len(), 6);
}

/// A network reaching into a chunk that unloads is frozen; it thaws, whole
/// again, when the chunk loads.
#[test]
fn a_network_freezes_across_an_unloaded_chunk_and_thaws() {
    let mut app = app();
    spawn(&mut app, IVec3::X, Chunk::default());
    app.update();
    // A line across the border at x = 16.
    set(&mut app, &line(v(12, 4, 4), v(19, 4, 4), pipe()));
    assert_eq!(count(&app), 1);
    let whole = network(&app, v(12, 4, 4));
    assert_eq!(whole.conduits().len(), 8);
    assert_eq!(whole.chunks().count(), 2);
    assert!(!whole.is_frozen());
    give(&mut app, v(12, 4, 4), 800);

    // Save the chunk's voxels as streaming would, and unload it.
    let saved = {
        let world = app.world();
        let entity = world
            .resource::<ChunkMap>()
            .get(ChunkCoord(IVec3::X))
            .unwrap();
        world.get::<Chunk>(entity).unwrap().clone()
    };
    unload(&mut app, IVec3::X);
    let half = network(&app, v(12, 4, 4));
    assert_eq!(half.conduits().len(), 4);
    assert!(half.is_frozen());
    assert_eq!(half.open_faces(), 1);
    assert_eq!(at(&app, v(16, 4, 4)), None);
    // The half left holds its share.
    assert_eq!(units(&app, v(12, 4, 4)), 400);
    assert_eq!(total(&app), 800);
    // A frozen network still grows, and stays frozen.
    set(&mut app, &[(v(11, 4, 4), pipe())]);
    assert!(network(&app, v(11, 4, 4)).is_frozen());

    // It loads again: whole, and running.
    spawn(&mut app, IVec3::X, saved);
    app.update();
    let whole = network(&app, v(12, 4, 4));
    assert_eq!(whole.conduits().len(), 9);
    assert!(!whole.is_frozen());
    assert_eq!(at(&app, v(19, 4, 4)), at(&app, v(11, 4, 4)));
    assert_eq!(count(&app), 1);
}

/// A pipe placed at the edge of the loaded world isn't joined into the
/// unloaded chunk (nothing was there to join), so it isn't frozen; once
/// the chunk loads with a pipe against it, they join.
#[test]
fn a_pipe_at_the_edge_joins_the_chunk_that_loads_next_to_it() {
    let mut app = app();
    set(&mut app, &line(v(14, 4, 4), v(15, 4, 4), pipe()));
    assert!(!network(&app, v(15, 4, 4)).is_frozen());
    let mut next = Chunk::default();
    next.set_at(v(0, 4, 4), pipe());
    next.set_at(v(1, 4, 4), pipe());
    spawn(&mut app, IVec3::X, next);
    app.update();
    assert_eq!(count(&app), 1);
    assert_eq!(network(&app, v(15, 4, 4)).conduits().len(), 4);
}

/// A chunk that loads with pipes already in it has them as networks.
#[test]
fn networks_are_rebuilt_from_chunks_as_they_load() {
    let mut app = app_with(base().clone());
    let mut chunk = Chunk::default();
    for x in 0..5 {
        chunk.set_at(v(x, 2, 2), pipe());
    }
    chunk.set_at(v(9, 2, 2), block("wire"));
    spawn(&mut app, IVec3::ZERO, chunk);
    app.update();
    assert_eq!(count(&app), 2);
    assert_eq!(network(&app, v(0, 2, 2)).conduits().len(), 5);
    // Unloading every chunk leaves no networks.
    unload(&mut app, IVec3::ZERO);
    assert_eq!(count(&app), 0);
}

/// A long line broken in the middle splits into its two halves, and
/// nothing is made or lost.
#[test]
fn a_long_line_splits_in_the_middle() {
    let mut app = app_with(base().clone());
    for x in 0..8 {
        spawn(&mut app, IVec3::new(x, 0, 0), Chunk::default());
    }
    app.update();
    set(&mut app, &line(v(0, 4, 4), v(127, 4, 4), pipe()));
    assert_eq!(network(&app, v(0, 4, 4)).conduits().len(), 128);
    give(&mut app, v(0, 4, 4), 128_000);
    set(&mut app, &[(v(64, 4, 4), BlockState::AIR)]);
    assert_eq!(network(&app, v(0, 4, 4)).conduits().len(), 64);
    assert_eq!(network(&app, v(127, 4, 4)).conduits().len(), 63);
    assert_eq!(total(&app), 128_000);
    assert_eq!(units(&app, v(0, 4, 4)), 64_000);
}

#[test]
fn describe_says_what_the_network_is() {
    let mut app = app();
    spawn(&mut app, IVec3::X, Chunk::default());
    app.update();
    set(&mut app, &line(v(14, 4, 4), v(17, 4, 4), pipe()));
    let content = base();
    let line = |app: &App, pos| {
        describe(app.world().resource::<Networks>(), content, pos, |_, _| {
            None
        })
    };
    let text = line(&app, v(14, 4, 4));
    assert!(text.starts_with("network at 14 4 4: #"), "{text}");
    assert!(
        text.ends_with(
            "defaria:fluid (Fluid), 4 conduits, 2 chunks, capacity 2, running, no ports"
        ),
        "{text}"
    );
    unload(&mut app, IVec3::X);
    let text = line(&app, v(14, 4, 4));
    assert!(
        text.ends_with("2 conduits, 1 chunk, capacity 1, frozen (1 open face), no ports"),
        "{text}"
    );
    assert_eq!(line(&app, v(1, 1, 1)), "network at 1 1 1: none");
}

/// The networks' parts: each network's conduits, sorted, with its open
/// faces, ports and capacity, in a canonical order.
fn partition(networks: &Networks) -> Vec<(Vec<[i32; 3]>, u32, Vec<([i32; 3], u8)>, i64)> {
    let mut out: Vec<_> = networks
        .iter()
        .map(|(_, network)| {
            let mut conduits: Vec<[i32; 3]> =
                network.conduits().iter().map(|p| p.to_array()).collect();
            conduits.sort();
            let mut ports: Vec<([i32; 3], u8)> = network
                .ports()
                .iter()
                .map(|port| (port.pos.to_array(), port.face as u8))
                .collect();
            ports.sort();
            (
                conduits,
                network.open_faces(),
                ports,
                (network.capacity() * 1000.0).round() as i64,
            )
        })
        .collect();
    out.sort();
    out
}

/// A wire joined to a machine across a chunk border is frozen when the
/// machine's chunk unloads, as one rebuilt without that chunk would be,
/// and joined to it again when it loads.
#[test]
fn a_port_across_a_chunk_border_opens_when_its_chunk_unloads() {
    let mut app = app();
    spawn(&mut app, IVec3::X, Chunk::default());
    app.update();
    let battery = base()
        .block_state("defaria:battery", &[("facing", "east")])
        .unwrap();
    set(
        &mut app,
        &[(v(15, 4, 4), block("wire")), (v(16, 4, 4), battery)],
    );
    let wire = network(&app, v(15, 4, 4));
    assert_eq!(wire.ports().len(), 1);
    assert!(!wire.is_frozen());

    let entity = app
        .world()
        .resource::<ChunkMap>()
        .get(ChunkCoord(IVec3::X))
        .unwrap();
    let saved = app.world().get::<Chunk>(entity).cloned().unwrap();
    unload(&mut app, IVec3::X);
    let wire = network(&app, v(15, 4, 4));
    assert!(wire.ports().is_empty());
    assert!(wire.is_frozen());

    spawn(&mut app, IVec3::X, saved);
    app.update();
    let wire = network(&app, v(15, 4, 4));
    assert_eq!(wire.ports().len(), 1);
    assert!(!wire.is_frozen());
}

/// After any series of edits, loads and unloads, the networks kept up to
/// date one change at a time are the ones built from scratch, and nothing
/// a network held was made or lost.
#[derive(Debug, Clone, PartialEq)]
struct FuzzSink;

impl Sink for FuzzSink {
    fn sink() -> &'static AtomicU64 {
        static SINK: AtomicU64 = AtomicU64::new(0);
        &SINK
    }
}

type Fuzzed = Stored<FuzzSink>;

#[test]
fn incremental_networks_match_a_rebuild() {
    let content = base();
    let mut app = app();
    app.add_network_state::<Fuzzed>();
    spawn(&mut app, IVec3::X, Chunk::default());
    app.update();
    let copper = content
        .block_state("defaria:pipe", &[("metal", "defaria:copper")])
        .unwrap();
    let choices = [
        pipe(),
        pipe(),
        pipe(),
        copper,
        block("wire"),
        block("stone"),
        // Machines, joined by ports, across the chunk border too.
        block("tank"),
        block("battery"),
        BlockState::AIR,
        BlockState::AIR,
    ];
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut random = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut x_loaded = true;
    let mut saved = None;
    let mut given = 0;
    for step in 0..400 {
        if step % 10 == 9 {
            if x_loaded {
                let entity = app
                    .world()
                    .resource::<ChunkMap>()
                    .get(ChunkCoord(IVec3::X))
                    .unwrap();
                saved = app.world().get::<Chunk>(entity).cloned();
                unload(&mut app, IVec3::X);
            } else {
                spawn(&mut app, IVec3::X, saved.take().unwrap());
                app.update();
            }
            x_loaded = !x_loaded;
        } else {
            let mut voxels = Vec::new();
            for _ in 0..1 + random(4) {
                let x = 12 + random(if x_loaded { 8 } else { 4 }) as i32;
                let pos = v(x, 3 + random(3) as i32, 3 + random(3) as i32);
                voxels.push((pos, choices[random(choices.len() as u64) as usize]));
            }
            set(&mut app, &voxels);
        }
        // Give some network something to hold.
        let first = app
            .world()
            .resource::<Networks>()
            .iter()
            .next()
            .map(|(id, _)| id);
        if let Some(id) = first {
            app.world_mut()
                .resource_mut::<NetworkMap<Fuzzed>>()
                .get_mut(id)
                .units += 10;
            given += 10;
        }

        // Build the networks from scratch, from every loaded voxel.
        let mut fresh = Networks::default();
        let world = app.world_mut();
        let mut loaded = Vec::new();
        for (coord, chunk) in world.query::<(&ChunkCoord, &Chunk)>().iter(world) {
            loaded.push((coord.0, chunk.clone()));
        }
        let lookup = |pos: IVec3| {
            let (coord, local) = ChunkCoord::split(pos);
            loaded
                .iter()
                .find(|(c, _)| *c == coord.0)
                .map(|(_, chunk)| chunk.at(local))
        };
        let everything: Vec<IVec3> = loaded
            .iter()
            .flat_map(|(coord, chunk)| {
                let origin = ChunkCoord(*coord).origin();
                (0..chunk.voxels().len()).map(move |i| origin + Chunk::position_of(i))
            })
            .collect();
        fresh.update(everything, content, lookup);
        assert_eq!(
            partition(world.resource::<Networks>()),
            partition(&fresh),
            "step {step}"
        );
        // Every state is a network's, and every unit given is in one or
        // went with a conduit.
        let networks = world.resource::<Networks>();
        let map = world.resource::<NetworkMap<Fuzzed>>();
        let mut held = FuzzSink::sink().load(Relaxed);
        for (id, stored) in map.iter() {
            assert!(
                networks.get(id).is_some(),
                "step {step}: {id:?} has {stored:?} but isn't a network"
            );
            held += stored.units + stored.spilled;
        }
        assert_eq!(held, given, "step {step}");
    }
    assert!(given > 0);
}

/// A conduit changed in place into another that joins the same network
/// (another metal) stays on its network, with what it holds: no split.
#[test]
fn a_conduit_changed_in_place_keeps_its_network() {
    let mut app = app();
    let copper = base()
        .block_state("defaria:pipe", &[("metal", "defaria:copper")])
        .unwrap();
    // A lone pipe, and a line of two.
    set(&mut app, &[(v(2, 4, 4), pipe())]);
    set(&mut app, &line(v(6, 4, 4), v(7, 4, 4), pipe()));
    let (lone, line_id) = (at(&app, v(2, 4, 4)).unwrap(), at(&app, v(6, 4, 4)).unwrap());
    give(&mut app, v(2, 4, 4), 100);
    give(&mut app, v(6, 4, 4), 200);
    // Let the frame's messages go by.
    app.update();
    app.update();

    set(&mut app, &[(v(2, 4, 4), copper), (v(6, 4, 4), copper)]);
    assert_eq!(at(&app, v(2, 4, 4)), Some(lone));
    assert_eq!(at(&app, v(6, 4, 4)), Some(line_id));
    assert_eq!(at(&app, v(7, 4, 4)), Some(line_id));
    assert_eq!(network(&app, v(6, 4, 4)).conduits().len(), 2);
    assert_eq!(network(&app, v(6, 4, 4)).capacity(), 1.0);
    assert_eq!(
        (units(&app, v(2, 4, 4)), units(&app, v(6, 4, 4))),
        (100, 200)
    );
    let changes = app
        .world_mut()
        .run_system_once(|mut changes: MessageReader<NetworkChanged>| {
            changes.read().cloned().collect::<Vec<_>>()
        })
        .unwrap();
    assert!(changes.is_empty(), "{changes:?}");
}

/// A conduit changed in place into one with the same capacity and
/// throughput but other stats changes the networks' revision, so what's
/// worked out from its stats is worked out again.
#[test]
fn a_conduit_changed_only_in_its_stats_is_a_change() {
    let content = test_mod(&[
        (
            "networks/fluid.json5",
            r#"{ name: "Fluid", solver: "fluid" }"#,
        ),
        (
            "blocks/warm.json5",
            r#"{
                model: "quad", textures: { all: "a" },
                properties: { connect: { connections: true } },
                stats: { max_temperature: 1000 },
                conduit: { network: "fluid", capacity: 1, throughput: 1 },
            }"#,
        ),
        (
            "blocks/cool.json5",
            r#"{
                model: "quad", textures: { all: "a" },
                properties: { connect: { connections: true } },
                stats: { max_temperature: 300 },
                conduit: { network: "fluid", capacity: 1, throughput: 1 },
            }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let warm = content.default_state("test:warm").unwrap();
    let cool = content.default_state("test:cool").unwrap();
    assert_eq!(content.conduit(warm), content.conduit(cool));
    let mut app = app_with(content);
    spawn(&mut app, IVec3::ZERO, Chunk::default());
    app.update();
    set(&mut app, &[(v(2, 4, 4), warm)]);
    let id = at(&app, v(2, 4, 4)).unwrap();
    let revision = app.world().resource::<Networks>().revision();
    set(&mut app, &[(v(2, 4, 4), cool)]);
    assert_eq!(at(&app, v(2, 4, 4)), Some(id));
    assert_ne!(app.world().resource::<Networks>().revision(), revision);
}

/// Whether `Networks` was marked changed on the last frame.
#[derive(Resource, Default)]
struct NetworksChanged(bool);

/// Voxels changing away from any conduit, or conduits re-evaluated and
/// found the same, don't mark the networks changed; a conduit placed does.
#[test]
fn only_real_changes_mark_the_networks_changed() {
    let mut app = app();
    app.init_resource::<NetworksChanged>().add_systems(
        Update,
        (|networks: Res<Networks>, mut flag: ResMut<NetworksChanged>| {
            flag.0 = networks.is_changed();
        })
        .after(NetworkSystems),
    );
    let changed = |app: &App| app.world().resource::<NetworksChanged>().0;
    set(&mut app, &line(v(2, 4, 4), v(4, 4, 4), pipe()));
    assert!(changed(&app));
    app.update();
    assert!(!changed(&app));
    // Stone far away, and beside the line (which joins nothing).
    set(&mut app, &[(v(10, 4, 4), block("stone"))]);
    assert!(!changed(&app));
    set(&mut app, &[(v(3, 5, 4), block("stone"))]);
    assert!(!changed(&app));
    // A pipe put back as it was.
    set(&mut app, &[(v(3, 4, 4), pipe())]);
    assert!(!changed(&app));
    // Another pipe on the end.
    set(&mut app, &[(v(5, 4, 4), pipe())]);
    assert!(changed(&app));
}
