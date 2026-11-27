//! The hotbar: [`HOTBAR_SLOTS`] quick picks, one of them active (drawn
//! along the bottom of the screen and picked with the keys by
//! `hotbar_ui`). Right click places the active slot's block
//! (see `block_edit`).
//!
//! A slot doesn't hold items: it names a kind of item, and shows how many
//! of it the player's [`Inventory`] has. Running out leaves the slot as it
//! was, at 0 and dimmed (in survival), ready for when more turn up (as in
//! Factorio).
//!
//! - The number keys 1–9 pick the active slot, in game or in a screen
//!   (unless a text field has the keyboard).
//! - The mouse wheel steps through the slots while playing: away from you
//!   is the slot to the left, toward you the one to the right, wrapping.
//! - Switching slots shows the item's name above the hotbar for a moment.
//! - With a screen open, clicking a slot makes it active and right clicking
//!   clears it. Items are dragged onto slots from the inventory screen, and
//!   slots dragged onto each other (see `inventory::gesture`).
//!
//! The slots ([`Hotbar`]) and [`ActiveSlot`] are components of the player
//! (the entity with [`LocalPlayer`]), so every player or mob has its own;
//! the hotbar shows the local player's.

use bevy::prelude::*;

use crate::content::{BlockState, Content, ItemState};

pub const HOTBAR_SLOTS: usize = 9;

pub struct HotbarPlugin;

impl Plugin for HotbarPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Hotbar>().register_type::<ActiveSlot>();
    }
}

/// The kind of item in each hotbar slot, slot 1 first.
#[derive(
    Component, Reflect, Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[reflect(Component)]
pub struct Hotbar(pub [Option<ItemState>; HOTBAR_SLOTS]);

impl Hotbar {
    /// The item the active slot names.
    pub fn item(&self, active: ActiveSlot) -> Option<ItemState> {
        self.0.get(active.0).copied().flatten()
    }

    /// The block the active slot's item places, if it places one.
    pub fn block(&self, active: ActiveSlot, content: &Content) -> Option<BlockState> {
        content.item_state(self.item(active)?)?.block
    }

    /// Puts `item` in `slot`, taking it out of any other slot it was in,
    /// so each kind is in one slot at most.
    pub fn assign(&mut self, slot: usize, item: Option<ItemState>) {
        if let Some(item) = item {
            for other in &mut self.0 {
                if *other == Some(item) {
                    *other = None;
                }
            }
        }
        if let Some(there) = self.0.get_mut(slot) {
            *there = item;
        }
    }
}

/// The hotbar slot in hand, from 0: whose item right click uses.
#[derive(
    Component,
    Reflect,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Component)]
pub struct ActiveSlot(pub usize);

/// The slot `steps` along from `slot`, wrapping around the ends.
pub fn step_slot(slot: usize, steps: i32) -> usize {
    (slot as i32 + steps).rem_euclid(HOTBAR_SLOTS as i32) as usize
}
