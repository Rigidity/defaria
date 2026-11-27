//! The local player's input: the mouse and keys turned into what the
//! player does (its intents), which gameplay then acts on the same way for
//! any player.

pub mod game_mode;
pub mod mining;
pub mod movement;
pub mod using;

#[cfg(test)]
mod tests;

use bevy::prelude::*;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            game_mode::GameModeInputPlugin,
            mining::MiningInputPlugin,
            movement::MovementInputPlugin,
            using::UseInputPlugin,
        ));
    }
}
