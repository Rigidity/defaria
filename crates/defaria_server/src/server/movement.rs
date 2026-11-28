//! Players moved by their clients' inputs. A client steps its own player
//! at once and sends what it pressed and where it looked each step
//! ([`ClientMessage::Inputs`]); the server steps the player the same way
//! ([`player::step`]) for each input, in order, and tells the client where
//! that left it ([`ServerMessage::Moved`]), which the client checks its
//! prediction against.
//!
//! Inputs don't arrive as evenly as they were made: after a run of lost
//! datagrams, the next brings all their inputs at once. So the server
//! holds each player's inputs a little, in a [`Playout`], and steps them
//! at the spacing they were made at, a physics step apart, so other
//! players see the player move the same distance each frame. How long it
//! holds them follows how late they've arrived lately, up to [`MAX_HOLD`];
//! inputs that arrive evenly (single player, a good connection) aren't
//! held at all.
//!
//! A client can't move its player faster than time passes: each player
//! has a budget of steps that refills at the physics rate (a little over,
//! for jitter), and inputs past it wait.

use std::collections::{BTreeMap, VecDeque};

use bevy::prelude::*;

use super::Connection;
use defaria_core::{
    fluid::FluidView,
    net::{ClientMessage, Input, ServerMessage},
    physics::{Body, STEP, STEP_HZ, WorldShapes},
    player::{self, Look, Motion, Player},
};

/// The most steps a player may take at once, after a pause in its inputs
/// (a third of a second's).
const BURST: f64 = 20.0;
/// How much faster than the physics rate the budget refills, so a client
/// whose clock runs a little fast, or whose inputs come in bunches, isn't
/// held back.
const SLACK: f64 = 1.05;
/// The most inputs kept waiting; later ones are dropped (and sent again).
const MAX_WAITING: usize = 128;

/// The longest inputs are held past the earliest any arrived lately, in
/// seconds (six steps). Inputs later than that are stepped at once, and a
/// client whose newest input is later or earlier than that is timed
/// afresh from it.
const MAX_HOLD: f64 = 0.1;
/// How far back, in seconds, a [`Playout`] looks at how late inputs
/// arrived.
const WINDOW: f64 = 3.0;
/// How quickly a [`Playout`] stops holding inputs once they arrive less
/// late: seconds of hold let go a second. So while it catches up the
/// player steps at most this much faster than time passes, inside the
/// budget's [`SLACK`].
const CATCH_UP: f64 = 0.04;
/// How close to its turn an input must be to be stepped, for rounding.
const EPSILON: f64 = 1e-6;

/// When a player's inputs are stepped: each input `tick` at
/// `tick * STEP + offset` on the server's clock, where `offset` is the
/// latest an input arrived lately, relative to when it was made.
///
/// An input's own offset is when it first arrived less when it was made
/// (`tick * STEP`, on the client's clock). Inputs that arrive evenly all
/// have the same one, and an input that waited out a run of losses has a
/// bigger one. Holding every input to the biggest offset of the last
/// [`WINDOW`] steps them at the spacing they were made at. An input later
/// than that is stepped as it arrives and raises the offset: the player
/// already stopped for it, and the inputs after it keep its spacing (but
/// the offset never goes past [`MAX_HOLD`] over the earliest: what's
/// later than that is stepped at once). Once the late ones leave the
/// window the offset comes down by [`CATCH_UP`], so the inputs held come
/// out a few percent faster than time passes until it's down to the
/// latest left.
///
/// So inputs arriving evenly, as in single player (where they arrive the
/// frame they're made), aren't held, or held no longer than the frames
/// are uneven; and a run of losses stops the player once, the first time
/// it's that long, and not again while the playout remembers it.
#[derive(Debug, Clone, Default)]
pub struct Playout {
    /// When each recent input arrived, and its offset.
    arrivals: VecDeque<(f64, f64)>,
    /// The offset inputs are stepped at; `None` before the first arrives.
    offset: Option<f64>,
}

