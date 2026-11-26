//! Lights that move: anything with a [`DynamicLight`] and a transform.
//!
//! Dynamic lights aren't spread through the voxels like block light (that
//! would remesh the chunks around a light every time it crossed a block).
//! The shader lights each pixel from them directly, with the same falloff as
//! block light: a light's level minus the distance to it, per colour channel,
//! combined with block light by taking the brighter. So they move smoothly
//! and cost nothing on the CPU, but they don't see walls: a light shines
//! through blocks within its reach. Faces turned away from a light don't get
//! it.
//!
//! Only the [`MAX_DYNAMIC_LIGHTS`] nearest the camera are drawn.
//!
//! The player carries one: [`PlayerLight`], on the fly camera. L switches it
//! on and off.

use bevy::prelude::*;

use crate::{
    fly_camera::FlyCamera,
    light::SkyLight,
    voxel_material::{
        BlockMaterial, GpuDynamicLight, LightingUniform, MAX_DYNAMIC_LIGHTS, VoxelMaterial,
    },
};

/// Switches the player's light on and off.
pub const TOGGLE_PLAYER_LIGHT: KeyCode = KeyCode::KeyL;
/// The player's light: red, green and blue levels. A warm torch light.
pub const PLAYER_LIGHT: Vec3 = Vec3::new(15.0, 14.0, 12.0);

pub struct DynamicLightPlugin;

impl Plugin for DynamicLightPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(give_the_player_a_light)
            .add_systems(Update, toggle_player_light.run_if(crate::ui::playing))
            .add_systems(
                PostUpdate,
                // After transforms propagate, so lights are drawn where they
                // are this frame.
                update_lighting.after(TransformSystems::Propagate),
            );
    }
}

/// A light that moves with its entity.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq)]
#[reflect(Component)]
pub struct DynamicLight {
    /// Red, green and blue levels, 0 to 15, as for a block's `light`: each
    /// channel reaches that many blocks, fading as it goes.
    pub levels: Vec3,
    pub on: bool,
}

/// Marks the light the player carries.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct PlayerLight;

fn give_the_player_a_light(add: On<Add<FlyCamera>>, mut commands: Commands) {
    commands.entity(add.entity).insert((
        PlayerLight,
        DynamicLight {
            levels: PLAYER_LIGHT,
            on: true,
        },
    ));
}

fn toggle_player_light(
    keys: Res<ButtonInput<KeyCode>>,
    mut lights: Query<&mut DynamicLight, With<PlayerLight>>,
) {
    if keys.just_pressed(TOGGLE_PLAYER_LIGHT) {
        for mut light in &mut lights {
            light.on = !light.on;
        }
    }
}

/// Puts the sky colour and the dynamic lights nearest the camera into the
/// chunk material, when they've changed.
fn update_lighting(
    sky: Res<SkyLight>,
    lights: Query<(&DynamicLight, &GlobalTransform)>,
    camera: Query<&GlobalTransform, With<FlyCamera>>,
    block: Res<BlockMaterial>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
) {
    let viewer = camera.iter().next().map_or(Vec3::ZERO, |t| t.translation());
    let uniform = lighting_uniform(
        sky.0,
        viewer,
        lights
            .iter()
            .map(|(light, transform)| (*light, transform.translation())),
    );
    // Only touch a material when something changed: modifying it makes
    // the renderer prepare it again, along with every chunk that uses it.
    // That's cheap with a few hundred chunks (a moving light costs ~0.2 ms a
    // frame); with many more, move the lights to a buffer of their own.
    // All three chunk materials (opaque, cut-out, translucent) get it.
    for handle in block.all() {
        if materials
            .get(handle)
            .is_some_and(|material| material.lighting != uniform)
            && let Some(mut material) = materials.get_mut(handle)
        {
            material.lighting = uniform;
        }
    }
}

