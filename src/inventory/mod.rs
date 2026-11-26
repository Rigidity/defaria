//! Inventories: what's carried, and the screen for it.
//!
//! - [`model`]: [`Inventory`], how many of each item ([`ItemStack`]s),
//!   limited only by an optional weight, as a component anything can
//!   have.
//! - [`player`]: the local player's inventory: what a new player starts
//!   with, its weight limit, and saving it with the world.
//! - [`transfer`]: moving items between inventories and the catalog.
//! - [`list`]: lists of items on screen, sorted ([`list::ItemList`]).
//! - [`gesture`]: the mouse on lists and the hotbar.
//! - [`search`]: search queries for item lists.
//! - [`screen`]: the inventory screen (E).

pub mod gesture;
pub mod list;
pub mod model;
pub mod player;
pub mod screen;
pub mod search;
pub mod transfer;

use bevy::prelude::*;

pub use model::{Inventory, ItemStack};

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Inventory>().add_plugins((
            player::PlayerInventoryPlugin,
            list::ListPlugin,
            search::SearchPlugin,
            gesture::GesturePlugin,
            screen::ScreenPlugin,
        ));
    }
}