impl Playout {
    /// Notes that the inputs `ticks` (in order, oldest first) arrived, for
    /// the first time, at `now`.
    pub fn arrived(&mut self, ticks: &[u32], now: f64) {
        let (Some(&oldest), Some(&newest)) = (ticks.first(), ticks.last()) else {
            return;
        };
        let offset = |tick: u32| now - tick as f64 * STEP.as_secs_f64();
        self.forget(now);
        // The newest input says how the client's clock stands to ours now.
        let current = offset(newest);
        let retime = match (self.offset, self.earliest()) {
            (None, _) => true,
            // Even the newest is late past the cap (a client that paused
            // while the server went on, or whose clock runs slow), or
            // early past it (one whose clock runs fast, or that caught up
            // after its inputs were delayed): time it afresh.
            (Some(at), earliest) => {
                at - current > MAX_HOLD + EPSILON
                    || earliest.is_some_and(|earliest| current - earliest > MAX_HOLD + EPSILON)
            }
        };
        if retime {
            self.arrivals.clear();
            self.offset = Some(current);
        }
        self.arrivals
            .extend(ticks.iter().map(|&tick| (now, offset(tick))));
        let earliest = self.earliest().unwrap_or(current);
        let at = self.offset.unwrap_or(current).max(offset(oldest));
        self.offset = Some(at.min(earliest + MAX_HOLD));
    }

    /// Moves on to `now`, `elapsed` seconds after the last: forgets old
    /// arrivals, and eases the offset down toward the latest left.
    pub fn advance(&mut self, now: f64, elapsed: f64) {
        self.forget(now);
        let latest = self
            .arrivals
            .iter()
            .map(|(_, offset)| *offset)
            .reduce(f64::max);
        if let (Some(at), Some(latest)) = (self.offset, latest)
            && latest < at
        {
            self.offset = Some(latest.max(at - elapsed * CATCH_UP));
        }
    }

    /// Whether input `tick` is due by `now`.
    pub fn due(&self, tick: u32, now: f64) -> bool {
        self.offset
            .is_some_and(|offset| tick as f64 * STEP.as_secs_f64() + offset <= now + EPSILON)
    }

    /// How long inputs are held past the earliest any arrived lately, in
    /// seconds.
    #[cfg(test)]
    fn hold(&self) -> f64 {
        match (self.offset, self.earliest()) {
            (Some(offset), Some(earliest)) => (offset - earliest).max(0.0),
            _ => 0.0,
        }
    }

    /// The smallest offset of the recent arrivals.
    fn earliest(&self) -> Option<f64> {
        self.arrivals
            .iter()
            .map(|(_, offset)| *offset)
            .reduce(f64::min)
    }

    /// Forgets the arrivals more than [`WINDOW`] before `now`.
    fn forget(&mut self, now: f64) {
        while self
            .arrivals
            .front()
            .is_some_and(|(arrived, _)| now - arrived > WINDOW)
        {
            self.arrivals.pop_front();
        }
    }
}

/// A player's inputs from its client: those waiting, when they're due, the
/// last applied, and how many steps it may take now.
#[derive(Component, Reflect, Debug)]
#[reflect(Component)]
pub struct PlayerInputs {
    #[reflect(ignore)]
    waiting: BTreeMap<u32, Input>,
    #[reflect(ignore)]
    playout: Playout,
    applied: u32,
    budget: f64,
    /// Whether it moved (or was put somewhere) since its client was last
    /// told.
    moved: bool,
}

impl Default for PlayerInputs {
    fn default() -> Self {
        Self {
            waiting: BTreeMap::new(),
            playout: Playout::default(),
            applied: 0,
            budget: BURST,
            moved: true,
        }
    }
}

impl PlayerInputs {
    /// The last input applied.
    pub fn applied(&self) -> u32 {
        self.applied
    }

    #[cfg(test)]
    pub(crate) fn set_applied(&mut self, tick: u32) {
        self.applied = tick;
    }
}

/// Takes the inputs clients sent that haven't been applied yet, noting
/// when each arrived.
pub(super) fn take_inputs(time: Res<Time>, mut players: Query<(&Connection, &mut PlayerInputs)>) {
    let now = time.elapsed_secs_f64();
    for (connection, mut inputs) in &mut players {
        let inputs = &mut *inputs;
        let mut new = Vec::new();
        for message in &connection.inbox {
            let ClientMessage::Inputs(sent) = message else {
                continue;
            };
            for input in sent {
                if input.tick > inputs.applied
                    && inputs.waiting.len() < MAX_WAITING
                    && !inputs.waiting.contains_key(&input.tick)
                {
                    new.push(input.tick);
                    inputs.waiting.insert(input.tick, input.clone());
                }
            }
        }
        new.sort_unstable();
        inputs.playout.arrived(&new, now);
    }
}

