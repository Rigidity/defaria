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
mod fluid_container;
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
mod meshing;
mod mining;
pub mod network;
mod perlin;
mod physics;
mod player;
pub mod recipe;
pub mod save;
pub mod script;
pub mod streaming;
mod support;
#[cfg(test)]
mod testing;
pub mod ui;
mod use_action;
mod view_bob;
mod voxel_material;
mod world;
mod worldgen;
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
            meshing::MeshingPlugin,
            voxel_material::VoxelMaterialPlugin,
            world::WorldPlugin,
            block_edit::BlockEditPlugin,
            light::LightPlugin,
            dynamic_light::DynamicLightPlugin,
            streaming::StreamingPlugin,
        ))
        // Bevy takes at most 15 plugins in one tuple.
        .add_plugins((
            player::LocalPlayerPlugin,
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
            recipe::RecipePlugin,
            script::ScriptPlugin,
            script::machine::MachineScriptPlugin,
            wrench::WrenchPlugin,
            fluid_container::FluidContainerPlugin,
            support::SupportPlugin,
            use_action::UseActionPlugin,
        ));
    }
}
