//! The local player, moved at once and checked against the server's.
//!
//! Each physics step, the client notes the local player's input (its
//! [`Controls`] and [`Look`], numbered) and steps the player as any player
//! steps. It sends every input the server hasn't applied yet; the server
//! steps the player on them the same way and says where the last one left
//! it ([`ServerMessage::Moved`]). If that isn't where the client had the
//! player after the same input (the world was different, an input was
//! lost, something pushed it), the client puts the player where the server
//! says, steps it again through the inputs since, and draws the difference
//! fading out ([`Correction`]), so a correction is a glide rather than a
//! jump.

use std::collections::VecDeque;

use bevy::prelude::*;
use defaria_core::{
    fluid::FluidView,
    net::{ClientMessage, Input, ServerMessage},
    physics::{Body, STEP, WorldShapes},
    player::{
        Controls, LocalPlayer, Look, Motion, PlacePlayers, Player, PlayerMovement, ReadControls,
        Teleported, step,
    },
};

use crate::{
    net::{FromServer, ReceiveSystems, SendSystems, ServerConnection},
    session::GameState,
};

/// The most inputs sent at once: those the server hasn't applied yet, up
/// to this many of the newest. So losses are covered while the round trip
/// and the server's hold on inputs (up to a tenth of a second, see
/// `defaria_server`'s `movement`) come to less than 32 steps, about 530
/// ms: a round trip up to about 430 ms. Inputs are about 32 bytes, so 32
/// of them (about 1,030 bytes) still fit a QUIC datagram (about 1,200);
/// more would go on the reliable stream instead.
const MAX_SENT: usize = 32;
/// The most steps kept to step again, should the server not answer.
const MAX_KEPT: usize = 256;
/// How quickly a correction fades: the seconds it takes to fall to
/// about a third.
const CORRECTION_FADE: f32 = 0.08;
/// A correction farther than this, in blocks, is a jump, not a glide.
const MAX_GLIDE: f32 = 4.0;

pub struct PredictionPlugin;

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Correction>()
            .init_resource::<Prediction>()
            .add_systems(OnExit(GameState::Playing), forget)
            .add_systems(
                FixedUpdate,
                (
                    note_input.after(ReadControls).before(PlayerMovement),
                    note_outcome.after(PlayerMovement),
                ),
            )
            .add_systems(
                PreUpdate,
                reconcile.in_set(ReconcileSystems).after(ReceiveSystems),
            )
            .add_systems(
                RunFixedMainLoop,
                draw_correction
                    .after(PlacePlayers)
                    .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            )
            .add_systems(PostUpdate, send_inputs.in_set(SendSystems));
    }
}

/// Where the local player is put where the server has it, in
/// `PreUpdate`: whatever else moves it that frame (a scenario's `camera`)
/// goes after.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReconcileSystems;

/// The steps the server hasn't confirmed yet, and the numbering.
#[derive(Resource, Debug, Default)]
pub struct Prediction {
    /// The last input numbered.
    tick: u32,
    /// The last the server applied, if it's said.
    acked: Option<u32>,
    /// The last sent.
    sent: u32,
    /// The inputs since the server's last, each with where it left the
    /// player, oldest first.
    steps: VecDeque<(Input, Motion)>,
    /// The newest input that put the player somewhere (a teleport), until
    /// the server has applied it: sent with every batch, however many
    /// inputs came after it, since losing it would leave the player where
    /// it was on the server for good.
    teleport: Option<Input>,
    /// How many times the client's player was put where the server had it.
    pub corrections: u32,
}

impl Prediction {
    /// The last input numbered: the step the player has been taken to.
    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// Keeps the input for the step about to be taken.
    fn note(&mut self, input: Input, motion: Motion) {
        if input.teleport.is_some() {
            self.teleport = Some(input.clone());
        }
        self.steps.push_back((input, motion));
        while self.steps.len() > MAX_KEPT {
            self.steps.pop_front();
        }
    }

