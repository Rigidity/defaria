//! Test helpers: the whole game with no GPU, driven by scripts.

use bevy::prelude::*;

use super::{HeadlessPlugin, SETTLE_TIMEOUT_FRAMES, Script, default_plugins, runner::ScriptRunner};
use defaria_core::{
    light::{ChunkLight, Light},
    player::{LocalPlayer, Look, Player, eyes},
    world::{ChunkCoord, ChunkMap},
};

/// The whole game with no GPU, running `script`, loading a smaller world
/// than the game does so tests are quick.
pub fn test_game(script: Script) -> App {
    let mut app = App::new();
    // Before the game's plugins, so its scripts load with them: what a
    // script may do then doesn't depend on how busy the machine is.
    app.insert_resource(defaria_core::script::ScriptSettings::untimed())
        .insert_resource(defaria_core::script::hooks::HookSettings::untimed());
    app.add_plugins((
        default_plugins(false).disable::<bevy::log::LogPlugin>(),
        crate::GamePlugin,
        HeadlessPlugin {
            script,
            rendering: false,
        },
    ))
    .insert_resource(defaria_core::streaming::StreamingSettings {
        radius: 4,
        ..default()
    });
    app.finish();
    app.cleanup();
    app
}

/// Runs `script` on a [`test_game`] app that has already finished its own,
/// updating it until this one has finished too, so a test can look at the
/// game between steps.
pub fn run_more(app: &mut App, script: &str) {
    let script = Script::parse(script).unwrap();
    app.world_mut()
        .resource_mut::<ScriptRunner>()
        .commands
        .extend(script.commands);
    for _ in 0..SETTLE_TIMEOUT_FRAMES {
        app.update();
        if app.world().resource::<ScriptRunner>().finished() {
            return;
        }
    }
    panic!("the script never finished");
}

pub use defaria_core::testing::settle;

/// Runs the whole game with no GPU until the script ends, and returns
/// how it exited.
pub(super) fn run_game(script: &str) -> (App, AppExit) {
    let mut app = test_game(Script::parse(script).unwrap());
    for _ in 0..SETTLE_TIMEOUT_FRAMES + 200 {
        app.update();
        if let Some(exit) = app.should_exit() {
            return (app, exit);
        }
    }
    panic!("script never finished");
}

pub(super) fn light_at(app: &mut App, pos: IVec3) -> Light {
    let (coord, local) = ChunkCoord::split(pos);
    let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
    app.world().get::<ChunkLight>(entity).unwrap().at(local)
}

/// Runs `script` in the test game until it ends.
pub(super) fn play(script: &str) -> App {
    let mut app = test_game(Script::parse(script).unwrap());
    let mut frames = 0;
    let exit = loop {
        app.update();
        frames += 1;
        if let Some(exit) = app.should_exit() {
            break exit;
        }
        assert!(frames < SETTLE_TIMEOUT_FRAMES, "script never finished");
    };
    assert_eq!(exit, AppExit::Success);
    app
}

/// The local player's look, and its eyes.
pub(super) fn camera(app: &mut App) -> (Look, Transform) {
    let (transform, player, look) = app
        .world_mut()
        .query_filtered::<(&Transform, &Player, &Look), With<LocalPlayer>>()
        .single(app.world())
        .unwrap();
    (*look, eyes(transform, player, look))
}

/// The world of the server a [`test_game`] plays on: where the world's
/// state is decided (block entities, networks, fluid flow, scripts). The
/// client's world has copies of the chunks and its player.
pub fn server(app: &mut App) -> &mut World {
    app.world_mut()
        .non_send_mut::<crate::IntegratedServer>()
        .into_inner()
        .app
        .world_mut()
}

/// [`server`], to read.
pub fn server_world(app: &App) -> &World {
    app.world()
        .non_send::<crate::IntegratedServer>()
        .app
        .world()
}
