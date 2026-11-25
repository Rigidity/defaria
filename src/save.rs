//! Saved worlds: where they live on disk and how chunks get there and
//! back. The file formats are in `docs/world-format.md`.
//!
//! The game's own folder is [`data_dir`]: `Defaria` in Application Support
//! on macOS (`%APPDATA%` on Windows, `$XDG_DATA_HOME` on Linux). Each world
//! is a folder in its `worlds` folder holding `world.json5` (the seed and
//! format version) and a `regions` folder of region files.
//!
//! [`WorldStore`] is shared between the main thread and the tasks that load
//! chunks. It keeps every region it has touched in memory, as the saved
//! blobs of its chunks, plus chunks saved since the last flush as they are
//! (encoding is left to the flush, off the main thread). Loading a chunk
//! always sees the latest save, flushed or not. [`WorldStore::flush`]
//! encodes and writes the regions that changed, each to a temporary file
//! that then replaces the old one, so a crash mid-write leaves the old file
//! whole. A region that can't safely be read (its file is in a newer
//! format, or the disk won't give it up) is played but never written, so
//! what's in its file isn't lost. A store with no folder keeps everything
//! in memory instead: edits
//! survive their chunks unloading but not the game closing. Headless runs
//! use one, so scenarios always start from the same world.

pub mod chunk;
pub mod region;

use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use bevy::{platform::collections::HashMap, prelude::*};
use serde::Deserialize;

use crate::{content::Content, voxel::Chunk};
use chunk::StateNames;

/// The `world.json5` format this build writes.
pub const WORLD_FORMAT: u32 = 1;
/// The world's settings file, in its folder.
const WORLD_FILE: &str = "world.json5";
const REGIONS_DIR: &str = "regions";

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

/// The world the game opens when it isn't told which.
pub fn default_world() -> Option<PathBuf> {
    Some(data_dir()?.join("worlds").join("world"))
}

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

/// Writes `data` to a temporary file next to `path`, then moves it over
/// `path`, so `path` is always either the old file or the new one.
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, data)?;
    std::fs::rename(&temp, path)
}

/// A chunk as the store holds it.
#[derive(Debug, Clone)]
enum Stored {
    /// Saved since the last flush; encoded by the next one.
    Chunk(Arc<Chunk>),
    Blob(Arc<[u8]>),
}

#[derive(Debug, Default)]
struct Region {
    /// Whether its file (if any) has been read into `chunks`.
    read: bool,
    chunks: HashMap<u16, Stored>,
    /// Whether it holds saves its file doesn't have yet.
    dirty: bool,
    /// Whether its file mustn't be written: it couldn't be read, or holds
    /// something newer than this build reads. Saves to it stay in memory.
    read_only: bool,
}

/// The saved chunks of one world. See the module docs.
#[derive(Debug)]
pub struct WorldStore {
    /// The `regions` folder, or `None` to keep everything in memory.
    regions_dir: Option<PathBuf>,
    names: StateNames,
    regions: Mutex<HashMap<IVec3, Arc<Mutex<Region>>>>,
}

impl WorldStore {
    /// A store for the world in `folder`, or in memory for `None`.
    pub fn new(folder: Option<&Path>, content: &Content) -> Self {
        Self {
            regions_dir: folder.map(|folder| folder.join(REGIONS_DIR)),
            names: StateNames::new(content),
            regions: default(),
        }
    }

    /// The region with the given coordinate, read from disk the first time.
    fn region(&self, coord: IVec3) -> Arc<Mutex<Region>> {
        let region = self
            .regions
            .lock()
            .unwrap()
            .entry(coord)
            .or_default()
            .clone();
        {
            let mut guard = region.lock().unwrap();
            if !guard.read {
                guard.read = true;
                (guard.chunks, guard.read_only) = self.read_region(coord);
            }
        }
        region
    }

