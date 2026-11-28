//! Saved worlds' folders: where they live on disk, the world list and
//! each world's `world.json5`. What's in them (regions of chunks, the
//! players) is the server's to read and write (`defaria_server::save`).
//! The file formats are in `docs/world-format.md`.
//!
//! The game's own folder is [`data_dir`]: `Defaria` in Application Support
//! on macOS (`%APPDATA%` on Windows, `$XDG_DATA_HOME` on Linux), which
//! also holds the install's identity (`defaria::identity`). Each world is
//! a folder in its `worlds` folder holding `world.json5` (the seed and
//! format version), a `regions` folder of region files and a `players`
//! folder of players' files.

use std::{
    io,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use serde::Deserialize;

/// The `world.json5` format this build writes. 2 keeps players by id, in
/// `players/` (a format 1 world's `player.json5` is left alone, unread).
pub const WORLD_FORMAT: u32 = 2;
/// The world's settings file, in its folder.
pub const WORLD_FILE: &str = "world.json5";
/// The folder of region files, in the world's.
pub const REGIONS_DIR: &str = "regions";

/// The game's folder for saves and settings, or `None` if the system
/// doesn't say where home is.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/Defaria"))
    }
    #[cfg(windows)]
    {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Defaria"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))?;
        Some(data.join("defaria"))
    }
}

/// The game's data folder ([`data_dir`], or `--data`): where the
/// install's identity, profile and saved servers are kept (and, for the
/// game itself, its worlds). Without it, they're kept in memory.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct DataFolder(pub PathBuf);

/// The folder of the world to load and save. Without this resource the
/// world is kept in memory only.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct WorldFolder(pub PathBuf);

/// `world.json5`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldInfo {
    pub format: u32,
    pub seed: u64,
}

impl WorldInfo {
    fn to_json5(&self) -> String {
        format!(
            "// A Defaria world. Its chunks are in regions/; see docs/world-format.md.\n\
             {{\n  format: {},\n  seed: {},\n}}\n",
            self.format, self.seed
        )
    }
}

/// Reads the world's `world.json5`, or writes a new one with `seed` for a
/// new world. Returns the world's seed.
pub fn open_world(folder: &Path, seed: u64) -> Result<u64, String> {
    let path = folder.join(WORLD_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let info: WorldInfo =
                json5::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            if info.format > WORLD_FORMAT {
                return Err(format!(
                    "{} is world format {}, newer than this build's {WORLD_FORMAT}",
                    path.display(),
                    info.format
                ));
            }
            // Written again, so when it was last written is when the world
            // was last played (see `list_worlds`), and as this build's
            // format, which it's now played as.
            let info = WorldInfo {
                format: WORLD_FORMAT,
                ..info
            };
            write_atomically(&path, info.to_json5().as_bytes())
                .map_err(|error| format!("can't write {}: {error}", path.display()))?;
            Ok(info.seed)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(folder.join(REGIONS_DIR))
                .map_err(|error| format!("can't create {}: {error}", folder.display()))?;
            let info = WorldInfo {
                format: WORLD_FORMAT,
                seed,
            };
            write_atomically(&path, info.to_json5().as_bytes())
                .map_err(|error| format!("can't write {}: {error}", path.display()))?;
            Ok(seed)
        }
        Err(error) => Err(format!("can't read {}: {error}", path.display())),
    }
}

/// The folder of saved worlds, in the game's folder ([`data_dir`]).
pub fn worlds_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("worlds"))
}

/// A saved world, as the world list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWorld {
    pub folder: PathBuf,
    /// Its folder's name.
    pub name: String,
    pub seed: u64,
    /// When it was last played: when its `world.json5` was last written.
    pub last_played: std::time::SystemTime,
}

/// The worlds in `worlds`, the most recently played first. Folders that
/// aren't worlds (no `world.json5`, or one this build can't read) are left
/// out.
pub fn list_worlds(worlds: &Path) -> Vec<SavedWorld> {
    let Ok(entries) = std::fs::read_dir(worlds) else {
        return Vec::new();
    };
    let mut found: Vec<SavedWorld> = entries
        .flatten()
        .filter_map(|entry| {
            let folder = entry.path();
            let path = folder.join(WORLD_FILE);
            let text = std::fs::read_to_string(&path).ok()?;
            let info: WorldInfo = json5::from_str(&text).ok()?;
            // A newer build's: it couldn't be played as it is.
            if info.format > WORLD_FORMAT {
                return None;
            }
            let last_played = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some(SavedWorld {
                name: folder.file_name()?.to_string_lossy().into_owned(),
                folder,
                seed: info.seed,
                last_played,
            })
        })
        .collect();
    found.sort_by(|a, b| b.last_played.cmp(&a.last_played).then(a.name.cmp(&b.name)));
    found
}

