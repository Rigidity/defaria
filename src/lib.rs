//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`].
//!
//! Each gameplay feature is its own plugin in its own module, registered in
//! [`GamePlugin`], so parallel work rarely touches the same file.

mod block_edit;
pub mod block_entity;
mod block_texture;
mod connections;
pub mod content;
mod cracks;
mod devtools;
mod dynamic_light;
mod fluid;
pub mod fluid_network;
mod fly_camera;
mod fog;
mod fov;
pub mod game_mode;
pub mod headless;
mod hotbar;
pub mod inventory;
mod light;
pub mod machine;
mod mining;
pub mod network;
mod perlin;
mod physics;
mod player;
pub mod recipe;
pub mod save;
pub mod script;
mod spline;
pub mod streaming;
mod terrain;
pub mod ui;
mod view_bob;
mod voxel;
mod voxel_material;
mod wrench;

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
        .add_plugins((
            fog::FogPlugin,
            fov::FovPlugin,
            mining::MiningPlugin,
            cracks::CracksPlugin,
            fluid::FluidPlugin,
            game_mode::GameModePlugin,
            connections::ConnectionsPlugin,
            network::NetworkPlugin,
            block_entity::BlockEntityPlugin,
        ))
        .add_plugins((
            machine::MachinePlugin,
            machine::screen::MachineScreenPlugin,
            network::energy::EnergyPlugin,
            fluid_network::FluidNetworkPlugin,
            recipe::RecipePlugin,
            network::ItemNetworkPlugin,
            script::ScriptPlugin,
            script::machine::MachineScriptPlugin,
            wrench::WrenchPlugin,
        ));
    }
}
