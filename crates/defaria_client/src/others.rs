//! The other players, drawn where the server says they are: a figure as
//! tall and wide as a player's body, turned the way they look, lower while
//! they sneak. Placeholder art; smoothing their movement between the
//! server's updates is #214's.

use bevy::prelude::*;
use defaria_core::{
    net::{OtherPlayer, ServerMessage},
    player::{EYE_HEIGHT, SIZE, SNEAK_EYE_HEIGHT},
};

use crate::{net::FromServer, session::GameState};

pub struct OthersPlugin;

impl Plugin for OthersPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<OtherPlayerView>()
            .add_systems(Update, show_other_players);
    }
}

/// Another player, drawn: which one ([`OtherPlayer::id`]).
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct OtherPlayerView(pub u64);

/// Spawns, moves and despawns the other players as the server sends them.
fn show_other_players(
    mut commands: Commands,
    mut messages: MessageReader<FromServer>,
    mut views: Query<(Entity, &OtherPlayerView, &mut Transform)>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut look: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    let Some(others) = messages
        .read()
        .fold(None, |last, FromServer(message)| match message {
            ServerMessage::Players(others) => Some(others.clone()),
            _ => last,
        })
    else {
        return;
    };
    // Where there's drawing (not in tests without it), one mesh and
    // material for every figure.
    if look.is_none()
        && let (Some(mut meshes), Some(mut materials)) = (meshes, materials)
    {
        let size = SIZE.as_vec3();
        *look = Some((
            meshes.add(Cuboid::new(size.x, size.y, size.z)),
            materials.add(Color::srgb(0.85, 0.55, 0.35)),
        ));
    }
    for (entity, view, mut transform) in &mut views {
        match others.iter().find(|other| other.id == view.0) {
            Some(other) => *transform = figure(other),
            None => commands.entity(entity).despawn(),
        }
    }
    for other in &others {
        if views.iter().any(|(_, view, _)| view.0 == other.id) {
            continue;
        }
        let mut figure_entity = commands.spawn((
            Name::new("Other player"),
            OtherPlayerView(other.id),
            figure(other),
            Visibility::default(),
            DespawnOnExit(GameState::Playing),
        ));
        if let Some((mesh, material)) = &*look {
            figure_entity.insert((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
        }
    }
}

/// Where a player's figure is drawn: its body's middle, turned with its
/// look and lowered as much as its eyes are while it sneaks.
fn figure(other: &OtherPlayer) -> Transform {
    let height = SIZE.y;
    let sink = if other.sneaking {
        EYE_HEIGHT - SNEAK_EYE_HEIGHT
    } else {
        0.0
    };
    let middle = other.feet + bevy::math::DVec3::Y * (height / 2.0 - sink);
    Transform::from_translation(middle.as_vec3()).with_rotation(other.look.yaw_rotation())
}
