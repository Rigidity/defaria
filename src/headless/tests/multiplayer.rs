//! A server and two clients, in one process: what one player does reaches
//! the other.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_client::others::OtherPlayerView;
use defaria_core::{
    content::{BlockState, block},
    world::Voxels,
};

use crate::headless::test_multiplayer;

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
        let mut views = client.world_mut().query::<&OtherPlayerView>();
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
    let mut views = world.query::<(&OtherPlayerView, &Transform)>();
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
        .resource::<defaria_core::content::Content>()
        .fingerprint();
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = crate::quic::listen(local, content).unwrap();
    let address = listener.address.to_string();
    crate::dedicated::take_clients_from(&mut game.server, listener);
    let link = crate::quic::connect(&address, content).unwrap();
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
