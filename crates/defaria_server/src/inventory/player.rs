//! The player's inventory and hotbar. (One player's, saved in the
//! world's `player.json5`, until players have ids: #216.)
//!
//! A new player starts in creative, with what
//! `defaria_core::inventory::player` says. In a world on disk, the
//! inventory, the hotbar, the active slot and the game mode are read from
//! `player.json5` when the player appears, and written back every
//! [`StreamingSettings::autosave_seconds`] and when the game exits.

use bevy::prelude::*;

use defaria_core::inventory::{
    Inventory, ItemStack,
    model::{ItemData, is_unique},
    player::new_player,
};

use crate::{
    save::player::{self as file, ItemNames, PLAYER_FORMAT, PlayerFile, SavedFluid, SavedStack},
    streaming::SaveOnExit,
};
use defaria_core::{
    content::Content,
    fluid_container::saved_data,
    game_mode::GameMode,
    hotbar::{ActiveSlot, HOTBAR_SLOTS, Hotbar},
    player::Player,
    save::WorldFolder,
    streaming::StreamingSettings,
};

pub struct PlayerInventoryPlugin;

impl Plugin for PlayerInventoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(equip_new_player)
            .add_systems(SaveOnExit, save_player);
    }
}

/// Gives the local player their inventory, hotbar and active slot as they
/// appear: the saved ones in a world that has them, or a new player's.
fn equip_new_player(
    add: On<Add<Player>>,
    content: Res<Content>,
    folder: Option<Res<WorldFolder>>,
    mut commands: Commands,
) {
    let saved = folder.and_then(|folder| match file::read(&folder.0) {
        Ok(saved) => saved,
        Err(error) => {
            // Its file is left as it is: it may be from a newer build, or
            // just unreadable for now, and still hold the player's things.
            error!("{error}; starting as a new player, and not saving over it");
            commands.insert_resource(KeepPlayerFile);
            None
        }
    });
    let player = match saved {
        Some(saved) => {
            let (inventory, hotbar, active) = load(&saved, &content);
            (inventory, hotbar, active, saved.game_mode)
        }
        None => {
            let (inventory, hotbar) = new_player(&content);
            (
                inventory,
                hotbar,
                ActiveSlot::default(),
                GameMode::default(),
            )
        }
    };
    commands.entity(add.entity).insert_if_new(player);
}

/// What `saved` holds. Items that no longer exist are left out with a
/// warning.
pub fn load(saved: &PlayerFile, content: &Content) -> (Inventory, Hotbar, ActiveSlot) {
    let names = ItemNames::new(content);
    let mut inventory = Inventory::new();
    for stack in &saved.inventory {
        match names.item(&stack.item) {
            // Saved, so it fitted: the limit only applies from here on.
            Some(item) => {
                // Each copy of a unique item is saved on its own.
                let count = if is_unique(content, item) && stack.count > 1 {
                    warn!(
                        "the saved inventory has {} of unique `{}` in one entry; keeping one",
                        stack.count, stack.item
                    );
                    1
                } else {
                    stack.count
                };
                let data = match &stack.fluid {
                    Some(saved) => saved_data(content, item, &saved.fluid, saved.amount)
                        .unwrap_or_else(|error| {
                            warn!(
                                "the saved inventory's `{}` held fluid, but {error}; it's lost",
                                stack.item
                            );
                            ItemData::default()
                        }),
                    None => ItemData::default(),
                };
                inventory.add(content, ItemStack { item, data, count });
            }
            None => warn!(
                "the saved inventory has `{}`, which no mod defines",
                stack.item
            ),
        }
    }
    let mut hotbar = Hotbar::default();
    for (slot, name) in saved.hotbar.iter().take(HOTBAR_SLOTS).enumerate() {
        hotbar.0[slot] = name.as_deref().and_then(|name| names.item(name));
    }
    let active = ActiveSlot(saved.active_slot.min(HOTBAR_SLOTS - 1));
    (inventory, hotbar, active)
}