/// Steps each player once for each of its waiting inputs that's due, in
/// order, as far as its budget allows.
pub(super) fn move_by_inputs(
    time: Res<Time>,
    shapes: WorldShapes,
    fluids: FluidView,
    mut players: Query<(
        &mut PlayerInputs,
        &mut Body,
        &mut Player,
        &mut Look,
        &mut player::Controls,
        &mut Transform,
    )>,
) {
    let dt = STEP.as_secs_f64();
    let now = time.elapsed_secs_f64();
    let elapsed = time.delta_secs_f64();
    for (mut inputs, mut body, mut player, mut look, mut controls, mut transform) in &mut players {
        let inputs = &mut *inputs;
        inputs.budget = (inputs.budget + elapsed * STEP_HZ as f64 * SLACK).min(BURST);
        inputs.playout.advance(now, elapsed);
        let mut stepped = false;
        while inputs.budget >= 1.0 {
            let Some(next) = inputs.waiting.first_entry() else {
                break;
            };
            if !inputs.playout.due(*next.key(), now) {
                break;
            }
            let input = next.remove();
            inputs.budget -= 1.0;
            if let Some(teleport) = input.teleport {
                teleport.put(&mut body, &mut player);
            }
            look.set_if_neq(input.look);
            *controls = input.controls;
            body.previous = body.position;
            player::step(
                dt,
                &input.look,
                &mut controls,
                &mut player,
                &mut body,
                &shapes,
                &fluids,
            );
            inputs.applied = input.tick;
            stepped = true;
        }
        if stepped {
            inputs.moved = true;
            transform.translation = body.position.as_vec3();
            transform.rotation = look.yaw_rotation();
            player.drawn_eye_height = player.eye_height;
        }
    }
}

