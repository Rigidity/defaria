//! Players' saved state: what they carry, their hotbar and the slot in
//! hand, their game mode and where they were, kept by their id
//! (`PlayerId`) in the world's `players/<id>.json5`.
//!
//! A player who joins gets what they had when they were last saved: from
//! this run, if they left since the server started (kept in memory, so a
//! world kept in memory keeps its players too), or from their file. A
//! player new to the world starts in creative, with what
//! `defaria_core::inventory::player` says, where new players start.
//! Every player is saved when they leave, every
//! [`StreamingSettings::autosave_seconds`] and when the server stops. A
//! player without an id (a test's) starts new and isn't saved.

use bevy::{platform::collections::HashMap, prelude::*};

use defaria_core::inventory::{
    Inventory, ItemStack,
    model::{ItemData, is_unique},
    player::new_player,
};

use crate::{
    save::player::{
        self as file, ItemNames, PLAYER_FORMAT, PlayerFile, SavedFluid, SavedPosition, SavedStack,
    },
    streaming::SaveOnExit,
};
use defaria_core::{
    content::Content,
    fluid_container::saved_data,
    game_mode::GameMode,
    hotbar::{ActiveSlot, HOTBAR_SLOTS, Hotbar},
    identity::PlayerId,
    physics::Body,
    player::Player,
    save::WorldFolder,
    streaming::StreamingSettings,
};

pub struct PlayerInventoryPlugin;

impl Plugin for PlayerInventoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LeftPlayers>()
            .add_observer(equip_new_player)
            .add_observer(save_leaving_player)
            .add_systems(SaveOnExit, save_players);
    }
}

/// The players who left since the server started, as they were saved:
/// who they are when they come back, before their files (which a world in
/// memory hasn't got).
#[derive(Resource, Debug, Default)]
pub struct LeftPlayers(pub HashMap<PlayerId, PlayerFile>);

/// A player whose file couldn't be read: they play as a new player, and
/// their file isn't saved over (it may be from a newer build, or only
/// unreadable for now, and still hold their things).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct KeepSavedPlayer;

/// Gives each player what they had as they appear: what they had when
/// they were last saved, or a new player's.
fn equip_new_player(
    add: On<Add<Player>>,
    ids: Query<(&PlayerId, Option<&Name>)>,
    content: Res<Content>,
    folder: Option<Res<WorldFolder>>,
    left: Res<LeftPlayers>,
    mut commands: Commands,
) {
    let entity = add.entity;
    let saved = match ids.get(entity) {
        Ok((id, name)) => match left.0.get(id) {
            Some(saved) => Some(saved.clone()),
            None => folder.and_then(|folder| match file::read(&folder.0, *id) {
                Ok(saved) => saved,
                Err(error) => {
                    let name = name.map_or("a player", Name::as_str);
                    error!("{error}; {name} starts as a new player, and isn't saved over it");
                    commands.entity(entity).insert(KeepSavedPlayer);
                    None
                }
            }),
        },
        Err(_) => None,
    };
    let Some(saved) = saved else {
        let (inventory, hotbar) = new_player(&content);
        commands.entity(entity).insert_if_new((
            inventory,
            hotbar,
            ActiveSlot::default(),
            GameMode::default(),
        ));
        return;
    };
    let (inventory, hotbar, active) = load(&saved, &content);
    commands
        .entity(entity)
        .insert_if_new((inventory, hotbar, active, saved.game_mode));
    if let Some(position) = saved.position {
        commands
            .entity(entity)
            .queue(move |mut entity: EntityWorldMut| {
                put_at(&mut entity, position);
            });
    }
}

/// Puts a player where `position` says, at once.
fn put_at(entity: &mut EntityWorldMut, position: SavedPosition) {
    let feet = bevy::math::DVec3::from_array(position.feet);
    if !feet.is_finite() {
        warn!("a saved player was at {feet}; they start where new players do");
        return;
    }
    if let Some(mut body) = entity.get_mut::<Body>() {
        body.teleport(feet);
        body.velocity = default();
    }
    if let Some(mut transform) = entity.get_mut::<Transform>() {
        transform.translation = feet.as_vec3();
    }
    if let Some(mut player) = entity.get_mut::<Player>() {
        player.flying = position.flying;
    }
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

/// What a player's file saves for this inventory, hotbar, active slot and
/// game mode (with no name or position: [`saved_player`] adds them).
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
        name: String::new(),
        position: None,
        game_mode: mode,
        active_slot: active.0,
        hotbar: hotbar.collect(),
        inventory: stacks.collect(),
    }
}

/// What's saved of a player.
type Saved<'a> = (
    &'a PlayerId,
    Option<&'a Name>,
    &'a Inventory,
    &'a Hotbar,
    &'a ActiveSlot,
    &'a GameMode,
    Option<(&'a Body, &'a Player)>,
);

/// A player's file, as they are now.
fn saved_player(
    (_, name, inventory, hotbar, active, mode, body): Saved,
    content: &Content,
) -> PlayerFile {
    PlayerFile {
        name: name
            .map(|name| name.as_str().to_owned())
            .unwrap_or_default(),
        position: body.map(|(body, player)| SavedPosition {
            feet: body.position.to_array(),
            flying: player.flying,
        }),
        ..save(inventory, hotbar, *active, *mode, content)
    }
}

/// Keeps player `id`'s file: in memory, for when they come back this run,
/// and in the world's folder, if it has one.
fn keep(left: &mut LeftPlayers, folder: Option<&WorldFolder>, id: PlayerId, saved: PlayerFile) {
    if let Some(folder) = folder
        && let Err(error) = file::write(&folder.0, id, &saved)
    {
        error!("{error}");
    }
    left.0.insert(id, saved);
}

