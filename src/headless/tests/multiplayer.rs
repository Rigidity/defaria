//! A server and two clients, in one process: what one player does reaches
//! the other.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_client::replication::{Playback, Replica};
use defaria_core::{
    content::{BlockState, block},
    physics::Body,
    world::Voxels,
};

use crate::headless::test_multiplayer;
use defaria_server::server::PlayerInputs;

fn voxel(app: &mut App, pos: IVec3) -> Option<BlockState> {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos))
        .unwrap()
}

/// Each player sees the other; a block one places reaches the other's
/// world, and the other sees the first where it went.
#[test]
fn an_edit_by_one_client_reaches_the_other() {
    let mut game = test_multiplayer(2);
    game.settle();
    for client in &mut game.clients {
        let mut views = client.world_mut().query::<&Replica>();
        assert_eq!(views.iter(client.world()).count(), 1);
    }

    // A stone in the air, and the first player over it, looking down.
    let below = IVec3::new(64, 60, 88);
    game.server
        .world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            voxels.set(below, block("stone"));
        })
        .unwrap();
    game.run(0, "camera 64.5 66 88.5 0 -90\nwait 2\nsettle");
    game.settle();
    // Lock the cursor, and place the stone in hand on top of it.
    game.run(0, "hold MouseLeft 1\nwait 1\nhold MouseRight 1\nwait 4");
    let placed = below + IVec3::Y;
    assert_eq!(voxel(&mut game.server, placed), Some(block("stone")));
    assert_eq!(voxel(&mut game.clients[1], placed), Some(block("stone")));

    // The second player sees the first where it went.
    let world = game.clients[1].world_mut();
    let mut views = world.query::<(&Replica, &Transform)>();
    let (_, at) = views.single(world).unwrap();
    assert!(
        at.translation
            .distance(Vec3::new(64.5, 66.0 - 1.62 + 0.9, 88.5))
            < 0.01,
        "{}",
        at.translation
    );
}

/// A client plays on a server over the network (QUIC on this machine):
/// every kind of message it's sent crosses as bytes, it loads the world
/// around it, and its edit reaches the server.
#[test]
fn a_client_plays_over_the_network() {
    use std::{net::SocketAddr, time::Duration};

    let mut game = test_multiplayer(0);
    let content = game
        .server
        .world()
        .resource::<defaria_core::content::Content>();
    let offer = crate::mod_transfer::Offer::new(content).unwrap();
    let library = crate::mod_transfer::Library::new(content, None);
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener =
        crate::quic::listen(local, offer, &crate::identity::Identity::generate()).unwrap();
    let address = listener.address.to_string();
    crate::dedicated::take_clients_from(&mut game.server, listener);
    let me = crate::identity::Identity::generate();
    let link = crate::quic::connect(&address, library, &me, "Tester", None)
        .unwrap()
        .link;
    let mut client = crate::headless::test_client();
    client.insert_resource(defaria_client::net::ServerConnection(link));
    game.clients.push(client);
    // The network's on other threads: give it a moment each frame.
    let mut settled = false;
    for _ in 0..6000 {
        game.update();
        std::thread::sleep(Duration::from_millis(1));
        if game.clients[0]
            .world()
            .resource::<defaria_core::streaming::StreamingStatus>()
            .settled
        {
            settled = true;
            break;
        }
    }
    assert!(settled, "never settled over the network");
    let loaded = game.clients[0]
        .world()
        .resource::<defaria_core::world::ChunkMap>()
        .len();
    assert!(loaded > 100, "{loaded} chunks");

    let below = IVec3::new(64, 60, 88);
    game.server
        .world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            voxels.set(below, block("stone"));
        })
        .unwrap();
    game.run(0, "camera 64.5 66 88.5 0 -90\nwait 30");
    game.run(0, "hold MouseLeft 1\nwait 1\nhold MouseRight 1\nwait 30");
    let placed = below + IVec3::Y;
    for _ in 0..1000 {
        if voxel(&mut game.clients[0], placed) == Some(block("stone")) {
            break;
        }
        game.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(voxel(&mut game.server, placed), Some(block("stone")));
    assert_eq!(voxel(&mut game.clients[0], placed), Some(block("stone")));
}

/// A client whose server lets only a few chunks wait for it at a time
/// (as for a slow connection) still gets its whole world, held back and
/// sent as the network takes it: it settles, with as many chunks as the
/// server has in its range.
#[test]
fn a_held_back_client_still_gets_its_world() {
    use std::{net::SocketAddr, time::Duration};

    let mut game = test_multiplayer(0);
    let content = game
        .server
        .world()
        .resource::<defaria_core::content::Content>();
    let offer = crate::mod_transfer::Offer::new(content).unwrap();
    let library = crate::mod_transfer::Library::new(content, None);
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let limits = crate::quic::Limits {
        bulk_budget: 16 << 10,
        ..default()
    };
    let identity = crate::identity::Identity::generate();
    let listener = crate::quic::listen_with(local, offer, &identity, limits).unwrap();
    let address = listener.address.to_string();
    crate::dedicated::take_clients_from(&mut game.server, listener);
    let me = crate::identity::Identity::generate();
    let link = crate::quic::connect(&address, library, &me, "Tester", None)
        .unwrap()
        .link;
    let mut client = crate::headless::test_client();
    client.insert_resource(defaria_client::net::ServerConnection(link));
    game.clients.push(client);
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut held_back = false;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "never settled over the network"
        );
        game.update();
        std::thread::sleep(Duration::from_millis(1));
        let world = game.server.world_mut();
        let mut connections = world.query::<&defaria_server::server::Connection>();
        held_back |= (connections.iter(world)).any(|c| c.held_back() > 0);
        if game.clients[0]
            .world()
            .resource::<defaria_core::streaming::StreamingStatus>()
            .settled
        {
            break;
        }
    }
    assert!(held_back, "no chunks were held back");
    let world = game.server.world_mut();
    let mut connections = world.query::<&defaria_server::server::Connection>();
    let connection = connections.single(world).unwrap();
    assert_eq!(connection.held_back(), 0);
    let loaded = game.clients[0]
        .world()
        .resource::<defaria_core::world::ChunkMap>()
        .len();
    assert_eq!(loaded, connection.chunks_sent());
    assert!(loaded > 100, "{loaded} chunks");
}

