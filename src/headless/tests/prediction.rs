//! A client moves its player at once and the server checks it: through a
//! slow connection that loses messages, the two agree, and when the
//! server moves the player itself the client follows, gliding.

use bevy::prelude::*;
use defaria_client::prediction::{Correction, Prediction};
use defaria_core::{physics::Body, player::LocalPlayer, server::PlayerInputs};

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
