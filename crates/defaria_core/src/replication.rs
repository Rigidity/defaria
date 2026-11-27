//! Entities the server shows its clients as they move: players now, mobs
//! and dropped items later, by one mechanism rather than one per kind.
//!
//! An entity marked [`Replicated`] is sent to each client that has the
//! chunk it's in: once, reliably, when it comes into the client's range
//! ([`ServerMessage::Spawn`](crate::net::ServerMessage::Spawn)) and when
//! it leaves it ([`ServerMessage::Despawn`](crate::net::ServerMessage::Despawn)),
//! and in between as snapshots of where it is, many times a second, which
//! may be lost ([`ServerMessage::Snapshot`](crate::net::ServerMessage::Snapshot)).
//! A client draws it a little in the past, between the two snapshots
//! around that moment, so it moves smoothly whatever gets lost
//! (`defaria_client::replication`).

use bevy::{math::DVec3, prelude::*};

use crate::player::Look;

/// How often the server sends snapshots: twenty a second. A client draws
/// entities a couple of these behind.
pub const SNAPSHOT_INTERVAL: f64 = 1.0 / 20.0;

/// An entity clients are shown, and what kind it is.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct Replicated(pub Kind);

/// What kind of entity a replicated one is: what a client draws.
#[derive(
    Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Kind {
    Player,
}

/// Where a replicated entity is at a moment, and which way it looks.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pose {
    /// Its feet.
    pub feet: DVec3,
    pub look: Look,
    /// Lower, as a sneaking player is.
    pub crouched: bool,
}

impl Pose {
    /// The pose `t` of the way from this one to `next` (0 to 1): the feet
    /// and look between them, the shorter way round, and crouched as
    /// this one is.
    pub fn lerp(&self, next: &Pose, t: f64) -> Pose {
        let turn = (next.look.yaw - self.look.yaw + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        Pose {
            feet: self.feet.lerp(next.feet, t),
            look: Look {
                yaw: self.look.yaw + turn * t as f32,
                pitch: self.look.pitch + (next.look.pitch - self.look.pitch) * t as f32,
            },
            crouched: self.crouched,
        }
    }
}

/// One entity in a snapshot: which, and its pose.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EntityPose {
    pub id: u64,
    pub pose: Pose,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f64, yaw: f32) -> Pose {
        Pose {
            feet: DVec3::new(x, 0.0, 0.0),
            look: Look { yaw, pitch: 0.0 },
            crouched: false,
        }
    }

    /// Between two poses: halfway along, and turning the short way across
    /// the half turn where yaw wraps.
    #[test]
    fn poses_blend_the_short_way_round() {
        let a = pose(0.0, 3.0);
        let b = pose(2.0, -3.0);
        let half = a.lerp(&b, 0.5);
        assert_eq!(half.feet.x, 1.0);
        // From 3 to -3 radians is about 0.28 the short way, through π.
        let expected = 3.0 + (std::f32::consts::TAU - 6.0) / 2.0;
        assert!((half.look.yaw - expected).abs() < 1e-5, "{}", half.look.yaw);
        assert_eq!(a.lerp(&b, 0.0), a);
    }
}
