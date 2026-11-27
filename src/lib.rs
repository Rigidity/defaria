//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`].
//!
//! The game's code is in two crates: `defaria_core` (what the game is)
//! and `defaria_client` (how it's shown and played). Each feature is its
//! own plugin in its own module there, registered in [`GamePlugin`].

mod devtools;
pub mod headless;

pub use devtools::{Cli, DevToolsPlugin, ScreenshotMode};

use bevy::prelude::*;
use defaria_client::*;
use defaria_core::*;

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
            input::InputPlugin,
            crosshair::CrosshairPlugin,
            cracks::CracksPlugin,
            fluid::FluidPlugin,
            game_mode::GameModePlugin,
            connections::ConnectionsPlugin,
            network::NetworkPlugin,
            block_entity::BlockEntityPlugin,
        ))
        .add_plugins((
            machine::MachinePlugin,
            machine_screen::MachineScreenPlugin,
            inventory_screen::InventoryScreenPlugin,
            recipe::RecipePlugin,
            script::ScriptPlugin,
            script::machine::MachineScriptPlugin,
            wrench::WrenchPlugin,
            fluid_container::FluidContainerPlugin,
            support::SupportPlugin,
            use_action::UseActionPlugin,
            hotbar_ui::HotbarUiPlugin,
        ));
    }
}
