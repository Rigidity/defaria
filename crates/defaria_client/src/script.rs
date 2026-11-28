//! Mods' scripts on the client: the same runtime the server runs them in
//! (`defaria_core::script`), with the client's hooks, which read the
//! client's copy of the world and change nothing in it.
//!
//! - `on_join()`: the client has joined a world (its player's first
//!   state arrived), once a world.
//! - `on_message(channel, value)`: the same mod's server scripts sent
//!   this client a message.
//!
//! The gameplay hooks (`on_used`, `on_tick`, ...) are the server's, and
//! never run here. A client script reads the loaded chunks as the
//! server's do, but its world is read-only: changing a block is an error
//! (`script::api::call_read_only`). What it has to change it asks its
//! server's scripts for, with `net.send`, which this module sends at the
//! end of the frame, held to the budget a client's messages get
//! (`script::channel`).
//!
//! The scripts reload on leaving a world, so the next starts afresh.

use bevy::prelude::*;

use crate::{
    net::{FromServer, Joined, SendSystems, ToServer},
    session::GameState,
};
use defaria_core::{
    net::{ClientMessage, ServerMessage},
    script::{
        Caller, ScriptRuntime, ScriptRuntimePlugin, Side,
        api::{ChunkReader, call_read_only},
        channel::{ON_MESSAGE, Recipient, check, value_to_lua},
    },
};

/// The hook a client calls once it has joined a world.
pub const ON_JOIN: &str = "on_join";

pub struct ClientScriptPlugin;

impl Plugin for ClientScriptPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ScriptRuntimePlugin { side: Side::Client })
            .init_resource::<JoinRan>()
            .add_systems(OnExit(GameState::Playing), start_afresh)
            .add_systems(
                Update,
                (run_on_join, receive_messages)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(PostUpdate, send_messages.before(SendSystems));
    }
}

/// Whether this world's `on_join`s have run.
#[derive(Resource, Debug, Default)]
struct JoinRan(bool);

/// A world left: the scripts reload, and what they hadn't sent is
/// dropped.
fn start_afresh(mut runtime: ResMut<ScriptRuntime>, mut ran: ResMut<JoinRan>) {
    runtime.reload(None);
    runtime.outbox().take();
    ran.0 = false;
}

/// Every script's `on_join`, once the client has joined.
fn run_on_join(
    joined: Res<Joined>,
    mut ran: ResMut<JoinRan>,
    mut runtime: ResMut<ScriptRuntime>,
    world: ChunkReader,
) {
    if ran.0 || !joined.has_joined() {
        return;
    }
    ran.0 = true;
    let mods: Vec<String> = runtime
        .sources()
        .mods
        .iter()
        .map(|m| m.id.clone())
        .collect();
    for mod_id in mods {
        let scripts: Vec<String> = runtime
            .scripts_with_hook(&mod_id, ON_JOIN)
            .into_iter()
            .map(String::from)
            .collect();
        for script in scripts {
            // A failure is logged, and disables the script's hooks here.
            let _ = call_read_only::<_, ()>(
                &mut runtime,
                Caller::script(&script, None),
                &script,
                ON_JOIN,
                &world,
                |_| Ok(()),
            );
        }
    }
}

/// Calls `on_message` for what the server's scripts sent.
fn receive_messages(
    time: Res<Time<Real>>,
    mut messages: MessageReader<FromServer>,
    mut runtime: ResMut<ScriptRuntime>,
    world: ChunkReader,
) {
    runtime.outbox().refill(time.delta_secs_f64());
    for FromServer(message) in messages.read() {
        let ServerMessage::Script(message) = message else {
            continue;
        };
        if let Err(error) = check(message) {
            warn!(
                "dropped the server's message for mod `{}`: {error}",
                message.mod_id
            );
            continue;
        }
        let scripts: Vec<String> = runtime
            .scripts_with_hook(&message.mod_id, ON_MESSAGE)
            .into_iter()
            .map(String::from)
            .collect();
        for script in scripts {
            let _ = call_read_only::<_, ()>(
                &mut runtime,
                Caller::script(&script, None),
                &script,
                ON_MESSAGE,
                &world,
                |lua| Ok((message.channel.as_str(), value_to_lua(lua, &message.value)?)),
            );
        }
    }
}

/// Sends the server what the scripts sent this frame.
fn send_messages(runtime: Res<ScriptRuntime>, mut to_server: MessageWriter<ToServer>) {
    for sent in runtime.outbox().take() {
        if sent.to == Recipient::Server {
            to_server.write(ToServer(ClientMessage::Script(sent.message)));
        }
    }
}
