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
//! holds them follows how late they've arrived lately, up to [`MAX_HOLD`].
//! Inputs a client made in one of its frames and sent together don't
//! count as late (a client at 20 fps makes three a frame), so a client
//! on a steady connection isn't held, at any frame rate. The integrated
//! server's own client (single player) is never held: its inputs arrive
//! the frame they're made, and nobody else watches it.
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
/// The most inputs kept waiting; later ones are dropped (and sent again),
/// except one that puts the player somewhere, which takes the place of
/// the newest that doesn't (see [`PlayerInputs::wait`]).
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
/// (`tick * STEP`, on the client's clock), less the time it waited for the
/// rest of its client's frame: the inputs made in the frame a message was
/// sent count as made with its newest, and the older ones (from messages
/// lost) wait as long again for their own frames. Inputs that arrive
/// evenly all have the same offset, and an input that waited out a run of
/// losses has a bigger one. Holding every input to the biggest offset of the last
/// [`WINDOW`] steps them at the spacing they were made at. An input later
/// than that is stepped as it arrives and raises the offset: the player
/// already stopped for it, and the inputs after it keep its spacing (but
/// the offset never goes past [`MAX_HOLD`] over the earliest: what's
/// later than that is stepped at once). Once the late ones leave the
/// window the offset comes down by [`CATCH_UP`], so the inputs held come
/// out a few percent faster than time passes until it's down to the
/// latest left.
///
/// So inputs arriving evenly aren't held but for jitter, at any frame rate
/// of the client's (its newest input can be made up to a step before its
/// frame ends, so up to a step), and a run of losses stops the player
/// once, the first time it's that long, and not again while the playout
/// remembers it.
#[derive(Debug, Clone, Default)]
pub struct Playout {
    /// When each recent input arrived, and its offset.
    arrivals: VecDeque<(f64, f64)>,
    /// The offset inputs are stepped at; `None` before the first arrives.
    offset: Option<f64>,
}

impl Playout {
    /// Notes that the inputs `ticks` (in order, oldest first) arrived, for
    /// the first time, at `now`, in a message whose newest input was
    /// `newest` and which the client sent after the one with `made_after`.
    pub fn arrived(&mut self, ticks: &[u32], newest: u32, made_after: u32, now: f64) {
        if ticks.is_empty() {
            return;
        }
        let step = STEP.as_secs_f64();
        let offset = |tick: u32| now - tick as f64 * step;
        // The newest input says how the client's clock stands to ours now;
        // the others in its frame are as on time as it is, and older ones
        // waited out their frames too.
        let current = offset(newest);
        let frame = newest.saturating_sub(made_after.saturating_add(1)) as f64 * step;
        let own = |tick: u32| {
            if tick > made_after {
                current
            } else {
                offset(tick) - frame
            }
        };
        self.forget(now);
        let retime = match (self.offset, self.earliest()) {
            (None, _) => true,
            // Even the newest is early past the cap (a client whose clock
            // runs fast, or that caught up after its inputs were delayed),
            // or late past it (one that paused while the server went on,
            // or whose clock runs slow): time it afresh.
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
            .extend(ticks.iter().map(|&tick| (now, own(tick))));
        let earliest = self.earliest().unwrap_or(current);
        let at = self.offset.unwrap_or(current).max(own(ticks[0]));
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
    /// Whether its inputs are stepped as they arrive, with no
    /// [`Playout`]: the integrated server's own client's.
    in_process: bool,
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
            in_process: false,
            applied: 0,
            budget: BURST,
            moved: true,
        }
    }
}

impl PlayerInputs {
    /// A remote client's player's inputs, or (`in_process`) the integrated
    /// server's own client's.
    pub fn new(in_process: bool) -> Self {
        Self {
            in_process,
            ..default()
        }
    }

    /// Whether input `tick` is due to be stepped by `now`.
    fn due(&self, tick: u32, now: f64) -> bool {
        self.in_process || self.playout.due(tick, now)
    }

