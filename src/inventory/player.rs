//! The local player's inventory and hotbar.
//!
//! The inventory is limited only by weight, [`PLAYER_MAX_WEIGHT`] (in
//! survival; see `game_mode`). A new player starts in creative, with [`DEFAULT_HOTBAR`] in their hotbar and
//! [`STARTING_COUNT`] of each of those items. In a world on disk, the
//! inventory, the hotbar, the active slot and the game mode are read from `player.json5`
//! when the player appears, and written back every
//! [`StreamingSettings::autosave_seconds`] and when the game exits.

use bevy::prelude::*;

use super::{Inventory, ItemStack, model::is_unique};
use crate::{
    content::Content,
    game_mode::GameMode,
    hotbar::{ActiveSlot, HOTBAR_SLOTS, Hotbar},
    player::LocalPlayer,
    save::{
        WorldFolder,
        player::{self as file, ItemNames, PLAYER_FORMAT, PlayerFile, SavedStack},
    },
    streaming::{SaveOnExit, StreamingSettings},
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

pub struct PlayerInventoryPlugin;

impl Plugin for PlayerInventoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(equip_new_player)
            .add_systems(SaveOnExit, save_player);
    }
}

/// What a new player starts with: [`DEFAULT_HOTBAR`], and
/// [`STARTING_COUNT`] of each of its items. An item that didn't load is
/// reported and leaves its slot empty.
pub fn new_player(content: &Content) -> (Inventory, Hotbar) {
    let mut inventory = Inventory::new().with_max_weight(PLAYER_MAX_WEIGHT);
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

/// Gives the local player their inventory, hotbar and active slot as they
/// appear: the saved ones in a world that has them, or a new player's.
fn equip_new_player(
    add: On<Add<LocalPlayer>>,
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
                inventory.add(content, ItemStack::new(item, count));
            }
            None => warn!(
                "the saved inventory has `{}`, which no mod defines",
                stack.item
            ),
        }
    }
    inventory.max_weight = Some(PLAYER_MAX_WEIGHT);
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
    player: Option<Single<(&Inventory, &Hotbar, &ActiveSlot, &GameMode), With<LocalPlayer>>>,
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
    use crate::{
        content::base,
        headless::{HeadlessPlugin, Script, default_plugins},
        save::tests::temp_folder,
    };

    #[test]
    fn a_new_player_starts_with_the_default_hotbar_and_some_of_each() {
        let content = base();
        let (inventory, hotbar) = new_player(content);
        assert_eq!(inventory.max_weight, Some(PLAYER_MAX_WEIGHT));
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
                },
                SavedStack {
                    item: "defaria:lamp".into(),
                    count: 99_999,
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
            }],
            ..save(
                &Inventory::new(),
                &Hotbar::default(),
                ActiveSlot(0),
                content,
            )
        };
        let (inventory, ..) = load(&saved, content);
        assert_eq!(inventory.entries().len(), 1);
    }

    /// Through the real game, in a world on disk: the active slot is saved
    /// when the game exits, and the next run starts with it.
    #[test]
    fn the_player_is_saved_with_the_world() {
        let folder = temp_folder("player-inventory");
        let run = |script: &str| {
            let mut app = App::new();
            app.insert_resource(WorldFolder(folder.clone()))
                .add_plugins((
                    default_plugins(false).disable::<bevy::log::LogPlugin>(),
                    crate::GamePlugin,
                    HeadlessPlugin {
                        script: Script::parse(script).unwrap(),
                        rendering: false,
                    },
                ));
            app.insert_resource(StreamingSettings {
                radius: 2,
                ..default()
            });
            app.finish();
            app.cleanup();
            for _ in 0..crate::headless::SETTLE_TIMEOUT_FRAMES {
                app.update();
                if app.should_exit().is_some() {
                    break;
                }
            }
            let mut players = app
                .world_mut()
                .query_filtered::<(&Inventory, &Hotbar, &ActiveSlot), With<LocalPlayer>>();
            let (inventory, hotbar, active) = players.single(app.world()).unwrap();
            (inventory.clone(), hotbar.clone(), *active)
        };
        let first = run("hold Digit3 1\nwait 1");
        assert_eq!(first.2, ActiveSlot(2));
        let second = run("wait 1");
        assert_eq!(second, first);
        assert!(folder.join(file::PLAYER_FILE).exists());
    }

    /// A `player.json5` the game can't read (here, from a newer build) is
    /// left as it is: the player starts new, and nothing saves over it.
    #[test]
    fn an_unreadable_player_file_is_kept() {
        let folder = temp_folder("player-newer-kept");
        let path = folder.join(file::PLAYER_FILE);
        let newer = "{ format: 99, active_slot: 0, hotbar: [], inventory: [] }";
        std::fs::write(&path, newer).unwrap();
        let mut app = App::new();
        app.insert_resource(WorldFolder(folder.clone()))
            .add_plugins((
                default_plugins(false).disable::<bevy::log::LogPlugin>(),
                crate::GamePlugin,
                HeadlessPlugin {
                    script: Script::parse("wait 1").unwrap(),
                    rendering: false,
                },
            ));
        app.insert_resource(StreamingSettings {
            radius: 2,
            autosave_seconds: 0.0,
            ..default()
        });
        app.finish();
        app.cleanup();
        for _ in 0..crate::headless::SETTLE_TIMEOUT_FRAMES {
            app.update();
            if app.should_exit().is_some() {
                break;
            }
        }
        assert!(app.world().contains_resource::<KeepPlayerFile>());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    }
}
