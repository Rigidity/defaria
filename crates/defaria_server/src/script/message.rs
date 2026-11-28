//! The server's end of the mod message channel
//! (`defaria_core::script::channel`): what a client's scripts send runs
//! the same mod's `on_message(channel, value, player)` hooks here, and
//! what the server's scripts send (`net.send(player, ...)`,
//! `net.broadcast(...)`) goes to the players' clients.
//!
//! Clients aren't trusted: each connection may send only as many
//! messages as [`MessageBudget::server`] lets it (a client's own budget,
//! with room for messages that arrive bunched up), and what's over it,
//! too big, or for a mod with no `on_message` is dropped. A message's
//! hooks are called with the player's [`Caller::script`], so one player's
//! message that breaks a script stops only that script's handling of
//! that player's messages, until the scripts reload or the player goes.
//!
//! What arrives waits in [`PendingMessages`], oldest first, and runs a
//! frame at a time (in `Update`) within as many steps and as much time as
//! block hooks get in a tick ([`HookSettings`]), so however many players
//! send at once, messages slow down rather than the game. The queue is
//! bounded: past [`MAX_PENDING_MESSAGES`] in all, or [`MAX_PENDING_PER_PLAYER`]
//! of one player's, new ones are dropped. What the server's hooks sent
//! goes at the end of the frame they ran in.

use std::{collections::VecDeque, time::Instant};

use bevy::{platform::collections::HashSet, prelude::*};

use super::{
    api::{PlayerView, ScriptWorld, player_table},
    hooks::HookSettings,
};
use crate::server::Connection;
use defaria_core::{
    game_mode::GameMode,
    identity::PlayerId,
    net::{ClientMessage, ScriptMessage, ServerMessage},
    player::{Look, Player, eyes},
    script::{
        Caller, ScriptRuntime,
        channel::{
            MESSAGE_BURST, MessageBudget, ON_MESSAGE, Recipient, SERVER_SLACK, check, value_to_lua,
        },
    },
};

/// The most messages waiting to run, from every player.
pub const MAX_PENDING_MESSAGES: usize = 1024;
/// The most messages waiting to run from one player: as many as its
/// connection may send at once.
pub const MAX_PENDING_PER_PLAYER: usize = (MESSAGE_BURST * SERVER_SLACK) as usize;

pub(super) fn plugin(app: &mut App) {
    app.register_type::<ScriptMessages>()
        .register_required_components::<Connection, ScriptMessages>()
        .init_resource::<PendingMessages>()
        .add_observer(forget_players)
        .add_systems(Update, (receive_messages, run_messages).chain())
        .add_systems(Last, send_messages);
}

/// How many more script messages a player's client may send now.
#[derive(Component, Reflect, Debug)]
#[reflect(Component)]
pub struct ScriptMessages {
    budget: MessageBudget,
    /// Whether it's been warned about since it last kept to it.
    warned: bool,
}

impl Default for ScriptMessages {
    fn default() -> Self {
        Self {
            budget: MessageBudget::server(),
            warned: false,
        }
    }
}

/// The messages players' clients sent that haven't run yet, oldest
/// first, with who sent each.
#[derive(Resource, Debug, Default)]
pub struct PendingMessages(VecDeque<(Entity, ScriptMessage)>);

impl PendingMessages {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Queues what each player's client's scripts sent, within its budget.
///
/// The budget refills from `Time<Real>`, not the virtual clock: the
/// server's virtual clock stops while its players are paused, and their
/// clients' scripts may still send. Headless runs and tests step real
/// time by the fixed frame too (`TimeUpdateStrategy::ManualDuration`), so
/// it's the same every run there.
fn receive_messages(
    time: Res<Time<Real>>,
    mut pending: ResMut<PendingMessages>,
    mut players: Query<(Entity, &Connection, &mut ScriptMessages)>,
) {
    for (entity, connection, mut allowance) in &mut players {
        allowance.budget.refill(time.delta_secs_f64());
        let mut waiting = pending.0.iter().filter(|(from, _)| *from == entity).count();
        for message in connection.inbox() {
            let ClientMessage::Script(message) = message else {
                continue;
            };
            let room = waiting < MAX_PENDING_PER_PLAYER && pending.0.len() < MAX_PENDING_MESSAGES;
            if !allowance.budget.take() || !room {
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
            pending.0.push_back((entity, message.clone()));
            waiting += 1;
        }
    }
}

/// Calls `on_message` for the waiting messages, oldest first, until the
/// frame's steps or time are spent (the first always runs, and a frame
/// may go over by one message's calls). The rest wait for the next frame.
fn run_messages(
    settings: Res<HookSettings>,
    runtime: Option<ResMut<ScriptRuntime>>,
    mut pending: ResMut<PendingMessages>,
    mut world: ScriptWorld,
    players: Query<
        (
            Option<&PlayerId>,
            Option<(&Transform, &Player, &Look)>,
            Option<&GameMode>,
        ),
        With<Connection>,
    >,
) {
    let Some(mut runtime) = runtime else {
        pending.0.clear();
        return;
    };
    let start = runtime.steps_spent();
    let started = Instant::now();
    let mut count = 0;
    while (count == 0 || started.elapsed() < settings.time_per_tick)
        && runtime.steps_spent().saturating_sub(start) < settings.steps_per_tick
    {
        let Some((entity, message)) = pending.0.pop_front() else {
            break;
        };
        count += 1;
        // A player who's gone since is forgotten, with what they sent.
        let Ok((id, body, mode)) = players.get(entity) else {
            continue;
        };
        let view = PlayerView {
            id: id.copied(),
            pos: body.map_or(Vec3::ZERO, |(t, p, l)| eyes(t, p, l).translation),
            look: body.map_or(Vec3::NEG_Z, |(t, p, l)| *eyes(t, p, l).forward()),
            mode: mode.copied().unwrap_or_default(),
            sneaking: body.is_some_and(|(_, player, _)| player.sneaking),
            target: None,
        };
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

/// A player who's gone: what its messages disabled is enabled again (for
/// whoever gets its entity next), and what it sent that hasn't run is
/// dropped.
fn forget_players(
    remove: On<Remove<Connection>>,
    runtime: Option<ResMut<ScriptRuntime>>,
    mut pending: ResMut<PendingMessages>,
) {
    let player = remove.entity;
    if let Some(mut runtime) = runtime {
        runtime.forget_player(player);
    }
    pending.0.retain(|(from, _)| *from != player);
}

/// Sends what the server's scripts sent this frame to the players' clients.
fn send_messages(
    runtime: Option<Res<ScriptRuntime>>,
    mut players: Query<(Option<&PlayerId>, &mut Connection)>,
) {
    // Each id that isn't a player's logged once a frame.
    let mut unknown = HashSet::new();
    let Some(runtime) = runtime else {
        return;
    };
    for sent in runtime.outbox().take() {
        match sent.to {
            Recipient::Player(id) => {
                let player = players
                    .iter_mut()
                    .find(|(playing, _)| playing.is_some_and(|playing| *playing == id));
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
