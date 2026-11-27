//! Minecraft-style first-person camera: what the local player sees.
//!
//! - Left click locks and hides the cursor; Escape releases it.
//! - While locked, the mouse turns the player (yaw) and looks up/down
//!   (pitch): its [`Look`]. Pitch is clamped just short of straight up/down
//!   so the view never flips.
//!
//! The [`FlyCamera`] is a child of the local player, at its eyes: raised to
//! [`Player::drawn_eye_height`] above its feet and pitched by its look (the
//! player's own transform has the yaw). It only shows what the player's
//! eyes see; gameplay works from the player's [`Eyes`](defaria_core::player::Eyes)
//! and never from here. What's drawn is seen through the camera's child,
//! the [`EyeView`], which `view_bob` sways as the player walks and `fov`
//! widens, so neither changes what the player aims at.

use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    transform::TransformSystems,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use defaria_core::player::{LocalPlayer, Look, Player};

/// The vertical field of view, in degrees.
pub const FOV: f32 = 70.0;

pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(give_the_player_a_camera)
            .add_systems(Update, (grab_cursor, look).chain().in_set(FlyCameraSystems))
            // A screen (the inventory) has the mouse while it's open, and
            // Escape closing it leaves the cursor locked.
            .configure_sets(
                Update,
                FlyCameraSystems
                    .run_if(crate::ui::playing)
                    .before(crate::ui::screen::CloseOnEscapeSystems),
            )
            .add_systems(
                PostUpdate,
                place_the_camera.before(TransformSystems::Propagate),
            );
    }
}

/// The camera's input systems: cursor grabbing and looking.
/// Systems that need the cursor's state from before this frame's click
/// (like block editing) run before them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlyCameraSystems;

/// The first-person camera: a child of the player it's the eyes of.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
#[require(Transform, Visibility)]
pub struct FlyCamera {
    /// Radians turned per pixel of mouse motion.
    pub sensitivity: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self { sensitivity: 0.002 }
    }
}

/// How near the camera draws, in blocks. Close, so that with your eyes
/// just above or below a water surface you still see it, rather than
/// through it. Depth is reversed, so this costs no precision further out.
pub const NEAR: f32 = 0.01;

/// The camera that draws the view: a child of the [`FlyCamera`], at the
/// eyes but for view bobbing.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
#[require(Camera3d)]
pub struct EyeView;

/// Gives the local player a camera at its eyes: the [`FlyCamera`], with the
/// [`EyeView`] under it.
pub fn give_the_player_a_camera(
    add: On<Add<LocalPlayer>>,
    players: Query<(&Player, &Look)>,
    mut commands: Commands,
) {
    let transform = players
        .get(add.entity)
        .map_or(Transform::IDENTITY, |(player, look)| {
            camera_transform(player, look)
        });
    commands.spawn((
        Name::new("Fly Camera"),
        FlyCamera::default(),
        transform,
        ChildOf(add.entity),
        children![(
            Name::new("Eye View"),
            EyeView,
            Projection::Perspective(PerspectiveProjection {
                fov: FOV.to_radians(),
                near: NEAR,
                ..default()
            }),
            // Show the shader's colors as they are: Bevy's default
            // tonemapping shifts hues and contrast, and dithering adds
            // noise, both of which fight hand-picked pixel art colors.
            Tonemapping::None,
            DebandDither::Disabled,
        )],
    ));
}

/// Where the camera goes on its player: at the eyes, pitched.
fn camera_transform(player: &Player, look: &Look) -> Transform {
    Transform::from_xyz(0.0, player.drawn_eye_height as f32, 0.0)
        .with_rotation(look.pitch_rotation())
}

/// Puts each camera at its player's eyes, as the frame is drawn.
fn place_the_camera(
    players: Query<(&Player, &Look)>,
    mut cameras: Query<(&ChildOf, &mut Transform), With<FlyCamera>>,
) {
    for (parent, mut transform) in &mut cameras {
        if let Ok((player, look)) = players.get(parent.parent()) {
            transform.set_if_neq(camera_transform(player, look));
        }
    }
}

