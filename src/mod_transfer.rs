//! A server's mods, sent to the clients that join it (see `quic` for when,
//! and `docs/modding.md`, "Multiplayer", for modders).
//!
//! A server offers every `both` mod its content loaded ([`Offer`]): a list
//! of each one's id, version and hash ([`ModEntry`]), and the files
//! themselves ([`Package`]) to whoever asks. Only a mod's manifest, its
//! definitions (`.json5`), images (`.png`) and scripts (`.luau`) are ever
//! in a package, each at most [`MAX_FILE`] bytes and the mod at most
//! [`MAX_MOD`], every path checked ([`check_path`]). Its `server` mods
//! are never offered.
//!
//! A client's [`Library`] says which of the offered mods it lacks: a mod
//! of its own with the same hash (the same game's base mod, usually) is
//! used as it is, and one fetched before is in the cache, the data
//! folder's `mod-cache/<hash>/`. What it fetches is untrusted: every path,
//! size and the hash are checked again before it's cached or used. It
//! then builds [`Content`] from exactly the offered mods, and its own
//! `client` mods, and the two sides compare the content's fingerprint.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use bevy::prelude::*;
use defaria_core::{
    content::{
        Content, ModSide, check_namespace,
        load::{self, LoadedMod, MANIFEST, ModFiles},
        schema::Manifest,
    },
    identity::{hex, parse_hex},
};
use serde::{Deserialize, Serialize};

/// The data folder's cache of mods fetched from servers, by hash.
pub const CACHE_DIR: &str = "mod-cache";
/// The biggest file a mod may send.
pub const MAX_FILE: usize = 1 << 20;
/// The most a mod's files may add up to.
pub const MAX_MOD: usize = 16 << 20;
/// The most files a mod may send.
pub const MAX_FILES: usize = 4096;
/// The most mods a server may offer.
pub const MAX_MODS: usize = 256;
/// The most a server's mods may add up to.
pub const MAX_OFFER: usize = 64 << 20;
/// The most the cache keeps: past it, the mods used longest ago go.
pub const MAX_CACHE: u64 = 256 << 20;
/// The longest path a mod's file may have, in bytes.
pub const MAX_PATH: usize = 128;
/// The deepest a mod's file may be, in folders.
const MAX_DEPTH: usize = 16;
/// The longest id or version a server may say a mod has.
const MAX_NAME: usize = 64;
/// The kinds of file a mod sends: definitions, images and scripts.
pub const SENT: [&str; 3] = ["json5", "png", "luau"];
/// Where a mod a server sent says it's from, in error messages and asset
/// paths: `received/<id>`. Not a folder: its files are in memory.
const RECEIVED: &str = "received";

/// A hash of a mod's files (SHA-256; see [`Package::hash`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModHash(pub [u8; 32]);

impl fmt::Display for ModHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for ModHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ModHash({self})")
    }
}

/// A mod a server offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModEntry {
    pub id: String,
    pub version: Option<String>,
    pub hash: ModHash,
    /// Its files' bytes, all together.
    pub size: u64,
}

/// A mod's files, as sent: each by its path in the mod folder
/// (`/`-separated), the manifest (`mod.json5`) among them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Package {
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Checks a path inside a mod, as sent or received: lowercase ASCII
/// letters, digits, `_`, `-` and `.`, in folders split by `/`, none of
/// them empty or starting with a `.` (so no `..`, no hidden files, no
/// absolute paths or drive letters), at most [`MAX_PATH`] bytes and
/// [`MAX_DEPTH`] folders deep, naming a kind of file mods send.
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(format!(
            "`{}` isn't 1 to {MAX_PATH} bytes long",
            path.escape_debug()
        ));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return Err(format!("`{path}` is more than {MAX_DEPTH} folders deep"));
    }
    for part in &parts {
        if part.is_empty() || part.starts_with('.') {
            return Err(format!("`{}` isn't a path in a mod", path.escape_debug()));
        }
        // Names Windows keeps for devices, whatever follows the dot.
        let stem = part.split('.').next().unwrap_or_default();
        let device = matches!(stem, "con" | "nul" | "prn" | "aux")
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if device {
            return Err(format!("`{path}` is named as a device is on Windows"));
        }
        if let Some(odd) = part
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.".contains(*c)))
        {
            return Err(format!(
                "`{}` has a `{}` in it: paths in a mod are lowercase letters, digits, `_`, `-` and `.`",
                path.escape_debug(),
                odd.escape_debug()
            ));
        }
    }
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    if !extension.is_some_and(|extension| SENT.contains(&extension)) {
        return Err(format!(
            "`{path}` isn't a definition, an image or a script, the only files a mod sends"
        ));
    }
    Ok(())
}