    /// The server applied the inputs up to `tick`.
    fn applied(&mut self, tick: u32) {
        self.acked = Some(tick);
        if self
            .teleport
            .as_ref()
            .is_some_and(|input| input.tick <= tick)
        {
            self.teleport = None;
        }
    }

    /// The inputs to send: the newest [`MAX_SENT`] the server hasn't
    /// applied, after the last teleport among them if it's older than
    /// those.
    fn batch(&self) -> Vec<Input> {
        let skip = self.steps.len().saturating_sub(MAX_SENT);
        let newest = self.steps.iter().skip(skip).map(|(input, _)| input.clone());
        let oldest_sent = self.steps.get(skip).map(|(input, _)| input.tick);
        let teleport = self
            .teleport
            .clone()
            .filter(|input| oldest_sent.is_none_or(|oldest| input.tick < oldest));
        teleport.into_iter().chain(newest).collect()
    }
}

/// Where the local player is drawn away from its body after a correction,
/// fading out.
#[derive(Component, Reflect, Debug, Default, Clone, Copy)]
#[reflect(Component)]
pub struct Correction(pub Vec3);

/// A world left: the next starts afresh.
fn forget(mut prediction: ResMut<Prediction>) {
    *prediction = Prediction::default();
}

/// Numbers and keeps the input for the step about to be taken, saying
/// where the player was put if it was.
fn note_input(
    mut commands: Commands,
    mut prediction: ResMut<Prediction>,
    player: Query<(Entity, &Controls, &Look, &Body, &Player, Has<Teleported>), With<LocalPlayer>>,
) {
    let Ok((entity, controls, look, body, state, teleported)) = player.single() else {
        return;
    };
    if teleported {
        commands.entity(entity).remove::<Teleported>();
    }
    prediction.tick += 1;
    let input = Input {
        tick: prediction.tick,
        controls: *controls,
        look: *look,
        teleport: teleported.then(|| Motion::of(body, state)),
    };
    let motion = Motion::of(body, state);
    prediction.note(input, motion);
}

/// Notes where the step left the player.
fn note_outcome(
    mut prediction: ResMut<Prediction>,
    player: Query<(&Body, &Player), With<LocalPlayer>>,
) {
    let Ok((body, state)) = player.single() else {
        return;
    };
    if let Some((_, motion)) = prediction.steps.back_mut() {
        *motion = Motion::of(body, state);
    }
}

/// Checks the server's word on the player against what the client had
/// after the same input, and if they differ, takes the server's and steps
/// again through the inputs since.
fn reconcile(
    mut messages: MessageReader<FromServer>,
    mut prediction: ResMut<Prediction>,
    shapes: WorldShapes,
    fluids: FluidView,
    mut player: Query<
        (
            Entity,
            &mut Body,
            &mut Player,
            &mut Controls,
            Option<&mut Correction>,
        ),
        With<LocalPlayer>,
    >,
    mut commands: Commands,
) {
    // The newest: they're not reliable, so they may come out of order.
    let Some((tick, server)) = messages
        .read()
        .filter_map(|FromServer(message)| match message {
            ServerMessage::Moved { tick, motion } => Some((*tick, *motion)),
            _ => None,
        })
        .max_by_key(|(tick, _)| *tick)
    else {
        return;
    };
    if prediction.acked.is_some_and(|acked| tick < acked) {
        return;
    }
    let Ok((entity, mut body, mut state, mut controls, correction)) = player.single_mut() else {
        return;
    };
    prediction.applied(tick);
    let predicted = prediction
        .steps
        .iter()
        .find(|(input, _)| input.tick == tick)
        .map(|(_, motion)| *motion);
    while prediction
        .steps
        .front()
        .is_some_and(|(input, _)| input.tick <= tick)
    {
        prediction.steps.pop_front();
    }
    if predicted.is_some_and(|predicted| predicted.matches(&server)) {
        return;
    }
    prediction.corrections += 1;
    let drawn = body.position;
    server.put(&mut body, &mut state);
    let held = *controls;
    let dt = STEP.as_secs_f64();
    for (input, motion) in prediction.steps.iter_mut() {
        if let Some(teleport) = input.teleport {
            teleport.put(&mut body, &mut state);
        }
        body.previous = body.position;
        *controls = input.controls;
        step(
            dt,
            &input.look,
            &mut controls,
            &mut state,
            &mut body,
            &shapes,
            &fluids,
        );
        *motion = Motion::of(&body, &state);
    }
    *controls = held;
    let off = (drawn - body.position).as_vec3();
    let off = if off.length() > MAX_GLIDE {
        Vec3::ZERO
    } else {
        off + correction.as_ref().map_or(Vec3::ZERO, |c| c.0)
    };
    match correction {
        Some(mut correction) => correction.0 = off,
        None => {
            commands.entity(entity).insert(Correction(off));
        }
    }
}

