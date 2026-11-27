//! The network clock: network ticks, [`TICKS_PER_SECOND`] a second,
//! counted out of fixed time in `FixedUpdate`. Every solver (energy,
//! fluid, items) and what runs on what they move (recipe machines,
//! machines' scripts) runs on them, in the phases of [`NetworkTick`],
//! gated by [`on_network_tick`].

use std::time::Duration;

use bevy::prelude::*;
use defaria_core::network::TICKS_PER_SECOND;

/// How long a network tick is.
pub const TICK: Duration = Duration::from_millis(1000 / TICKS_PER_SECOND as u64);

pub struct NetworkTickPlugin;

impl Plugin for NetworkTickPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkClock>()
            .configure_sets(
                FixedUpdate,
                (
                    NetworkTick::Clock,
                    NetworkTick::Solve,
                    NetworkTick::Machines,
                )
                    .chain(),
            )
            .add_systems(FixedUpdate, advance_clock.in_set(NetworkTick::Clock));
    }
}

/// The phases of a network tick, in `FixedUpdate`, in this order. The
/// clock's runs on every fixed step; the systems in the others that run
/// only on a network tick say so with [`on_network_tick`].
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkTick {
    /// The clock counts fixed time and says whether a tick is due.
    Clock,
    /// The solvers move energy, fluid and items along the networks.
    Solve,
    /// Machines run on what the solvers moved: recipes, scripts.
    Machines,
}

/// Counts network ticks out of fixed time.
#[derive(Resource, Debug, Default)]
pub struct NetworkClock {
    /// Fixed time not yet spent on a tick.
    since: Duration,
    /// Network ticks so far, counting the one due now.
    pub ticks: u64,
    /// Whether a network tick runs on this fixed step.
    pub due: bool,
}

impl NetworkClock {
    /// What a rate of `per_second` gives on the current tick: whole units
    /// that add up to exactly `per_second` over each second's ticks.
    pub fn share(&self, per_second: u64) -> u64 {
        per_tick(per_second, self.ticks.saturating_sub(1))
    }
}

/// A run condition: whether a network tick runs on this fixed step.
pub fn on_network_tick(clock: Res<NetworkClock>) -> bool {
    clock.due
}

/// What a rate of `per_second` gives on network tick `tick`: the ticks of
/// each second add up to exactly `per_second`.
pub fn per_tick(per_second: u64, tick: u64) -> u64 {
    let n = TICKS_PER_SECOND as u128;
    let t = (tick % TICKS_PER_SECOND as u64) as u128;
    let rate = per_second as u128;
    (rate * (t + 1) / n - rate * t / n) as u64
}

fn advance_clock(time: Res<Time>, mut clock: ResMut<NetworkClock>) {
    clock.since += time.delta();
    clock.due = clock.since >= TICK;
    if clock.due {
        clock.since -= TICK;
        clock.ticks += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_add_up_over_each_second() {
        for rate in [0, 1, 7, 19, 20, 170, 1000, 12_345] {
            let second: u64 = (0..TICKS_PER_SECOND as u64)
                .map(|t| per_tick(rate, t))
                .sum();
            assert_eq!(second, rate);
            // And again the next second.
            let next: u64 = (20..40).map(|t| per_tick(rate, t)).sum();
            assert_eq!(next, rate);
        }
        assert_eq!(per_tick(20, 3), 1);
    }
}