/// Tells each client where its player is after the inputs applied, when
/// that's changed.
pub(super) fn send_moves(mut players: Query<(&mut Connection, &mut PlayerInputs, &Body, &Player)>) {
    for (mut connection, mut inputs, body, player) in &mut players {
        if !inputs.moved {
            continue;
        }
        inputs.moved = false;
        connection.send(ServerMessage::Moved {
            tick: inputs.applied,
            motion: Motion::of(body, player),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// A player's inputs on their way through a [`Playout`], a frame at a
    /// time, as `take_inputs` and `move_by_inputs` take them (without the
    /// budget, which the server's tests check).
    #[derive(Default)]
    struct Sim {
        playout: Playout,
        now: f64,
        waiting: BTreeSet<u32>,
        applied: u32,
        /// How many inputs were stepped each frame.
        stepped: Vec<usize>,
    }

    impl Sim {
        /// A frame `elapsed` seconds after the last, in which the inputs
        /// up to `newest` that haven't been applied (those the client
        /// still sends) arrive, or none if `None`.
        fn frame(&mut self, elapsed: f64, newest: Option<u32>) {
            self.now += elapsed;
            let newest = newest.unwrap_or(self.applied);
            let new: Vec<u32> = (self.applied + 1..=newest)
                .filter(|tick| self.waiting.insert(*tick))
                .collect();
            self.playout.arrived(&new, self.now);
            self.playout.advance(self.now, elapsed);
            let mut stepped = 0;
            while let Some(&tick) = self.waiting.first() {
                if !self.playout.due(tick, self.now) {
                    break;
                }
                self.waiting.pop_first();
                self.applied = tick;
                stepped += 1;
            }
            self.stepped.push(stepped);
        }

        /// How many steps the inputs are held.
        fn hold(&self) -> f64 {
            self.playout.hold() / DT
        }
    }

    /// A step, in seconds.
    const DT: f64 = STEP.as_secs_f64();

    /// Inputs arriving a step apart are stepped as they arrive, held not
    /// at all.
    #[test]
    fn even_inputs_are_not_held() {
        let mut sim = Sim::default();
        for tick in 1..=600 {
            sim.frame(DT, Some(tick));
            assert_eq!(sim.applied, tick, "input {tick} held");
        }
        assert!(sim.hold() < 1e-6, "{}", sim.hold());
    }

    /// Whether the datagram sent in `frame` is lost: runs of one, two and
    /// three, as a quarter lost leaves them.
    fn lost(frame: u32) -> bool {
        matches!(frame % 20, 3 | 7 | 8 | 13 | 14 | 15)
    }

    /// Inputs that arrive in bunches, after runs of lost datagrams, are
    /// stepped one a frame once the playout has seen the longest run: none
    /// stops the player, or steps it twice.
    #[test]
    fn bunched_inputs_are_stepped_evenly() {
        let mut sim = Sim::default();
        for frame in 1..=600 {
            sim.frame(DT, (!lost(frame)).then_some(frame));
        }
        // The first run of each length stops it for a frame, and raises
        // the hold to it.
        assert_eq!(sim.stepped[..16].iter().filter(|&&n| n == 0).count(), 3);
        assert!(
            sim.stepped[16..].iter().all(|&n| n == 1),
            "{:?}",
            sim.stepped
        );
        assert!((sim.hold() - 3.0).abs() < 1e-6, "held {} steps", sim.hold());
        assert_eq!(sim.applied, 597);
    }

    /// Once the late inputs are out of the window, the hold comes down a
    /// few percent faster than time passes, never two steps in a frame
    /// more than once in a while.
    #[test]
    fn a_long_hold_catches_up_gently() {
        let mut sim = Sim::default();
        for frame in 1..=120 {
            sim.frame(DT, (!(60..65).contains(&frame)).then_some(frame));
        }
        assert!((sim.hold() - 5.0).abs() < 1e-6, "{}", sim.hold());
        assert_eq!(sim.applied, 115);
        let mut behind = 0;
        for frame in 121..=1200 {
            sim.frame(DT, Some(frame));
            if sim.applied < frame {
                behind += 1;
            }
        }
        let catching_up = &sim.stepped[120..];
        assert!(catching_up.iter().all(|&n| n <= 2));
        assert_eq!(catching_up.iter().filter(|&&n| n == 2).count(), 5);
        assert_eq!(sim.applied, 1200, "caught up");
        assert!(sim.hold() < 1e-6);
        // Until input 60, which arrived late in frame 65, is out of the
        // window, then the five steps let go at CATCH_UP.
        let expected = (65.0 * DT + WINDOW + 5.0 * DT / CATCH_UP) / DT - 120.0;
        assert!(
            (behind as f64 - expected).abs() < 5.0,
            "{behind} frames behind, not {expected}"
        );
    }

    /// Single player steps the server with the client's frames, in which
    /// the client may make no inputs or two: they're held no longer than
    /// the frames are uneven, and never stop.
    fn uneven_frames(frames: &[f64]) {
        let longest = frames.iter().copied().fold(0.0, f64::max);
        let mut sim = Sim::default();
        let (mut made, mut clock) = (0, 0.0);
        for frame in 0..1200 {
            let elapsed = frames[frame % frames.len()];
            clock += elapsed;
            while (made + 1) as f64 * DT <= clock + 1e-9 {
                made += 1;
            }
            sim.frame(elapsed, Some(made));
            assert!(made - sim.applied <= 2, "frame {frame}: {made} made");
            assert!(sim.playout.hold() <= longest + 1e-9, "{}", sim.hold());
        }
    }

    /// See [`uneven_frames`]: a frame a little over a step or under.
    #[test]
    fn uneven_frames_are_barely_held() {
        uneven_frames(&[0.015, 0.018, 0.017, 0.016]);
    }

    /// See [`uneven_frames`]: frames all over the place.
    #[test]
    fn very_uneven_frames_are_held_at_most_a_frame() {
        uneven_frames(&[0.02, 0.007, 0.03, 0.016, 0.011, 0.025]);
    }

    /// A client that stops sending (paused, while the server goes on) is
    /// timed afresh when it comes back, not held up or burst through.
    #[test]
    fn a_paused_client_is_timed_afresh() {
        let mut sim = Sim::default();
        for tick in 1..=60 {
            sim.frame(DT, Some(tick));
        }
        for _ in 0..120 {
            sim.frame(DT, None);
        }
        for tick in 61..=120 {
            sim.frame(DT, Some(tick));
            assert_eq!(sim.applied, tick);
        }
    }

    /// A hold is never more than [`MAX_HOLD`]: inputs later than that are
    /// stepped at once, as many as they're late past it.
    #[test]
    fn the_hold_is_capped() {
        let mut sim = Sim::default();
        for frame in 1..=200 {
            sim.frame(DT, (!(100..120).contains(&frame)).then_some(frame));
            assert!(sim.playout.hold() <= MAX_HOLD + 1e-9, "{}", sim.hold());
        }
        assert_eq!(sim.stepped[119], 15);
        assert_eq!(sim.applied, 194);
    }
}
