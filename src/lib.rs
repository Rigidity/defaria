//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`].
//!
//! Each gameplay feature is its own plugin in its own module, registered in
//! [`GamePlugin`], so parallel work rarely touches the same file.

mod block_texture;
mod devtools;
mod fly_camera;
pub mod headless;
mod perlin;
mod terrain;
mod voxel;
mod voxel_material;

pub use devtools::{Cli, DevToolsPlugin, ScreenshotMode};

use bevy::prelude::*;

/// Registers every gameplay plugin.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            fly_camera::FlyCameraPlugin,
            voxel::VoxelPlugin,
            voxel_material::VoxelMaterialPlugin,
            terrain::TerrainPlugin,
        ));
    }
}
