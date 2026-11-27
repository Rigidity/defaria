//! The entities the server shows the client (`defaria_core::replication`):
//! other players now. Each is spawned as a [`Replica`] when the server
//! says it came into range, and despawned when it says it left.
//!
//! The server sends where they are in snapshots, a few a second, which may
//! be lost or late. The client draws every replica [`DELAY`] behind the
//! server's clock ([`Playback`]), between the two snapshots around that
//! moment, so they move smoothly through a lost snapshot or two. Past the
//! newest it holds them there rather than guess.
//!
//! A player is drawn as a box as tall and wide as its body, turned the way
//! it looks, lower while it sneaks: placeholder art.

use std::collections::VecDeque;

use bevy::{platform::collections::HashMap, prelude::*};
use defaria_core::{
    net::ServerMessage,
    player::{EYE_HEIGHT, SIZE, SNEAK_EYE_HEIGHT},
    replication::{Kind, Pose, SNAPSHOT_INTERVAL},
};

use crate::{net::FromServer, session::GameState};

/// How far behind the server's clock replicas are drawn: two snapshots'
/// time, so one can be lost without a stop.
pub const DELAY: f64 = 2.0 * SNAPSHOT_INTERVAL;
/// How far the playback clock may drift from where it should be before
/// it's put there, rather than eased there.
const MAX_DRIFT: f64 = 0.5;

pub struct ReplicationPlugin;

impl Plugin for ReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Replica>()
            .init_resource::<Playback>()
            .init_resource::<Replicas>()
            .add_systems(OnExit(GameState::Playing), forget)
            .add_systems(Update, (take_entities, play_back).chain());
    }
}

/// An entity the server shows: which (its id while it's in range) and
/// what kind.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct Replica {
    pub id: u64,
    pub kind: Kind,
}

/// Each replica's entity, by its id: kept as messages are taken, so a
/// replica spawned and despawned in one frame (in and out of range) goes,
/// though its entity isn't there yet.
#[derive(Resource, Debug, Default)]
struct Replicas(HashMap<u64, Entity>);

/// A replica's poses from the snapshots still to be drawn, by the server's
/// time, oldest first.
#[derive(Component, Debug, Default)]
struct Snapshots(VecDeque<(f64, Pose)>);

/// The server time replicas are drawn at, and the newest snapshot's.
#[derive(Resource, Debug, Default)]
pub struct Playback {
    pub time: Option<f64>,
    newest: f64,
}

/// A world left: the next starts afresh.
fn forget(mut playback: ResMut<Playback>, mut replicas: ResMut<Replicas>) {
    *playback = Playback::default();
    replicas.0.clear();
}

