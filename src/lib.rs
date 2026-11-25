//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`].
//!
//! Each gameplay feature is its own plugin in its own module, registered in
//! [`GamePlugin`], so parallel work rarely touches the same file.

mod block_edit;
mod block_texture;
pub mod content;
mod devtools;
mod dynamic_light;
mod fly_camera;
mod fog;
mod fov;
pub mod headless;
mod hotbar;
mod inventory;
mod light;
mod mining;
mod perlin;
mod physics;
mod player;
pub mod save;
mod spline;
mod streaming;
mod terrain;
pub mod ui;
mod view_bob;
mod voxel;
mod voxel_material;

pub use devtools::{Cli, DevToolsPlugin, ScreenshotMode};

use bevy::prelude::*;

/// Registers every gameplay plugin.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // First: the others read the content as they start.
            content::ContentPlugin,
            ui::UiPlugin,
            hotbar::HotbarPlugin,
            inventory::InventoryPlugin,
            fly_camera::FlyCameraPlugin,
            physics::PhysicsPlugin,
            player::PlayerPlugin,
            view_bob::ViewBobPlugin,
            voxel::VoxelPlugin,
            voxel_material::VoxelMaterialPlugin,
            terrain::TerrainPlugin,
            block_edit::BlockEditPlugin,
            light::LightPlugin,
            dynamic_light::DynamicLightPlugin,
            streaming::StreamingPlugin,
        ))
        // Bevy takes at most 15 plugins in one tuple.
        .add_plugins((fog::FogPlugin, fov::FovPlugin, mining::MiningPlugin));
    }
}
