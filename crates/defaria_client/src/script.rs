//! Mods' scripts on the client: the same runtime the server runs them in
//! (`defaria_core::script`), with the client's hooks, which read the
//! client's copy of the world and change nothing in it.
//!
//! - `on_join()`: the client has joined a world (its player's first
//!   state arrived), once a world, and again after the scripts reload
//!   (a script changed, in a dev build), since they start afresh.
//! - `on_message(channel, value)`: the same mod's server scripts sent
//!   this client a message. What arrives before `on_join` has run waits
//!   for it (up to [`MAX_HELD_MESSAGES`]; past that the oldest go), so a
//!   script's `on_join` always comes first.
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

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::{
    net::{FromServer, Joined, SendSystems, ToServer},
    session::GameState,
};
use defaria_core::{
    net::{ClientMessage, ScriptMessage, ServerMessage},
    script::{
        Caller, ScriptRuntime, ScriptRuntimePlugin, ScriptsReloaded, Side,
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
            .init_resource::<HeldMessages>()
            .add_systems(OnExit(GameState::Playing), start_afresh)
            .add_systems(
                Update,
                (
                    join_again_on_reload.run_if(on_message::<ScriptsReloaded>),
                    run_on_join,
                    receive_messages,
                )
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(PostUpdate, send_messages.before(SendSystems));
    }
}

/// The most messages from the server held for `on_join` to run first.
pub const MAX_HELD_MESSAGES: usize = 256;

/// Whether this world's `on_join`s have run, since the scripts last
/// loaded.
#[derive(Resource, Debug, Default)]
struct JoinRan(bool);

/// The server's script messages that arrived before `on_join` ran,
/// oldest first.
#[derive(Resource, Debug, Default)]
struct HeldMessages(VecDeque<ScriptMessage>);

/// A world left: the scripts reload, and what they hadn't sent, or
/// hadn't been handed, is dropped.
fn start_afresh(
    mut runtime: ResMut<ScriptRuntime>,
    mut ran: ResMut<JoinRan>,
    mut held: ResMut<HeldMessages>,
) {
    runtime.reload(None);
    runtime.outbox().take();
    ran.0 = false;
    held.0.clear();
}

/// The scripts reloaded while playing: they start afresh, so they're
/// told they've joined again.
fn join_again_on_reload(mut ran: ResMut<JoinRan>) {
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

/// Calls `on_message` for what the server's scripts sent, once `on_join`
/// has run; until then it's held.
///
/// The budget refills from `Time<Real>` (as the server's does), which
/// headless runs and tests step by the fixed frame, so it's the same
/// every run there.
fn receive_messages(
    time: Res<Time<Real>>,
    ran: Res<JoinRan>,
    mut held: ResMut<HeldMessages>,
    mut messages: MessageReader<FromServer>,
    mut runtime: ResMut<ScriptRuntime>,
    world: ChunkReader,
) {
    runtime.outbox().refill(time.delta_secs_f64());
    for FromServer(message) in messages.read() {
        if let ServerMessage::Script(message) = message {
            if held.0.len() == MAX_HELD_MESSAGES {
                held.0.pop_front();
            }
            held.0.push_back(message.clone());
        }
    }
    if !ran.0 {
        return;
    }
    for message in std::mem::take(&mut held.0) {
        let message = &message;
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
