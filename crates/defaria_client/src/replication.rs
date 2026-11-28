//! The entities the server shows the client (`defaria_core::replication`):
//! other players now. Each is spawned as a [`Replica`] when the server
//! says it came into range, and despawned when it says it left.
//!
//! The server sends where they are in snapshots, twenty a second, which
//! may be lost or late. The client draws every replica a little behind
//! the server's clock ([`Playback`]), between the two snapshots around
//! that moment, so they move smoothly through lost snapshots. How far
//! behind follows how late snapshots have been arriving lately
//! (`Arrivals`): just over a snapshot's time on a good connection, up to
//! [`MAX_DELAY`] on a bad one. The playback clock runs a few percent
//! slower or faster to change it, so the change doesn't show. Past the
//! newest snapshot it holds them there rather than guess, and the clock
//! waits there too.
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

/// The least replicas are drawn behind the server's clock: a snapshot's
/// time and half again, so on a steady connection the next snapshot is
/// always there before it's needed, with room for a frame's rounding.
pub const MIN_DELAY: f64 = 1.5 * SNAPSHOT_INTERVAL;
/// The most replicas are drawn behind the server's clock: a run of lost
/// snapshots longer than this holds them each time.
pub const MAX_DELAY: f64 = 0.4;
/// How much more than the latest a snapshot arrived lately the delay is,
/// for frames and jitter.
const MARGIN: f64 = 0.02;
/// How far back, in seconds, [`Arrivals`] looks at how late snapshots
/// arrived.
const WINDOW: f64 = 10.0;
/// A gap between snapshots longer than this is the server having had
/// nothing to send (nobody in range), not snapshots lost.
const BREAK: f64 = 1.0;
/// How much faster or slower than time passes the playback clock may run
/// to reach the delay it should be at.
const MAX_RATE: f64 = 0.05;
/// How quickly the playback clock closes on where it should be: the
/// change in its rate, per second it's off.
const GAIN: f64 = 1.0;
/// How much earlier than any lately a snapshot can arrive before it means
/// the clocks moved apart (the client's lost time in a long frame, say)
/// rather than the connection getting quicker.
const EARLIER: f64 = 0.1;
/// How quickly the typical frame time follows each frame's: the share of
/// the difference taken a frame.
const FRAME_EASING: f64 = 0.1;

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

/// The server time replicas are drawn at, and when snapshots arrived.
#[derive(Resource, Debug, Default)]
pub struct Playback {
    pub time: Option<f64>,
    arrivals: Arrivals,
}

impl Playback {
    /// Whether the clock has reached the newest snapshot, so replicas hold
    /// where it put them until the next comes (after a run of lost ones).
    pub fn holding(&self) -> bool {
        self.time
            .zip(self.arrivals.newest)
            .is_some_and(|(time, newest)| time >= newest)
    }

    /// How far behind the server's clock replicas should be drawn now.
    pub fn delay(&self) -> f64 {
        self.arrivals.delay()
    }

    /// The newest snapshot's time, on the server's clock.
    pub fn newest(&self) -> Option<f64> {
        self.arrivals.newest
    }

    /// Notes that the snapshot of server time `time` arrived at `now` on
    /// the client's clock, in a frame `elapsed` seconds long.
    fn arrived(&mut self, time: f64, now: f64, elapsed: f64) {
        self.arrivals.arrived(time, now, elapsed);
    }

    /// The server's clock now, as far as the snapshots lately say: when a
    /// message sent now on the server's clock would have arrived.
    fn server_now(&self, now: f64) -> Option<f64> {
        Some(now - self.arrivals.base()?)
    }

    /// Moves the clock on by `elapsed` seconds, to `now` on the client's
    /// clock: toward the delay behind the server's clock it should be,
    /// a few percent faster or slower than time passes, and never past
    /// the newest snapshot. It's put where it should be only when it
    /// starts, and when snapshots are timed afresh (the clocks moved
    /// apart, or snapshots start again after a break) and that's ahead
    /// of it: so a delay coming down, however far, comes down a few
    /// percent of the time passing, and replicas never jump.
    fn advance(&mut self, now: f64, elapsed: f64) {
        self.arrivals.forget(now);
        self.arrivals.frame(elapsed);
        let Some(newest) = self.arrivals.newest else {
            return;
        };
        let retimed = std::mem::take(&mut self.arrivals.retimed);
        let target = self.arrivals.target(now);
        let time = match (self.time, target) {
            (Some(time), Some(target)) => {
                let time = time + elapsed;
                if retimed && target > time {
                    target
                } else {
                    time + elapsed * (GAIN * (target - time)).clamp(-MAX_RATE, MAX_RATE)
                }
            }
            (Some(time), None) => time + elapsed,
            (None, Some(target)) => target,
            (None, None) => return,
        };
        self.time = Some(time.min(newest));
    }
}

