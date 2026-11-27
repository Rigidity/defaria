//! The inventory screen (E), and the item lists, search and mouse
//! gestures it and other screens are built from.
//!
//! - [`list`]: lists of items on screen, sorted ([`list::ItemList`]).
//! - [`gesture`]: the mouse on lists and the hotbar.
//! - [`search`]: search queries for item lists.
//! - [`screen`]: the inventory screen.

pub mod gesture;
pub mod list;
pub mod screen;
pub mod search;

use bevy::prelude::*;

pub struct InventoryScreenPlugin;

impl Plugin for InventoryScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            list::ListPlugin,
            search::SearchPlugin,
            gesture::GesturePlugin,
            screen::ScreenPlugin,
        ));
    }
}
