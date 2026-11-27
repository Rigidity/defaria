//! Inventories: what's carried (the screen for it is `inventory_screen`).
//!
//! - [`model`]: [`Inventory`], how many of each item ([`ItemStack`]s),
//!   limited only by an optional weight, as a component anything can
//!   have.
//! - [`player`]: the local player's inventory: what a new player starts
//!   with, its weight limit, and saving it with the world.
//! - [`transfer`]: moving items between inventories and the catalog.

pub mod model;
pub mod player;
pub mod transfer;

use bevy::prelude::*;

pub use model::{Inventory, ItemStack};

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Inventory>()
            .add_plugins(player::PlayerInventoryPlugin);
    }
}