/// When snapshots arrived lately, which says how far behind the server's
/// clock to draw replicas so the next snapshot is there before it's
/// needed.
///
/// Each snapshot's offset is when it arrived, on the client's clock, less
/// its time on the server's. The earliest of the last [`WINDOW`] seconds
/// is the server's clock as seen from here, at its quickest: snapshots
/// arriving evenly all have it, and one that arrives late has a bigger
/// one. How stale the newest snapshot was just before one arrived is that
/// one's offset over the earliest (how late it was) plus the time since
/// the one before (how many were lost before it). The delay covers the
/// stalest of the window, with a [`MARGIN`], between [`MIN_DELAY`] and
/// [`MAX_DELAY`]: so a run of lost snapshots holds replicas once, the
/// first time it's that long, and not again while it's remembered.
///
/// The client's own frames aren't the network's lateness: snapshots that
/// arrived during a long frame are all taken at its end, so each counts
/// as arriving as much earlier as the frame was longer than usual (but
/// not earlier than the earliest lately).
#[derive(Debug, Default, Clone)]
struct Arrivals {
    /// The snapshots of the last [`WINDOW`] seconds that were the newest
    /// when they arrived, oldest first.
    recent: VecDeque<Arrival>,
    /// The newest snapshot's time, on the server's clock.
    newest: Option<f64>,
    /// How long the client's frames usually are, in seconds.
    usual_frame: Option<f64>,
    /// Whether the snapshots were timed afresh since the clock last moved
    /// on: the first, the window cleared, or the first after a break.
    retimed: bool,
}

/// A snapshot's arrival: when, on the client's clock; its offset; and the
/// time since the newest before it, on the server's clock.
#[derive(Debug, Clone, Copy)]
struct Arrival {
    at: f64,
    offset: f64,
    gap: f64,
}

impl Arrivals {
    /// Notes that the snapshot of server time `time` arrived at `now` on
    /// the client's clock, in a frame `elapsed` seconds long; whether it's
    /// the newest yet (one behind a newer one is too late to draw, and
    /// doesn't count).
    fn arrived(&mut self, time: f64, now: f64, elapsed: f64) -> bool {
        if self.newest.is_some_and(|newest| time <= newest) {
            return false;
        }
        let gap = match self.newest {
            Some(newest) if time - newest <= BREAK => time - newest,
            _ => {
                self.retimed = true;
                0.0
            }
        };
        self.newest = Some(time);
        self.forget(now);
        let mut offset = now - time;
        // Much earlier than any lately, or later by more than the cap:
        // the clocks have moved apart (a hitch on one side, or a pause),
        // so what arrived before says nothing of what's to come.
        if let Some(base) = self.base()
            && (offset < base - EARLIER || offset > base + MAX_DELAY)
        {
            self.recent.clear();
            self.retimed = true;
        }
        // Taken at the end of a long frame, it may have come any time in
        // it.
        let long = self
            .usual_frame
            .map_or(0.0, |usual| (elapsed - usual).max(0.0));
        if long > 0.0 {
            offset = self
                .base()
                .map_or(offset, |base| (offset - long).max(base).min(offset));
        }
        self.recent.push_back(Arrival {
            at: now,
            offset,
            gap,
        });
        true
    }

    /// Notes a frame `elapsed` seconds long, for how long they usually are.
    fn frame(&mut self, elapsed: f64) {
        let usual = self.usual_frame.get_or_insert(elapsed);
        *usual += (elapsed - *usual) * FRAME_EASING;
    }

    /// The earliest offset lately: the client's clock less the server's,
    /// as the quickest snapshots saw it.
    fn base(&self) -> Option<f64> {
        self.recent
            .iter()
            .map(|arrival| arrival.offset)
            .reduce(f64::min)
    }

