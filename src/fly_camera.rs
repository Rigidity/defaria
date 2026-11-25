//! Minecraft-style first-person camera: where the player looks.
//!
//! - Left click locks and hides the cursor; Escape releases it.
//! - While locked, the mouse turns the camera (yaw) and looks up/down (pitch).
//!   Pitch is clamped just short of straight up/down so the view never flips.
//!
//! The [`FlyCamera`] is the player's eyes: `player` moves it (walking,
//! jumping and flying) and keeps it at the player's body. What's drawn is
//! seen through its child, the [`EyeView`], which `view_bob` sways as the
//! player walks, so the swaying never moves what the player aims at.

use std::f32::consts::FRAC_PI_2;

use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

/// Keep pitch just inside ±90° so the camera never looks exactly straight up
/// or down, where yaw becomes ambiguous.
pub const MAX_PITCH: f32 = FRAC_PI_2 - 0.01;

/// Where the camera spawns: just above the hills, so there's ground right
/// below. The scenarios' coordinates are picked around it.
pub const START_POSITION: Vec3 = Vec3::new(64.0, 56.0, 92.0);
/// The point the camera looks at when it spawns.
pub const START_TARGET: Vec3 = Vec3::new(64.0, 28.0, 56.0);
/// The vertical field of view, in degrees.
pub const FOV: f32 = 70.0;

pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, (grab_cursor, look).chain().in_set(FlyCameraSystems))
            // A screen (the inventory) has the mouse while it's open.
            .configure_sets(Update, FlyCameraSystems.run_if(crate::ui::playing));
    }
}

/// The camera's input systems: cursor grabbing and looking.
/// Systems that need the cursor's state from before this frame's click
/// (like block editing) run before them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlyCameraSystems;

#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct FlyCamera {
    /// Radians turned per pixel of mouse motion.
    pub sensitivity: f32,
    /// Rotation around the world Y axis, in radians.
    pub yaw: f32,
    /// Rotation up (+) or down (-), in radians, within ±[`MAX_PITCH`].
    pub pitch: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            sensitivity: 0.002,
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

impl FlyCamera {
    /// Builds the camera's rotation from yaw and pitch. No roll, ever.
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }
}

/// The camera that draws the view: a child of the [`FlyCamera`], at the
/// eyes but for view bobbing.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
#[require(Camera3d)]
pub struct EyeView;

fn spawn_camera(mut commands: Commands) {
    let (position, target) = (START_POSITION, START_TARGET);
    let (yaw, pitch, _) = Transform::from_translation(position)
        .looking_at(target, Vec3::Y)
        .rotation
        .to_euler(EulerRot::YXZ);
    let camera = FlyCamera {
        yaw,
        pitch,
        ..default()
    };
    commands.spawn((
        Name::new("Fly Camera"),
        Transform::from_translation(position).with_rotation(camera.rotation()),
        Visibility::default(),
        camera,
        children![(
            Name::new("Eye View"),
            EyeView,
            Projection::Perspective(PerspectiveProjection {
                fov: FOV.to_radians(),
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

fn look(
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    motion: Res<AccumulatedMouseMotion>,
    mut cameras: Query<(&mut FlyCamera, &mut Transform)>,
) {
    if cursor.grab_mode == CursorGrabMode::None || motion.delta == Vec2::ZERO {
        return;
    }
    for (mut camera, mut transform) in &mut cameras {
        let delta = motion.delta * camera.sensitivity;
        camera.yaw -= delta.x;
        camera.pitch = (camera.pitch - delta.y).clamp(-MAX_PITCH, MAX_PITCH);
        transform.rotation = camera.rotation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
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
            .add_systems(Update, (grab_cursor, look).chain());
        app.world_mut()
            .spawn((Window::default(), CursorOptions::default(), PrimaryWindow));
        app
    }

    fn spawn_camera(app: &mut App, camera: FlyCamera) -> Entity {
        app.world_mut()
            .spawn((
                Transform::default().with_rotation(camera.rotation()),
                camera,
            ))
            .id()
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
        let entity = spawn_camera(&mut app, FlyCamera::default());
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(100.0, 0.0);
        app.update();
        assert_eq!(app.world().get::<FlyCamera>(entity).unwrap().yaw, 0.0);
    }

    #[test]
    fn pitch_is_clamped_so_the_view_never_flips() {
        let mut app = app();
        let entity = spawn_camera(&mut app, FlyCamera::default());
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        // A huge upward flick: far more than 90° worth of motion.
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(0.0, -10_000.0);
        app.update();

        let camera = app.world().get::<FlyCamera>(entity).unwrap();
        assert_eq!(camera.pitch, MAX_PITCH);
        let forward = app.world().get::<Transform>(entity).unwrap().forward();
        assert!(forward.y > 0.99, "looking almost straight up: {forward:?}");
        assert!(forward.y < 1.0, "but never exactly: {forward:?}");
    }
}
