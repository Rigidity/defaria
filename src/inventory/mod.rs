//! Inventories: what's carried, and the screen for it.
//!
//! - [`model`]: [`Inventory`], numbered slots of [`ItemStack`]s with an
//!   optional weight limit, as a component anything can have.
//! - [`player`]: the local player's inventory: what a new player starts
//!   with, its weight limit, and saving it with the world.
//! - [`screen`]: the inventory screen (E).

pub mod model;
pub mod player;
pub mod screen;

use bevy::prelude::*;

pub use model::{Inventory, ItemStack};

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Inventory>()
            .add_plugins((player::PlayerInventoryPlugin, screen::ScreenPlugin));
    }
}