/// Where client `watcher` draws the one other player.
fn other_player(game: &mut crate::headless::Multiplayer, watcher: usize) -> Vec3 {
    let world = game.clients[watcher].world_mut();
    let mut views = world.query::<(&Replica, &Transform)>();
    views.single(world).unwrap().1.translation
}

/// Where each player is on the server, in the order of their entities.
fn players_on_server(game: &mut crate::headless::Multiplayer) -> Vec<Vec3> {
    let world = game.server.world_mut();
    let mut bodies = world.query_filtered::<(Entity, &Body), With<PlayerInputs>>();
    let mut players: Vec<(Entity, Vec3)> = (bodies.iter(world))
        .map(|(entity, body)| (entity, body.position.as_vec3()))
        .collect();
    players.sort_by_key(|(entity, _)| *entity);
    players.into_iter().map(|(_, at)| at).collect()
}

/// How far each of `frames` is from the one before.
fn steps(frames: &[Vec3]) -> Vec<f32> {
    frames.windows(2).map(|w| w[0].distance(w[1])).collect()
}

/// The mean of `steps`, and the frames off it by more than `share` of it.
fn off_the_mean(steps: &[f32], share: f32) -> (f32, Vec<usize>) {
    let mean = steps.iter().sum::<f32>() / steps.len() as f32;
    let off = (steps.iter().enumerate())
        .filter(|(_, step)| (*step - mean).abs() > mean * share)
        .map(|(frame, _)| frame)
        .collect();
    (mean, off)
}

/// One player flies along over a connection a tenth of a second each way
/// that loses a quarter of the inputs and snapshots. The server steps it
/// the same distance every frame, holding its inputs a little to even out
/// the losses, and the other player sees it glide there, a little behind,
/// stopping only when a run of lost snapshots outlasts the tenth of a
/// second it's drawn behind (it holds then, rather than guess ahead).
#[test]
fn another_player_moves_smoothly() {
    let mut game = crate::headless::test_multiplayer_over(
        2,
        crate::headless::Conditions {
            delay: 6,
            loss: 0.25,
        },
    );
    game.settle();
    // Long enough for each move to reach the server, and back, so the
    // settling below is around where they are now; then everything the
    // walk crosses is loaded on both sides (the world loads in the
    // background, so without it a busy machine could leave the walker
    // stopped at a chunk the server hasn't made yet).
    game.run(0, "camera 64.5 66 88.5 0 0\nwait 20");
    game.run(1, "camera 60.5 66 92.5 0 0\nwait 20");
    game.settle();
    game.run(0, "wait 30");
    // The same losses from here on, however many messages loading took.
    game.reseed(1);
    let (mut on_server, mut seen, mut holding) = (Vec::new(), Vec::new(), Vec::new());
    game.run_watching(0, "hold KeyW 150", |game| {
        on_server.push(players_on_server(game));
        seen.push(other_player(game, 1));
        holding.push(game.clients[1].world().resource::<Playback>().holding());
    });
    // Once it's up to speed, and for as long as it's seen at speed.
    let cruising = 60..140;

    // On the server, the walker (the one that moved) goes the same
    // distance every frame but where a run of losses is longer than any
    // lately: each such run stops it once. Measured over 40 seeds: none in
    // most runs, two at most; four leaves room for the seeds and timing
    // to change, and is still a tenth of what stepping inputs as they
    // arrive gave (26 to 53).
    let walker = (0..on_server[0].len())
        .max_by(|a, b| {
            let moved = |player: usize| on_server[0][player].distance(on_server[149][player]);
            moved(*a).total_cmp(&moved(*b))
        })
        .unwrap();
    let walked: Vec<Vec3> = on_server.iter().map(|players| players[walker]).collect();
    let walked = &steps(&walked)[cruising.clone()];
    let (mean, off) = off_the_mean(walked, 0.05);
    assert!(mean > 0.05, "it moved: {mean} a frame");
    assert!(
        off.len() <= 4,
        "the server stepped it unevenly in frames {off:?}: {walked:?}"
    );

    // Seen by the other: every frame within 40% of the mean but where it
    // holds (measured: within 20% in most runs, 31% at worst, as the
    // playback clock eases toward the snapshots). Drawn at the newest
    // snapshot, rather than between them, frames would be two or three
    // times the mean, then none.
    let seen = &steps(&seen)[cruising.clone()];
    let held =
        |frame: usize| holding[cruising.start + frame] || holding[cruising.start + frame + 1];
    let holds = (0..seen.len()).filter(|frame| held(*frame)).count();
    assert!(holds <= seen.len() / 5, "held {holds} frames");
    let (mean, off) = off_the_mean(seen, 0.4);
    let off: Vec<usize> = off.into_iter().filter(|frame| !held(*frame)).collect();
    assert!(
        off.is_empty(),
        "frames {off:?} are off the mean of {mean} a frame: {seen:?}"
    );
}
