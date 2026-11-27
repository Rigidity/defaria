//! What an inventory holds: how many of each item, limited only by their
//! total weight.
//!
//! An [`Inventory`] is a component, so anything can have one: the player
//! today, and later chests and machines (on an entity for the block),
//! mobs and other players. Everything here is plain data and functions of
//! it, so the rules are the same wherever an inventory is and can be
//! tested without a game.
//!
//! - It holds [`ItemStack`]s, each some number of one item with the same
//!   [`ItemData`]: equal stacks merge, so there's one entry per kind of
//!   item, whatever the number. Nothing limits a count but weight.
//! - A fluid container (a bucket) keeps the fluid it holds in its data,
//!   so filled ones count together only with ones holding the same fluid
//!   and amount, and the fluid adds to its weight.
//! - A `unique` item (see `docs/modding.md`) is the exception: every copy
//!   is its own entry, with a count of one, as it will carry data of its
//!   own (wear, say).
//! - With a [`Inventory::max_weight`], nothing goes in that would take the
//!   total over it.
//! - Entries are kept in the order they arrived. How they're shown (sorted,
//!   grouped, filtered) is up to the screen.

use bevy::prelude::*;

use crate::{
    content::{Content, FULL_FLUID, ItemState},
    fluid_network::FluidAmount,
};

/// What sets one copy of an item apart from another of the same state:
/// the fluid a container holds (see `fluid_container`). Wear, names and
/// contents (a bag's) will go here too. Stacks only merge when their data
/// is equal.
#[derive(Reflect, Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ItemData {
    /// What a fluid container holds; `None` empty (never an amount of 0).
    pub fluid: Option<FluidAmount>,
}

impl ItemData {
    /// Data holding `amount` of `fluid`, or nothing for none.
    pub fn holding(fluid: &str, amount: u64) -> Self {
        Self {
            fluid: (amount > 0).then(|| FluidAmount {
                fluid: fluid.to_owned(),
                amount,
            }),
        }
    }

    /// What this data adds to one item's weight, in kilograms: its fluid,
    /// at the fluid's block's weight per full block.
    pub fn weight(&self, content: &Content) -> f32 {
        self.fluid.as_ref().map_or(0.0, |held| {
            let per_block = content
                .default_state(&held.fluid)
                .map_or(0.0, |state| content.state(state).weight);
            per_block * held.amount as f32 / FULL_FLUID as f32
        })
    }
}

/// Some number of one item state, all with the same data.
#[derive(Reflect, Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ItemStack {
    pub item: ItemState,
    pub data: ItemData,
    /// At least 1 in an inventory.
    pub count: u32,
}

impl ItemStack {
    pub fn new(item: ItemState, count: u32) -> Self {
        Self {
            item,
            data: ItemData::default(),
            count,
        }
    }

    /// One of `item`.
    pub fn one(item: ItemState) -> Self {
        Self::new(item, 1)
    }

    /// What one of it weighs, in kilograms, with what it holds.
    pub fn weight_each(&self, content: &Content) -> f32 {
        item_weight(content, self.item) + self.data.weight(content)
    }

    /// What this many of it weigh, in kilograms.
    pub fn weight(&self, content: &Content) -> f32 {
        self.weight_each(content) * self.count as f32
    }

    /// Whether `other` is the same kind: they count together, unless the
    /// item is unique.
    pub fn same_kind(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.data == other.data
    }
}

/// What one `item` weighs, in kilograms.
pub fn item_weight(content: &Content, item: ItemState) -> f32 {
    content.item_state(item).map_or(0.0, |info| info.weight)
}

/// Whether every copy of `item` is its own entry.
pub fn is_unique(content: &Content, item: ItemState) -> bool {
    content.item_of(item).is_some_and(|item| item.unique)
}

/// Items and how many of each, optionally capped by weight.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[reflect(Component)]
pub struct Inventory {
    entries: Vec<ItemStack>,
    /// The most its items may weigh together, in kilograms. `None` for no
    /// limit.
    pub max_weight: Option<f32>,
}

impl Inventory {
    /// An empty inventory with no weight limit.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_weight(mut self, max_weight: f32) -> Self {
        self.max_weight = Some(max_weight);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in the order they arrived.
    pub fn entries(&self) -> &[ItemStack] {
        &self.entries
    }

