//! Players' files, `players/<id>.json5`: one per player who has played
//! in the world, named by their id (`defaria_core::identity::PlayerId`),
//! with what they carry, their hotbar, game mode and where they were. See
//! `docs/world-format.md`.
//!
//! Items are saved by name (`defaria:stone`,
//! `defaria:pickaxe[handle=defaria:oak,head=defaria:iron]`), like blocks in
//! chunks, since an [`ItemState`]'s number changes whenever a mod adds,
//! removes or reorders items.

use std::{io, path::Path};

use bevy::platform::collections::HashMap;
use serde::{Deserialize, Serialize};

use super::write_atomically;
use defaria_core::{
    content::{Content, ItemState, StateName},
    game_mode::GameMode,
    identity::PlayerId,
};

/// The player file format this build writes. 2 added the fluid in a
/// container (`fluid` on a stack); 3 moved the file to `players/<id>.json5`
/// and added the name and position.
pub const PLAYER_FORMAT: u32 = 3;
/// The folder of players' files, in the world's folder.
pub const PLAYERS_DIR: &str = "players";

/// Where player `id`'s file is, in the world in `folder`.
pub fn path(folder: &Path, id: PlayerId) -> std::path::PathBuf {
    folder.join(PLAYERS_DIR).join(format!("{id}.json5"))
}

/// `players/<id>.json5`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerFile {
    pub format: u32,
    /// What they were called when they were last saved: only for people
    /// reading the file (who they are is the file's name).
    #[serde(default)]
    pub name: String,
    /// Where they were, if they've been anywhere: a new player starts
    /// where new players do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<SavedPosition>,
    /// Creative or survival.
    #[serde(default)]
    pub game_mode: GameMode,
    /// The hotbar slot in hand, from 0.
    pub active_slot: usize,
    /// Each hotbar slot's item, by name, or `None` for an empty one.
    pub hotbar: Vec<Option<String>>,
    /// What the inventory holds.
    pub inventory: Vec<SavedStack>,
}

/// Where a saved player was: their feet, and whether they were flying.
/// (Not which way they looked: the client keeps that, and isn't told it.)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedPosition {
    pub feet: [f64; 3],
    #[serde(default)]
    pub flying: bool,
}

/// Some number of one item in a saved inventory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedStack {
    pub item: String,
    pub count: u32,
    /// What each of them holds, for a fluid container that isn't empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fluid: Option<SavedFluid>,
}

/// Some amount of one fluid, in a saved fluid container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedFluid {
    /// The fluid's block id.
    pub fluid: String,
    /// In thousandths of a block.
    pub amount: u64,
}

/// Reads player `id`'s file in the world in `folder`: `None` if there
/// isn't one yet (they've never played here), and an error if it can't be
/// read or is newer than this build.
pub fn read(folder: &Path, id: PlayerId) -> Result<Option<PlayerFile>, String> {
    let path = path(folder, id);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("can't read {}: {error}", path.display())),
    };
    let file: PlayerFile =
        json5::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if file.format > PLAYER_FORMAT {
        return Err(format!(
            "{} is player format {}, newer than this build's {PLAYER_FORMAT}",
            path.display(),
            file.format
        ));
    }
    Ok(Some(file))
}

/// Writes player `id`'s file in the world in `folder`, replacing the old
/// one whole.
pub fn write(folder: &Path, id: PlayerId, file: &PlayerFile) -> Result<(), String> {
    let path = path(folder, id);
    if let Some(players) = path.parent() {
        std::fs::create_dir_all(players)
            .map_err(|error| format!("can't create {}: {error}", players.display()))?;
    }
    let text = json5::to_string(file).map_err(|error| format!("{}: {error}", path.display()))?;
    let text = format!("// A player in a Defaria world; see docs/world-format.md.\n{text}\n");
    write_atomically(&path, text.as_bytes())
        .map_err(|error| format!("can't write {}: {error}", path.display()))
}

/// Converts between item states and the names they're saved under.
#[derive(Debug, Clone)]
pub struct ItemNames {
    /// By state index.
    names: Vec<String>,
    by_name: HashMap<String, ItemState>,
    /// Each item's default state, by item id, for a saved state whose
    /// properties don't exist any more.
    by_item: HashMap<String, ItemState>,
}

impl ItemNames {
    pub fn new(content: &Content) -> Self {
        let names: Vec<String> = (0..content.item_states.len())
            .map(|i| {
                content
                    .saved_item_name(ItemState(i as u32))
                    .expect("every state has a name")
            })
            .collect();
        let by_name = names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), ItemState(i as u32)))
            .collect();
        let by_item = content
            .items
            .iter()
            .map(|item| (item.id.clone(), item.default_state()))
            .collect();
        Self {
            names,
            by_name,
            by_item,
        }
    }

    /// The name `item` is saved under. `None` for a state that doesn't
    /// exist.
    pub fn name(&self, item: ItemState) -> Option<&str> {
        self.names.get(item.0 as usize).map(String::as_str)
    }

    /// The state saved as `name`. An item whose properties changed comes
    /// back as its default state; one that's gone (its mod was removed)
    /// doesn't come back.
    pub fn item(&self, name: &str) -> Option<ItemState> {
        if let Some(&item) = self.by_name.get(name) {
            return Some(item);
        }
        self.by_item.get(StateName::id_of(name)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defaria_core::{content::base, testing::temp_folder};

    #[test]
    fn items_are_named_with_their_properties() {
        let content = base();
        let names = ItemNames::new(content);
        let lamp = content.default_item("defaria:lamp").unwrap();
        assert_eq!(names.name(lamp), Some("defaria:lamp"));
        assert_eq!(names.item("defaria:lamp"), Some(lamp));
        let pickaxe = content.default_item("defaria:pickaxe").unwrap();
        let name = names.name(pickaxe).unwrap();
        assert!(name.starts_with("defaria:pickaxe["), "{name}");
        assert_eq!(names.item(name), Some(pickaxe));
        // Properties that changed: the default state. Gone: nothing.
        assert_eq!(names.item("defaria:pickaxe[head=nope]"), Some(pickaxe));
        assert_eq!(names.item("gone:thing"), None);
    }

    #[test]
    fn the_file_round_trips() {
        let folder = temp_folder("player-file");
        let id = PlayerId([3; 32]);
        assert_eq!(read(&folder, id), Ok(None));
        let file = PlayerFile {
            format: PLAYER_FORMAT,
            name: "Ada".into(),
            position: Some(SavedPosition {
                feet: [1.5, -2.0, 1e9],
                flying: true,
            }),
            game_mode: GameMode::Survival,
            active_slot: 3,
            hotbar: vec![Some("defaria:lamp".into()), None],
            inventory: vec![SavedStack {
                item: "defaria:stone".into(),
                count: 12,
                fluid: None,
            }],
        };
        write(&folder, id, &file).unwrap();
        assert_eq!(read(&folder, id), Ok(Some(file)));
        assert!(
            folder
                .join(PLAYERS_DIR)
                .join(format!("{id}.json5"))
                .exists()
        );
        // Another player's is their own.
        assert_eq!(read(&folder, PlayerId([4; 32])), Ok(None));
    }

    #[test]
    fn a_newer_file_isnt_read() {
        let folder = temp_folder("player-newer");
        let id = PlayerId([3; 32]);
        std::fs::create_dir_all(folder.join(PLAYERS_DIR)).unwrap();
        std::fs::write(
            path(&folder, id),
            "{ format: 99, active_slot: 0, hotbar: [], inventory: [] }",
        )
        .unwrap();
        assert!(read(&folder, id).unwrap_err().contains("newer"));
    }
}
