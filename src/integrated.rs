//! A server in the game's own process, for single player: its own Bevy
//! app ([`ServerPlugin`]), held by the client app and stepped once a
//! client frame, connected to the client through a link in memory.
//!
//! The server's time follows the client's frames: it steps at the end of
//! each, as long as it, so headless runs and tests stay deterministic, and
//! what the player does reaches the server the frame it does it. Running it on its own thread instead is a later step.
//!
//! What the client app was set up with that the server needs (the world's
//! folder, the render distance, terrain and script settings, the content
//! already loaded) is copied over when the server starts, as play starts
//! (`GameState::Playing`), so settings given after the plugins count. When
//! play ends (back to the main menu), the server saves the world and goes.

use std::time::Duration;

use bevy::{
    app::{MainScheduleOrder, PluginsState},
    ecs::schedule::ScheduleLabel,
    prelude::*,
    time::TimeUpdateStrategy,
};
use defaria_client::{net::ServerConnection, pause::Paused, session::GameState};
use defaria_core::{
    content::Content,
    fluid::FluidSettings,
    net::link,
    save::WorldFolder,
    script::{ScriptSettings, hooks::HookSettings, random_tick::RandomTicks},
    server::{Incoming, PauseWithPlayers},
    streaming::StreamingSettings,
    worldgen::TerrainSettings,
};

use crate::ServerPlugin;

pub struct IntegratedServerPlugin;

impl Plugin for IntegratedServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Playing), start_server)
            .add_systems(OnExit(GameState::Playing), stop_server)
            .add_systems(Update, pause_with_the_player)
            .add_systems(Last, step_server)
            .init_schedule(StopServerOnExit)
            .add_systems(StopServerOnExit, stop_server_on_exit);
        app.world_mut()
            .resource_mut::<MainScheduleOrder>()
            .insert_after(Last, StopServerOnExit);
    }
}

/// Runs after [`Last`], so it sees an [`AppExit`] sent anywhere in the
/// frame (it's the frame's end: the app stops after it).
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct StopServerOnExit;

/// The server app this client plays on.
pub struct IntegratedServer {
    pub app: App,
}

/// Builds the server, connects to it, and lets its first frame run.
fn start_server(world: &mut World) {
    let mut app = App::new();
    forward::<Content>(world, &mut app);
    forward::<WorldFolder>(world, &mut app);
    forward::<StreamingSettings>(world, &mut app);
    forward::<TerrainSettings>(world, &mut app);
    forward::<ScriptSettings>(world, &mut app);
    forward::<HookSettings>(world, &mut app);
    forward::<RandomTicks>(world, &mut app);
    forward::<FluidSettings>(world, &mut app);
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        AssetPlugin::default(),
        ServerPlugin,
    ))
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    // In single player, pausing pauses the game.
    app.insert_resource(PauseWithPlayers(true));
    let (client, server) = link();
    app.world_mut().resource_mut::<Incoming>().0.push(server);
    world.insert_resource(ServerConnection(client));
    world.insert_non_send(IntegratedServer { app });
}

/// Copies the client's `R`, if it has one, to the server app.
fn forward<R: Resource + Clone>(client: &World, server: &mut App) {
    if let Some(resource) = client.get_resource::<R>() {
        server.insert_resource(resource.clone());
    }
}

/// Runs a server frame as long as the client's, at the end of the
/// client's: it takes what the client sent this frame and sends what
/// changed, which the client applies at the start of its next.
fn step_server(world: &mut World) {
    let delta = world.resource::<Time>().delta();
    let Some(mut server) = world.remove_non_send::<IntegratedServer>() else {
        return;
    };
    server
        .app
        .insert_resource(TimeUpdateStrategy::ManualDuration(delta));
    server.app.update();
    world.insert_non_send(server);
}

/// When the client exits, so does the server, saving the world. (A test
/// may go on updating an app whose script has ended, as the game does
/// after an exit it ignores; the server goes on with it.)
fn stop_server_on_exit(world: &mut World) {
    if world
        .get_resource::<Messages<AppExit>>()
        .is_none_or(|exits| exits.iter_current_update_messages().next().is_none())
    {
        return;
    }
    with_server(world, |server| {
        server.write_message(AppExit::Success);
    });
    if let Some(mut server) = world.remove_non_send::<IntegratedServer>() {
        server.app.update();
        world.insert_non_send(server);
    }
}

/// Leaving the world: the server saves it (as it does when the game
/// exits) and goes, and so does the connection to it.
fn stop_server(world: &mut World) {
    if let Some(mut server) = world.remove_non_send::<IntegratedServer>() {
        server.app.world_mut().write_message(AppExit::Success);
        server.app.update();
    }
    world.remove_resource::<ServerConnection>();
    world.remove_resource::<WorldFolder>();
    if let Some(mut time) = world.get_resource_mut::<Time<Virtual>>() {
        time.unpause();
    }
}

/// In single player the client pauses with its player too, so its own
/// player stops where it is.
fn pause_with_the_player(
    paused: Res<Paused>,
    server: Option<NonSend<IntegratedServer>>,
    mut time: ResMut<Time<Virtual>>,
) {
    let pause = paused.0 && server.is_some();
    if pause != time.is_paused() {
        if pause {
            time.pause();
        } else {
            time.unpause();
        }
    }
}

/// Runs `f` on the integrated server's world, if there is one.
pub fn with_server<R>(world: &mut World, f: impl FnOnce(&mut World) -> R) -> Option<R> {
    let mut server = world.remove_non_send::<IntegratedServer>()?;
    let result = f(server.app.world_mut());
    world.insert_non_send(server);
    Some(result)
}
