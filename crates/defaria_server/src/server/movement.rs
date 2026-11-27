//! Players moved by their clients' inputs. A client steps its own player
//! at once and sends what it pressed and where it looked each step
//! ([`ClientMessage::Inputs`]); the server steps the player the same way
//! ([`player::step`]) for each input as it arrives, in order, and tells
//! the client where that left it ([`ServerMessage::Moved`]), which the
//! client checks its prediction against.
//!
//! A client can't move its player faster than time passes: each player
//! has a budget of steps that refills at the physics rate (a little over,
//! for jitter), and inputs past it wait.

use std::collections::BTreeMap;

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

/// A player's inputs from its client: those waiting, the last applied,
/// and how many steps it may take now.
#[derive(Component, Reflect, Debug)]
#[reflect(Component)]
pub struct PlayerInputs {
    #[reflect(ignore)]
    waiting: BTreeMap<u32, Input>,
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

/// Takes the inputs clients sent that haven't been applied yet.
pub(super) fn take_inputs(mut players: Query<(&Connection, &mut PlayerInputs)>) {
    for (connection, mut inputs) in &mut players {
        for message in &connection.inbox {
            let ClientMessage::Inputs(sent) = message else {
                continue;
            };
            for input in sent {
                if input.tick > inputs.applied && inputs.waiting.len() < MAX_WAITING {
                    inputs.waiting.insert(input.tick, input.clone());
                }
            }
        }
    }
}

/// Steps each player once for each of its waiting inputs, in order, as
/// far as its budget allows.
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
    for (mut inputs, mut body, mut player, mut look, mut controls, mut transform) in &mut players {
        let inputs = &mut *inputs;
        inputs.budget = (inputs.budget + time.delta_secs_f64() * STEP_HZ as f64 * SLACK).min(BURST);
        let mut stepped = false;
        while inputs.budget >= 1.0 {
            let Some((_, input)) = inputs.waiting.pop_first() else {
                break;
            };
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