/// The lighting the shader gets: the sky colour, and the lights that are
/// on, nearest `viewer` first (by how close their reach comes), as many as
/// it draws.
pub fn lighting_uniform(
    sky: Color,
    viewer: Vec3,
    lights: impl Iterator<Item = (DynamicLight, Vec3)>,
) -> LightingUniform {
    let mut lit: Vec<(f32, GpuDynamicLight)> = lights
        .filter(|(light, _)| light.on && light.levels.max_element() > 0.0)
        .map(|(light, position)| {
            let gap = position.distance(viewer) - light.levels.max_element();
            let gpu = GpuDynamicLight {
                position: position.extend(0.0),
                levels: light.levels.extend(0.0),
            };
            (gap, gpu)
        })
        .collect();
    lit.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut uniform = LightingUniform {
        sky: Vec4::from_array(sky.to_linear().to_f32_array()).with_w(0.0),
        ..default()
    };
    for (slot, (_, light)) in uniform.dynamic.iter_mut().zip(&lit) {
        *slot = *light;
    }
    uniform.dynamic_count = lit.len().min(MAX_DYNAMIC_LIGHTS) as u32;
    uniform
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(levels: f32) -> DynamicLight {
        DynamicLight {
            levels: Vec3::splat(levels),
            on: true,
        }
    }

    #[test]
    fn the_uniform_holds_the_sky_and_the_lights_that_are_on() {
        let off = DynamicLight {
            on: false,
            ..light(10.0)
        };
        let lights = [
            (light(10.0), Vec3::X),
            (off, Vec3::Y),
            (light(0.0), Vec3::Z),
        ];
        let uniform = lighting_uniform(Color::WHITE, Vec3::ZERO, lights.into_iter());
        assert_eq!(uniform.sky, Vec4::new(1.0, 1.0, 1.0, 0.0));
        assert_eq!(uniform.dynamic_count, 1);
        assert_eq!(uniform.dynamic[0].position, Vec4::new(1.0, 0.0, 0.0, 0.0));
        assert_eq!(uniform.dynamic[0].levels, Vec4::new(10.0, 10.0, 10.0, 0.0));
    }

    /// With more lights than the shader draws, it gets the ones whose reach
    /// comes nearest the camera.
    #[test]
    fn only_the_nearest_lights_are_drawn() {
        let far = (0..MAX_DYNAMIC_LIGHTS).map(|i| (light(5.0), Vec3::X * (100.0 + i as f32)));
        // Farther away, but bright enough to reach the camera.
        let bright = (light(15.0), Vec3::X * 20.0);
        let near = (light(5.0), Vec3::X * 2.0);
        let uniform = lighting_uniform(Color::WHITE, Vec3::ZERO, far.chain([bright, near]));
        assert_eq!(uniform.dynamic_count, MAX_DYNAMIC_LIGHTS as u32);
        // The near light's reach covers the camera by 3, the bright one's
        // misses by 5.
        assert_eq!(uniform.dynamic[0].position.x, 2.0);
        assert_eq!(uniform.dynamic[1].position.x, 20.0);
        assert!(uniform.dynamic.iter().all(|l| l.position.x < 100.0 + 14.0));
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            DynamicLightPlugin,
        ))
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<SkyLight>()
        .init_asset::<VoxelMaterial>();
        let mut materials = app.world_mut().resource_mut::<Assets<VoxelMaterial>>();
        let mut add = |alpha_mode| {
            materials.add(VoxelMaterial {
                blocks: Handle::default(),
                lighting: default(),
                alpha_mode,
            })
        };
        let block = BlockMaterial {
            cutout: add(AlphaMode::AlphaToCoverage),
            translucent: add(AlphaMode::Blend),
            ..BlockMaterial::unloaded(add(AlphaMode::Opaque))
        };
        app.insert_resource(block);
        app
    }

    /// The chunk materials' lighting: the same in all three (opaque,
    /// cut-out and translucent), so see-through blocks are lit alike.
    fn lighting(app: &App) -> LightingUniform {
        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        let [opaque, rest @ ..] = block.all().map(|h| materials.get(h).unwrap().lighting);
        assert!(
            rest.iter().all(|l| *l == opaque),
            "the materials' lighting differs"
        );
        opaque
    }

    /// The player's light reaches the chunk material where the player is,
    /// and L takes it away and brings it back.
    #[test]
    fn the_player_carries_a_light_that_l_toggles() {
        let mut app = app();
        let at = Vec3::new(3.0, 40.0, -7.0);
        let player = app
            .world_mut()
            .spawn((FlyCamera::default(), Transform::from_translation(at)))
            .id();
        app.update();
        let drawn = |app: &App| lighting(app).dynamic_count;
        assert_eq!(drawn(&app), 1);
        assert_eq!(lighting(&app).dynamic[0].position, at.extend(0.0));
        assert_eq!(lighting(&app).dynamic[0].levels, PLAYER_LIGHT.extend(0.0));
        let on = |app: &App| app.world().get::<DynamicLight>(player).unwrap().on;
        assert!(on(&app), "the player's light starts on");
        assert_eq!(
            app.world().get::<DynamicLight>(player).unwrap().levels,
            PLAYER_LIGHT
        );
        let press = |app: &mut App| {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(TOGGLE_PLAYER_LIGHT);
            app.update();
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(TOGGLE_PLAYER_LIGHT);
            keys.clear();
            app.update();
        };
        press(&mut app);
        assert!(!on(&app));
        assert_eq!(drawn(&app), 0);
        press(&mut app);
        assert!(on(&app));
        assert_eq!(drawn(&app), 1);
        // While a screen (the inventory) is open, L is the screen's.
        app.world_mut().spawn(crate::ui::UiScreen);
        press(&mut app);
        assert!(on(&app));
    }
}
