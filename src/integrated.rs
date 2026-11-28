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
//!
//! With a [`RemoteServer`] (joined from the Multiplayer screen or with
//! `--connect`, in the background: `connecting`), play is on that server
//! instead, over the network (`quic`), and no server starts here. The
//! client plays there with the content built from that server's mods
//! (`mod_transfer`), fetched into the data folder's cache as needed, and
//! its own again (single player's, [`SinglePlayerContent`]) once it
//! leaves.
//!
//! Either way the player is this install's ([`local_identity`]): the
//! server in the game's own process takes its id as it is (there's no
//! handshake in memory), so a world played alone and then hosted keeps
//! the same player. A remote server's key is checked against the one
//! remembered for its address (`servers.json5`,
//! `defaria_core::server_list`), or remembered, the first time.

use std::time::Duration;

use bevy::{
    app::MainScheduleOrder, ecs::schedule::ScheduleLabel, prelude::*, time::TimeUpdateStrategy,
};
use defaria_client::{
    content_swap::swap_content, net::ServerConnection, pause::Paused, session::GameState,
};
use defaria_core::{
    content::Content,
    net::link,
    save::WorldFolder,
    script::ScriptSettings,
    server_list::{Fingerprint, ServerList, normalize_address},
    streaming::StreamingSettings,
};
use defaria_server::{
    fluid::FluidSettings,
    script::{hooks::HookSettings, random_tick::RandomTicks},
    server::{Incoming, Joining, PauseWithPlayers},
    worldgen::TerrainSettings,
};

use crate::{
    connecting::{RemoteServer, leave_the_server},
    identity::{LocalIdentity, local_identity},
    mod_transfer::Library,
    quic::{ConnectError, ConnectOptions, Connected},
};

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

/// The client's own content, while it plays with a server's: put back
/// when it leaves.
#[derive(Resource)]
pub struct SinglePlayerContent(pub Content);

/// Builds the server, connects to it, and lets its first frame run;
/// unless the client is playing on a [`RemoteServer`], already connected.
fn start_server(world: &mut World) {
    if world.contains_resource::<RemoteServer>() {
        return;
    }
    let local = local_identity(world);
    let mut app = App::new();
    forward::<Content>(world, &mut app);
    forward::<WorldFolder>(world, &mut app);
    forward::<StreamingSettings>(world, &mut app);
    forward::<TerrainSettings>(world, &mut app);
    forward::<ScriptSettings>(world, &mut app);
    forward::<HookSettings>(world, &mut app);
    forward::<RandomTicks>(world, &mut app);
    forward::<FluidSettings>(world, &mut app);
    crate::server::build_server(&mut app);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    // In single player, pausing pauses the game.
    app.insert_resource(PauseWithPlayers(true));
    let (client, server) = link();
    app.world_mut().resource_mut::<Incoming>().0.push(Joining {
        id: local.identity.id,
        name: local.name,
        link: server,
        in_process: true,
    });
    world.insert_resource(ServerConnection(client));
    world.insert_non_send(IntegratedServer { app });
}

/// Connects to the server at `address` (normalized, as the server list
/// keeps addresses) as `local`, taking its mods from `library` where it
/// has them, checking the server's key against the one the saved servers
/// in `folder` remember for it, and remembering it if there's none yet
/// (the file read again just before it's written, so what another copy of
/// the game added meanwhile is kept). A key that can't be saved fails the
/// connection: otherwise every connection would be a first visit, trusting
/// any key. An install whose key couldn't be read doesn't connect: on a
/// server its stand-in would be a stray player, not them.
pub fn connect_remembering(
    address: &str,
    library: Library,
    local: &LocalIdentity,
    folder: Option<&std::path::Path>,
) -> Result<Connected, ConnectError> {
    connect_remembering_with(
        address,
        library,
        local,
        folder,
        None,
        ConnectOptions::default(),
    )
}

/// [`connect_remembering`], as `options` say (`quic::connect_with`). With
/// `trust_key`, the server's key must be that one rather than the one
/// remembered (the player chose to trust it, seeing it), and replaces the
/// remembered one once the connection shows the server holds it, if the
/// remembered one is still what it was. Called off (`options`), it never
/// remembers anything.
pub fn connect_remembering_with(
    address: &str,
    library: Library,
    local: &LocalIdentity,
    folder: Option<&std::path::Path>,
    trust_key: Option<Fingerprint>,
    options: ConnectOptions,
) -> Result<Connected, ConnectError> {
    let address = &normalize_address(address)?;
    if let Some(error) = &local.damaged {
        return Err(ConnectError::Failed(format!(
            "not joining {address}: this game's key couldn't be read ({error}), so the \
             server wouldn't know you. Fix or move that file and start the game again"
        )));
    }
    let mut known = ServerList::load(folder)?;
    let remembered = known.fingerprint(address);
    let expected = trust_key.or(remembered);
    let cancelled = options.cancelled.clone();
    let connected = crate::quic::connect_with(
        address,
        library,
        &local.identity,
        &local.name,
        expected,
        options,
    )?;
    if cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
        return Err(ConnectError::Failed(crate::quic::CANCELLED.to_string()));
    }
    let saved = match (remembered, trust_key) {
        (None, _) => {
            info!(
                "trusting {address}, the first time, as the server with fingerprint {}",
                connected.fingerprint
            );
            known.remember(address, connected.fingerprint)
        }
        (Some(old), Some(new)) if old != new => {
            info!("trusting {address}'s new key {new}, in place of {old}");
            known.replace_key(address, old, new)
        }
        _ => Ok(()),
    };
    if let Err(error) = saved {
        return Err(ConnectError::Failed(format!(
            "connected, but couldn't save the server's key, so it couldn't be checked next \
             time; not joining ({error})"
        )));
    }
    Ok(connected)
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
    leave_the_server(world);
    world.remove_resource::<ServerConnection>();
    world.remove_resource::<WorldFolder>();
    // Off a server: single player's content again.
    if let Some(SinglePlayerContent(own)) = world.remove_resource::<SinglePlayerContent>() {
        swap_content(world, own);
    }
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
