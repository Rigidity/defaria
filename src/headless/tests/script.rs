//! Scripts in the game.

use bevy::prelude::*;
use defaria_core::{
    block_entity::{BlockData, BlockEntities, Value},
    machine::EnergyBuffer,
    script::ScriptRuntime,
};

use crate::headless::{Script, run_more, settle, test_game};

/// The base game's scripts load with none failing.
#[test]
fn the_game_starts_its_script_runtime() {
    let mut app = crate::headless::test_game(crate::headless::Script::default());
    app.update();
    let runtime = crate::headless::server_world(&app).resource::<ScriptRuntime>();
    assert!(runtime.load_errors().is_empty());
}

/// In open sky a panel makes its 60 W, and counts it in its data;
/// roofed over, it makes nothing. What it counted survives its chunk
/// being unloaded and loaded again.
#[test]
fn a_panel_makes_energy_from_the_sky_and_keeps_its_count() {
    let script = Script::parse("camera 40.5 125 40.5 0 -60\nwait 2").unwrap();
    let mut app = test_game(script);
    settle(&mut app);
    let (open, roofed) = (v(40, 120, 38), v(42, 120, 38));
    run_more(
        &mut app,
        "fill 40 120 38 40 120 38 solar_panel\n\
         fill 42 120 38 42 120 38 solar_panel\n\
         fill 42 121 38 42 121 38 stone\n\
         wait 2",
    );
    let (made, stored) = panel_data(&app, open);
    run_more(&mut app, "wait 120");
    let (made_later, stored_later) = panel_data(&app, open);
    // Two seconds of full sky: 120 J, give or take a tick at either
    // end.
    let gained = made_later - made;
    assert!((114..=126).contains(&gained), "made {gained} J");
    assert_eq!(stored_later - stored, gained as u64);
    assert_eq!(made_later as u64, stored_later, "nothing else filled it");
    assert_eq!(panel_data(&app, roofed), (0, 0));

    // Roofed over, it stops, and its count stays as it is.
    run_more(&mut app, "fill 40 121 38 40 121 38 stone\nwait 2");
    let (kept, _) = panel_data(&app, open);
    run_more(&mut app, "wait 20");
    assert_eq!(panel_data(&app, open).0, kept);
    // Away, so its chunk unloads, and back: it's read from the save.
    let before = crate::headless::server_world(&app)
        .resource::<BlockEntities>()
        .get(open);
    run_more(
        &mut app,
        "camera 5000.5 125 40.5 0 -60\nsettle\ncamera 40.5 125 40.5 0 -60\nsettle\nwait 2",
    );
    assert_ne!(
        crate::headless::server_world(&app)
            .resource::<BlockEntities>()
            .get(open),
        before
    );
    assert_eq!(panel_data(&app, open), (kept, kept as u64));
}

fn panel_data(app: &App, pos: IVec3) -> (i64, u64) {
    let entity = crate::headless::server_world(&app)
        .resource::<BlockEntities>()
        .get(pos)
        .unwrap();
    let made = match crate::headless::server_world(&app)
        .get::<BlockData>(entity)
        .unwrap()
        .0["made"]
    {
        Value::Int(n) => n,
        Value::Float(n) => n as i64,
        ref other => panic!("made is {other:?}"),
    };
    let stored = crate::headless::server_world(&app)
        .get::<EnergyBuffer>(entity)
        .unwrap()
        .stored;
    (made, stored)
}

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}
