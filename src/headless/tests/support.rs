//! Blocks that need support, in the game.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_core::{
    content::{BlockState, block},
    world::Voxels,
};

/// End to end, in the whole game: plants a scenario's `fill` leaves
/// without support break, and the rest stay.
#[test]
fn fills_in_the_game_break_unsupported_plants() {
    use crate::headless::{Script, test_game};
    let script = Script::parse(
        "wait 2\n\
         fill 48 15 48 58 25 58 air\n\
         fill 50 20 50 52 20 52 soil[turf=defaria:grass]\n\
         fill 50 21 50 52 21 52 daisy\n\
         fill 55 21 55 55 21 55 daisy\n\
         wait 1\n\
         fill 51 20 51 51 20 51 air\n\
         wait 1\n",
    )
    .unwrap();
    let mut app = test_game(script);
    let mut frames = 0;
    while app.should_exit().is_none() {
        app.update();
        frames += 1;
        assert!(frames < 20_000, "the script never finished");
    }
    let mut at = |pos: IVec3| at(&mut app, pos);
    assert_eq!(at(IVec3::new(50, 21, 50)), block("daisy"), "on grass");
    assert_eq!(at(IVec3::new(51, 21, 51)), BlockState::AIR, "over the hole");
    assert_eq!(at(IVec3::new(55, 21, 55)), BlockState::AIR, "in mid-air");
}

fn at(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
        .unwrap()
}