    /// Keeps `input` waiting to be stepped, if it's new; returns whether
    /// it did. Past [`MAX_WAITING`], a plain input is dropped (its client
    /// sends it again), but a teleport isn't: its client may not have it
    /// to send again for long, and without it the player would stay where
    /// it was. It takes the place of the newest plain input waiting.
    fn wait(&mut self, input: &Input) -> bool {
        if input.tick <= self.applied || self.waiting.contains_key(&input.tick) {
            return false;
        }
        if self.waiting.len() >= MAX_WAITING {
            if input.teleport.is_none() {
                return false;
            }
            let plain = self
                .waiting
                .iter()
                .rev()
                .find(|(_, waiting)| waiting.teleport.is_none())
                .map(|(&tick, _)| tick);
            let Some(plain) = plain else {
                return false;
            };
            self.waiting.remove(&plain);
        }
        self.waiting.insert(input.tick, input.clone());
        true
    }

    /// The last input applied.
    pub fn applied(&self) -> u32 {
        self.applied
    }

    /// The player was put somewhere (a command's `tp`): its client is
    /// told where it is next, and puts it right there.
    pub fn moved_now(&mut self) {
        self.moved = true;
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
        for message in &connection.inbox {
            let ClientMessage::Inputs {
                inputs: sent,
                made_after,
            } = message
            else {
                continue;
            };
            let mut new = Vec::new();
            for input in sent {
                if inputs.wait(input) {
                    new.push(input.tick);
                }
            }
            let newest = sent.iter().map(|input| input.tick).max().unwrap_or(0);
            if !inputs.in_process {
                new.sort_unstable();
                inputs.playout.arrived(&new, newest, *made_after, now);
            }
        }
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
            let Some(&next) = inputs.waiting.keys().next() else {
                break;
            };
            if !inputs.due(next, now) {
                break;
            }
            let input = inputs.waiting.remove(&next).unwrap();
            inputs.budget -= 1.0;
            // A teleport out of the world (or of NaNs) would break
            // streaming and terrain generation: it's ignored, and the
            // client is put right.
            if let Some(teleport) = input.teleport.filter(Motion::is_sane) {
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

    /// A player's inputs on their way from its client through a
    /// [`Playout`], a frame at a time, as `take_inputs` and
    /// `move_by_inputs` take them (without the budget, which the server's
    /// tests check).
    #[derive(Default)]
    struct Sim {
        inputs: PlayerInputs,
        now: f64,
        waiting: BTreeSet<u32>,
        /// The newest input the client has sent.
        sent: u32,
        /// How many inputs were stepped each frame.
        stepped: Vec<usize>,
    }

    impl Sim {
        /// The integrated server's own client's.
        fn in_process() -> Self {
            Self {
                inputs: PlayerInputs::new(true),
                ..default()
            }
        }

        /// A frame `elapsed` seconds after the last, by the end of which
        /// the client has made the inputs up to `made`, and sent those the
        /// server hasn't applied if it made new ones; they arrive unless
        /// `lost`.
        fn frame(&mut self, elapsed: f64, made: u32, lost: bool) {
            self.now += elapsed;
            if made > self.sent {
                let made_after = std::mem::replace(&mut self.sent, made);
                if !lost {
                    let new: Vec<u32> = (self.inputs.applied + 1..=made)
                        .filter(|tick| self.waiting.insert(*tick))
                        .collect();
                    if !self.inputs.in_process {
                        (self.inputs.playout).arrived(&new, made, made_after, self.now);
                    }
                }
            }
            self.inputs.playout.advance(self.now, elapsed);
            let mut stepped = 0;
            while let Some(&tick) = self.waiting.first() {
                if !self.inputs.due(tick, self.now) {
                    break;
                }
                self.waiting.pop_first();
                self.inputs.applied = tick;
                stepped += 1;
            }
            self.stepped.push(stepped);
        }

        fn applied(&self) -> u32 {
            self.inputs.applied
        }

        /// How many steps the inputs are held.
        fn hold(&self) -> f64 {
            self.inputs.playout.hold() / DT
        }
    }

    /// A step, in seconds.
    const DT: f64 = STEP.as_secs_f64();

    /// A client making one input a frame, a step apart, the datagram sent
    /// in a frame lost if `lost` says so.
    fn one_a_frame(
        sim: &mut Sim,
        frames: std::ops::RangeInclusive<u32>,
        lost: impl Fn(u32) -> bool,
    ) {
        for frame in frames {
            sim.frame(DT, frame, lost(frame));
        }
    }

    /// Inputs arriving a step apart are stepped as they arrive, held not
    /// at all.
    #[test]
    fn even_inputs_are_not_held() {
        let mut sim = Sim::default();
        for tick in 1..=600 {
            sim.frame(DT, tick, false);
            assert_eq!(sim.applied(), tick, "input {tick} held");
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
        one_a_frame(&mut sim, 1..=600, lost);
        // The first run of each length stops it for a frame, and raises
        // the hold to it.
        assert_eq!(sim.stepped[..16].iter().filter(|&&n| n == 0).count(), 3);
        assert!(
            sim.stepped[16..].iter().all(|&n| n == 1),
            "{:?}",
            sim.stepped
        );
        assert!((sim.hold() - 3.0).abs() < 1e-6, "held {} steps", sim.hold());
        assert_eq!(sim.applied(), 597);
    }

    /// Once the late inputs are out of the window, the hold comes down a
    /// few percent faster than time passes, never two steps in a frame
    /// more than once in a while.
    #[test]
    fn a_long_hold_catches_up_gently() {
        let mut sim = Sim::default();
        one_a_frame(&mut sim, 1..=120, |frame| (60..65).contains(&frame));
        assert!((sim.hold() - 5.0).abs() < 1e-6, "{}", sim.hold());
        assert_eq!(sim.applied(), 115);
        let mut behind = 0;
        for frame in 121..=1200 {
            sim.frame(DT, frame, false);
            if sim.applied() < frame {
                behind += 1;
            }
        }
        let catching_up = &sim.stepped[120..];
        assert!(catching_up.iter().all(|&n| n <= 2));
        assert_eq!(catching_up.iter().filter(|&&n| n == 2).count(), 5);
        assert_eq!(sim.applied(), 1200, "caught up");
        assert!(sim.hold() < 1e-6);
        // Until input 60, which arrived late in frame 65, is out of the
        // window, then the five steps let go at CATCH_UP.
        let expected = (65.0 * DT + WINDOW + 5.0 * DT / CATCH_UP) / DT - 120.0;
        assert!(
            (behind as f64 - expected).abs() < 5.0,
            "{behind} frames behind, not {expected}"
        );
    }

    /// A client whose frames take `frames` in turn, and whose inputs
    /// arrive at the end of the frame they're made in (as in single
    /// player, or on a steady connection), for `count` frames. Returns how
    /// far behind the inputs made the server was at worst, in inputs, and
    /// how many steps it held them at worst.
    fn frames_of(sim: &mut Sim, frames: &[f64], count: usize) -> (u32, f64) {
        let (mut made, mut clock) = (0, 0.0);
        let (mut behind, mut held) = (0, 0.0f64);
        for frame in 0..count {
            let elapsed = frames[frame % frames.len()];
            clock += elapsed;
            while (made + 1) as f64 * DT <= clock + 1e-9 {
                made += 1;
            }
            sim.frame(elapsed, made, false);
            behind = behind.max(made - sim.applied());
            held = held.max(sim.hold());
        }
        (behind, held)
    }

    /// Single player at 20 fps, three inputs a frame: none held.
    #[test]
    fn in_process_inputs_at_20_fps_are_not_held() {
        let mut sim = Sim::in_process();
        assert_eq!(frames_of(&mut sim, &[0.05], 600).0, 0);
    }

    /// Single player through a hitch of a quarter of a second, and frames
    /// all over the place: none held.
    #[test]
    fn in_process_inputs_are_not_held_through_hitches() {
        let mut sim = Sim::in_process();
        let frames = [DT, DT, 0.25, DT, 0.02, 0.007, 0.03, 0.05, DT];
        assert_eq!(frames_of(&mut sim, &frames, 600).0, 0);
    }

    /// A remote client at 20 fps on a steady connection sends three inputs
    /// a frame: they're its frame's, not late, and aren't held.
    #[test]
    fn a_slow_clients_frames_are_not_held() {
        let mut sim = Sim::default();
        let (behind, held) = frames_of(&mut sim, &[0.05], 600);
        assert_eq!(behind, 0);
        assert!(held < 1e-3, "held {held} steps");
    }

    /// Nor is a remote client's hitch: what it made in one long frame
    /// comes together, as made.
    #[test]
    fn a_clients_hitch_is_not_held() {
        let mut sim = Sim::default();
        let frames = [DT, DT, DT, 0.25, DT, DT, DT, DT];
        let (behind, held) = frames_of(&mut sim, &frames, 600);
        assert_eq!(behind, 0);
        assert!(held < 1e-3, "held {held} steps");
    }

    /// A remote client whose frames are uneven, so the newest input of a
    /// frame is made up to a step before it's sent: held a step at most.
    #[test]
    fn uneven_frames_are_held_a_step_at_most() {
        for frames in [
            &[0.015, 0.018, 0.017, 0.016][..],
            &[0.02, 0.007, 0.03, 0.016, 0.011, 0.025],
        ] {
            let mut sim = Sim::default();
            let (behind, held) = frames_of(&mut sim, frames, 1200);
            assert!(behind <= 1, "{behind} behind");
            assert!(held <= 1.0 + 1e-6, "held {held} steps");
        }
    }

    /// A slow client's lost datagram is held for as long as the frame it
    /// lost, not counting the frame the next was sent in.
    #[test]
    fn a_slow_clients_losses_are_held_for_their_frames() {
        let mut sim = Sim::default();
        for frame in 1..=20 {
            sim.frame(0.05, 3 * frame, frame == 10);
        }
        assert!((sim.hold() - 3.0).abs() < 1e-3, "held {} steps", sim.hold());
    }

    /// A client that stops sending (paused, while the server goes on) is
    /// timed afresh when it comes back, not held up or burst through.
    #[test]
    fn a_paused_client_is_timed_afresh() {
        let mut sim = Sim::default();
        one_a_frame(&mut sim, 1..=60, |_| false);
        for _ in 0..120 {
            sim.frame(DT, 60, false);
        }
        for tick in 61..=120 {
            sim.frame(DT, tick, false);
            assert_eq!(sim.applied(), tick);
        }
    }

    /// A hold is never more than [`MAX_HOLD`]: inputs later than that are
    /// stepped at once, as many as they're late past it.
    #[test]
    fn the_hold_is_capped() {
        let mut sim = Sim::default();
        for frame in 1..=200 {
            sim.frame(DT, frame, (100..120).contains(&frame));
            assert!(sim.hold() * DT <= MAX_HOLD + 1e-9, "{}", sim.hold());
        }
        assert_eq!(sim.stepped[119], 15);
        assert_eq!(sim.applied(), 194);
    }

    /// The integrated server's own client's inputs are due as they arrive,
    /// whatever a playout would say.
    #[test]
    fn in_process_inputs_are_always_due() {
        let mut held = PlayerInputs::new(false);
        held.playout.arrived(&[1, 2, 3], 3, 0, 1.0);
        held.playout.arrived(&[4], 4, 3, 1.0);
        assert!(!held.due(4, 1.0));
        held.in_process = true;
        assert!(held.due(4, 1.0));
    }

    /// Past the most inputs kept waiting, plain ones are dropped, but a
    /// teleport takes the place of the newest plain one: it's never lost
    /// for being one too many.
    #[test]
    fn a_teleport_waits_however_many_are_waiting() {
        let plain = |tick| Input {
            tick,
            controls: player::Controls::default(),
            look: Look::default(),
            teleport: None,
        };
        let mut inputs = PlayerInputs::new(false);
        for tick in 1..=MAX_WAITING as u32 {
            assert!(inputs.wait(&plain(tick)));
        }
        let over = MAX_WAITING as u32 + 1;
        assert!(!inputs.wait(&plain(over)), "one too many");
        let teleport = Input {
            teleport: Some(Motion {
                feet: bevy::math::DVec3::new(600.0, 80.0, 600.0),
                velocity: bevy::math::DVec3::ZERO,
                on_ground: false,
                flying: true,
                sneaking: false,
                sprinting: false,
                eye_height: player::EYE_HEIGHT,
            }),
            ..plain(over + 5)
        };
        assert!(inputs.wait(&teleport));
        assert_eq!(inputs.waiting.len(), MAX_WAITING);
        assert!(inputs.waiting[&(over + 5)].teleport.is_some());
        assert!(!inputs.waiting.contains_key(&(MAX_WAITING as u32)));
        // Sent again, it's not taken twice.
        assert!(!inputs.wait(&teleport));
        assert_eq!(inputs.waiting.len(), MAX_WAITING);
    }
}