/// What `player.json5` saves for this inventory, hotbar and active slot.
pub fn save(
    inventory: &Inventory,
    hotbar: &Hotbar,
    active: ActiveSlot,
    mode: GameMode,
    content: &Content,
) -> PlayerFile {
    let names = ItemNames::new(content);
    let stacks = inventory.entries().iter().filter_map(|stack| {
        Some(SavedStack {
            item: names.name(stack.item)?.to_owned(),
            count: stack.count,
            fluid: stack.data.fluid.as_ref().map(|held| SavedFluid {
                fluid: held.fluid.clone(),
                amount: held.amount,
            }),
        })
    });
    let hotbar = hotbar
        .0
        .iter()
        .map(|item| item.and_then(|item| names.name(item)).map(str::to_owned));
    PlayerFile {
        format: PLAYER_FORMAT,
        game_mode: mode,
        active_slot: active.0,
        hotbar: hotbar.collect(),
        inventory: stacks.collect(),
    }
}

/// The world's `player.json5` couldn't be read, so it isn't written over.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct KeepPlayerFile;

/// Writes the player's file every autosave, and when the game exits. Runs
/// after `Last`, so it sees an exit sent anywhere in the frame.
fn save_player(
    time: Res<Time>,
    mut exits: MessageReader<AppExit>,
    settings: Option<Res<StreamingSettings>>,
    folder: Option<Res<WorldFolder>>,
    keep: Option<Res<KeepPlayerFile>>,
    content: Res<Content>,
    player: Option<Single<(&Inventory, &Hotbar, &ActiveSlot, &GameMode), With<Player>>>,
    mut since_save: Local<f32>,
) {
    if keep.is_some() {
        return;
    }
    let exiting = exits.read().count() > 0;
    *since_save += time.delta_secs();
    let due = settings.is_some_and(|settings| *since_save >= settings.autosave_seconds);
    let (Some(folder), Some(player)) = (folder, player) else {
        return;
    };
    if !exiting && !due {
        return;
    }
    *since_save = 0.0;
    let (inventory, hotbar, active, mode) = *player;
    let file = save(inventory, hotbar, *active, *mode, &content);
    if let Err(error) = file::write(&folder.0, &file) {
        error!("{error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defaria_core::content::base;

    #[test]
    fn saving_and_loading_round_trips() {
        let content = base();
        let (mut inventory, mut hotbar) = new_player(content);
        let pickaxe = content.default_item("defaria:pickaxe").unwrap();
        inventory.add(content, ItemStack::new(pickaxe, 2));
        hotbar.assign(0, None);
        hotbar.assign(3, Some(pickaxe));
        let saved = save(
            &inventory,
            &hotbar,
            ActiveSlot(4),
            GameMode::Survival,
            content,
        );
        assert_eq!(saved.game_mode, GameMode::Survival);
        assert_eq!(load(&saved, content), (inventory, hotbar, ActiveSlot(4)));
    }

    #[test]
    fn loading_leaves_out_what_no_longer_exists() {
        let content = base();
        let saved = PlayerFile {
            format: PLAYER_FORMAT,
            game_mode: GameMode::Creative,
            active_slot: 40,
            hotbar: vec![Some("gone:thing".into()), Some("defaria:lamp".into())],
            inventory: vec![
                SavedStack {
                    item: "gone:thing".into(),
                    count: 3,
                    fluid: None,
                },
                SavedStack {
                    item: "defaria:lamp".into(),
                    count: 99_999,
                    fluid: None,
                },
            ],
        };
        let (inventory, hotbar, active) = load(&saved, content);
        assert_eq!(active, ActiveSlot(HOTBAR_SLOTS - 1));
        let lamp = content.default_item("defaria:lamp").unwrap();
        assert_eq!(inventory.count(lamp), 99_999);
        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(hotbar.0[..2], [None, Some(lamp)]);
    }

    /// A unique item saved as many in one entry (a hand-edited file) loads
    /// as one, rather than as an entry per count.
    #[test]
    fn a_unique_item_loads_one_per_entry() {
        let content = base();
        let saved = PlayerFile {
            inventory: vec![SavedStack {
                item: ItemNames::new(content)
                    .name(content.default_item("defaria:pickaxe").unwrap())
                    .unwrap()
                    .to_owned(),
                count: 5,
                fluid: None,
            }],
            ..save(
                &Inventory::new(),
                &Hotbar::default(),
                ActiveSlot(0),
                GameMode::Creative,
                content,
            )
        };
        let (inventory, ..) = load(&saved, content);
        assert_eq!(inventory.entries().len(), 1);
    }
}
