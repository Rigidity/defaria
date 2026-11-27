//! C switches the local player between creative and survival while
//! playing (see `game_mode`).

use bevy::prelude::*;

use crate::{
    net::ToServer,
    ui::{playing, typing},
};
use defaria_core::{game_mode::GameModeSystems, net::ClientMessage};

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

/// C asks the server to switch between creative and survival.
fn switch_mode(keys: Res<ButtonInput<KeyCode>>, mut to_server: MessageWriter<ToServer>) {
    if keys.just_pressed(KeyCode::KeyC) {
        to_server.write(ToServer(ClientMessage::ToggleGameMode));
    }
}
