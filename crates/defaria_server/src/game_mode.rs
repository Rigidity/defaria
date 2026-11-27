//! A player's game mode taking effect on the server: the weight limit
//! that goes with it. The modes and their rules are
//! `defaria_core::game_mode`.

use bevy::prelude::*;
use defaria_core::{
    game_mode::{GameMode, GameModeSystems},
    inventory::{Inventory, player::PLAYER_MAX_WEIGHT},
};

pub struct GameModePlugin;

impl Plugin for GameModePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<GameMode>()
            .add_systems(Update, limit_weight.in_set(GameModeSystems));
    }
}

/// The player's weight limit: none in creative.
fn limit_weight(mut players: Query<(&GameMode, &mut Inventory), Changed<GameMode>>) {
    for (mode, mut inventory) in &mut players {
        let limit = (!mode.is_creative()).then_some(PLAYER_MAX_WEIGHT);
        if inventory.max_weight != limit {
            inventory.max_weight = limit;
        }
    }
}
