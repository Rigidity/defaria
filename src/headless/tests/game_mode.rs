//! Creative and survival in the game.

use bevy::prelude::*;
use defaria_client::{hotbar_ui::HotbarSlot, ui::ItemSlot};
use defaria_core::{
    content::{BlockState, base, block},
    game_mode::*,
    inventory::Inventory,
    player::LocalPlayer,
};

use crate::headless::{Script, run_more, settle, test_game};

/// Through the real game: in survival, breaking a stone puts one in the
/// inventory and placing it takes it out again; in creative, neither
/// changes the inventory. C switches between them.
#[test]
fn breaking_and_placing_round_trip_in_survival() {
    use crate::headless::{Script, run_more, settle, test_game};
    // A stone in the air, the camera two blocks over it looking down.
    let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
    let mut app = test_game(script);
    settle(&mut app);
    let at = IVec3::new(40, 120, 40);
    let stone = base().default_item("defaria:stone").unwrap();
    run_more(&mut app, "fill 40 120 40 40 120 40 stone\nwait 2");
    run_more(&mut app, "hold MouseLeft 1  # lock the cursor\nwait 1");
    assert_eq!(player(&mut app).1, GameMode::Creative);
    assert_eq!(player(&mut app).0.max_weight, None);
    let before = player(&mut app).0.count(stone);

    // Creative: it breaks, and nothing comes of it.
    run_more(&mut app, "hold MouseLeft 90\nwait 2");
    assert!(voxel(&mut app, at).is_air());
    assert_eq!(player(&mut app).0.count(stone), before);
    // Placing (slot 1 is stone) doesn't use any up.
    run_more(
        &mut app,
        "fill 40 119 40 40 119 40 soil\nwait 2\nhold MouseRight 1\nwait 2",
    );
    assert_eq!(voxel(&mut app, at), block("stone"));
    assert_eq!(player(&mut app).0.count(stone), before);

    run_more(&mut app, "hold KeyC 1\nwait 2");
    assert_eq!(player(&mut app).1, GameMode::Survival);
    assert_eq!(
        player(&mut app).0.max_weight,
        Some(crate::inventory::player::PLAYER_MAX_WEIGHT)
    );
    run_more(&mut app, "hold MouseLeft 90\nwait 2");
    assert!(voxel(&mut app, at).is_air());
    assert_eq!(player(&mut app).0.count(stone), before + 1);
    run_more(&mut app, "hold MouseRight 1\nwait 2");
    assert_eq!(voxel(&mut app, at), block("stone"));
    assert_eq!(player(&mut app).0.count(stone), before);
}

/// In survival a block whose item is too heavy to carry stays where it
/// is.
#[test]
fn a_full_inventory_leaves_the_block() {
    use crate::headless::{Script, run_more, settle, test_game};
    let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
    let mut app = test_game(script);
    settle(&mut app);
    run_more(&mut app, "fill 40 120 40 40 120 40 glass\nwait 2");
    run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold KeyC 1\nwait 2");
    {
        // Carrying as much as they can.
        let mut players = app
            .world_mut()
            .query_filtered::<&mut Inventory, With<LocalPlayer>>();
        let mut inventory = players.single_mut(app.world_mut()).unwrap();
        inventory.max_weight = Some(inventory.weight(base()));
    }
    run_more(&mut app, "hold MouseLeft 90\nwait 2");
    assert_eq!(
        voxel(&mut app, IVec3::new(40, 120, 40)),
        base().default_state("defaria:glass").unwrap()
    );
}

/// In survival, a hotbar slot whose item ran out places nothing, and
/// shows dimmed; in creative it places all the same, undimmed.
#[test]
fn survival_needs_the_item_to_place() {
    let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
    let mut app = test_game(script);
    settle(&mut app);
    let at = IVec3::new(40, 120, 40);
    let stone = base().default_item("defaria:stone").unwrap();
    run_more(&mut app, "fill 40 119 40 40 119 40 soil\nwait 2");
    run_more(&mut app, "hold MouseLeft 1\nwait 1");
    {
        let mut players = app
            .world_mut()
            .query_filtered::<&mut Inventory, With<LocalPlayer>>();
        let mut inventory = players.single_mut(app.world_mut()).unwrap();
        let all = inventory.count(stone);
        inventory.remove(stone, all);
    }
    let first_slot = |app: &mut App| {
        let mut slots = app.world_mut().query::<(&HotbarSlot, &ItemSlot)>();
        *slots
            .iter(app.world())
            .find(|(slot, _)| slot.0 == 0)
            .unwrap()
            .1
    };
    run_more(&mut app, "wait 2");
    assert!(!first_slot(&mut app).dimmed);
    run_more(&mut app, "hold KeyC 1\nwait 2");
    assert!(first_slot(&mut app).dimmed);
    run_more(&mut app, "hold MouseRight 1\nwait 2");
    assert!(voxel(&mut app, at).is_air());
    run_more(&mut app, "hold KeyC 1\nwait 2\nhold MouseRight 1\nwait 2");
    assert_eq!(voxel(&mut app, at), block("stone"));
}

fn voxel(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_cached_with(
            |pos: In<IVec3>, voxels: crate::world::Voxels| voxels.get(*pos),
            pos,
        )
        .unwrap()
        .unwrap()
}

fn player(app: &mut App) -> (Inventory, GameMode) {
    let mut players = app
        .world_mut()
        .query_filtered::<(&Inventory, &GameMode), With<LocalPlayer>>();
    let (inventory, mode) = players.single(app.world()).unwrap();
    (inventory.clone(), *mode)
}
