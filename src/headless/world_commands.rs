//! The commands that act on this process's game directly rather than
//! through the server: `camera`, which puts the client's player
//! somewhere (as the mouse and keys move it), and `lookups`, which times
//! reading the server's voxels here. The scene-building ones (`fill`,
//! `give`, ...) and the `fluids` and `network` reports are server
//! commands (`defaria_server::command`), which the runner sends.

use bevy::prelude::*;

use super::{memory, runner::ScriptRunner, script::Command};
use defaria_client::view_bob::ViewBob;
use defaria_core::{
    physics::Body,
    player::{EYE_HEIGHT, LocalPlayer, Look, MAX_PITCH, Player, Teleported, put_eyes},
};

/// Applies this frame's `camera` and `lookups` commands. A mistake in one
/// (no camera, a box too big) fails the run.
pub(super) fn apply_world_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<ScriptRunner>().world_commands);
    for command in commands {
        let result = match command {
            Command::Camera {
                position,
                yaw,
                pitch,
            } => world
                .run_system_cached_with(place_camera, (position, yaw, pitch))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Lookups { min, max } => {
                let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
                match crate::integrated::with_server(world, |server| {
                    server
                        .run_system_cached_with(memory::measure_lookups, (min, max))
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                }) {
                    Some(Ok(cost)) => {
                        println!("[frame {frame}] {cost}");
                        Ok(())
                    }
                    Some(Err(error)) => Err(error),
                    None => Err("lookups: there's no server in this process".into()),
                }
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            error!("{error}");
            world.write_message(AppExit::error());
        }
    }
}

/// Puts the local player's eyes at `position`, looking along `yaw` and
/// `pitch`, flying, so the view stays put.
fn place_camera(
    In((position, yaw, pitch)): In<(Vec3, f32, f32)>,
    mut commands: Commands,
    mut players: Query<
        (
            Entity,
            &mut Look,
            &mut Transform,
            &mut Player,
            &mut Body,
            Option<&mut ViewBob>,
        ),
        With<LocalPlayer>,
    >,
) -> Result<(), String> {
    let (entity, mut look, mut transform, mut player, mut body, bob) = players
        .single_mut()
        .map_err(|error| format!("camera: {error}"))?;
    // The body turns to face the look as the frame is drawn.
    *look = Look {
        yaw: yaw.to_radians(),
        pitch: pitch.to_radians().clamp(-MAX_PITCH, MAX_PITCH),
    };
    player.flying = true;
    player.eye_height = EYE_HEIGHT;
    player.previous_eye_height = EYE_HEIGHT;
    player.drawn_eye_height = EYE_HEIGHT;
    if let Some(mut bob) = bob {
        bob.settle();
    }
    put_eyes(position.as_dvec3(), EYE_HEIGHT, &mut body, &mut transform);
    // The server puts it there too, with its next input.
    commands.entity(entity).insert(Teleported);
    Ok(())
}

#[cfg(test)]
mod tests;