    /// What everything in it weighs, in kilograms.
    pub fn weight(&self, content: &Content) -> f32 {
        self.entries.iter().map(|stack| stack.weight(content)).sum()
    }

    /// How many of `item` it has, in all its entries (of any data).
    pub fn count(&self, item: ItemState) -> u32 {
        self.entries
            .iter()
            .filter(|stack| stack.item == item)
            .fold(0, |sum: u32, stack| sum.saturating_add(stack.count))
    }

    /// How many of `kind`'s item with its data it has.
    pub fn count_kind(&self, kind: &ItemStack) -> u32 {
        self.entries
            .iter()
            .filter(|stack| stack.same_kind(kind))
            .fold(0, |sum: u32, stack| sum.saturating_add(stack.count))
    }

    /// How many more kilograms its weight limit lets in, with a little
    /// slack so float error doesn't refuse something that fits exactly.
    /// `None` without a limit.
    pub fn spare_weight(&self, content: &Content) -> Option<f32> {
        let max = self.max_weight?;
        let spare = (max - self.weight(content)).max(0.0);
        Some(spare + spare.max(1.0) * 1e-6)
    }

    /// How many more of `kind`'s item with its data its weight limit lets
    /// in: unlimited (`u32::MAX`) without a limit or for weightless items.
    pub fn room_for(&self, content: &Content, kind: &ItemStack) -> u32 {
        let each = kind.weight_each(content);
        match self.spare_weight(content) {
            Some(spare) if each > 0.0 => (spare / each).floor().min(u32::MAX as f32) as u32,
            _ => u32::MAX,
        }
    }

    /// Puts as much of `stack` in as the weight limit allows, counted with
    /// what's there of the same kind (or, for a unique item, an entry per
    /// copy). Returns what didn't fit, if anything.
    pub fn add(&mut self, content: &Content, mut stack: ItemStack) -> Option<ItemStack> {
        let mut allowed = stack.count.min(self.room_for(content, &stack));
        // A count can't go past what a u32 holds; the rest doesn't fit.
        let unique = is_unique(content, stack.item);
        if !unique && let Some(existing) = self.entries.iter().find(|there| there.same_kind(&stack))
        {
            allowed = allowed.min(u32::MAX - existing.count);
        }
        let refused = stack.count - allowed;
        if allowed > 0 {
            if unique {
                let one = ItemStack {
                    count: 1,
                    ..stack.clone()
                };
                self.entries
                    .extend(std::iter::repeat_n(one, allowed as usize));
            } else if let Some(existing) = self
                .entries
                .iter_mut()
                .find(|there| there.same_kind(&stack))
            {
                existing.count += allowed;
            } else {
                self.entries.push(ItemStack {
                    count: allowed,
                    ..stack.clone()
                });
            }
        }
        stack.count = refused;
        (refused > 0).then_some(stack)
    }

    /// Whether all of `stack` would fit.
    pub fn fits(&self, content: &Content, stack: &ItemStack) -> bool {
        stack.count <= self.room_for(content, stack)
    }

    /// Takes `count` of `item` out (of any data, the latest entries first),
    /// or nothing at all if it doesn't have that many. Returns whether it
    /// did.
    pub fn remove(&mut self, item: ItemState, count: u32) -> bool {
        if self.count(item) < count {
            return false;
        }
        let mut left = count;
        for stack in self.entries.iter_mut().rev() {
            if left == 0 {
                break;
            }
            if stack.item == item {
                let taken = stack.count.min(left);
                stack.count -= taken;
                left -= taken;
            }
        }
        self.entries.retain(|stack| stack.count > 0);
        true
    }

    /// Changes the data of one copy like `kind` to `data` (what a fluid
    /// container holds, say), whatever that does to the weight: the caller
    /// keeps within the limit (see [`Inventory::spare_weight`]). Refusing
    /// would lose the copy. Returns whether it had one to change.
    pub fn replace_data(&mut self, content: &Content, kind: &ItemStack, data: ItemData) -> bool {
        if !self.remove_kind(kind, 1) {
            return false;
        }
        let limit = self.max_weight.take();
        let changed = ItemStack {
            item: kind.item,
            data,
            count: 1,
        };
        let refused = self.add(content, changed);
        self.max_weight = limit;
        debug_assert!(refused.is_none(), "with no limit, one always fits");
        true
    }