fn grab_cursor(
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        // macOS doesn't support Confined and Locked isn't on X11; Bevy falls
        // back gracefully either way.
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// The mouse turns the player each camera is the eyes of.
fn look(
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    motion: Res<AccumulatedMouseMotion>,
    cameras: Query<(&FlyCamera, &ChildOf)>,
    mut looks: Query<&mut Look>,
) {
    if cursor.grab_mode == CursorGrabMode::None || motion.delta == Vec2::ZERO {
        return;
    }
    for (camera, parent) in &cameras {
        if let Ok(mut look) = looks.get_mut(parent.parent()) {
            let delta = motion.delta * camera.sensitivity;
            look.turn(-delta.x, -delta.y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use defaria_core::player::MAX_PITCH;
    use std::time::Duration;

    /// Headless app with a fake primary window, manual time and raw input
    /// resources we can poke directly.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .add_systems(Update, (grab_cursor, look).chain())
            .add_systems(PostUpdate, place_the_camera);
        app.world_mut()
            .spawn((Window::default(), CursorOptions::default(), PrimaryWindow));
        app
    }

    /// A player with a camera; the player.
    fn spawn_camera(app: &mut App) -> Entity {
        let player = app.world_mut().spawn(Player::default()).id();
        app.world_mut()
            .spawn((FlyCamera::default(), ChildOf(player)));
        player
    }

    fn camera(app: &mut App) -> Transform {
        let mut cameras = app
            .world_mut()
            .query_filtered::<&Transform, With<FlyCamera>>();
        *cameras.single(app.world()).unwrap()
    }

    fn cursor(app: &mut App) -> CursorOptions {
        app.world_mut()
            .query_filtered::<&CursorOptions, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap()
            .clone()
    }

    #[test]
    fn click_locks_cursor_and_escape_releases_it() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        let locked = cursor(&mut app);
        assert_eq!(locked.grab_mode, CursorGrabMode::Locked);
        assert!(!locked.visible);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        let released = cursor(&mut app);
        assert_eq!(released.grab_mode, CursorGrabMode::None);
        assert!(released.visible);
    }

    #[test]
    fn mouse_is_ignored_until_cursor_is_locked() {
        let mut app = app();
        let player = spawn_camera(&mut app);
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(100.0, 0.0);
        app.update();
        assert_eq!(app.world().get::<Look>(player).unwrap().yaw, 0.0);
    }

    #[test]
    fn pitch_is_clamped_so_the_view_never_flips() {
        let mut app = app();
        let player = spawn_camera(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        // A huge upward flick: far more than 90° worth of motion.
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(0.0, -10_000.0);
        app.update();

        let look = *app.world().get::<Look>(player).unwrap();
        assert_eq!(look.pitch, MAX_PITCH);
        let forward = camera(&mut app).forward();
        assert!(forward.y > 0.99, "looking almost straight up: {forward:?}");
        assert!(forward.y < 1.0, "but never exactly: {forward:?}");
    }

    /// The mouse turns the player: the yaw is the player's, and the camera,
    /// at its eyes, only pitches.
    #[test]
    fn the_mouse_turns_the_player_and_pitches_the_camera() {
        let mut app = app();
        let player = spawn_camera(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(-200.0, 100.0);
        app.update();
        let look = *app.world().get::<Look>(player).unwrap();
        assert!((look.yaw - 0.4).abs() < 1e-6, "{look:?}");
        assert!((look.pitch + 0.2).abs() < 1e-6, "{look:?}");
        let camera = camera(&mut app);
        assert_eq!(camera.rotation, look.pitch_rotation());
        assert_eq!(
            camera.translation,
            Vec3::Y * defaria_core::player::EYE_HEIGHT as f32
        );
    }

    /// The camera follows the eye height drawn this frame, so it lowers as
    /// the player sneaks.
    #[test]
    fn the_camera_lowers_with_the_eyes() {
        let mut app = app();
        let player = spawn_camera(&mut app);
        app.update();
        app.world_mut()
            .get_mut::<Player>(player)
            .unwrap()
            .drawn_eye_height = 1.4;
        app.update();
        assert_eq!(camera(&mut app).translation, Vec3::Y * 1.4);
    }
}
