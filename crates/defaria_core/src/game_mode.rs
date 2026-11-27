//! Creative and survival: whether items are used up.
//!
//! - **Survival**: breaking a block puts its item in the breaker's
//!   inventory (and a block whose item is too heavy to carry won't break),
//!   placing one uses it up (and needs one to place), and what's carried is
//!   capped by weight. A fluid's block (water, lava) isn't placed at all:
//!   that would make fluid from nothing, so fluid comes only from
//!   containers (a bucket, see `fluid_container`) and the terrain.
//! - **Creative**: breaking gives nothing, placing uses nothing up (the
//!   hotbar places its items whether you have any or not), there's no
//!   weight limit, and the inventory screen has the catalog of every item.
//!
//! The mode is a component of the player ([`GameMode`]), saved with them;
//! anything without one plays as in creative, the default. The rules for
//! using items up are [`harvest`] and [`use_up`].
//! C switches it while playing (`input::game_mode`). New players start in
//! creative.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    content::{BlockState, Content, ItemState},
    inventory::{Inventory, ItemStack, player::PLAYER_MAX_WEIGHT},
};

pub struct GameModePlugin;

impl Plugin for GameModePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<GameMode>()
            .add_systems(Update, limit_weight.in_set(GameModeSystems));
    }
}

/// Where a player's game mode takes effect; what switches it runs before.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct GameModeSystems;

/// Whether items are used up (survival) or not (creative).
#[derive(
    Component, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[reflect(Component)]
#[serde(rename_all = "lowercase")]
pub enum GameMode {
    #[default]
    Creative,
    Survival,
}

impl GameMode {
    pub fn is_creative(self) -> bool {
        self == Self::Creative
    }
}

/// The player's weight limit: none in creative.
fn limit_weight(mut players: Query<(&GameMode, &mut Inventory), Changed<GameMode>>) {
    for (mode, mut inventory) in &mut players {
        let limit = (!mode.is_creative()).then_some(PLAYER_MAX_WEIGHT);
        if inventory.max_weight != limit {
            inventory.max_weight = limit;
        }
    }
}

/// The item a block state is broken into: an item that places the same
/// block, with as many of the same property values as can be (a birch log
/// lying on its side gives a birch log, whatever way it's placed).
pub fn item_for_block(content: &Content, state: BlockState) -> Option<ItemState> {
    let broken = content.state(state);
    if state.is_air() || broken.is_fluid() {
        return None;
    }
    content
        .item_states
        .iter()
        .enumerate()
        .filter_map(|(index, info)| {
            let places = content.state(info.block?);
            (places.block == broken.block).then(|| {
                let same = places
                    .values
                    .iter()
                    .filter(|value| broken.values.contains(value))
                    .count();
                (same, std::cmp::Reverse(index))
            })
        })
        .max()
        .map(|(_, std::cmp::Reverse(index))| ItemState(index as u32))
}

/// Whether someone in `mode` can place `item`, using one up from
/// `inventory` if so: in creative always, using nothing; in survival only
/// with one to use. No inventory has nothing to use.
pub fn use_up(mode: GameMode, inventory: Option<&mut Inventory>, item: ItemState) -> bool {
    if mode.is_creative() {
        return true;
    }
    inventory.is_some_and(|inventory| inventory.remove(item, 1))
}

/// What breaking `state` gives someone in `mode`, if it can be broken: in
/// survival its item goes into `inventory`, and a block whose item is too
/// heavy to carry isn't broken (`false`). Blocks with no item always break.
pub fn harvest(
    mode: GameMode,
    inventory: &mut Inventory,
    content: &Content,
    state: BlockState,
) -> bool {
    if mode.is_creative() {
        return true;
    }
    let Some(item) = item_for_block(content, state) else {
        return true;
    };
    let one = ItemStack::one(item);
    if !inventory.fits(content, &one) {
        return false;
    }
    inventory.add(content, one);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{base, block};

    #[test]
    fn blocks_break_into_their_items() {
        let content = base();
        let stone = block("stone");
        assert_eq!(
            item_for_block(content, stone),
            content.default_item("defaria:stone")
        );
        // A log on its side gives the same log as one standing up.
        let log = content.block("defaria:log").unwrap();
        let birch = log
            .state(&[("axis", "x"), ("wood", "defaria:birch")])
            .unwrap();
        let item = item_for_block(content, birch).unwrap();
        let places = content.item_state(item).unwrap().block.unwrap();
        assert_eq!(
            content
                .state(places)
                .values
                .iter()
                .find(|(k, _)| k == "wood"),
            Some(&("wood".to_owned(), "defaria:birch".to_owned()))
        );
        assert_eq!(item_for_block(content, BlockState::AIR), None);
        assert_eq!(
            item_for_block(content, content.default_state("defaria:water").unwrap()),
            None
        );
    }

    #[test]
    fn survival_uses_items_up_and_creative_doesnt() {
        let content = base();
        let stone = content.default_item("defaria:stone").unwrap();
        let mut inventory = Inventory::new();
        inventory.add(content, ItemStack::one(stone));
        assert!(use_up(GameMode::Creative, Some(&mut inventory), stone));
        assert!(use_up(GameMode::Creative, None, stone));
        assert_eq!(inventory.count(stone), 1);
        assert!(use_up(GameMode::Survival, Some(&mut inventory), stone));
        assert!(!use_up(GameMode::Survival, Some(&mut inventory), stone));
        assert!(!use_up(GameMode::Survival, None, stone));
        assert!(inventory.is_empty());
    }

    #[test]
    fn survival_harvests_what_fits() {
        let content = base();
        let stone = block("stone");
        let item = item_for_block(content, stone).unwrap();
        let each = crate::inventory::model::item_weight(content, item);
        let mut inventory = Inventory::new().with_max_weight(each * 1.5);
        assert!(harvest(GameMode::Creative, &mut inventory, content, stone));
        assert!(inventory.is_empty());
        assert!(harvest(GameMode::Survival, &mut inventory, content, stone));
        assert_eq!(inventory.count(item), 1);
        // Too heavy for a second: the block stays.
        assert!(!harvest(GameMode::Survival, &mut inventory, content, stone));
        assert_eq!(inventory.count(item), 1);
    }
}
