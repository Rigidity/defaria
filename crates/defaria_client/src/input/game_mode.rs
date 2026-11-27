//! C switches the local player between creative and survival while
//! playing (see `game_mode`).

use bevy::prelude::*;

use crate::ui::{playing, typing};
use defaria_core::{
    game_mode::{GameMode, GameModeSystems},
    player::LocalPlayer,
};

pub struct GameModeInputPlugin;

impl Plugin for GameModeInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            switch_mode
                .run_if(playing)
                .run_if(not(typing))
                .before(GameModeSystems),
        );
    }
}

/// C switches between creative and survival.
fn switch_mode(
    keys: Res<ButtonInput<KeyCode>>,
    player: Option<Single<&mut GameMode, With<LocalPlayer>>>,
) {
    if keys.just_pressed(KeyCode::KeyC)
        && let Some(mut mode) = player
    {
        **mode = match **mode {
            GameMode::Creative => GameMode::Survival,
            GameMode::Survival => GameMode::Creative,
        };
        info!("game mode: {:?}", **mode);
    }
}