/// A player who leaves is saved as they go.
fn save_leaving_player(
    remove: On<Remove<PlayerId>>,
    players: Query<Saved, Without<KeepSavedPlayer>>,
    content: Res<Content>,
    folder: Option<Res<WorldFolder>>,
    mut left: ResMut<LeftPlayers>,
) {
    let Ok(player) = players.get(remove.entity) else {
        return;
    };
    let id = *player.0;
    let saved = saved_player(player, &content);
    keep(&mut left, folder.as_deref(), id, saved);
}

/// Saves every player every autosave, and when the server stops. Runs
/// after `Last`, so it sees an exit sent anywhere in the frame.
fn save_players(
    time: Res<Time>,
    mut exits: MessageReader<AppExit>,
    settings: Option<Res<StreamingSettings>>,
    folder: Option<Res<WorldFolder>>,
    content: Res<Content>,
    players: Query<Saved, Without<KeepSavedPlayer>>,
    mut since_save: Local<f32>,
    save_now: Option<ResMut<crate::streaming::SaveNow>>,
) {
    let exiting = exits.read().count() > 0;
    *since_save += time.delta_secs();
    let due = settings.is_some_and(|settings| *since_save >= settings.autosave_seconds);
    let asked = save_now.is_some_and(|mut save_now| std::mem::take(&mut save_now.players));
    if !exiting && !due && !asked {
        return;
    }
    *since_save = 0.0;
    // Only to disk: those playing aren't looked up in memory, and are kept
    // there as they leave.
    let Some(folder) = folder else {
        return;
    };
    for player in &players {
        let id = *player.0;
        let saved = saved_player(player, &content);
        if let Err(error) = file::write(&folder.0, id, &saved) {
            error!("{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use defaria_core::{
        content::base,
        player::{Look, player},
        testing::temp_folder,
    };

    /// An app with players' saves and nothing else, in the world in
    /// `folder` (in memory without).
    fn app(folder: Option<std::path::PathBuf>) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins(PlayerInventoryPlugin);
        if let Some(folder) = folder {
            app.insert_resource(WorldFolder(folder));
        }
        app
    }

    /// A player joining, as the server spawns them: their id (if any) with
    /// the rest.
    fn join(app: &mut App, id: Option<PlayerId>) -> Entity {
        let body = player(DVec3::new(1.0, 2.0, 3.0), Look::default());
        let entity = match id {
            Some(id) => app.world_mut().spawn((id, body)).id(),
            None => app.world_mut().spawn(body).id(),
        };
        app.world_mut().flush();
        entity
    }

    /// A player who leaves is kept, and comes back as they were: what they
    /// carried, their game mode and where they were; written to the
    /// world's folder too, as they left. Another player starts new.
    #[test]
    fn a_player_who_leaves_comes_back_as_they_were() {
        let folder = temp_folder("players-leave");
        for folder in [None, Some(folder.clone())] {
            let mut app = app(folder.clone());
            let content = base();
            let lamp = content.default_item("defaria:lamp").unwrap();
            let id = PlayerId([9; 32]);
            let first = join(&mut app, Some(id));
            let world = app.world_mut();
            world
                .get_mut::<Inventory>(first)
                .unwrap()
                .add(content, ItemStack::new(lamp, 5));
            *world.get_mut::<GameMode>(first).unwrap() = GameMode::Survival;
            world
                .get_mut::<Body>(first)
                .unwrap()
                .teleport(DVec3::new(10.0, -20.0, 30.5));
            world.get_mut::<Player>(first).unwrap().flying = true;
            let had = world.get::<Inventory>(first).unwrap().count(lamp);
            world.despawn(first);
            assert!(
                world.resource::<LeftPlayers>().0.contains_key(&id),
                "kept as they left"
            );
            if let Some(folder) = &folder {
                let saved = file::read(folder, id).unwrap().unwrap();
                assert_eq!(saved.game_mode, GameMode::Survival);
            }

            let again = join(&mut app, Some(id));
            let world = app.world();
            assert_eq!(world.get::<Inventory>(again).unwrap().count(lamp), had);
            assert_eq!(world.get::<GameMode>(again), Some(&GameMode::Survival));
            assert_eq!(
                world.get::<Body>(again).unwrap().position,
                DVec3::new(10.0, -20.0, 30.5)
            );
            assert_eq!(
                world.get::<Transform>(again).unwrap().translation,
                Vec3::new(10.0, -20.0, 30.5)
            );
            assert!(world.get::<Player>(again).unwrap().flying);

            let someone = join(&mut app, Some(PlayerId([8; 32])));
            let nobody = join(&mut app, None);
            for new in [someone, nobody] {
                let world = app.world();
                assert_eq!(world.get::<GameMode>(new), Some(&GameMode::Creative));
                assert_eq!(world.get::<Body>(new).unwrap().position.x, 1.0);
            }
        }
    }

    /// In a world on disk, every player is saved when the server stops.
    #[test]
    fn everyone_is_saved_when_the_server_stops() {
        let folder = temp_folder("players-stop");
        let mut app = app(Some(folder.clone()));
        let ids = [PlayerId([1; 32]), PlayerId([2; 32])];
        for id in ids {
            join(&mut app, Some(id));
        }
        app.world_mut().write_message(AppExit::Success);
        app.world_mut().run_schedule(SaveOnExit);
        for id in ids {
            assert!(file::read(&folder, id).unwrap().is_some(), "{id}");
        }
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
            name: String::new(),
            position: None,
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
