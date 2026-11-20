//! Placeholder feature: a spinning square. Shows the module/plugin/test layout.

use bevy::prelude::*;

pub struct SpinnerPlugin;

impl Plugin for SpinnerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(Update, spin);
    }
}

/// Rotates the entity around Z. `Reflect` makes it visible and editable over BRP.
#[derive(Component, Reflect, Debug)]
#[reflect(Component)]
pub struct Spin {
    /// Radians per second.
    pub speed: f32,
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    commands.spawn(Camera2d);
    commands.spawn((
        Name::new("Spinner"),
        Spin { speed: 1.0 },
        Mesh2d(meshes.add(Rectangle::new(160.0, 160.0))),
        MeshMaterial2d(materials.add(Color::srgb(0.58, 0.2, 0.92))),
    ));
}

fn spin(time: Res<Time>, mut query: Query<(&Spin, &mut Transform)>) {
    for (spin, mut transform) in &mut query {
        transform.rotate_z(spin.speed * time.delta_secs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn z_angle(app: &App, entity: Entity) -> f32 {
        let transform = app.world().get::<Transform>(entity).unwrap();
        transform.rotation.to_euler(EulerRot::XYZ).2
    }

    /// Headless test pattern: MinimalPlugins (no window/GPU), manual time steps.
    #[test]
    fn spin_rotates_by_speed_times_delta() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                250,
            )))
            .add_systems(Update, spin);
        let entity = app
            .world_mut()
            .spawn((Spin { speed: 2.0 }, Transform::default()))
            .id();

        app.update();
        let before = z_angle(&app, entity);
        app.update();
        let after = z_angle(&app, entity);

        assert!(
            (after - before - 0.5).abs() < 1e-4,
            "rotated {}",
            after - before
        );
    }
}