    /// How far behind the server's clock replicas should be drawn.
    fn delay(&self) -> f64 {
        let Some(base) = self.base() else {
            return MIN_DELAY;
        };
        let stalest = (self.recent.iter())
            .map(|arrival| arrival.offset - base + arrival.gap)
            .fold(0.0, f64::max);
        (stalest + MARGIN).clamp(MIN_DELAY, MAX_DELAY)
    }

    /// The server time replicas should be drawn at, at `now` on the
    /// client's clock; `None` with no snapshots lately.
    fn target(&self, now: f64) -> Option<f64> {
        Some(now - self.base()? - self.delay())
    }

    /// Forgets the arrivals more than [`WINDOW`] before `now`.
    fn forget(&mut self, now: f64) {
        while self
            .recent
            .front()
            .is_some_and(|arrival| now - arrival.at > WINDOW)
        {
            self.recent.pop_front();
        }
    }
}

/// A world left: the next starts afresh.
fn forget(mut playback: ResMut<Playback>, mut replicas: ResMut<Replicas>) {
    *playback = Playback::default();
    replicas.0.clear();
}

/// Spawns and despawns replicas, and adds snapshots' poses to them.
fn take_entities(
    mut commands: Commands,
    time: Res<Time>,
    mut messages: MessageReader<FromServer>,
    mut playback: ResMut<Playback>,
    mut ids: ResMut<Replicas>,
    mut replicas: Query<&mut Snapshots>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut look: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    let (now, elapsed) = (time.elapsed_secs_f64(), time.delta_secs_f64());
    for FromServer(message) in messages.read() {
        match message {
            ServerMessage::Spawn { id, kind, pose } => {
                // Where it was when it came into range: on the server's
                // clock, about when the message was sent (or, before any
                // snapshot, from the start).
                let since = playback.server_now(now).unwrap_or(f64::NEG_INFINITY);
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
                playback.arrived(*time, now, elapsed);
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

/// Moves the playback clock on and draws each replica at it.
fn play_back(
    time: Res<Time>,
    mut playback: ResMut<Playback>,
    mut replicas: Query<(&Replica, &mut Snapshots, &mut Transform)>,
) {
    playback.advance(time.elapsed_secs_f64(), time.delta_secs_f64());
    let Some(now) = playback.time else {
        return;
    };
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

    /// Plays `seconds` of snapshots into `playback` at 60 frames a second
    /// from frame `start`: one every third frame, the server's clock 100
    /// seconds (and `skew`) ahead of the client's, arriving six frames
    /// later and `late(index)` more, unless `lost(index)`. Gives each
    /// frame's playback clock and whether it held.
    fn feed(
        playback: &mut Playback,
        start: u32,
        skew: f64,
        seconds: u32,
        lost: impl Fn(u32) -> bool,
        late: impl Fn(u32) -> u32,
    ) -> Vec<(f64, bool)> {
        let mut coming: Vec<(u32, f64)> = Vec::new();
        let mut played = Vec::new();
        for frame in start..start + seconds * 60 {
            let now = frame as f64 / 60.0;
            if frame % 3 == 0 && !lost(frame / 3) {
                coming.push((frame + 6 + late(frame / 3), 100.0 + skew + now));
            }
            coming.sort_by_key(|(due, _)| *due);
            while coming.first().is_some_and(|(due, _)| *due <= frame) {
                let (_, time) = coming.remove(0);
                playback.arrived(time, now, 1.0 / 60.0);
            }
            playback.advance(now, 1.0 / 60.0);
            played.push((playback.time.unwrap_or(f64::NAN), playback.holding()));
        }
        played
    }

    /// How fast the playback clock ran each frame, over how fast time
    /// passed.
    fn rates(played: &[(f64, bool)]) -> Vec<f64> {
        (played.windows(2))
            .map(|pair| (pair[1].0 - pair[0].0) * 60.0)
            .filter(|rate| !rate.is_nan())
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Snapshots arriving evenly are drawn at the least delay, a snapshot
    /// and a half behind, and the clock runs as time does.
    #[test]
    fn a_steady_connection_plays_at_the_floor() {
        let mut playback = Playback::default();
        let played = feed(&mut playback, 0, 0.0, 20, |_| false, |_| 0);
        assert!(close(playback.delay(), MIN_DELAY), "{}", playback.delay());
        assert!(played[1..].iter().all(|(_, held)| !held));
        let last = rates(&played[600..]);
        assert!(last.iter().all(|rate| close(*rate, 1.0)), "{last:?}");
        // As far behind the newest snapshot as the delay says, less the
        // frames since it arrived (two at most).
        let behind = playback.newest().unwrap() - playback.time.unwrap();
        assert!(
            behind > MIN_DELAY - 2.0 / 60.0 - 1e-6 && behind < MIN_DELAY + 1e-6,
            "{behind}"
        );
    }

    /// A run of three lost snapshots holds the replicas once, the first
    /// time; the delay then covers it, so the runs after it don't, and the
    /// clock changes speed by at most 5% to get there.
    #[test]
    fn runs_of_losses_are_covered() {
        let mut playback = Playback::default();
        // Three lost every two seconds, from the fourth second.
        let lost = |index: u32| index >= 80 && (10..13).contains(&(index % 40));
        let played = feed(&mut playback, 0, 0.0, 20, lost, |_| 0);
        // Four snapshots' time since the one before, and the margin.
        assert!(
            close(playback.delay(), 0.2 + MARGIN),
            "{}",
            playback.delay()
        );
        let holds: Vec<usize> = (0..played.len()).filter(|frame| played[*frame].1).collect();
        assert!(!holds.is_empty() && holds.iter().all(|frame| (240..300).contains(frame)));
        // Within 5% of time but where it holds.
        let unheld: Vec<(f64, bool)> = played[7..].to_vec();
        for (frame, pair) in unheld.windows(2).enumerate() {
            let rate = (pair[1].0 - pair[0].0) * 60.0;
            if !pair[0].1 && !pair[1].1 {
                assert!((rate - 1.0).abs() < MAX_RATE + 1e-9, "{rate} at {frame}");
            }
        }
    }

    /// Snapshots arriving late (some two frames later than the rest) are
    /// covered too.
    #[test]
    fn late_arrivals_are_covered() {
        let mut playback = Playback::default();
        let late = |index: u32| if index % 7 == 3 { 2 } else { 0 };
        let played = feed(&mut playback, 0, 0.0, 10, |_| false, late);
        let expected = SNAPSHOT_INTERVAL + 2.0 / 60.0 + MARGIN;
        assert!(close(playback.delay(), expected), "{}", playback.delay());
        assert!(played[300..].iter().all(|(_, held)| !held));
    }

    /// Once the losses stop, the delay comes back down to the floor a
    /// window later, and the clock catches up a few percent faster than
    /// time passes, no faster.
    #[test]
    fn the_delay_recovers() {
        let mut playback = Playback::default();
        feed(&mut playback, 0, 0.0, 5, |index| index % 20 < 3, |_| 0);
        assert!(playback.delay() > 0.15, "{}", playback.delay());
        let played = feed(&mut playback, 300, 0.0, 20, |_| false, |_| 0);
        assert!(close(playback.delay(), MIN_DELAY), "{}", playback.delay());
        assert!(played.iter().all(|(_, held)| !held));
        let rates = rates(&played);
        assert!(
            (rates.iter()).all(|rate| (rate - 1.0).abs() < MAX_RATE + 1e-9),
            "{:?}",
            (rates.iter()).find(|rate| (*rate - 1.0).abs() >= MAX_RATE + 1e-9)
        );
        assert!(rates.iter().any(|rate| *rate > 1.0 + MAX_RATE / 2.0));
        // Caught up, but for the last of its easing.
        let last = &rates[rates.len() - 60..];
        assert!(
            last.iter().all(|rate| (rate - 1.0).abs() < 1e-3),
            "{last:?}"
        );
    }

    /// However long a run of losses, the delay stays under the cap; and a
    /// gap of more than a second is nothing to send, not losses, so it
    /// leaves the delay alone.
    #[test]
    fn the_delay_is_capped() {
        let mut playback = Playback::default();
        feed(
            &mut playback,
            0,
            0.0,
            4,
            |index| (40..52).contains(&index),
            |_| 0,
        );
        assert!(close(playback.delay(), MAX_DELAY), "{}", playback.delay());

        let mut playback = Playback::default();
        let played = feed(
            &mut playback,
            0,
            0.0,
            6,
            |index| (40..70).contains(&index),
            |_| 0,
        );
        assert!(close(playback.delay(), MIN_DELAY), "{}", playback.delay());
        // The clock is put where it should be when they start again.
        assert!(played[300..].iter().all(|(_, held)| !held));
    }

    /// When the clocks move apart (here, the client's loses a second, as
    /// in a hitch longer than a frame can be), snapshots are timed afresh,
    /// rather than all seeming a second early (or late) for a window.
    #[test]
    fn a_hitch_times_snapshots_afresh() {
        let mut playback = Playback::default();
        feed(&mut playback, 0, 0.0, 5, |_| false, |_| 0);
        // The client loses a second: the server is a second further ahead.
        let played = feed(&mut playback, 300, 1.0, 5, |_| false, |_| 0);
        assert!(close(playback.delay(), MIN_DELAY), "{}", playback.delay());
        // (Held only until the first after it arrives: those on the way
        // were lost.)
        assert!(played[10..].iter().all(|(_, held)| !held));
        let rates = rates(&played[60..]);
        assert!(rates.iter().all(|rate| close(*rate, 1.0)), "{rates:?}");
    }

    /// A run of losses long enough to put the delay at the cap: once the
    /// window forgets it, the delay comes down to the floor, the clock
    /// running at most 5% fast to get there, so replicas never move much
    /// faster than they did.
    #[test]
    fn the_delay_comes_down_from_the_cap_smoothly() {
        let mut playback = Playback::default();
        // Twelve lost at the second second.
        let played = feed(
            &mut playback,
            0,
            0.0,
            30,
            |index| (40..52).contains(&index),
            |_| 0,
        );
        assert!(close(playback.delay(), MIN_DELAY), "{}", playback.delay());
        let after_the_hold = (0..played.len()).rfind(|frame| played[*frame].1).unwrap() + 1;
        let rates = rates(&played[after_the_hold..]);
        let fastest = rates.iter().copied().fold(0.0, f64::max);
        assert!(fastest < 1.0 + MAX_RATE + 1e-9, "{fastest}");
        assert!(fastest > 1.0 + MAX_RATE / 2.0, "it caught up: {fastest}");
        let last = &rates[rates.len() - 60..];
        assert!(
            last.iter().all(|rate| (rate - 1.0).abs() < 1e-3),
            "{last:?}"
        );
    }

    /// The client's own long frame (a quarter of a second, the longest
    /// that loses no time) isn't the network's lateness: the five
    /// snapshots it takes at once don't raise the delay.
    #[test]
    fn a_long_frame_isnt_lateness() {
        let mut playback = Playback::default();
        let mut last: i32 = -1;
        let mut played = Vec::new();
        // Frames 301 to 314 never happen: 315 comes a quarter of a second
        // after 300.
        for frame in (0..900i32).filter(|frame| !(301..315).contains(frame)) {
            let (now, elapsed) = (frame as f64 / 60.0, (frame - last) as f64 / 60.0);
            // Each sent every third frame, arriving six frames later: those
            // due since the last frame.
            for sent in (last + 1 - 6..=frame - 6).filter(|sent| *sent >= 0 && sent % 3 == 0) {
                playback.arrived(100.0 + sent as f64 / 60.0, now, elapsed);
            }
            last = frame;
            playback.advance(now, elapsed);
            played.push(playback.holding());
        }
        assert!(close(playback.delay(), MIN_DELAY), "{}", playback.delay());
        // (It holds for that frame: the snapshots ran out a little into
        // it.)
        assert!(played[302..].iter().all(|held| !held));
    }

    /// The server's clock now, as the snapshots say, is the time of a
    /// snapshot arriving now: what a replica spawned now is stamped with,
    /// so the snapshots after it carry on from it rather than back.
    #[test]
    fn the_servers_clock_now() {
        let mut playback = Playback::default();
        feed(&mut playback, 0, 0.0, 5, |_| false, |_| 0);
        // The last frame was 299: a snapshot arriving then was sent in
        // 293, six frames before.
        let now = 299.0 / 60.0;
        let server_now = playback.server_now(now).unwrap();
        assert!(close(server_now, 100.0 + 293.0 / 60.0), "{server_now}");
    }

    /// A snapshot behind a newer one is too late to draw, and doesn't
    /// count as arriving.
    #[test]
    fn a_snapshot_out_of_order_is_ignored() {
        let mut arrivals = Arrivals::default();
        let frame = 1.0 / 60.0;
        assert!(arrivals.arrived(10.0, 0.0, frame));
        assert!(arrivals.arrived(10.05, 0.05, frame));
        assert!(!arrivals.arrived(10.02, 0.5, frame));
        assert!(close(arrivals.delay(), MIN_DELAY));
    }
}