impl Package {
    /// The hash of its files: SHA-256 over each path and its bytes, each
    /// with its length first, in path order.
    pub fn hash(&self) -> ModHash {
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        for (path, bytes) in &self.files {
            context.update(&(path.len() as u64).to_le_bytes());
            context.update(path.as_bytes());
            context.update(&(bytes.len() as u64).to_le_bytes());
            context.update(bytes);
        }
        ModHash(
            context
                .finish()
                .as_ref()
                .try_into()
                .expect("SHA-256 is 32 bytes"),
        )
    }

    /// Its files' bytes, all together.
    pub fn size(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }

    /// Checks it keeps to the limits: a manifest, at most [`MAX_FILES`]
    /// files of at most [`MAX_FILE`] bytes each and [`MAX_MOD`] together,
    /// every path a good one ([`check_path`]).
    pub fn check(&self) -> Result<(), String> {
        if !self.files.contains_key(MANIFEST) {
            return Err(format!("it has no {MANIFEST}"));
        }
        if self.files.len() > MAX_FILES {
            return Err(format!(
                "it has {} files, more than the {MAX_FILES} a mod may send",
                self.files.len()
            ));
        }
        for (path, bytes) in &self.files {
            check_path(path)?;
            if bytes.len() > MAX_FILE {
                return Err(format!(
                    "{path} is {} bytes, more than the {MAX_FILE} a file may be",
                    bytes.len()
                ));
            }
            // What parsing or decoding them could choke on, refused first.
            if path.ends_with(".json5") {
                let text =
                    std::str::from_utf8(bytes).map_err(|_| format!("{path} isn't UTF-8 text"))?;
                load::check_nesting(text).map_err(|error| format!("{path}: {error}"))?;
            } else if path.ends_with(".png") {
                load::check_png(bytes).map_err(|error| format!("{path}: {error}"))?;
            }
        }
        let size = self.size();
        if size > MAX_MOD {
            return Err(format!(
                "its files are {size} bytes, more than the {MAX_MOD} a mod may send"
            ));
        }
        Ok(())
    }

    /// The files of a mod the content loaded: its manifest, definitions,
    /// scripts and images (from memory, or its folder on disk), checked.
    pub fn of(loaded: &LoadedMod) -> Result<Self, String> {
        let mod_files = &loaded.files;
        let mut files = BTreeMap::new();
        files.insert(
            MANIFEST.to_string(),
            mod_files.manifest.clone().into_bytes(),
        );
        for (path, text) in mod_files.files.iter().chain(&mod_files.scripts) {
            files.insert(path.clone(), text.clone().into_bytes());
        }
        for path in &mod_files.images {
            let bytes = match (mod_files.image_bytes.get(path), &mod_files.folder) {
                (Some(bytes), _) => bytes.to_vec(),
                (None, Some(folder)) => {
                    // The size first: a huge file isn't read only to be
                    // refused.
                    let file = folder.join(path);
                    let size = fs::metadata(&file)
                        .map_err(|e| format!("{path}: {e}"))?
                        .len();
                    if size > MAX_FILE as u64 {
                        return Err(format!(
                            "{path} is {size} bytes, more than the {MAX_FILE} a file may be"
                        ));
                    }
                    fs::read(&file).map_err(|e| format!("{path}: {e}"))?
                }
                (None, None) => return Err(format!("{path} isn't on disk or in memory")),
            };
            files.insert(path.clone(), bytes);
        }
        let package = Self { files };
        package.check()?;
        Ok(package)
    }

    /// The mod's files, for loading: definitions and scripts as text,
    /// images in memory, at `dir` (its asset path, for errors).
    pub fn into_mod_files(self, dir: String) -> Result<ModFiles, String> {
        let mut mod_files = ModFiles { dir, ..default() };
        for (path, bytes) in self.files {
            check_path(&path)?;
            if path.ends_with(".png") {
                mod_files.images.insert(path.clone());
                mod_files.image_bytes.insert(path, bytes.into());
                continue;
            }
            let text = String::from_utf8(bytes).map_err(|_| format!("{path} isn't UTF-8 text"))?;
            if path == MANIFEST {
                mod_files.manifest = text;
            } else if path.ends_with(".luau") {
                mod_files.scripts.insert(path, text);
            } else {
                mod_files.files.insert(path, text);
            }
        }
        Ok(mod_files)
    }

