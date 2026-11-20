//! The game. `main.rs` only parses CLI flags and runs [`GamePlugin`].
//!
//! Each gameplay feature is its own plugin in its own module, registered in
//! [`GamePlugin`], so parallel work rarely touches the same file.

mod devtools;
mod spinner;

pub use devtools::{DevToolsPlugin, ScreenshotMode};

use bevy::prelude::*;

/// Registers every gameplay plugin.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(spinner::SpinnerPlugin);
    }
}
