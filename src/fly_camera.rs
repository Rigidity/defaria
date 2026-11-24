//! Minecraft-style fly camera.
//!
//! - Left click locks and hides the cursor; Escape releases it.
//! - While locked, the mouse turns the camera (yaw) and looks up/down (pitch).
//!   Pitch is clamped just short of straight up/down so the view never flips.
//! - WASD moves on the horizontal plane relative to where you face, Space
//!   flies up and Shift flies down. Looking up or down doesn't change the
//!   direction WASD moves you.

use std::f32::consts::FRAC_PI_2;

use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

/// Keep pitch just inside ±90° so the camera never looks exactly straight up
/// or down, where yaw becomes ambiguous.
const MAX_PITCH: f32 = FRAC_PI_2 - 0.01;

pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, (grab_cursor, look, fly).chain());
    }
}

#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
#[require(Camera3d)]
pub struct FlyCamera {
    /// Movement speed in units per second.
    pub speed: f32,
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
            speed: 10.0,
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

fn spawn_camera(mut commands: Commands) {
    // Start outside the voxel grid, looking back at it from above.
    let position = Vec3::new(36.0, 26.0, 36.0);
    let target = Vec3::splat(8.0);
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
        camera,
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

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cameras: Query<(&FlyCamera, &mut Transform)>,
) {
    let axis = |positive: KeyCode, negative: KeyCode| {
        keys.pressed(positive) as i8 as f32 - keys.pressed(negative) as i8 as f32
    };
    let forward_input = axis(KeyCode::KeyW, KeyCode::KeyS);
    let right_input = axis(KeyCode::KeyD, KeyCode::KeyA);
    let up_input = keys.pressed(KeyCode::Space) as i8 as f32
        - (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)) as i8 as f32;

    for (camera, mut transform) in &mut cameras {
        // Horizontal movement follows yaw only, like Minecraft's creative flight.
        let yaw = Quat::from_rotation_y(camera.yaw);
        let forward = yaw * Vec3::NEG_Z;
        let right = yaw * Vec3::X;
        let horizontal = (forward * forward_input + right * right_input).normalize_or_zero();
        let velocity = (horizontal + Vec3::Y * up_input) * camera.speed;
        transform.translation += velocity * time.delta_secs();
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
            .add_systems(Update, (grab_cursor, look, fly).chain());
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

    #[test]
    fn wasd_moves_horizontally_even_when_looking_down() {
        let mut app = app();
        let entity = spawn_camera(
            &mut app,
            FlyCamera {
                speed: 20.0,
                pitch: -1.2,
                ..default()
            },
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        // First update has zero delta time; the second moves by 0.1s.
        // (Time<Virtual> clamps steps to 250ms, so keep them small.)
        app.update();
        app.update();

        let position = app.world().get::<Transform>(entity).unwrap().translation;
        assert!(
            position.abs_diff_eq(Vec3::new(0.0, 0.0, -2.0), 1e-4),
            "{position:?}"
        );
    }

    #[test]
    fn space_flies_up_and_shift_flies_down() {
        let mut app = app();
        let entity = spawn_camera(
            &mut app,
            FlyCamera {
                speed: 20.0,
                ..default()
            },
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        app.update();
        app.update();
        let y = app.world().get::<Transform>(entity).unwrap().translation.y;
        assert!((y - 2.0).abs() < 1e-4, "{y}");

        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Space);
        keys.press(KeyCode::ShiftLeft);
        app.update();
        let y = app.world().get::<Transform>(entity).unwrap().translation.y;
        assert!(y.abs() < 1e-4, "{y}");
    }
}
