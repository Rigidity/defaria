//! The player's file, `player.json5`: what they carry and their hotbar.
//! See `docs/world-format.md`.
//!
//! Items are saved by name (`defaria:stone`,
//! `defaria:pickaxe[handle=defaria:oak,head=defaria:iron]`), like blocks in
//! chunks, since an [`ItemState`]'s number changes whenever a mod adds,
//! removes or reorders items.

use std::{io, path::Path};

use bevy::platform::collections::HashMap;
use serde::{Deserialize, Serialize};

use super::write_atomically;
use crate::{
    content::{Content, ItemState},
    game_mode::GameMode,
};

/// The `player.json5` format this build writes.
pub const PLAYER_FORMAT: u32 = 1;
/// The player's file, in the world's folder.
pub const PLAYER_FILE: &str = "player.json5";

/// `player.json5`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerFile {
    pub format: u32,
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

/// Some number of one item in a saved inventory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedStack {
    pub item: String,
    pub count: u32,
}

/// Reads the world's `player.json5`: `None` if there isn't one yet (a new
/// world), and an error if it can't be read or is newer than this build.
pub fn read(folder: &Path) -> Result<Option<PlayerFile>, String> {
    let path = folder.join(PLAYER_FILE);
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

/// Writes the world's `player.json5`, replacing the old one whole.
pub fn write(folder: &Path, file: &PlayerFile) -> Result<(), String> {
    let path = folder.join(PLAYER_FILE);
    let text = json5::to_string(file).map_err(|error| format!("{}: {error}", path.display()))?;
    let text = format!("// The player in a Defaria world; see docs/world-format.md.\n{text}\n");
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
        let names: Vec<String> = content
            .item_states
            .iter()
            .map(|state| {
                let id = &content.items[state.item].id;
                if state.values.is_empty() {
                    id.clone()
                } else {
                    let values: Vec<String> = state
                        .values
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    format!("{id}[{}]", values.join(","))
                }
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
        let id = name.split_once('[').map_or(name, |(id, _)| id);
        self.by_item.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{content::base, save::tests::temp_folder};

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
        assert_eq!(read(&folder), Ok(None));
        let file = PlayerFile {
            format: PLAYER_FORMAT,
            game_mode: GameMode::Survival,
            active_slot: 3,
            hotbar: vec![Some("defaria:lamp".into()), None],
            inventory: vec![SavedStack {
                item: "defaria:stone".into(),
                count: 12,
            }],
        };
        write(&folder, &file).unwrap();
        assert_eq!(read(&folder), Ok(Some(file)));
    }

    #[test]
    fn a_newer_file_isnt_read() {
        let folder = temp_folder("player-newer");
        std::fs::write(
            folder.join(PLAYER_FILE),
            "{ format: 99, active_slot: 0, hotbar: [], inventory: [] }",
        )
        .unwrap();
        assert!(read(&folder).unwrap_err().contains("newer"));
    }
}