/// Draws the local player where its body is plus what's left of a
/// correction, fading it out.
fn draw_correction(
    time: Res<Time>,
    mut players: Query<(&mut Correction, &mut Transform), With<LocalPlayer>>,
) {
    for (mut correction, mut transform) in &mut players {
        if correction.0 == Vec3::ZERO {
            continue;
        }
        transform.translation += correction.0;
        correction.0 *= (-time.delta_secs() / CORRECTION_FADE).exp();
        if correction.0.length() < 1e-3 {
            correction.0 = Vec3::ZERO;
        }
    }
}

/// Sends the inputs the server hasn't applied, when there are new ones.
fn send_inputs(mut prediction: ResMut<Prediction>, connection: Option<ResMut<ServerConnection>>) {
    let Some(mut connection) = connection else {
        return;
    };
    if prediction.tick == prediction.sent {
        return;
    }
    let made_after = prediction.sent;
    prediction.sent = prediction.tick;
    let inputs = prediction.batch();
    connection
        .0
        .send(ClientMessage::Inputs { inputs, made_after });
}

#[cfg(test)]
mod tests {
    use bevy::math::DVec3;

    use super::*;

    fn input(tick: u32, teleport: bool) -> Input {
        let motion = Motion {
            feet: DVec3::new(600.0, 80.0, 600.0),
            velocity: DVec3::ZERO,
            on_ground: false,
            flying: true,
            sneaking: false,
            sprinting: false,
            eye_height: defaria_core::player::EYE_HEIGHT,
        };
        Input {
            tick,
            controls: Controls::default(),
            look: Look::default(),
            teleport: teleport.then_some(motion),
        }
    }

    fn ticks(batch: &[Input]) -> Vec<u32> {
        batch.iter().map(|input| input.tick).collect()
    }

    /// A teleport the server hasn't applied is sent with every batch, even
    /// once more than a batch's worth of inputs came after it (a client
    /// outrunning the server), and kept even past the steps kept to step
    /// again; once applied, it's dropped.
    #[test]
    fn a_teleport_is_sent_until_applied() {
        let mut prediction = Prediction::default();
        let motion = input(0, true).teleport.unwrap();
        for tick in 1..=300 {
            prediction.note(input(tick, tick == 5), motion);
        }
        let batch = prediction.batch();
        assert_eq!(batch.len(), MAX_SENT + 1);
        assert_eq!(batch[0].tick, 5);
        assert!(batch[0].teleport.is_some());
        assert_eq!(ticks(&batch[1..]), (269..=300).collect::<Vec<_>>());

        // Among the newest, it's sent once, in order.
        let mut recent = Prediction::default();
        for tick in 1..=40 {
            recent.note(input(tick, tick == 20), motion);
        }
        assert_eq!(ticks(&recent.batch()), (9..=40).collect::<Vec<_>>());

        prediction.applied(5);
        assert_eq!(prediction.batch().len(), MAX_SENT);
        assert!(prediction.teleport.is_none());
    }
}
