//! The server's side of replication (see `defaria_core::replication`): each
//! client is told of the replicated entities in the chunks it has, and
//! sent snapshots of where they are.

use bevy::{platform::collections::HashSet, prelude::*};

use super::Connection;
use defaria_core::{
    net::ServerMessage,
    physics::Body,
    player::{Look, Player},
    replication::{EntityPose, Pose, Replicated, SNAPSHOT_INTERVAL},
    world::ChunkCoord,
};

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
        if seen.is_empty() || !(snapshot_due(&mut interest.next_snapshot, now) || new) {
            continue;
        }
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

/// How close to its time a snapshot must be to be sent, for rounding.
const EPSILON: f64 = 1e-6;

/// Whether a snapshot is due at `now`, with the next due at `next`; if so,
/// the one after is set an interval on, so they keep to twenty a second
/// whatever the frame rate (at 60 fps, every third frame), unless they've
/// fallen more than an interval behind (the first, or after a long
/// frame), when it's an interval from now.
fn snapshot_due(next: &mut f64, now: f64) -> bool {
    if now + EPSILON < *next {
        return false;
    }
    *next = if now - *next < SNAPSHOT_INTERVAL {
        *next + SNAPSHOT_INTERVAL
    } else {
        now + SNAPSHOT_INTERVAL
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot goes every third frame at 60 frames a second, twenty a
    /// second, however the frames' times round; every seventh or eighth
    /// at 144, and every frame or two at 30: twenty a second still.
    #[test]
    fn snapshots_keep_to_twenty_a_second() {
        for (fps, gaps) in [(60.0, &[3][..]), (144.0, &[7, 8][..]), (30.0, &[1, 2][..])] {
            let (mut next, mut now, mut sent) = (0.0, 0.0, Vec::new());
            for frame in 0..(fps as usize * 10) {
                if snapshot_due(&mut next, now) {
                    sent.push(frame);
                }
                now += 1.0 / fps;
            }
            assert!(
                (199..=201).contains(&sent.len()),
                "{fps} fps: {}",
                sent.len()
            );
            for pair in sent.windows(2) {
                assert!(gaps.contains(&(pair[1] - pair[0])), "{fps} fps: {sent:?}");
            }
        }
    }
}
