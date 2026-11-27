//! Mining: holding left click breaks the block under the crosshair after a
//! time set by its hardness.
//!
//! - A block takes [`break_time`] seconds of holding to break: its state's
//!   hardness times [`SECONDS_PER_HARDNESS`]. A block with no hardness
//!   breaks on the frame it's targeted.
//! - Releasing the button, looking at another block, the block changing
//!   (say, to another state), unlocking the cursor or opening a screen
//!   starts over.
//! - Keep holding and the next block under the crosshair starts once
//!   [`BREAK_DELAY`] has passed, so one click can't sweep through a row
//!   of blocks that break at once.
//! - The click that locks the cursor mines nothing, even if it's held.
//!
//! Breaking blocks is the server's (`defaria_server::mining`); this is
//! what both sides know of it. [`Mining`], on a player, says whether it holds the button
//! ([`Mining::holding`], which `mining_input` sets from the mouse for the
//! local player), which block is being mined and how far along it is, for
//! the cracks drawn on it and for `inspect`.

use std::time::Duration;

use bevy::prelude::*;

use crate::content::BlockState;

/// Seconds of mining per point of hardness. Stone, at a hardness of 1.5,
/// takes a little over a second.
pub const SECONDS_PER_HARDNESS: f32 = 0.75;

/// How long after a block breaks holding on starts on the next one.
pub const BREAK_DELAY: Duration = Duration::from_millis(250);

/// How long a block of this hardness takes to break, in seconds.
pub fn break_time(hardness: f32) -> f32 {
    hardness.max(0.0) * SECONDS_PER_HARDNESS
}

/// Where mining progresses and blocks break.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MiningSystems;

/// The block a player is mining, if any, and how far along it is.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq)]
#[reflect(Component)]
pub struct Mining {
    /// Whether the player holds the break button, and it counts: for the
    /// local player, pressed while playing with the cursor locked. Mining
    /// starts over whenever it isn't.
    pub holding: bool,
    /// The block being mined, and its state now: in a voxel of two
    /// halves, the half being mined.
    pub target: Option<(IVec3, BlockState)>,
    /// From 0 when mining starts to 1 when the block breaks.
    pub progress: f32,
    /// What's left of [`BREAK_DELAY`].
    pub delay: Duration,
    /// How long the target has been mined; `progress` is this over its
    /// break time.
    pub elapsed: Duration,
}