/// Creates a new world named `name` in `worlds`, with `seed`, and returns
/// its folder. The folder is the name with characters folder names can't
/// have left out, and a number after it if one of that name is there
/// already.
pub fn create_world(worlds: &Path, name: &str, seed: u64) -> Result<PathBuf, String> {
    let clean: String = name
        .chars()
        .filter(|c| {
            !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.');
    let base = if clean.is_empty() { "New World" } else { clean };
    let folder = (1..)
        .map(|n| match n {
            1 => worlds.join(base),
            n => worlds.join(format!("{base} ({n})")),
        })
        .find(|folder| !folder.exists())
        .expect("some number is free");
    open_world(&folder, seed)?;
    Ok(folder)
}

/// The seed a new world's seed field means: a number as it is, other
/// text hashed (so a word makes the same world every time), and nothing
/// a random seed.
pub fn seed_from_text(text: &str) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let text = text.trim();
    if text.is_empty() {
        return std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
    }
    if let Ok(seed) = text.parse::<u64>() {
        return seed;
    }
    if let Ok(seed) = text.parse::<i64>() {
        return seed as u64;
    }
    // FNV-1a: the same on every build and platform.
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Writes `data` to a temporary file next to `path`, then moves it over
/// `path`, so `path` is always either the old file or the new one.
pub fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, data)?;
    std::fs::rename(&temp, path)
}

/// A temporary file's name next to `path`, unique to this process and
/// call, so two writers never share one.
fn temp_beside(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let mut name = path.as_os_str().to_owned();
    name.push(format!(
        ".{}.{}.{nanos}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(name)
}

/// Writes `data` to a new temporary file next to `path`, of its own (no
/// other writer shares its name), synced to disk, and returns its path.
/// A `secret` one is readable only by its owner from the start (on unix;
/// on Windows the data folder's own per-user permissions keep it).
pub fn write_temp(path: &Path, data: &[u8], secret: bool) -> io::Result<PathBuf> {
    use std::io::Write;
    let temp = temp_beside(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = secret;
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()
    });
    match written {
        Ok(()) => Ok(temp),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Writes `data` to `path` whole, through a temporary file of its own (so
/// two writers don't trip over each other's), replacing what's there.
pub fn write_replacing(path: &Path, data: &[u8]) -> io::Result<()> {
    let temp = write_temp(path, data, false)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_folder;

    #[test]
    fn world_info_keeps_the_first_seed() {
        let folder = temp_folder("info");
        assert_eq!(open_world(&folder, 42), Ok(42));
        assert_eq!(open_world(&folder, 7), Ok(42), "the saved seed wins");
        // An older world is played, and written back as this build's.
        std::fs::write(folder.join(WORLD_FILE), "{ format: 1, seed: 5 }").unwrap();
        assert_eq!(open_world(&folder, 7), Ok(5));
        let text = std::fs::read_to_string(folder.join(WORLD_FILE)).unwrap();
        let info: WorldInfo = json5::from_str(&text).unwrap();
        assert_eq!(info.format, WORLD_FORMAT);
        std::fs::write(folder.join(WORLD_FILE), "{ format: 99, seed: 1 }").unwrap();
        assert!(open_world(&folder, 7).unwrap_err().contains("newer"));
    }

    #[test]
    fn the_data_folder_is_named_for_the_game() {
        let dir = data_dir().expect("tests run with a home folder");
        let name = dir.file_name().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(name, "defaria");
        #[cfg(target_os = "macos")]
        assert!(dir.ends_with("Library/Application Support/Defaria"));
    }

    /// New worlds go in folders named after them (a number added when the
    /// name's taken, characters folders can't have left out), and the list
    /// shows them, the most recently played first.
    #[test]
    fn worlds_are_created_and_listed() {
        let worlds = temp_folder("world-list");
        let first = create_world(&worlds, "My World", 7).unwrap();
        let second = create_world(&worlds, "My World", 8).unwrap();
        let odd = create_world(&worlds, "  a/b:c  ", 9).unwrap();
        let unnamed = create_world(&worlds, "", 10).unwrap();
        assert_eq!(first, worlds.join("My World"));
        assert_eq!(second, worlds.join("My World (2)"));
        assert_eq!(odd, worlds.join("abc"));
        assert_eq!(unnamed, worlds.join("New World"));
        // Not a world, and a newer build's: left out.
        std::fs::create_dir_all(worlds.join("junk")).unwrap();
        let newer = worlds.join("newer");
        std::fs::create_dir_all(&newer).unwrap();
        std::fs::write(newer.join(WORLD_FILE), "{ format: 99, seed: 1 }").unwrap();
        // Played in this order, oldest first.
        let at = |seconds: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        for (folder, seconds) in [(&odd, 100), (&first, 200), (&unnamed, 300), (&second, 400)] {
            std::fs::File::options()
                .write(true)
                .open(folder.join(WORLD_FILE))
                .unwrap()
                .set_modified(at(seconds))
                .unwrap();
        }
        let listed = list_worlds(&worlds);
        let names: Vec<&str> = listed.iter().map(|world| world.name.as_str()).collect();
        assert_eq!(names, ["My World (2)", "New World", "My World", "abc"]);
        assert_eq!(listed[0].seed, 8);
        // Opening one makes it the most recent.
        assert_eq!(open_world(&first, 0), Ok(7));
        assert_eq!(list_worlds(&worlds)[0].name, "My World");
        assert!(list_worlds(&worlds.join("missing")).is_empty());
    }

    /// A seed field's text: numbers as they are, words hashed the same
    /// every time, nothing a random seed.
    #[test]
    fn seeds_come_from_text() {
        assert_eq!(seed_from_text(" 42 "), 42);
        assert_eq!(seed_from_text("-1"), u64::MAX);
        assert_eq!(seed_from_text("glacier"), seed_from_text("glacier"));
        assert_ne!(seed_from_text("glacier"), seed_from_text("Glacier"));
        assert_ne!(seed_from_text(""), seed_from_text(""));
    }
}
