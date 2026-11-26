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
//! C switches it while playing. New players start in creative.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    content::{BlockState, Content, ItemState},
    inventory::{Inventory, ItemStack, player::PLAYER_MAX_WEIGHT},
    player::LocalPlayer,
    ui::{playing, typing},
};

pub struct GameModePlugin;

impl Plugin for GameModePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<GameMode>().add_systems(
            Update,
            (
                switch_mode.run_if(playing).run_if(not(typing)),
                limit_weight,
            )
                .chain(),
        );
    }
}

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

/// C switches between creative and survival.
fn switch_mode(
    keys: Res<ButtonInput<KeyCode>>,
    player: Option<Single<&mut GameMode, With<LocalPlayer>>>,
) {
    if keys.just_pressed(KeyCode::KeyC)
        && let Some(mut mode) = player
    {
        **mode = match **mode {
            GameMode::Creative => GameMode::Survival,
            GameMode::Survival => GameMode::Creative,
        };
        info!("game mode: {:?}", **mode);
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

    fn voxel(app: &mut App, pos: IVec3) -> BlockState {
        app.world_mut()
            .run_system_cached_with(
                |pos: In<IVec3>, voxels: crate::world::Voxels| voxels.get(*pos),
                pos,
            )
            .unwrap()
            .unwrap()
    }

    fn player(app: &mut App) -> (Inventory, GameMode) {
        let mut players = app
            .world_mut()
            .query_filtered::<(&Inventory, &GameMode), With<LocalPlayer>>();
        let (inventory, mode) = players.single(app.world()).unwrap();
        (inventory.clone(), *mode)
    }

    /// Through the real game: in survival, breaking a stone puts one in the
    /// inventory and placing it takes it out again; in creative, neither
    /// changes the inventory. C switches between them.
    #[test]
    fn breaking_and_placing_round_trip_in_survival() {
        use crate::headless::{Script, run_more, settle, test_game};
        // A stone in the air, the camera two blocks over it looking down.
        let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
        let mut app = test_game(script);
        settle(&mut app);
        let at = IVec3::new(40, 120, 40);
        let stone = base().default_item("defaria:stone").unwrap();
        run_more(&mut app, "fill 40 120 40 40 120 40 stone\nwait 2");
        run_more(&mut app, "hold MouseLeft 1  # lock the cursor\nwait 1");
        assert_eq!(player(&mut app).1, GameMode::Creative);
        assert_eq!(player(&mut app).0.max_weight, None);
        let before = player(&mut app).0.count(stone);

        // Creative: it breaks, and nothing comes of it.
        run_more(&mut app, "hold MouseLeft 90\nwait 2");
        assert!(voxel(&mut app, at).is_air());
        assert_eq!(player(&mut app).0.count(stone), before);
        // Placing (slot 1 is stone) doesn't use any up.
        run_more(
            &mut app,
            "fill 40 119 40 40 119 40 soil\nwait 2\nhold MouseRight 1\nwait 2",
        );
        assert_eq!(voxel(&mut app, at), block("stone"));
        assert_eq!(player(&mut app).0.count(stone), before);

        run_more(&mut app, "hold KeyC 1\nwait 2");
        assert_eq!(player(&mut app).1, GameMode::Survival);
        assert_eq!(
            player(&mut app).0.max_weight,
            Some(crate::inventory::player::PLAYER_MAX_WEIGHT)
        );
        run_more(&mut app, "hold MouseLeft 90\nwait 2");
        assert!(voxel(&mut app, at).is_air());
        assert_eq!(player(&mut app).0.count(stone), before + 1);
        run_more(&mut app, "hold MouseRight 1\nwait 2");
        assert_eq!(voxel(&mut app, at), block("stone"));
        assert_eq!(player(&mut app).0.count(stone), before);
    }

    /// In survival a block whose item is too heavy to carry stays where it
    /// is.
    #[test]
    fn a_full_inventory_leaves_the_block() {
        use crate::headless::{Script, run_more, settle, test_game};
        let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
        let mut app = test_game(script);
        settle(&mut app);
        run_more(&mut app, "fill 40 120 40 40 120 40 glass\nwait 2");
        run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold KeyC 1\nwait 2");
        {
            // Carrying as much as they can.
            let mut players = app
                .world_mut()
                .query_filtered::<&mut Inventory, With<LocalPlayer>>();
            let mut inventory = players.single_mut(app.world_mut()).unwrap();
            inventory.max_weight = Some(inventory.weight(base()));
        }
        run_more(&mut app, "hold MouseLeft 90\nwait 2");
        assert_eq!(
            voxel(&mut app, IVec3::new(40, 120, 40)),
            base().default_state("defaria:glass").unwrap()
        );
    }

    /// In survival, a hotbar slot whose item ran out places nothing, and
    /// shows dimmed; in creative it places all the same, undimmed.
    #[test]
    fn survival_needs_the_item_to_place() {
        use crate::{
            headless::{Script, run_more, settle, test_game},
            hotbar::HotbarSlot,
            ui::ItemSlot,
        };
        let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
        let mut app = test_game(script);
        settle(&mut app);
        let at = IVec3::new(40, 120, 40);
        let stone = base().default_item("defaria:stone").unwrap();
        run_more(&mut app, "fill 40 119 40 40 119 40 soil\nwait 2");
        run_more(&mut app, "hold MouseLeft 1\nwait 1");
        {
            let mut players = app
                .world_mut()
                .query_filtered::<&mut Inventory, With<LocalPlayer>>();
            let mut inventory = players.single_mut(app.world_mut()).unwrap();
            let all = inventory.count(stone);
            inventory.remove(stone, all);
        }
        let first_slot = |app: &mut App| {
            let mut slots = app.world_mut().query::<(&HotbarSlot, &ItemSlot)>();
            *slots
                .iter(app.world())
                .find(|(slot, _)| slot.0 == 0)
                .unwrap()
                .1
        };
        run_more(&mut app, "wait 2");
        assert!(!first_slot(&mut app).dimmed);
        run_more(&mut app, "hold KeyC 1\nwait 2");
        assert!(first_slot(&mut app).dimmed);
        run_more(&mut app, "hold MouseRight 1\nwait 2");
        assert!(voxel(&mut app, at).is_air());
        run_more(&mut app, "hold KeyC 1\nwait 2\nhold MouseRight 1\nwait 2");
        assert_eq!(voxel(&mut app, at), block("stone"));
    }
}
