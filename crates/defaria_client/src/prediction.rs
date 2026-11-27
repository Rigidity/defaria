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
/// to this many of the newest.
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
        app.init_resource::<Prediction>()
            .add_systems(OnExit(GameState::Playing), forget)
            .add_systems(
                FixedUpdate,
                (
                    note_input.after(ReadControls).before(PlayerMovement),
                    note_outcome.after(PlayerMovement),
                ),
            )
            .add_systems(PreUpdate, reconcile.after(ReceiveSystems))
            .add_systems(
                RunFixedMainLoop,
                draw_correction
                    .after(PlacePlayers)
                    .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            )
            .add_systems(PostUpdate, send_inputs.in_set(SendSystems));
    }
}

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
    /// How many times the client's player was put where the server had it.
    pub corrections: u32,
}

/// Where the local player is drawn away from its body after a correction,
/// fading out.
#[derive(Component, Debug, Default, Clone, Copy)]
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
    prediction.steps.push_back((input, motion));
    while prediction.steps.len() > MAX_KEPT {
        prediction.steps.pop_front();
    }
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
    prediction.acked = Some(tick);
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
    prediction.sent = prediction.tick;
    let skip = prediction.steps.len().saturating_sub(MAX_SENT);
    let inputs: Vec<Input> = prediction
        .steps
        .iter()
        .skip(skip)
        .map(|(input, _)| input.clone())
        .collect();
    connection.0.send(ClientMessage::Inputs(inputs));
}
