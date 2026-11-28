//! The server's end of the mod message channel
//! (`defaria_core::script::channel`): what a client's scripts send runs
//! the same mod's `on_message(channel, value, player)` hooks here, and
//! what the server's scripts send (`net.send(player, ...)`,
//! `net.broadcast(...)`) goes to the players' clients.
//!
//! Clients aren't trusted: each connection may send only as many
//! messages as a client's scripts may ([`MessageBudget`]), and what's
//! over it, too big, or for a mod with no `on_message` is dropped. A
//! message's hooks are called with the player's [`Caller::script`], so
//! one player's message that breaks a script stops only that script's
//! handling of that player's messages, until the scripts reload.
//!
//! Messages are taken as they arrive, once a frame (in `Update`), not on
//! the simulation's ticks; what the server's hooks sent goes at the end
//! of the frame they ran in.

use bevy::{platform::collections::HashSet, prelude::*};

use super::api::{PlayerView, ScriptWorld, player_table};
use crate::server::Connection;
use defaria_core::{
    game_mode::GameMode,
    net::{ClientMessage, ServerMessage},
    player::{Look, Player, eyes},
    script::{
        Caller, ScriptRuntime,
        channel::{MessageBudget, ON_MESSAGE, Recipient, check, value_to_lua},
    },
};

pub(super) fn plugin(app: &mut App) {
    app.register_required_components::<Connection, ScriptMessages>()
        .add_systems(Update, receive_messages)
        .add_systems(Last, send_messages);
}

/// How many more script messages a player's client may send now.
#[derive(Component, Debug, Default)]
pub struct ScriptMessages {
    budget: MessageBudget,
    /// Whether it's been warned about since it last kept to it.
    warned: bool,
}

/// Calls `on_message` for what each player's client's scripts sent.
fn receive_messages(
    time: Res<Time<Real>>,
    runtime: Option<ResMut<ScriptRuntime>>,
    mut world: ScriptWorld,
    mut players: Query<(
        Entity,
        &Connection,
        &mut ScriptMessages,
        Option<(&Transform, &Player, &Look)>,
        Option<&GameMode>,
    )>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };
    for (entity, connection, mut allowance, body, mode) in &mut players {
        allowance.budget.refill(time.delta_secs_f64());
        let view = PlayerView {
            id: Some(entity.to_bits()),
            pos: body.map_or(Vec3::ZERO, |(t, p, l)| eyes(t, p, l).translation),
            look: body.map_or(Vec3::NEG_Z, |(t, p, l)| *eyes(t, p, l).forward()),
            mode: mode.copied().unwrap_or_default(),
            sneaking: body.is_some_and(|(_, player, _)| player.sneaking),
            target: None,
        };
        for message in connection.inbox() {
            let ClientMessage::Script(message) = message else {
                continue;
            };
            if !allowance.budget.take() {
                if !allowance.warned {
                    warn!("a player's scripts sent too many messages: dropping them");
                    allowance.warned = true;
                }
                continue;
            }
            allowance.warned = false;
            if let Err(error) = check(message) {
                warn!(
                    "dropped a player's message for mod `{}`: {error}",
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
                // Failures are logged, and disable that script for that
                // player.
                let _ = world.call::<_, ()>(
                    &mut runtime,
                    Caller::script(&script, Some(entity)),
                    &script,
                    ON_MESSAGE,
                    None,
                    |lua, _| {
                        Ok((
                            message.channel.as_str(),
                            value_to_lua(lua, &message.value)?,
                            player_table(lua, &view)?,
                        ))
                    },
                );
            }
        }
    }
}

/// Sends what the server's scripts sent this frame to the players' clients.
fn send_messages(
    runtime: Option<Res<ScriptRuntime>>,
    mut players: Query<(Entity, &mut Connection)>,
    mut unknown: Local<HashSet<u64>>,
) {
    let Some(runtime) = runtime else {
        return;
    };
    for sent in runtime.outbox().take() {
        match sent.to {
            Recipient::Player(id) => {
                let player = Entity::try_from_bits(id).and_then(|e| players.get_mut(e).ok());
                match player {
                    Some((_, mut connection)) => {
                        connection.send(ServerMessage::Script(sent.message));
                    }
                    // A player who's left, or an id that was never one.
                    None => {
                        if unknown.insert(id) {
                            debug!("a script sent a message to {id}, who isn't playing");
                        }
                    }
                }
            }
            Recipient::Everyone => {
                for (_, mut connection) in &mut players {
                    connection.send(ServerMessage::Script(sent.message.clone()));
                }
            }
            // Only a client's scripts send to the server.
            Recipient::Server => {}
        }
    }
}
