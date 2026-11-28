//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`]: a
//! client ([`ClientPlugin`]) and a server ([`ServerPlugin`]) in its own
//! app, in this process, talking through messages (`defaria_core::net`).
//!
//! The game's code is in three crates: `defaria_core` (what both sides
//! know: content, the world, the player and the protocol),
//! `defaria_server` (what only the server runs) and `defaria_client` (how
//! it's shown and played). Each feature is its own plugin in its own
//! module there, registered in [`ClientPlugin`] or [`ServerPlugin`]. The
//! client builds from `defaria_core` and `defaria_client` alone, the
//! server from `defaria_core` and `defaria_server`.

pub mod connecting;
pub mod dedicated;
mod devtools;
pub mod headless;
pub mod identity;
pub mod integrated;
pub mod mod_transfer;
pub mod quic;
pub mod server;

pub use devtools::{Cli, DevToolsPlugin, ScreenshotMode};
pub use integrated::{IntegratedServer, IntegratedServerPlugin};

use bevy::prelude::*;

/// The game as it's played: the client, drawing and playing what a server
/// sends, and a server for it in this process ([`IntegratedServerPlugin`]).
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ClientPlugin,
            IntegratedServerPlugin,
            crate::connecting::BackgroundConnectPlugin,
        ));
    }
}

/// The client: drawing the chunks a server sends, the UI, and the local
/// player, moved by the keys and mouse, whose intents go to the server.
pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        // Only core and the client: the client builds without the server.
        use defaria_client::*;
        use defaria_core::*;

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
            defaria_client::replication::ReplicationPlugin,
            edit_prediction::EditPredictionPlugin,
            prediction::PredictionPlugin,
            defaria_client::script::ClientScriptPlugin,
            defaria_client::connecting::ConnectStatusPlugin,
            defaria_client::chat::ChatPlugin,
        ));
    }
}

/// The server: the world, its simulation, scripts and saving, and its
/// players' connections. Everything the game decides happens here.
pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        // Only core and the server: the server builds without the client.
        use defaria_core::{content, hotbar, physics, player, use_action, world};
        use defaria_server::*;

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
                defaria_server::server::ServerNetPlugin,
                defaria_server::chat::ChatPlugin,
                defaria_server::command::CommandPlugin,
            ));
    }
}
