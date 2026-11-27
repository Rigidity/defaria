//! Players' inventories on the server: what they're registered as, and
//! giving each player theirs and saving it ([`player`]). Inventories
//! themselves are `defaria_core::inventory`.

pub mod player;

use bevy::prelude::*;

use defaria_core::inventory::Inventory;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Inventory>()
            .add_plugins(player::PlayerInventoryPlugin);
    }
}