    /// Writes its files into `folder`, which shouldn't exist yet: into a
    /// folder beside it first, renamed into place once every file's
    /// there, so a half-written mod is never found.
    pub fn write_to(&self, folder: &Path) -> Result<(), String> {
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = folder.file_name().unwrap_or_default().to_string_lossy();
        let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let partial =
            folder.with_file_name(format!(".{name}.{}.{write}.partial", std::process::id()));
        let _ = fs::remove_dir_all(&partial);
        let written = (|| {
            for (path, bytes) in &self.files {
                // Checked again: this is where a bad path would do harm.
                check_path(path)?;
                let file = partial.join(path);
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&file, bytes).map_err(|e| e.to_string())?;
            }
            fs::rename(&partial, folder).map_err(|e| e.to_string())
        })();
        if written.is_err() {
            let _ = fs::remove_dir_all(&partial);
        }
        written.map_err(|error| format!("can't write {}: {error}", folder.display()))
    }

    /// Reads a mod [`Package::write_to`] wrote, checked as if it had just
    /// arrived: every file in it must be one a mod sends.
    pub fn read_from(folder: &Path) -> Result<Self, String> {
        fn walk(
            folder: &Path,
            prefix: &str,
            files: &mut BTreeMap<String, Vec<u8>>,
        ) -> Result<(), String> {
            for entry in fs::read_dir(folder).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let relative = format!("{prefix}{name}");
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_dir() {
                    walk(&entry.path(), &format!("{relative}/"), files)?;
                } else if kind.is_file() {
                    check_path(&relative)?;
                    if files.len() >= MAX_FILES {
                        return Err(format!("more than {MAX_FILES} files"));
                    }
                    let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
                    files.insert(relative, bytes);
                } else {
                    return Err(format!("{relative} isn't a file"));
                }
            }
            Ok(())
        }
        let mut files = BTreeMap::new();
        walk(folder, "", &mut files)?;
        let package = Self { files };
        package.check()?;
        Ok(package)
    }
}

/// The mods a server sends its clients: every `both` mod its content
/// loaded, in load order, and the fingerprint of that content.
#[derive(Debug, Clone)]
pub struct Offer {
    pub entries: Vec<ModEntry>,
    packages: HashMap<ModHash, Arc<Package>>,
    /// The content's [`Content::fingerprint`], which the client's must
    /// match.
    pub content: u64,
}

impl Offer {
    /// What a server with `content` offers. Fails if a mod can't be sent
    /// (a file too big, a bad name), saying which and why: a server
    /// whose clients couldn't have its mods shouldn't start.
    pub fn new(content: &Content) -> Result<Self, String> {
        let mut entries = Vec::new();
        let mut packages = HashMap::new();
        for loaded in content.sources.iter() {
            if loaded.manifest.side != ModSide::Both {
                continue;
            }
            let id = &loaded.manifest.id;
            let package = Package::of(loaded).map_err(|error| {
                format!("mod `{id}` ({}) can't be sent: {error}", loaded.files.dir)
            })?;
            let hash = package.hash();
            entries.push(ModEntry {
                id: id.clone(),
                version: loaded.manifest.version.clone(),
                hash,
                size: package.size() as u64,
            });
            packages.insert(hash, Arc::new(package));
        }
        // What a client would refuse, refused here first.
        check_entries(&entries)?;
        Ok(Self {
            entries,
            packages,
            content: content.fingerprint(),
        })
    }

    /// The files of the offered mod with hash `hash`.
    pub fn package(&self, hash: ModHash) -> Option<&Arc<Package>> {
        self.packages.get(&hash)
    }
}

/// Where a client has each mod a server offers from.
#[derive(Debug, Clone)]
enum Have {
    /// One of its own, with the same files.
    Own(ModFiles),
    /// Fetched, now or before.
    Fetched(ModFiles),
}

/// A client's mods, for joining a server: its own (the mods its
/// single-player content loaded), the cache of mods fetched before, and
/// what it fetches.
#[derive(Debug, Clone)]
pub struct Library {
    /// The data folder's [`CACHE_DIR`], if it has one.
    cache: Option<PathBuf>,
    /// Its own mods, in load order.
    own: Vec<LoadedMod>,
    /// Its own `both` mods' hashes, worked out the first time they're
    /// needed.
    own_hashes: Option<HashMap<ModHash, usize>>,
    have: HashMap<ModHash, Have>,
    /// The mods it fetched, in the order they arrived.
    pub fetched: Vec<ModHash>,
    /// The most the cache keeps ([`MAX_CACHE`]).
    pub cache_limit: u64,
}

