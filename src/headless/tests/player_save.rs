//! The player saved with the world, by their install's id.

use bevy::prelude::*;
use defaria_core::{
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar},
    inventory::{Inventory, player::*},
    player::LocalPlayer,
    save::{DataFolder, WorldFolder},
    streaming::StreamingSettings,
    testing::temp_folder,
};
use defaria_server::{inventory::player::KeepSavedPlayer, save::player as file};

use crate::{
    headless::{HeadlessPlugin, Script, default_plugins},
    identity::LocalIdentity,
};

/// Through the real game, in a world on disk: the active slot is saved
/// when the game exits, and the next run, by the same install, starts
/// with it.
#[test]
fn the_player_is_saved_with_the_world() {
    let folder = temp_folder("player-inventory");
    let data = temp_folder("player-inventory-data");
    let run = |script: &str| {
        let mut app = App::new();
        app.insert_resource(WorldFolder(folder.clone()))
            .insert_resource(DataFolder(data.clone()))
            .add_plugins((
                default_plugins(false).disable::<bevy::log::LogPlugin>(),
                crate::GamePlugin,
                HeadlessPlugin {
                    script: Script::parse(script).unwrap(),
                    rendering: false,
                },
            ));
        app.insert_resource(StreamingSettings {
            radius: 2,
            ..default()
        });
        app.finish();
        app.cleanup();
        for _ in 0..crate::headless::SETTLE_TIMEOUT_FRAMES {
            app.update();
            if app.should_exit().is_some() {
                break;
            }
        }
        let mut players = app
            .world_mut()
            .query_filtered::<(&Inventory, &Hotbar, &ActiveSlot, &GameMode), With<LocalPlayer>>();
        let (inventory, hotbar, active, mode) = players.single(app.world()).unwrap();
        (inventory.clone(), hotbar.clone(), *active, *mode)
    };
    // Slot 3, and survival (C, with the cursor locked to play).
    let first = run("hold Digit3 1\nwait 1\nhold MouseLeft 1\nwait 1\nhold KeyC 1\nwait 2");
    assert_eq!(first.2, ActiveSlot(2));
    assert_eq!(first.3, GameMode::Survival);
    let second = run("wait 1");
    assert_eq!(second, first);
    // Back in survival, with survival's weight limit.
    assert_eq!(second.0.max_weight, Some(PLAYER_MAX_WEIGHT));
    let id = LocalIdentity::load(Some(&data)).identity.id;
    assert!(file::path(&folder, id).exists());
}

/// A player's file the game can't read (here, from a newer build) is
/// left as it is: the player starts new, and nothing saves over it.
#[test]
fn an_unreadable_player_file_is_kept() {
    let folder = temp_folder("player-newer-kept");
    let data = temp_folder("player-newer-kept-data");
    let id = LocalIdentity::load(Some(&data)).identity.id;
    let path = file::path(&folder, id);
    let newer = "{ format: 99, active_slot: 0, hotbar: [], inventory: [] }";
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, newer).unwrap();
    let mut app = App::new();
    app.insert_resource(WorldFolder(folder.clone()))
        .insert_resource(DataFolder(data))
        .add_plugins((
            default_plugins(false).disable::<bevy::log::LogPlugin>(),
            crate::GamePlugin,
            HeadlessPlugin {
                script: Script::parse("wait 1").unwrap(),
                rendering: false,
            },
        ));
    app.insert_resource(StreamingSettings {
        radius: 2,
        autosave_seconds: 0.0,
        ..default()
    });
    app.finish();
    app.cleanup();
    for _ in 0..crate::headless::SETTLE_TIMEOUT_FRAMES {
        app.update();
        if app.should_exit().is_some() {
            break;
        }
    }
    let server = crate::headless::server(&mut app);
    let mut kept = server.query_filtered::<(), With<KeepSavedPlayer>>();
    assert_eq!(kept.iter(server).count(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
}
