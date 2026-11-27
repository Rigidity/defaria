//! The server's side of replication (see `crate::replication`): each
//! client is told of the replicated entities in the chunks it has, and
//! sent snapshots of where they are.

use bevy::{platform::collections::HashSet, prelude::*};

use super::Connection;
use crate::{
    net::ServerMessage,
    physics::Body,
    player::{Look, Player},
    replication::{EntityPose, Pose, Replicated},
    world::ChunkCoord,
};

/// How often snapshots are sent: twenty a second.
pub const SNAPSHOT_INTERVAL: f64 = 1.0 / 20.0;

/// What a client knows of the replicated entities: which it's been told
/// of, and when its next snapshot is due.
#[derive(Component, Debug, Default)]
pub struct Interest {
    known: HashSet<u64>,
    next_snapshot: f64,
}

/// Tells each client of the replicated entities that came into or left
/// the chunks it has (its own player aside), and sends it snapshots of
/// them while there are any.
pub(super) fn send_entities(
    time: Res<Time>,
    entities: Query<(
        Entity,
        &Replicated,
        &Transform,
        Option<&Body>,
        Option<&Look>,
        Option<&Player>,
    )>,
    mut connections: Query<(Entity, &mut Connection, &mut Interest)>,
) {
    let now = time.elapsed_secs_f64();
    let all: Vec<(u64, Replicated, IVec3, Pose)> = entities
        .iter()
        .map(|(entity, replicated, transform, body, look, player)| {
            let feet = body.map_or(transform.translation.as_dvec3(), |body| body.position);
            let pose = Pose {
                feet,
                look: look.copied().unwrap_or_default(),
                crouched: player.is_some_and(|player| player.sneaking),
            };
            let chunk = ChunkCoord::split(feet.floor().as_ivec3()).0.0;
            (entity.to_bits(), *replicated, chunk, pose)
        })
        .collect();
    for (me, mut connection, mut interest) in &mut connections {
        let me = me.to_bits();
        let seen: Vec<&(u64, Replicated, IVec3, Pose)> = all
            .iter()
            .filter(|(id, _, chunk, _)| *id != me && connection.sent.contains(chunk))
            .collect();
        let ids: HashSet<u64> = seen.iter().map(|(id, ..)| *id).collect();
        let gone: Vec<u64> = interest.known.difference(&ids).copied().collect();
        for id in gone {
            interest.known.remove(&id);
            connection.send(ServerMessage::Despawn(id));
        }
        let mut new = false;
        for (id, Replicated(kind), _, pose) in &seen {
            if interest.known.insert(*id) {
                new = true;
                connection.send(ServerMessage::Spawn {
                    id: *id,
                    kind: *kind,
                    pose: *pose,
                });
            }
        }
        if seen.is_empty() || (now < interest.next_snapshot && !new) {
            continue;
        }
        interest.next_snapshot = now + SNAPSHOT_INTERVAL;
        let entities = seen
            .iter()
            .map(|(id, _, _, pose)| EntityPose {
                id: *id,
                pose: *pose,
            })
            .collect();
        connection.send(ServerMessage::Snapshot {
            time: now,
            entities,
        });
    }
}