impl Library {
    /// A client's mods: its own are those `own` loaded (the content it
    /// plays single player with), and the cache is in `data`, the data
    /// folder, if it has one (in memory, it fetches every mod it lacks
    /// each time).
    pub fn new(own: &Content, data: Option<&Path>) -> Self {
        Self {
            cache: data.map(|data| data.join(CACHE_DIR)),
            own: own.sources.to_vec(),
            own_hashes: None,
            have: HashMap::new(),
            fetched: Vec::new(),
            cache_limit: MAX_CACHE,
        }
    }

    /// A client with no mods of its own.
    pub fn empty(data: Option<&Path>) -> Self {
        Self {
            cache: data.map(|data| data.join(CACHE_DIR)),
            own: Vec::new(),
            own_hashes: None,
            have: HashMap::new(),
            fetched: Vec::new(),
            cache_limit: MAX_CACHE,
        }
    }

    fn own_hashes(&mut self) -> &HashMap<ModHash, usize> {
        self.own_hashes.get_or_insert_with(|| {
            (self.own.iter().enumerate())
                .filter(|(_, m)| m.manifest.side == ModSide::Both)
                .filter_map(|(i, m)| Some((Package::of(m).ok()?.hash(), i)))
                .collect()
        })
    }

    /// The mods offered in `entries` it has to fetch, in order, having
    /// found the rest among its own or in the cache. Refuses a list that
    /// breaks the limits.
    pub fn missing(&mut self, entries: &[ModEntry]) -> Result<Vec<ModHash>, String> {
        check_entries(entries)?;
        let mut missing = Vec::new();
        for entry in entries {
            if self.have.contains_key(&entry.hash) {
                continue;
            }
            if let Some(&i) = self.own_hashes().get(&entry.hash) {
                let files = self.own[i].files.clone();
                self.have.insert(entry.hash, Have::Own(files));
                continue;
            }
            match self.cached(entry) {
                Some(files) => {
                    self.have.insert(entry.hash, Have::Fetched(files));
                }
                None => missing.push(entry.hash),
            }
        }
        Ok(missing)
    }

    /// Mod `entry` from the cache, if it's there and still what it should
    /// be. One that isn't is removed, to be fetched again.
    fn cached(&self, entry: &ModEntry) -> Option<ModFiles> {
        let folder = self.cache.as_ref()?.join(entry.hash.to_string());
        if !folder.is_dir() {
            return None;
        }
        let read = Package::read_from(&folder).and_then(|package| {
            if package.hash() != entry.hash {
                return Err("its files changed".to_string());
            }
            package.into_mod_files(received_dir(&entry.id))
        });
        match read {
            Ok(files) => {
                touch(&folder);
                Some(files)
            }
            Err(error) => {
                warn!(
                    "the cached mod {} ({}) is damaged ({error}); fetching it again",
                    entry.id,
                    folder.display()
                );
                let _ = fs::remove_dir_all(&folder);
                None
            }
        }
    }

    /// Takes `package`, which arrived for `entry`: checked (its paths,
    /// sizes and hash), cached, and kept for [`Library::content`].
    pub fn receive(&mut self, entry: &ModEntry, package: Package) -> Result<(), String> {
        package.check()?;
        if package.size() as u64 != entry.size {
            return Err(format!(
                "its files are {} bytes, not the {} offered",
                package.size(),
                entry.size
            ));
        }
        let hash = package.hash();
        if hash != entry.hash {
            return Err(format!(
                "its files' hash is {hash}, not the {} offered",
                entry.hash
            ));
        }
        let manifest = std::str::from_utf8(&package.files[MANIFEST])
            .map_err(|error| error.to_string())
            .and_then(|text| json5::from_str::<Manifest>(text).map_err(|e| e.to_string()))
            .map_err(|error| format!("its {MANIFEST} doesn't read: {error}"))?;
        if manifest.id != entry.id {
            return Err(format!(
                "its {MANIFEST} says it's mod {}, not {}",
                manifest.id.escape_debug(),
                entry.id
            ));
        }
        if let Some(cache) = &self.cache {
            let folder = cache.join(hash.to_string());
            let written = fs::create_dir_all(cache)
                .map_err(|error| format!("can't make {}: {error}", cache.display()))
                .and_then(|()| match folder.is_dir() {
                    true => Ok(()),
                    false => package.write_to(&folder),
                });
            match written {
                Ok(()) => {
                    touch(&folder);
                    let keep: HashSet<ModHash> = self.have.keys().copied().chain([hash]).collect();
                    evict(cache, self.cache_limit, &keep);
                }
                Err(error) => warn!("{error}; mod {} isn't cached", entry.id),
            }
        }
        let files = package.into_mod_files(received_dir(&entry.id))?;
        self.have.insert(hash, Have::Fetched(files));
        self.fetched.push(hash);
        Ok(())
    }

