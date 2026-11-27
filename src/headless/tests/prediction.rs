//! A client moves its player and places blocks at once, and the server
//! checks: through a slow connection that loses messages, the two agree;
//! when the server moves the player itself the client follows, gliding;
//! and a block the server won't place goes again.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_client::{
    edit_prediction::PredictedEdits,
    prediction::{Correction, Prediction},
};
use defaria_core::{
    content::{BlockState, block},
    physics::Body,
    player::LocalPlayer,
    server::PlayerInputs,
    world::Voxels,
};

use crate::headless::{Conditions, Multiplayer, test_multiplayer, test_multiplayer_over};

/// Client `index`'s player's body as the client has it.
fn client_body(game: &mut Multiplayer, index: usize) -> Body {
    let world = game.clients[index].world_mut();
    let mut bodies = world.query_filtered::<&Body, With<LocalPlayer>>();
    bodies.single(world).unwrap().clone()
}

/// The (only) player's body as the server has it.
fn server_body(game: &mut Multiplayer) -> Body {
    let world = game.server.world_mut();
    let mut bodies = world.query_filtered::<&Body, With<PlayerInputs>>();
    bodies.single(world).unwrap().clone()
}

fn corrections(game: &Multiplayer) -> u32 {
    game.clients[0].world().resource::<Prediction>().corrections
}

/// Flying down to land, walking and jumping over a connection a tenth of a
/// second each way that loses a quarter of what may be lost: the client
/// moves at once, the server steps the same inputs, and they end in the
/// same place without the client having to be put right.
#[test]
fn the_player_moves_at_once_and_agrees_with_the_server() {
    let mut game = test_multiplayer_over(
        1,
        Conditions {
            delay: 6,
            loss: 0.25,
        },
    );
    game.settle();
    game.run(0, "camera 64.5 66 88.5 0 0\nwait 2\nsettle");
    let before = corrections(&game);
    game.run(
        0,
        "hold ShiftLeft 300\nhold KeyW 60\nhold Space 10\nhold KeyD 40\nwait 60",
    );
    let client = client_body(&mut game, 0);
    let server = server_body(&mut game);
    assert!(client.on_ground, "it landed");
    // (The server is a few inputs behind, as it's sent them late, and the
    // player still slides a little: what's checked exactly is each step
    // the server confirms, by the corrections.)
    assert!(
        client.position.distance(server.position) < 1e-3,
        "client {} server {}",
        client.position,
        server.position
    );
    assert_eq!(corrections(&game), before, "the client was put right");
}

/// The server moves the player itself (as a push would): the client takes
/// where the server has it, and draws the difference fading out.
#[test]
fn the_client_follows_where_the_server_puts_its_player() {
    let mut game = test_multiplayer(1);
    game.settle();
    game.run(0, "camera 64.5 66 88.5 0 0\nwait 2");
    let before = corrections(&game);
    {
        let world = game.server.world_mut();
        let mut bodies = world.query_filtered::<&mut Body, With<PlayerInputs>>();
        bodies.single_mut(world).unwrap().position.x += 3.0;
    }
    game.run(0, "wait 3");
    assert_eq!(corrections(&game), before + 1);
    let client = client_body(&mut game, 0);
    let server = server_body(&mut game);
    assert!(
        client.position.distance(server.position) < 1e-6,
        "client {} server {}",
        client.position,
        server.position
    );
    let glide = |game: &mut Multiplayer| {
        let world = game.clients[0].world_mut();
        let mut corrections = world.query_filtered::<&Correction, With<LocalPlayer>>();
        corrections.single(world).unwrap().0
    };
    let first = glide(&mut game);
    assert!(first.x < -1.0, "drawn where it was, at first: {first}");
    game.run(0, "wait 30");
    let last = glide(&mut game);
    assert!(last.length() < 0.01, "and then where it is: {last}");
}

fn voxel(app: &mut App, pos: IVec3) -> Option<BlockState> {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos))
        .unwrap()
}

fn edits(game: &Multiplayer, client: usize) -> (usize, u32) {
    let edits = game.clients[client].world().resource::<PredictedEdits>();
    (edits.pending(), edits.rolled_back)
}

/// Two players a tenth of a second from the server, a stone in the air,
/// and the first player over it looking down, cursor locked.
fn over_a_stone() -> (Multiplayer, IVec3) {
    let mut game = test_multiplayer_over(
        2,
        Conditions {
            delay: 6,
            loss: 0.0,
        },
    );
    game.settle();
    let below = IVec3::new(64, 60, 88);
    game.server
        .world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            voxels.set(below, block("stone"));
        })
        .unwrap();
    game.run(0, "camera 64.5 66 88.5 0 -90\nwait 2\nsettle");
    game.run(1, "camera 70.5 66 88.5 0 0\nwait 2");
    game.run(0, "hold MouseLeft 1\nwait 30");
    (game, below + IVec3::Y)
}

/// A block placed shows at once, long before the server has it, and the
/// server's answer leaves it there.
#[test]
fn a_placed_block_shows_at_once() {
    let (mut game, placed) = over_a_stone();
    game.run(0, "hold MouseRight 1\nwait 1");
    assert_eq!(voxel(&mut game.clients[0], placed), Some(block("stone")));
    assert_eq!(voxel(&mut game.server, placed), Some(BlockState::AIR));
    assert_eq!(edits(&game, 0).0, 1, "waiting for the server");
    game.run(0, "wait 30");
    assert_eq!(voxel(&mut game.server, placed), Some(block("stone")));
    assert_eq!(voxel(&mut game.clients[0], placed), Some(block("stone")));
    assert_eq!(edits(&game, 0), (0, 0));
}

/// A block placed where the server won't have it (another player stands
/// there, which the client doesn't check) shows at once, and goes when the
/// server answers.
#[test]
fn a_refused_edit_is_rolled_back() {
    let (mut game, placed) = over_a_stone();
    // The second player in the voxel the stone would go in.
    game.run(1, "camera 64.5 62.62 88.5 0 0\nwait 30");
    game.run(0, "hold MouseRight 1\nwait 1");
    assert_eq!(voxel(&mut game.clients[0], placed), Some(block("stone")));
    game.run(0, "wait 30");
    assert_eq!(voxel(&mut game.server, placed), Some(BlockState::AIR));
    assert_eq!(voxel(&mut game.clients[0], placed), Some(BlockState::AIR));
    assert_eq!(edits(&game, 0), (0, 1));
}

/// A slab, then its other half, placed one after the other faster than
/// the server answers: the client shows each at once and ends with what
/// the server has, the whole block.
#[test]
fn a_slab_and_its_other_half_placed_at_once() {
    let (mut game, placed) = over_a_stone();
    // The fifth slot holds stone slabs.
    game.run(0, "press Digit5\nwait 1\nrelease Digit5\nwait 1");
    game.run(0, "hold MouseRight 1\nwait 3");
    let half = voxel(&mut game.clients[0], placed).unwrap();
    assert_ne!(half, BlockState::AIR);
    game.run(0, "hold MouseRight 1\nwait 1");
    let whole = voxel(&mut game.clients[0], placed).unwrap();
    assert_ne!(whole, half, "the other half went in at once");
    assert_eq!(edits(&game, 0).0, 1, "one voxel waiting");
    game.run(0, "wait 40");
    assert_eq!(voxel(&mut game.server, placed), Some(whole));
    assert_eq!(voxel(&mut game.clients[0], placed), Some(whole));
    assert_eq!(edits(&game, 0), (0, 0));
}
