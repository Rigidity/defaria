//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`]: a
//! client ([`ClientPlugin`]) and a server ([`ServerPlugin`]) in its own
//! app, in this process, talking through messages (`defaria_core::net`).
//!
//! The game's code is in two crates: `defaria_core` (what the game is)
//! and `defaria_client` (how it's shown and played). Each feature is its
//! own plugin in its own module there, registered in [`GamePlugin`].

pub mod dedicated;
mod devtools;
pub mod headless;
pub mod integrated;
pub mod quic;
pub mod server;

pub use devtools::{Cli, DevToolsPlugin, ScreenshotMode};
pub use integrated::{IntegratedServer, IntegratedServerPlugin};

use bevy::prelude::*;
use defaria_client::*;
use defaria_core::*;

/// The game as it's played: the client, drawing and playing what a server
/// sends, and a server for it in this process ([`IntegratedServerPlugin`]).
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ClientPlugin, IntegratedServerPlugin));
    }
}

/// The client: drawing the chunks a server sends, the UI, and the local
/// player, moved by the keys and mouse, whose intents go to the server.
pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // First: the others read the content as they start.
            content::ContentPlugin,
            ui::UiPlugin,
            hotbar::HotbarPlugin,
            fly_camera::FlyCameraPlugin,
            physics::PhysicsPlugin,
            player::PlayerPlugin,
            view_bob::ViewBobPlugin,
            meshing::MeshingPlugin,
            voxel_material::VoxelMaterialPlugin,
            world::WorldPlugin,
            dynamic_light::DynamicLightPlugin,
            session::SessionPlugin,
            fog::FogPlugin,
            fov::FovPlugin,
            input::InputPlugin,
        ))
        // Bevy takes at most 15 plugins in one tuple.
        .add_plugins((
            crosshair::CrosshairPlugin,
            cracks::CracksPlugin,
            machine_screen::MachineScreenPlugin,
            inventory_screen::InventoryScreenPlugin,
            use_action::UseActionPlugin,
            hotbar_ui::HotbarUiPlugin,
            defaria_client::net::ClientNetPlugin,
            menu::MenuPlugin,
            pause::PausePlugin,
            others::OthersPlugin,
            prediction::PredictionPlugin,
        ));
    }
}

/// The server: the world, its simulation, scripts and saving, and its
/// players' connections. Everything the game decides happens here.
pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<player::Player>()
            .register_type::<player::Look>()
            .add_plugins((
                // First: the others read the content as they start.
                content::ContentPlugin,
                world::WorldPlugin,
                physics::PhysicsPlugin,
                hotbar::HotbarPlugin,
                inventory::InventoryPlugin,
                block_edit::BlockEditPlugin,
                light::LightPlugin,
                streaming::StreamingPlugin,
                mining::MiningPlugin,
                fluid::FluidPlugin,
                game_mode::GameModePlugin,
                connections::ConnectionsPlugin,
                network::NetworkPlugin,
                block_entity::BlockEntityPlugin,
                machine::MachinePlugin,
            ))
            .add_plugins((
                recipe::RecipePlugin,
                script::ScriptPlugin,
                script::machine::MachineScriptPlugin,
                wrench::WrenchPlugin,
                fluid_container::FluidContainerPlugin,
                support::SupportPlugin,
                use_action::UseActionPlugin,
                defaria_core::server::ServerNetPlugin,
            ));
    }
}
