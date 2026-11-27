//! Playing a world, or the menus: the client's [`GameState`].
//!
//! The windowed game starts at the main menu ([`StartAtMenu`]); headless
//! runs and tests start playing. While [`GameState::Playing`] the client
//! has a local player and the chunks the server sends; everything that
//! belongs to the world it's playing is despawned when it leaves
//! ([`DespawnOnExit`]), and what it kept of it is cleared, so the next
//! world starts afresh. (The server, which owns the world, is started and
//! stopped with it, by whoever runs it: `defaria::integrated`.)

use std::path::PathBuf;

use bevy::prelude::*;
use defaria_core::{
    player::{EYE_HEIGHT, Look, START_POSITION, START_TARGET, feet_below, local_player},
    streaming::StreamingStatus,
    world::ChunkMap,
};

pub struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        let start = if app.world().contains_resource::<StartAtMenu>() {
            GameState::Menu
        } else {
            GameState::Playing
        };
        app.insert_state(start)
            .add_systems(OnEnter(GameState::Playing), spawn_the_local_player)
            .add_systems(OnExit(GameState::Playing), forget_the_world);
    }
}

/// Whether the client is playing a world or in the menus.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameState {
    #[default]
    Playing,
    Menu,
}

/// The game starts at the main menu rather than in a world. Inserted
/// before the game's plugins.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartAtMenu;

/// Where saved worlds are, for the world list and new worlds.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct WorldsFolder(pub PathBuf);

/// The local player, where new players start, looking at where they look.
fn spawn_the_local_player(mut commands: Commands) {
    let eyes = Transform::from_translation(START_POSITION).looking_at(START_TARGET, Vec3::Y);
    let feet = feet_below(START_POSITION.as_dvec3(), EYE_HEIGHT);
    commands.spawn((
        Name::new("Player"),
        local_player(feet, Look::from_rotation(eyes.rotation)),
        DespawnOnExit(GameState::Playing),
    ));
}

/// What the client kept of the world it left: the chunks' index (the
/// chunks themselves are despawned), and how loading it was going.
fn forget_the_world(mut map: ResMut<ChunkMap>, mut status: ResMut<StreamingStatus>) {
    *map = ChunkMap::default();
    *status = StreamingStatus::default();
}