    /// Reads a region's file: its chunks, and whether the file mustn't be
    /// written.
    fn read_region(&self, coord: IVec3) -> (HashMap<u16, Stored>, bool) {
        let Some(dir) = &self.regions_dir else {
            return default();
        };
        let path = dir.join(region::file_name(coord));
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return default(),
            Err(error) => {
                // The game may just not be allowed to read it now: leave it
                // alone, and don't write over it.
                error!(
                    "can't read {} ({error}); this part of the world won't be saved",
                    path.display()
                );
                return (default(), true);
            }
        };
        if region::is_newer(&data) {
            error!(
                "{} was saved by a newer version of the game; this part of the world \
                 won't be saved, so it isn't lost",
                path.display()
            );
            return (default(), true);
        }
        match region::decode(&data) {
            Ok(chunks) => (
                chunks
                    .into_iter()
                    .map(|(index, blob)| (index, Stored::Blob(blob.into())))
                    .collect(),
                false,
            ),
            Err(error) => {
                // Keep the broken file for whoever wants to rescue it.
                let mut backup = path.as_os_str().to_owned();
                backup.push(".broken");
                error!(
                    "{} is damaged ({error}); moved it to {} and starting the region over",
                    path.display(),
                    PathBuf::from(&backup).display()
                );
                if let Err(error) = std::fs::rename(&path, &backup) {
                    error!("can't move it, so this part of the world won't be saved: {error}");
                    return (default(), true);
                }
                default()
            }
        }
    }

    /// The saved chunk, or `None` if it's never been saved (or its save
    /// can't be read, which is logged).
    pub fn load(&self, coord: IVec3) -> Option<Chunk> {
        let (region, index) = region::split(coord);
        let region = self.region(region);
        let stored = region.lock().unwrap().chunks.get(&index)?.clone();
        match stored {
            Stored::Chunk(chunk) => Some((*chunk).clone()),
            Stored::Blob(blob) => match chunk::decode(&blob, &self.names) {
                Ok(chunk) => Some(chunk),
                Err(_) if chunk::is_newer(&blob) => {
                    error!(
                        "chunk {coord} was saved by a newer version of the game; \
                         its region won't be saved, so it isn't lost"
                    );
                    region.lock().unwrap().read_only = true;
                    None
                }
                Err(error) => {
                    error!("chunk {coord} is damaged and will be generated again: {error}");
                    None
                }
            },
        }
    }

    /// Saves a chunk. It's written out by the next [`WorldStore::flush`].
    pub fn save(&self, coord: IVec3, chunk: Chunk) {
        let (region, index) = region::split(coord);
        let region = self.region(region);
        let mut region = region.lock().unwrap();
        region.chunks.insert(index, Stored::Chunk(Arc::new(chunk)));
        region.dirty = true;
    }

    /// Encodes every chunk saved since the last flush and writes each
    /// region that changed. Returns how many regions it wrote. A region
    /// that can't be written is logged and tried again next time.
    pub fn flush(&self) -> usize {
        let regions: Vec<(IVec3, Arc<Mutex<Region>>)> = self
            .regions
            .lock()
            .unwrap()
            .iter()
            .map(|(coord, region)| (*coord, region.clone()))
            .collect();
        regions
            .into_iter()
            .filter(|(coord, region)| self.flush_region(*coord, region, || {}))
            .count()
    }

    /// Flushes one region; whether it wrote it. `while_encoding` runs while
    /// the region's chunks are being encoded, with its lock free (for
    /// tests, which save into it then).
    fn flush_region(
        &self,
        coord: IVec3,
        region: &Mutex<Region>,
        while_encoding: impl FnOnce(),
    ) -> bool {
        let unencoded: Vec<(u16, Arc<Chunk>)> = {
            let mut region = region.lock().unwrap();
            if !region.dirty || region.read_only {
                return false;
            }
            region.dirty = false;
            region
                .chunks
                .iter()
                .filter_map(|(&index, stored)| match stored {
                    Stored::Chunk(chunk) => Some((index, chunk.clone())),
                    Stored::Blob(_) => None,
                })
                .collect()
        };
        // Encode without the lock, so saving and loading chunks in this
        // region (on the main thread, and loading tasks) don't wait.
        let encoded: Vec<(u16, Arc<Chunk>, Arc<[u8]>)> = unencoded
            .into_iter()
            .map(|(index, chunk)| {
                let blob = chunk::encode(&chunk, &self.names).into();
                (index, chunk, blob)
            })
            .collect();
        while_encoding();
        let (path, blobs) = {
            let mut region = region.lock().unwrap();
            for (index, chunk, blob) in encoded {
                // Unless it was saved again while this was encoding.
                if let Some(Stored::Chunk(current)) = region.chunks.get(&index)
                    && Arc::ptr_eq(current, &chunk)
                {
                    region.chunks.insert(index, Stored::Blob(blob));
                }
            }
            // What was saved meanwhile (rarely anything) is encoded here;
            // the region is dirty again, so it's written again too.
            for stored in region.chunks.values_mut() {
                if let Stored::Chunk(chunk) = stored {
                    *stored = Stored::Blob(chunk::encode(chunk, &self.names).into());
                }
            }
            let Some(dir) = &self.regions_dir else {
                return false;
            };
            let blobs: HashMap<u16, Arc<[u8]>> = region
                .chunks
                .iter()
                .map(|(&index, stored)| match stored {
                    Stored::Blob(blob) => (index, blob.clone()),
                    Stored::Chunk(_) => unreachable!("encoded above"),
                })
                .collect();
            (dir.join(region::file_name(coord)), blobs)
        };
        // Write without holding the lock, so loads from this region don't
        // wait on the disk.
        let result = std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|()| write_atomically(&path, &region::encode(&blobs)));
        match result {
            Ok(()) => true,
            Err(error) => {
                error!("can't save {}: {error}", path.display());
                region.lock().unwrap().dirty = true;
                false
            }
        }
    }

    /// Forgets the regions `in_use` says nothing needs and that have no
    /// unwritten saves; they're read from disk again when they're next
    /// needed. A store kept in memory forgets nothing.
    pub fn evict(&self, in_use: impl Fn(IVec3) -> bool) {
        if self.regions_dir.is_none() {
            return;
        }
        self.regions.lock().unwrap().retain(|&coord, region| {
            in_use(coord) || region.try_lock().map_or(true, |region| region.dirty)
        });
    }

    /// How many regions are held in memory.
    #[cfg(test)]
    pub fn regions_held(&self) -> usize {
        self.regions.lock().unwrap().len()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::content::{base, block};

    /// A fresh, empty folder under `target/`, unique to this test.
    pub fn temp_folder(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("defaria-tests/{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn chunk_with(state: crate::content::BlockState) -> Chunk {
        let mut chunk = Chunk::default();
        chunk.set(1, 2, 3, state);
        chunk
    }

    #[test]
    fn saves_survive_a_flush_and_a_new_store() {
        let folder = temp_folder("survive");
        let store = WorldStore::new(Some(&folder), base());
        let (a, b) = (IVec3::new(3, 2, -4), IVec3::new(40, -9, 7));
        assert_eq!(store.load(a), None);
        store.save(a, chunk_with(block("lamp")));
        store.save(b, chunk_with(block("sand")));
        // Unflushed saves are already visible to loads.
        assert_eq!(store.load(a), Some(chunk_with(block("lamp"))));
        assert_eq!(store.flush(), 2);
        assert_eq!(store.flush(), 0, "nothing new to write");
        assert!(folder.join("regions/r.0.0.-1.dfr").exists());
        assert!(folder.join("regions/r.2.-1.0.dfr").exists());

        let reopened = WorldStore::new(Some(&folder), base());
        assert_eq!(reopened.load(a), Some(chunk_with(block("lamp"))));
        assert_eq!(reopened.load(b), Some(chunk_with(block("sand"))));
        assert_eq!(reopened.load(IVec3::new(3, 3, -4)), None);

        // Saving into a region on disk keeps its other chunks.
        reopened.save(IVec3::new(4, 2, -4), chunk_with(block("dirt")));
        reopened.flush();
        let third = WorldStore::new(Some(&folder), base());
        assert_eq!(third.load(a), Some(chunk_with(block("lamp"))));
        assert_eq!(
            third.load(IVec3::new(4, 2, -4)),
            Some(chunk_with(block("dirt")))
        );
    }

    #[test]
    fn regions_nothing_needs_are_forgotten_once_written() {
        let folder = temp_folder("evict");
        let store = WorldStore::new(Some(&folder), base());
        store.save(IVec3::ZERO, chunk_with(block("lamp")));
        store.evict(|_| false);
        assert_eq!(store.regions_held(), 1, "not written yet");
        store.flush();
        store.evict(|region| region == IVec3::ZERO);
        assert_eq!(store.regions_held(), 1, "still in use");
        store.evict(|_| false);
        assert_eq!(store.regions_held(), 0);
        assert_eq!(store.load(IVec3::ZERO), Some(chunk_with(block("lamp"))));
    }

    #[test]
    fn a_store_in_memory_keeps_its_saves() {
        let store = WorldStore::new(None, base());
        store.save(IVec3::ONE, chunk_with(block("lamp")));
        store.flush();
        store.evict(|_| false);
        assert_eq!(store.load(IVec3::ONE), Some(chunk_with(block("lamp"))));
    }

    #[test]
    fn a_damaged_region_is_set_aside() {
        let folder = temp_folder("damaged");
        std::fs::create_dir_all(folder.join("regions")).unwrap();
        std::fs::write(folder.join("regions/r.0.0.0.dfr"), b"garbage").unwrap();
        let store = WorldStore::new(Some(&folder), base());
        assert_eq!(store.load(IVec3::ZERO), None);
        assert!(folder.join("regions/r.0.0.0.dfr.broken").exists());
    }

    /// A chunk saved again while a flush is encoding keeps its newer copy,
    /// in memory and on disk.
    #[test]
    fn saving_during_a_flush_keeps_the_newer_save() {
        let folder = temp_folder("during-flush");
        let store = WorldStore::new(Some(&folder), base());
        let coord = IVec3::new(1, 2, 3);
        store.save(coord, chunk_with(block("sand")));
        let (region_coord, _) = region::split(coord);
        let region = store.region(region_coord);
        assert!(store.flush_region(region_coord, &region, || {
            store.save(coord, chunk_with(block("lamp")));
        }));
        assert_eq!(store.load(coord), Some(chunk_with(block("lamp"))));
        store.flush();
        let reopened = WorldStore::new(Some(&folder), base());
        assert_eq!(reopened.load(coord), Some(chunk_with(block("lamp"))));
    }

    /// A region saved by a newer version of the game is left alone: it's
    /// played without its saves and never written over.
    #[test]
    fn regions_from_a_newer_version_are_never_written() {
        let folder = temp_folder("newer");
        let store = WorldStore::new(Some(&folder), base());
        store.save(IVec3::ZERO, chunk_with(block("lamp")));
        store.flush();
        let path = folder.join("regions/r.0.0.0.dfr");
        let mut newer = std::fs::read(&path).unwrap();
        newer[4] = 2;
        std::fs::write(&path, &newer).unwrap();

        let store = WorldStore::new(Some(&folder), base());
        assert_eq!(store.load(IVec3::ZERO), None);
        store.save(IVec3::ZERO, chunk_with(block("sand")));
        assert_eq!(store.load(IVec3::ZERO), Some(chunk_with(block("sand"))));
        assert_eq!(store.flush(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), newer);
        assert!(!folder.join("regions/r.0.0.0.dfr.broken").exists());
    }

    /// A region file the game isn't allowed to read is left alone too.
    #[cfg(unix)]
    #[test]
    fn unreadable_regions_are_never_written() {
        use std::os::unix::fs::PermissionsExt;
        let folder = temp_folder("unreadable");
        let store = WorldStore::new(Some(&folder), base());
        store.save(IVec3::ZERO, chunk_with(block("lamp")));
        store.flush();
        let path = folder.join("regions/r.0.0.0.dfr");
        let before = std::fs::read(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
        if std::fs::read(&path).is_ok() {
            return; // Running as root: nothing is unreadable.
        }

        let store = WorldStore::new(Some(&folder), base());
        assert_eq!(store.load(IVec3::ZERO), None);
        store.save(IVec3::ZERO, chunk_with(block("sand")));
        assert_eq!(store.flush(), 0);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// The same for a single chunk saved in a newer format.
    #[test]
    fn chunks_from_a_newer_version_keep_their_region_from_being_written() {
        let folder = temp_folder("newer-chunk");
        let store = WorldStore::new(Some(&folder), base());
        let (a, b) = (IVec3::ZERO, IVec3::X);
        store.save(a, chunk_with(block("lamp")));
        store.save(b, chunk_with(block("lamp")));
        store.flush();
        let path = folder.join("regions/r.0.0.0.dfr");
        let mut chunks = region::decode(&std::fs::read(&path).unwrap()).unwrap();
        chunks.get_mut(&0).unwrap()[0] = chunk::CHUNK_FORMAT + 1;
        let newer = region::encode(&chunks);
        std::fs::write(&path, &newer).unwrap();

        let store = WorldStore::new(Some(&folder), base());
        assert_eq!(store.load(b), Some(chunk_with(block("lamp"))));
        assert_eq!(store.load(a), None);
        store.save(a, chunk_with(block("sand")));
        assert_eq!(store.flush(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), newer);
    }

    #[test]
    fn world_info_keeps_the_first_seed() {
        let folder = temp_folder("info");
        assert_eq!(open_world(&folder, 42), Ok(42));
        assert_eq!(open_world(&folder, 7), Ok(42), "the saved seed wins");
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
}