    /// The content of the mods in `entries` (every one of them had, or
    /// [`Library::missing`] says otherwise), and its own `client` mods.
    pub fn content(&self, entries: &[ModEntry]) -> Result<Content, String> {
        let mut mods = Vec::new();
        for entry in entries {
            match self.have.get(&entry.hash) {
                Some(Have::Own(files) | Have::Fetched(files)) => mods.push(files.clone()),
                None => return Err(format!("mod {} never arrived", entry.id)),
            }
        }
        mods.extend(
            (self.own.iter())
                .filter(|m| m.manifest.side == ModSide::Client)
                .map(|m| m.files.clone()),
        );
        let content = Content::from_mods(mods);
        for error in &content.errors {
            warn!("the server's mods: {error}");
        }
        Ok(content)
    }
}

/// Marks a cached mod used now, so it's the last to go
/// ([`evict`]).
fn touch(folder: &Path) {
    let now = std::time::SystemTime::now();
    if let Err(error) = fs::File::open(folder).and_then(|dir| dir.set_modified(now)) {
        warn!("can't mark {} used: {error}", folder.display());
    }
}

/// Removes the mods in `cache` used longest ago (by their folders'
/// modified times, which [`touch`] sets) until the rest add up to at most
/// `limit` bytes, never those in `keep` (this game's). Whatever isn't a
/// mod's folder is left alone, but for half-written ones of copies of the
/// game long gone.
pub fn evict(cache: &Path, limit: u64, keep: &HashSet<ModHash>) {
    fn size(path: &Path) -> u64 {
        let Ok(entries) = fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => size(&entry.path()),
                _ => entry.metadata().map_or(0, |m| m.len()),
            })
            .sum()
    }
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    let mut mods = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let used = entry.metadata().and_then(|m| m.modified()).ok();
        if name.ends_with(".partial") {
            let day = std::time::Duration::from_secs(24 * 60 * 60);
            if used.is_some_and(|used| used.elapsed().is_ok_and(|age| age > day)) {
                let _ = fs::remove_dir_all(&path);
            }
            continue;
        }
        let Some(hash) = parse_hex(&name).map(ModHash) else {
            continue;
        };
        mods.push((used, hash, size(&path), path));
    }
    let mut total: u64 = mods.iter().map(|(_, _, size, _)| size).sum();
    // Oldest first.
    mods.sort_by_key(|(used, ..)| *used);
    for (_, hash, size, path) in mods {
        if total <= limit {
            break;
        }
        if keep.contains(&hash) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            total -= size;
        }
    }
}

/// Where a mod a server sent says it's from.
fn received_dir(id: &str) -> String {
    format!("{RECEIVED}/{id}")
}

