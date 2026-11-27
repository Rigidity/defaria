//! The crosshair in the middle of the screen (hidden while a screen is
//! open), and an outline around the block it targets (see `block_edit`).

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    fly_camera::FlyCameraSystems,
    ui::{UiScreen, playing},
};
use defaria_core::{
    block_edit::{aims_at_fluid, crosshair_target, fluid_shape, fluid_target, target_shape},
    content::{BlockState, Content},
    hotbar::{ActiveSlot, Hotbar},
    player::{Eyes, LocalPlayer},
    world::Voxels,
};

pub struct CrosshairPlugin;

impl Plugin for CrosshairPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_crosshair)
            // After `Update`, so a screen opened or closed there shows on
            // the same frame.
            .add_systems(
                PostUpdate,
                show_crosshair
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(
                Update,
                // After the camera moves, so the outline matches the frame
                // being drawn.
                outline_target
                    .in_set(BlockOutlineSystems)
                    .after(FlyCameraSystems),
            );
    }
}

/// Where the block under the crosshair is outlined. Systems that change
/// blocks run before it, so the outline never surrounds a block removed
/// on the frame being drawn.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockOutlineSystems;

/// Outlines the block under the crosshair while the cursor is locked: or,
/// with a fluid container in hand, the fluid or block it aims at.
fn outline_target(
    mut gizmos: Gizmos,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    eyes: Single<Eyes, With<LocalPlayer>>,
    player: Option<Single<(&Hotbar, &ActiveSlot), With<LocalPlayer>>>,
    content: Res<Content>,
    voxels: Voxels,
) {
    let camera = eyes.transform();
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    let at_fluid = player.is_some_and(|player| {
        let (hotbar, active) = player.into_inner();
        aims_at_fluid(&content, hotbar.item(*active))
    });
    let hit = if at_fluid {
        fluid_target(&camera, &content, &voxels)
    } else {
        crosshair_target(&camera, &content, &voxels)
    };
    let Some(hit) = hit else {
        return;
    };
    let (state, partial) = voxels
        .get_with_fluid(hit.block)
        .unwrap_or((BlockState::AIR, None));
    let shape = if at_fluid {
        fluid_shape(&content, state, partial)
    } else if content.halves(state).is_some() {
        // Just the half under the crosshair.
        let boxes = target_shape(&content, state);
        boxes.get(hit.shape_box).map_or(boxes, std::slice::from_ref)
    } else {
        target_shape(&content, state)
    };
    for b in shape {
        // A little bigger than the box, so it isn't hidden in its faces.
        let center = hit.block.as_vec3() + (b.min + b.max) / 2.0;
        let size = b.max - b.min + Vec3::splat(0.004);
        gizmos.cube(
            Transform::from_translation(center).with_scale(size),
            Color::srgba(0.05, 0.05, 0.05, 0.8),
        );
    }
}

/// The crosshair's root node.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct Crosshair;

/// Shows the crosshair only while playing.
pub fn show_crosshair(
    screens: Query<(), With<UiScreen>>,
    mut crosshairs: Query<&mut Visibility, With<Crosshair>>,
) {
    let wanted = if playing(screens) {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut crosshairs {
        visibility.set_if_neq(wanted);
    }
}

pub fn spawn_crosshair(mut commands: Commands) {
    // Two thin bars crossing at the middle of the screen.
    let bar = |width: f32, height: f32| {
        (
            Node {
                position_type: PositionType::Absolute,
                width: px(width),
                height: px(height),
                ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
        )
    };
    commands.spawn((
        Name::new("Crosshair"),
        Crosshair,
        Node {
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Pickable::IGNORE,
        children![bar(16.0, 2.0), bar(2.0, 16.0)],
    ));
}