/// Spawns and despawns replicas, and adds snapshots' poses to them.
fn take_entities(
    mut commands: Commands,
    mut messages: MessageReader<FromServer>,
    mut playback: ResMut<Playback>,
    mut ids: ResMut<Replicas>,
    mut replicas: Query<&mut Snapshots>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut look: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    for FromServer(message) in messages.read() {
        match message {
            ServerMessage::Spawn { id, kind, pose } => {
                // Where it was when it came into range, from now on.
                let since = playback.time.unwrap_or(f64::NEG_INFINITY);
                if let Some(old) = ids.0.remove(id) {
                    commands.entity(old).despawn();
                }
                let mut replica = commands.spawn((
                    Name::new(format!("{kind:?}")),
                    Replica {
                        id: *id,
                        kind: *kind,
                    },
                    Snapshots(VecDeque::from([(since, *pose)])),
                    figure(*kind, pose),
                    Visibility::default(),
                    DespawnOnExit(GameState::Playing),
                ));
                if let Some((mesh, material)) = player_look(&mut look, &mut meshes, &mut materials)
                {
                    replica.insert((Mesh3d(mesh), MeshMaterial3d(material)));
                }
                ids.0.insert(*id, replica.id());
            }
            ServerMessage::Despawn(id) => {
                if let Some(entity) = ids.0.remove(id) {
                    commands.entity(entity).despawn();
                }
            }
            ServerMessage::Snapshot { time, entities } => {
                playback.newest = playback.newest.max(*time);
                playback.time.get_or_insert(*time - DELAY);
                for entity in entities {
                    // (One spawned this frame isn't there yet: it starts
                    // from where it came into range.)
                    let Some(mut snapshots) = ids
                        .0
                        .get(&entity.id)
                        .and_then(|replica| replicas.get_mut(*replica).ok())
                    else {
                        continue;
                    };
                    // One late behind a newer one is too late.
                    if snapshots.0.back().is_none_or(|(last, _)| *last < *time) {
                        snapshots.0.push_back((*time, entity.pose));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The one mesh and material every player's figure shares, where there's
/// drawing (not in tests without it).
fn player_look(
    look: &mut Option<(Handle<Mesh>, Handle<StandardMaterial>)>,
    meshes: &mut Option<ResMut<Assets<Mesh>>>,
    materials: &mut Option<ResMut<Assets<StandardMaterial>>>,
) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
    if look.is_none()
        && let (Some(meshes), Some(materials)) = (meshes, materials)
    {
        let size = SIZE.as_vec3();
        *look = Some((
            meshes.add(Cuboid::new(size.x, size.y, size.z)),
            materials.add(Color::srgb(0.85, 0.55, 0.35)),
        ));
    }
    look.clone()
}

/// Moves the playback clock on, easing it toward [`DELAY`] behind the
/// newest snapshot, and draws each replica at it.
fn play_back(
    time: Res<Time>,
    mut playback: ResMut<Playback>,
    mut replicas: Query<(&Replica, &mut Snapshots, &mut Transform)>,
) {
    let Some(mut now) = playback.time else {
        return;
    };
    let dt = time.delta_secs_f64();
    let target = playback.newest - DELAY;
    now += dt;
    if (target - now).abs() > MAX_DRIFT {
        now = target;
    } else {
        now += (target - now) * (2.0 * dt).min(1.0);
    }
    playback.time = Some(now);
    for (replica, mut snapshots, mut transform) in &mut replicas {
        let snapshots = &mut snapshots.0;
        // Only the last before now is needed of those already past.
        while snapshots.len() > 2 && snapshots[1].0 <= now {
            snapshots.pop_front();
        }
        let pose = match (snapshots.front(), snapshots.get(1)) {
            (Some(&(from, a)), Some(&(to, b))) if from <= now => {
                if from.is_finite() {
                    a.lerp(&b, ((now - from) / (to - from)).clamp(0.0, 1.0))
                } else if now >= to {
                    b
                } else {
                    a
                }
            }
            (Some(&(_, a)), _) => a,
            (None, _) => continue,
        };
        *transform = figure(replica.kind, &pose);
    }
}

/// Where a replica's figure is drawn: for a player, its body's middle,
/// turned with its look and lowered as much as its eyes are while it
/// sneaks.
fn figure(kind: Kind, pose: &Pose) -> Transform {
    match kind {
        Kind::Player => {
            let sink = if pose.crouched {
                EYE_HEIGHT - SNEAK_EYE_HEIGHT
            } else {
                0.0
            };
            let middle = pose.feet + bevy::math::DVec3::Y * (SIZE.y / 2.0 - sink);
            Transform::from_translation(middle.as_vec3()).with_rotation(pose.look.yaw_rotation())
        }
    }
}

#[cfg(test)]
mod tests {
    use defaria_core::{player::Look, replication::EntityPose};

    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<GameState>()
            .add_message::<FromServer>()
            .add_plugins(ReplicationPlugin);
        app
    }

    fn pose(x: f64) -> Pose {
        Pose {
            feet: bevy::math::DVec3::new(x, 0.0, 0.0),
            look: Look::default(),
            crouched: false,
        }
    }

    fn send(app: &mut App, messages: impl IntoIterator<Item = ServerMessage>) {
        for message in messages {
            app.world_mut().write_message(FromServer(message));
        }
        app.update();
    }

    fn replicas(app: &mut App) -> usize {
        let world = app.world_mut();
        world.query::<&Replica>().iter(world).count()
    }

    /// A replica comes and goes as the server says, even both in one
    /// frame (in and out of range at once), and comes again with the same
    /// id.
    #[test]
    fn replicas_come_and_go() {
        let mut app = app();
        let spawn = ServerMessage::Spawn {
            id: 7,
            kind: Kind::Player,
            pose: pose(0.0),
        };
        send(&mut app, [spawn.clone(), ServerMessage::Despawn(7)]);
        assert_eq!(replicas(&mut app), 0);
        send(&mut app, [spawn.clone()]);
        assert_eq!(replicas(&mut app), 1);
        send(&mut app, [spawn]);
        assert_eq!(replicas(&mut app), 1, "once, however often it's said");
        send(
            &mut app,
            [ServerMessage::Snapshot {
                time: 1.0,
                entities: vec![EntityPose {
                    id: 7,
                    pose: pose(4.0),
                }],
            }],
        );
        send(&mut app, [ServerMessage::Despawn(7)]);
        assert_eq!(replicas(&mut app), 0);
    }
}