/// Checks a server's list of mods keeps to the limits, and names no mod
/// twice.
pub fn check_entries(entries: &[ModEntry]) -> Result<(), String> {
    if entries.len() > MAX_MODS {
        return Err(format!(
            "{} mods offered, more than the {MAX_MODS} a server may send",
            entries.len()
        ));
    }
    let mut hashes = HashSet::new();
    let mut total = 0u64;
    for entry in entries {
        // Ids as local mods have them, so one can't reach a path, a log
        // or an error screen as anything else.
        if entry.id.len() > MAX_NAME || check_namespace(&entry.id).is_err() {
            return Err(format!("`{}` isn't a mod id", entry.id.escape_debug()));
        }
        let version = entry.version.as_deref().unwrap_or("x");
        if version.is_empty()
            || version.len() > MAX_NAME
            || !version.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        {
            return Err(format!(
                "mod {}'s version `{}` isn't 1 to {MAX_NAME} printable ASCII characters",
                entry.id,
                version.escape_debug()
            ));
        }
        total = total.saturating_add(entry.size);
        if entry.size > MAX_MOD as u64 {
            return Err(format!(
                "mod {} is {} bytes, more than the {MAX_MOD} a mod may send",
                entry.id, entry.size
            ));
        }
        if !hashes.insert(entry.hash) {
            return Err(format!("mod {} is offered twice", entry.id));
        }
    }
    if total > MAX_OFFER as u64 {
        return Err(format!(
            "the mods offered are {total} bytes, more than the {MAX_OFFER} a server may send"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use defaria_core::{content::load::order, testing::temp_folder};

    use super::*;

    fn content(mods: Vec<ModFiles>) -> Content {
        let content = Content::from_mods(mods);
        assert!(content.errors.is_empty(), "{:#?}", content.errors);
        content
    }

    fn a_mod(id: &str, side: &str, files: &[(&str, &str)]) -> ModFiles {
        ModFiles::new(
            &format!("mods/{id}"),
            &format!(r#"{{ id: "{id}", name: "{id}", side: "{side}" }}"#),
            files,
        )
    }

    #[test]
    fn paths_are_checked() {
        for good in [
            "console.png",
            "com.png",
            "com0.png",
            "lpt10.png",
            "mod.json5",
            "blocks/ores/iron.json5",
            "images/a-b_c.2.png",
            "scripts/x.luau",
        ] {
            assert_eq!(check_path(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd.json5",
            "../up.json5",
            "blocks/../../up.json5",
            "./here.json5",
            "blocks//x.json5",
            "blocks/.hidden.json5",
            "C:/x.json5",
            "c:x.json5",
            "blocks\\x.json5",
            "Blocks/x.json5",
            "blocks/x y.json5",
            "blocks/x.json5/",
            "blocks/x.exe",
            "blocks/x",
            "notes.md",
            "é.png",
            "con.png",
            "images/nul.json5",
            "aux.tar.png",
            "com1.luau",
            "lpt9/x.png",
            &format!("{}.png", "a".repeat(MAX_PATH)),
            &format!("{}x.png", "a/".repeat(MAX_DEPTH)),
        ] {
            assert!(check_path(bad).is_err(), "{bad:?} passed");
        }
    }

    /// A server offers its `both` mods, never its `server` ones; a
    /// package holds only what mods send, and hashes the same however
    /// it's come by.
    #[test]
    fn a_server_offers_its_both_mods_only() {
        let mut shared = a_mod(
            "shared",
            "both",
            &[("images/a.png", ""), ("scripts/s.luau", "return 1")],
        );
        let stone = defaria_core::content::mods_dir().join("defaria/images/stone.png");
        let png = std::fs::read(stone).unwrap();
        shared.image_bytes.insert("images/a.png".into(), png.into());
        let content = content(vec![
            shared,
            a_mod("admin", "server", &[("scripts/kick.luau", "return 2")]),
        ]);
        let offer = Offer::new(&content).unwrap();
        assert_eq!(offer.entries.len(), 1);
        assert_eq!(offer.entries[0].id, "shared");
        let package = offer.package(offer.entries[0].hash).unwrap();
        let paths: Vec<&str> = package.files.keys().map(String::as_str).collect();
        assert_eq!(paths, ["images/a.png", MANIFEST, "scripts/s.luau"]);
        assert_eq!(package.hash(), offer.entries[0].hash);
        assert_eq!(offer.content, content.fingerprint());
        // Back into a mod, and packed again: the same.
        let files = (**package)
            .clone()
            .into_mod_files("received/shared".into())
            .unwrap();
        let again = Package::of(&order(vec![files], &mut Vec::new())[0]).unwrap();
        assert_eq!(again.hash(), package.hash());
    }

    /// The base game's mods, read from disk, can be sent.
    #[test]
    fn the_base_game_can_be_sent() {
        let offer = Offer::new(defaria_core::content::base()).unwrap();
        assert_eq!(offer.entries[0].id, "defaria");
        let package = offer.package(offer.entries[0].hash).unwrap();
        assert!(package.files.contains_key("images/stone.png"));
        assert!(!package.files.keys().any(|path| path.ends_with(".md")));
    }

    /// A mod with a file over the limit, or over the limit together,
    /// can't be sent, and isn't taken when it arrives.
    #[test]
    fn an_oversized_mod_is_refused() {
        let big = "x".repeat(MAX_FILE + 1);
        let content = Content::from_mods(vec![a_mod("big", "both", &[("scripts/big.luau", &big)])]);
        let error = Offer::new(&content).unwrap_err();
        assert!(
            error.contains("big.luau") && error.contains("more than"),
            "{error}"
        );

        let mut package = Package::default();
        package.files.insert(MANIFEST.into(), b"{}".to_vec());
        for i in 0..(MAX_MOD / MAX_FILE + 1) {
            package
                .files
                .insert(format!("scripts/{i}.luau"), vec![b' '; MAX_FILE]);
        }
        let entry = ModEntry {
            id: "big".into(),
            version: None,
            hash: package.hash(),
            size: package.size() as u64,
        };
        let mut library = Library::empty(None);
        let error = library.receive(&entry, package).unwrap_err();
        assert!(error.contains("more than"), "{error}");
        assert!(check_entries(&[entry]).is_err());
    }

    /// What arrives must be what was offered: another mod's files, or a
    /// file with a bad path, aren't taken.
    #[test]
    fn a_received_mod_is_checked() {
        let content = content(vec![a_mod(
            "shared",
            "both",
            &[("scripts/s.luau", "return 1")],
        )]);
        let offer = Offer::new(&content).unwrap();
        let entry = &offer.entries[0];
        let mut library = Library::empty(None);
        assert_eq!(library.missing(&offer.entries).unwrap(), [entry.hash]);

        let mut other = (**offer.package(entry.hash).unwrap()).clone();
        // The same size, other bytes.
        other
            .files
            .insert("scripts/s.luau".into(), b"return 2".to_vec());
        let error = library.receive(entry, other).unwrap_err();
        assert!(error.contains("hash"), "{error}");

        let mut bad = (**offer.package(entry.hash).unwrap()).clone();
        bad.files.insert("../../evil.luau".into(), Vec::new());
        let error = library
            .receive(
                &ModEntry {
                    hash: bad.hash(),
                    ..entry.clone()
                },
                bad,
            )
            .unwrap_err();
        assert!(error.contains("evil"), "{error}");
        assert!(library.content(&offer.entries).is_err());

        library
            .receive(entry, (**offer.package(entry.hash).unwrap()).clone())
            .unwrap();
        let built = library.content(&offer.entries).unwrap();
        assert_eq!(built.fingerprint(), content.fingerprint());
        assert_eq!(built.mods[0].dir, "received/shared");
    }

    /// A mod fetched once is in the cache, and isn't fetched again; one
    /// whose cached files changed is.
    #[test]
    fn a_cached_mod_is_not_fetched_again() {
        let data = temp_folder("mod-cache");
        let content = content(vec![a_mod(
            "shared",
            "both",
            &[("scripts/s.luau", "return 1")],
        )]);
        let offer = Offer::new(&content).unwrap();
        let entry = &offer.entries[0];
        let mut first = Library::empty(Some(&data));
        assert_eq!(first.missing(&offer.entries).unwrap(), [entry.hash]);
        first
            .receive(entry, (**offer.package(entry.hash).unwrap()).clone())
            .unwrap();
        let folder = data.join(CACHE_DIR).join(entry.hash.to_string());
        assert!(folder.join("scripts/s.luau").is_file());

        let mut second = Library::empty(Some(&data));
        assert!(second.missing(&offer.entries).unwrap().is_empty());
        let built = second.content(&offer.entries).unwrap();
        assert_eq!(built.fingerprint(), content.fingerprint());

        fs::write(folder.join("scripts/s.luau"), "return 666").unwrap();
        let mut third = Library::empty(Some(&data));
        assert_eq!(third.missing(&offer.entries).unwrap(), [entry.hash]);
        assert!(!folder.exists(), "the damaged copy is gone");
    }

    /// A client's own mod with the same files is used as it is; its own
    /// `client` mods join the server's, and its other mods don't.
    #[test]
    fn a_client_uses_its_own_copies_and_client_mods() {
        let shared = a_mod("shared", "both", &[("scripts/s.luau", "return 1")]);
        let server = content(vec![shared.clone()]);
        let offer = Offer::new(&server).unwrap();
        let own = content(vec![
            shared,
            a_mod("hud", "client", &[("scripts/hud.luau", "return 3")]),
            a_mod("extra", "both", &[("scripts/e.luau", "return 4")]),
        ]);
        let mut library = Library::new(&own, None);
        assert!(library.missing(&offer.entries).unwrap().is_empty());
        let built = library.content(&offer.entries).unwrap();
        let ids: Vec<&str> = built.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["hud", "shared"]);
        assert_eq!(built.mods[1].dir, "mods/shared");
        assert_eq!(built.fingerprint(), server.fingerprint());
    }

    /// A package with what would choke parsing or decoding is refused:
    /// JSON5 nested too deep, a PNG claiming a huge image.
    #[test]
    fn what_could_choke_the_client_is_refused() {
        let deep = format!("{{ a: {}{} }}", "[".repeat(100_000), "]".repeat(100_000));
        let content = Content::from_mods(vec![a_mod("deep", "both", &[("blocks/x.json5", &deep)])]);
        // The server's own copy fails to load that file, and won't send it.
        assert!(
            content.errors.iter().any(|e| e.contains("deep")),
            "{:?}",
            content.errors
        );
        assert!(Offer::new(&content).unwrap_err().contains("deep"));

        let mut package = Package::default();
        package
            .files
            .insert(MANIFEST.into(), br#"{ id: "x", name: "x" }"#.to_vec());
        package
            .files
            .insert("blocks/x.json5".into(), deep.into_bytes());
        assert!(package.check().unwrap_err().contains("deep"));
        package.files.remove("blocks/x.json5");
        let mut png =
            std::fs::read(defaria_core::content::mods_dir().join("defaria/images/stone.png"))
                .unwrap();
        png[20..24].copy_from_slice(&(1u32 << 30).to_be_bytes());
        package.files.insert("images/a.png".into(), png);
        assert!(package.check().unwrap_err().contains("images/a.png"));
    }

    /// The server's list is checked: ids as local mods have them, plain
    /// versions, at most [`MAX_OFFER`] bytes in all.
    #[test]
    fn a_server_list_is_checked() {
        let entry = |id: &str, version: Option<&str>, size: u64, n: u8| ModEntry {
            id: id.into(),
            version: version.map(Into::into),
            hash: ModHash([n; 32]),
            size,
        };
        assert_eq!(
            check_entries(&[entry("fine_1", Some("1.0 beta"), 10, 0)]),
            Ok(())
        );
        for bad in ["../up", "Bad", "a\nb", "", "x/y"] {
            assert!(check_entries(&[entry(bad, None, 1, 0)]).is_err(), "{bad:?}");
        }
        assert!(check_entries(&[entry("a", Some("1\u{1b}[2J"), 1, 0)]).is_err());
        let biggest = |n: u8| entry(&format!("m{n}"), None, MAX_MOD as u64, n);
        let full: Vec<ModEntry> = (0..(MAX_OFFER / MAX_MOD) as u8).map(biggest).collect();
        assert_eq!(check_entries(&full), Ok(()));
        let mut over = full.clone();
        over.push(entry("c", None, 1, 99));
        let error = check_entries(&over).unwrap_err();
        assert!(error.contains("more than"), "{error}");
    }

    /// A package must be the size offered, and its manifest the mod
    /// offered.
    #[test]
    fn a_received_mod_must_be_what_was_offered() {
        let content = content(vec![a_mod(
            "shared",
            "both",
            &[("scripts/s.luau", "return 1")],
        )]);
        let offer = Offer::new(&content).unwrap();
        let entry = &offer.entries[0];
        let package = (**offer.package(entry.hash).unwrap()).clone();
        let mut library = Library::empty(None);
        let wrong_size = ModEntry {
            size: entry.size + 1,
            ..entry.clone()
        };
        assert!(
            library
                .receive(&wrong_size, package.clone())
                .unwrap_err()
                .contains("bytes")
        );
        let renamed = ModEntry {
            id: "other".into(),
            ..entry.clone()
        };
        let error = library.receive(&renamed, package).unwrap_err();
        assert!(error.contains("says it's mod shared"), "{error}");
    }

    /// Past its limit, the cache drops the mods used longest ago, never
    /// the ones in use; using a cached mod makes it the last to go.
    #[test]
    fn the_cache_drops_what_was_used_longest_ago() {
        let data = temp_folder("mod-cache-eviction");
        let mods: Vec<Offer> = (0..3)
            .map(|i| {
                let text = format!("return {}", "x".repeat(1000 + i));
                Offer::new(&content(vec![a_mod(
                    &format!("m{i}"),
                    "both",
                    &[("scripts/s.luau", &text)],
                )]))
                .unwrap()
            })
            .collect();
        let folder = |offer: &Offer| data.join(CACHE_DIR).join(offer.entries[0].hash.to_string());
        let fetch = |library: &mut Library, offer: &Offer| {
            let entry = &offer.entries[0];
            if library.missing(&offer.entries).unwrap().is_empty() {
                return false;
            }
            library
                .receive(entry, (**offer.package(entry.hash).unwrap()).clone())
                .unwrap();
            true
        };
        // Room for two.
        let limit = 2 * 1200;
        let mut library = Library::empty(Some(&data));
        library.cache_limit = limit;
        assert!(fetch(&mut library, &mods[0]));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(fetch(&mut library, &mods[1]));
        std::thread::sleep(std::time::Duration::from_millis(20));
        // The first is used again: now the second is the oldest.
        let mut again = Library::empty(Some(&data));
        again.cache_limit = limit;
        assert!(!fetch(&mut again, &mods[0]));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(fetch(&mut again, &mods[2]));
        assert!(folder(&mods[0]).is_dir());
        assert!(!folder(&mods[1]).exists(), "the one used longest ago went");
        assert!(folder(&mods[2]).is_dir());

        // Ones in use stay, even over the limit.
        let mut tight = Library::empty(Some(&data));
        tight.cache_limit = 0;
        assert!(fetch(&mut tight, &mods[1]));
        assert!(folder(&mods[1]).is_dir());
        assert!(!folder(&mods[0]).exists() && !folder(&mods[2]).exists());
    }
}
