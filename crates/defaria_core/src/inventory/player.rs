//! What a new player has: [`DEFAULT_HOTBAR`] in their hotbar and
//! [`STARTING_COUNT`] of each of those items, carrying at most
//! [`PLAYER_MAX_WEIGHT`] (in survival; see `game_mode`). Giving players
//! theirs, and saving them in the world's `players/<id>.json5`, is the
//! server's (`defaria_server::inventory::player`).

use bevy::prelude::*;

use super::{Inventory, ItemStack};
use crate::{
    content::Content,
    hotbar::{HOTBAR_SLOTS, Hotbar},
};

/// The most the player carries, in kilograms: 10,000 tonnes. Provisional:
/// blocks weigh what a cubic metre of their material does (stone 2,500
/// kg), so this is about 4,000 stone. See `docs/decisions.md` and #87.
pub const PLAYER_MAX_WEIGHT: f32 = 10_000_000.0;

/// What a new player's hotbar holds, slot 1 first: each item's id and the
/// values of the properties it doesn't leave at their defaults.
pub const DEFAULT_HOTBAR: [(&str, &[(&str, &str)]); HOTBAR_SLOTS] = [
    ("defaria:stone", &[]),
    ("defaria:soil", &[("soil", "defaria:dirt")]),
    (
        "defaria:soil",
        &[("soil", "defaria:dirt"), ("turf", "defaria:grass")],
    ),
    ("defaria:soil", &[("soil", "defaria:sand")]),
    ("defaria:stone_slab", &[]),
    ("defaria:log", &[]),
    ("defaria:planks", &[]),
    ("defaria:lamp", &[]),
    ("defaria:red_lamp", &[]),
];

/// How many of each hotbar item a new player has.
pub const STARTING_COUNT: u32 = 64;

/// What a new player starts with: [`DEFAULT_HOTBAR`], and
/// [`STARTING_COUNT`] of each of its items. An item that didn't load is
/// reported and leaves its slot empty.
pub fn new_player(content: &Content) -> (Inventory, Hotbar) {
    // The weight limit comes with the game mode (`game_mode`).
    let mut inventory = Inventory::new();
    let mut hotbar = Hotbar::default();
    for (slot, (id, values)) in DEFAULT_HOTBAR.into_iter().enumerate() {
        match content.item(id).and_then(|item| item.state(values)) {
            Some(item) => {
                hotbar.0[slot] = Some(item);
                inventory.add(content, ItemStack::new(item, STARTING_COUNT));
            }
            None => {
                error!("a new player starts with item `{id}` with {values:?}, which didn't load")
            }
        }
    }
    (inventory, hotbar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::base;

    #[test]
    fn a_new_player_starts_with_the_default_hotbar_and_some_of_each() {
        let content = base();
        let (inventory, hotbar) = new_player(content);
        let ids: Vec<&str> = hotbar
            .0
            .iter()
            .map(|item| {
                let item = item.unwrap();
                assert_eq!(inventory.count(item), STARTING_COUNT);
                content.item_of(item).unwrap().id.as_str()
            })
            .collect();
        assert_eq!(ids, DEFAULT_HOTBAR.map(|(id, _)| id));
        // Slot 2 is plain dirt.
        assert_eq!(hotbar.0[1], Some(crate::content::dirt_item()));
        assert_eq!(inventory.entries().len(), HOTBAR_SLOTS);
    }

    /// Items that didn't load leave their slots empty.
    #[test]
    fn missing_default_items_leave_their_slots_empty() {
        let (inventory, hotbar) = new_player(&Content::from_mods(Vec::new()));
        assert!(inventory.is_empty());
        assert_eq!(hotbar, Hotbar::default());
    }
}
