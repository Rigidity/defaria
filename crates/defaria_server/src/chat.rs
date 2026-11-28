//! Chat on the server: what each player says goes to every player, with
//! their name, once it's cleaned (`defaria_core::chat::clean_text`) and
//! checked against [`MAX_MESSAGE`] and the player's [`ChatAllowance`];
//! players joining and leaving are announced. Everything said is logged,
//! so a dedicated server's console shows it.

use bevy::prelude::*;

use crate::server::{Connection, ReceiveSystems};
use defaria_core::{
    chat::{Allowance, ChatKind, ChatLine, MAX_MESSAGE, clean_text},
    net::{ClientMessage, ServerMessage},
};

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(announce_joining)
            .add_observer(announce_leaving)
            .add_systems(
                PreUpdate,
                (refill_allowances, relay_chat)
                    .chain()
                    .after(ReceiveSystems)
                    .in_set(ChatSystems),
            );
    }
}

/// Where what players said is passed on, in `PreUpdate`, after it's
/// received and before commands run.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatSystems;

/// How many messages (and, for a player who may not run commands,
/// commands) a player may still send.
#[derive(Component, Debug, Default)]
pub struct ChatAllowance(pub Allowance);

/// Sends every player `line`, and logs it.
pub fn broadcast(world: &mut World, line: ChatLine) {
    log_line(&line);
    let mut connections = world.query::<&mut Connection>();
    for mut connection in connections.iter_mut(world) {
        connection.send(ServerMessage::Chat(line.clone()));
    }
}

fn log_line(line: &ChatLine) {
    match &line.from {
        Some(from) => info!("<{from}> {}", line.text),
        None => info!("{}", line.text),
    }
}

fn refill_allowances(time: Res<Time<Real>>, mut players: Query<&mut ChatAllowance>) {
    for mut allowance in &mut players {
        allowance.0.refill(time.delta_secs());
    }
}

/// Passes on what each player said, to everyone, in the order it came;
/// what's empty, too long or too fast is refused, to the sender alone.
fn relay_chat(mut players: Query<(&mut Connection, Option<&Name>, &mut ChatAllowance)>) {
    let mut said = Vec::new();
    for (mut connection, name, mut allowance) in &mut players {
        let texts: Vec<String> = (connection.inbox().iter())
            .filter_map(|message| match message {
                ClientMessage::Chat(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        for text in texts {
            let refusal = |text: &str| {
                ServerMessage::Chat(ChatLine {
                    kind: ChatKind::Refused,
                    from: None,
                    text: text.into(),
                })
            };
            if !allowance.0.take() {
                connection.send(refusal("you're sending too fast; wait a moment"));
                continue;
            }
            let text = clean_text(&text);
            if text.chars().count() > MAX_MESSAGE {
                connection.send(refusal(&format!(
                    "that message is too long: at most {MAX_MESSAGE} characters"
                )));
                continue;
            }
            if text.is_empty() {
                continue;
            }
            said.push(ChatLine {
                kind: ChatKind::Said,
                from: Some(name.map_or("Player".into(), |name| name.to_string())),
                text,
            });
        }
    }
    for line in said {
        log_line(&line);
        for (mut connection, ..) in &mut players {
            connection.send(ServerMessage::Chat(line.clone()));
        }
    }
}

/// A player joining is told nothing new, but everyone else is told
/// they've joined; and each player gets an allowance.
fn announce_joining(
    add: On<Add<Connection>>,
    mut commands: Commands,
    names: Query<&Name>,
    mut connections: Query<(Entity, &mut Connection)>,
) {
    commands
        .entity(add.entity)
        .insert_if_new(ChatAllowance::default());
    let name = names
        .get(add.entity)
        .map_or("A player".into(), Name::to_string);
    let line = ChatLine {
        kind: ChatKind::Notice,
        from: None,
        text: format!("{name} joined"),
    };
    for (entity, mut connection) in &mut connections {
        if entity != add.entity {
            connection.send(ServerMessage::Chat(line.clone()));
        }
    }
}

/// Everyone else is told a player left.
fn announce_leaving(
    remove: On<Remove<Connection>>,
    names: Query<&Name>,
    mut connections: Query<(Entity, &mut Connection)>,
) {
    let name = names
        .get(remove.entity)
        .map_or("A player".into(), Name::to_string);
    let line = ChatLine {
        kind: ChatKind::Notice,
        from: None,
        text: format!("{name} left"),
    };
    for (entity, mut connection) in &mut connections {
        if entity != remove.entity {
            connection.send(ServerMessage::Chat(line.clone()));
        }
    }
}