    /// Takes `count` of `kind`'s item with its data out, the latest
    /// entries first, or nothing at all if it doesn't have that many.
    /// Returns whether it did.
    pub fn remove_kind(&mut self, kind: &ItemStack, count: u32) -> bool {
        if self.count_kind(kind) < count {
            return false;
        }
        let mut left = count;
        for stack in self.entries.iter_mut().rev() {
            if left == 0 {
                break;
            }
            if stack.same_kind(kind) {
                let taken = stack.count.min(left);
                stack.count -= taken;
                left -= taken;
            }
        }
        self.entries.retain(|stack| stack.count > 0);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::base;

    fn item(id: &str) -> ItemState {
        base().default_item(id).unwrap()
    }

    #[test]
    fn counts_of_one_kind_share_an_entry_without_limit() {
        let content = base();
        let (stone, dirt) = (item("defaria:stone"), crate::content::dirt_item());
        let mut inventory = Inventory::new();
        assert_eq!(inventory.add(content, ItemStack::new(stone, 10)), None);
        assert_eq!(inventory.add(content, ItemStack::new(dirt, 3)), None);
        assert_eq!(inventory.add(content, ItemStack::new(stone, 100_000)), None);
        assert_eq!(inventory.entries().len(), 2);
        assert_eq!(inventory.count(stone), 100_010);
        assert_eq!(inventory.entries()[1], ItemStack::new(dirt, 3));
    }

    #[test]
    fn unique_items_are_an_entry_each() {
        let content = base();
        let pickaxe = item("defaria:pickaxe");
        assert!(is_unique(content, pickaxe));
        let mut inventory = Inventory::new();
        inventory.add(content, ItemStack::new(pickaxe, 3));
        assert_eq!(inventory.entries().len(), 3);
        assert!(inventory.entries().iter().all(|stack| stack.count == 1));
        assert_eq!(inventory.count(pickaxe), 3);
        assert!(inventory.remove(pickaxe, 2));
        assert_eq!(inventory.entries().len(), 1);
    }

    #[test]
    fn nothing_goes_over_the_weight_limit() {
        let content = base();
        let stone = item("defaria:stone");
        let each = item_weight(content, stone);
        assert!(each > 0.0);
        let mut inventory = Inventory::new().with_max_weight(each * 5.0);
        // Five fit exactly; the other three come back.
        assert_eq!(
            inventory.add(content, ItemStack::new(stone, 8)),
            Some(ItemStack::new(stone, 3))
        );
        assert_eq!(inventory.count(stone), 5);
        assert!((inventory.weight(content) - each * 5.0).abs() < 1e-3);
        assert_eq!(inventory.room_for(content, &ItemStack::one(stone)), 0);
        assert!(!inventory.fits(content, &ItemStack::one(stone)));
        // Taking some out makes room again.
        assert!(inventory.remove(stone, 2));
        assert_eq!(inventory.room_for(content, &ItemStack::one(stone)), 2);
    }

    #[test]
    fn removing_needs_enough() {
        let content = base();
        let stone = item("defaria:stone");
        let mut inventory = Inventory::new();
        inventory.add(content, ItemStack::new(stone, 5));
        assert!(!inventory.remove(stone, 8));
        assert_eq!(inventory.count(stone), 5);
        assert!(inventory.remove(stone, 5));
        assert!(inventory.is_empty());
    }

    #[test]
    fn removing_a_kind() {
        let content = base();
        let stone = item("defaria:stone");
        let mut inventory = Inventory::new();
        inventory.add(content, ItemStack::new(stone, 3));
        let kind = ItemStack::one(stone);
        assert_eq!(inventory.count_kind(&kind), 3);
        assert!(!inventory.remove_kind(&kind, 4));
        assert!(inventory.remove_kind(&kind, 2));
        assert_eq!(inventory.count_kind(&kind), 1);
        assert!(inventory.remove_kind(&kind, 1));
        assert!(inventory.is_empty());
    }

    /// A count stops at what a u32 holds rather than overflowing.
    #[test]
    fn counts_stop_at_the_largest_number() {
        let content = base();
        let stone = item("defaria:stone");
        let mut inventory = Inventory::new();
        assert_eq!(
            inventory.add(content, ItemStack::new(stone, u32::MAX - 5)),
            None
        );
        assert_eq!(
            inventory.add(content, ItemStack::new(stone, 10)),
            Some(ItemStack::new(stone, 5))
        );
        assert_eq!(inventory.count(stone), u32::MAX);
    }
}
