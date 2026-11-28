//! The wrench in the game.

use bevy::prelude::*;
use defaria_core::content::Direction;
use defaria_server::{block_entity::BlockEntities, connections::BlockedFaces, network::Networks};

use crate::headless::{SETTLE_TIMEOUT_FRAMES, Script, test_game};

/// The whole game: right click with a wrench in hand, aimed down at the
/// arm between two pipes, blocks that face and splits the network; with
/// a pickaxe it does nothing.
#[test]
fn right_clicking_with_a_wrench_blocks_the_arm_aimed_at() {
    let run = |item: &str| {
        let script = format!(
            "wait 2\n\
             fill 48 20 48 56 24 52 air\n\
             fill 50 20 50 54 20 50 pipe\n\
             camera 52.9 23 50.5 0 -89.4\n\
             hotbar 1 {item}\n\
             wait 1\n\
             hold MouseLeft 1\n\
             wait 1\n\
             hold MouseRight 1\n\
             wait 2\n"
        );
        let mut app = test_game(Script::parse(&script).unwrap());
        for _ in 0..SETTLE_TIMEOUT_FRAMES + 200 {
            app.update();
            if let Some(exit) = app.should_exit() {
                assert_eq!(exit, AppExit::Success);
                let networks = crate::headless::server_world(&app).resource::<Networks>();
                let ids: Vec<_> = (50..=54)
                    .map(|x| networks.at(IVec3::new(x, 20, 50)).map(|id| id.0))
                    .collect();
                return (faces(&app, IVec3::new(52, 20, 50)), ids);
            }
        }
        panic!("script never finished");
    };
    let (blocked, ids) = run("wrench");
    let mut east = BlockedFaces::default();
    east.toggle(E);
    assert_eq!(blocked, Some(east));
    assert_eq!(distinct(&ids), 2, "{ids:?}");
    assert_eq!(ids[2], ids[0]);
    assert_eq!(ids[3], ids[4]);

    let (blocked, ids) = run("pickaxe");
    assert_eq!(blocked, None);
    assert_eq!(distinct(&ids), 1, "{ids:?}");
}

/// Runs `script` in the whole game until it ends, then returns the wire
/// at (52, 20, 49)'s blocked faces and whether a machine screen is open.
fn run_game(script: &str) -> (Option<BlockedFaces>, bool) {
    let mut app = test_game(Script::parse(script).unwrap());
    for _ in 0..SETTLE_TIMEOUT_FRAMES + 200 {
        app.update();
        if let Some(exit) = app.should_exit() {
            assert_eq!(exit, AppExit::Success);
            let mut screens = app
                .world_mut()
                .query::<&defaria_client::machine_screen::MachineScreen>();
            let open = screens.iter(app.world()).count() > 0;
            return (faces(&app, IVec3::new(52, 20, 49)), open);
        }
    }
    panic!("script never finished");
}

/// With a wrench in hand, right click on a machine is the wrench's: it
/// never opens the machine's screen. Aimed at the machine's face where a
/// wire's blocked arm used to be, it opens the wire's face again; where a
/// wire is joined, it blocks the wire's face.
#[test]
fn a_wrench_on_a_machine_turns_the_conduit_joined_to_it() {
    // A furnace facing south, its power port at its back (north), with a
    // wire there.
    let setup = "wait 2\n\
                 fill 48 20 46 56 24 54 air\n\
                 fill 52 20 50 52 20 50 electric_furnace[facing=south]\n\
                 fill 52 20 49 52 20 49 wire\n\
                 hotbar 1 wrench\n\
                 wait 1\n\
                 hold MouseLeft 1\n\
                 wait 1\n";
    // Down at the end of the wire's arm reaching the furnace.
    let block = "camera 52.5 23 49.9 0 -89.4\nwait 1\nhold MouseRight 1\nwait 2\n";
    // Level, at the furnace's back beside the wire, through the wire's
    // voxel but clear of its core.
    let furnace = "camera 52.2 20.8 48.5 180 0\nwait 1\nhold MouseRight 1\nwait 2\n";
    let mut south = BlockedFaces::default();
    south.toggle(Direction::South);
    assert_eq!(run_game(&format!("{setup}{block}")), (Some(south), false));
    assert_eq!(run_game(&format!("{setup}{block}{furnace}")), (None, false));
    // Straight at the furnace, with the wire joined: it blocks the wire's
    // face, and opens no screen; with a pickaxe in hand, its screen.
    assert_eq!(run_game(&format!("{setup}{furnace}")), (Some(south), false));
    let pickaxe = setup.replace("hotbar 1 wrench", "hotbar 1 pickaxe");
    assert_eq!(run_game(&format!("{pickaxe}{furnace}")), (None, true));
}

const E: Direction = Direction::East;

fn faces(app: &App, pos: IVec3) -> Option<BlockedFaces> {
    let entity = crate::headless::server_world(app)
        .resource::<BlockEntities>()
        .get(pos)?;
    crate::headless::server_world(app)
        .get::<BlockedFaces>(entity)
        .copied()
}

fn distinct(ids: &[Option<u64>]) -> usize {
    let mut ids: Vec<_> = ids.iter().flatten().collect();
    ids.dedup();
    ids.len()
}
