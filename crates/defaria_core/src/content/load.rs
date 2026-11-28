//! Reading mod folders and putting the mods in load order.
//!
//! Reading is separate from everything else so tests can build mods in
//! memory: [`ModFiles`] is a mod folder's contents, and [`read_mods`] is the
//! only part that touches the disk.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::schema::{Manifest, ModSide};

/// The manifest's file name, at the root of every mod folder.
pub const MANIFEST: &str = "mod.json5";

/// One mod folder's contents.
#[derive(Debug, Clone, Default)]
pub struct ModFiles {
    /// The folder's asset path, relative to `assets/`, such as
    /// `mods/defaria`. Images load from under it, and errors name files by
    /// it.
    pub dir: String,
    /// `mod.json5`'s text.
    pub manifest: String,
    /// Every `.json5` file other than the manifest, by its path in the
    /// folder (`/`-separated), with its text.
    pub files: BTreeMap<String, String>,
    /// Every `.png` file's path in the folder.
    pub images: BTreeSet<String>,
    /// The bytes of the images that are in memory rather than on disk (a
    /// mod a server sent, or one made in memory), by path in the folder.
    /// The others load from [`ModFiles::folder`] through the asset server.
    pub image_bytes: BTreeMap<String, Arc<[u8]>>,
    /// Every `.luau` file, by its path in the folder, with its text. They're
    /// mod scripts (`src/script.rs`, `docs/scripting.md`).
    pub scripts: BTreeMap<String, String>,
    /// The folder on disk it was read from, if it was.
    pub folder: Option<PathBuf>,
}

impl ModFiles {
    /// An in-memory mod with `files` (path, text). `.png` paths become
    /// images, their text ignored, and `.luau` paths scripts.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(dir: &str, manifest: &str, files: &[(&str, &str)]) -> Self {
        let mut mod_files = Self {
            dir: dir.into(),
            manifest: manifest.into(),
            ..Default::default()
        };
        for &(path, text) in files {
            if path.ends_with(".png") {
                mod_files.images.insert(path.into());
                mod_files
                    .image_bytes
                    .insert(path.into(), Arc::from(text.as_bytes()));
            } else if path.ends_with(".luau") {
                mod_files.scripts.insert(path.into(), text.into());
            } else {
                mod_files.files.insert(path.into(), text.into());
            }
        }
        mod_files
    }
}

/// Reads every mod folder in `root`, which is the asset folder `asset_dir`.
/// A folder that can't be read, or has no manifest, is reported and
/// skipped.
pub fn read_mods(root: &Path, asset_dir: &str) -> (Vec<ModFiles>, Vec<String>) {
    let mut mods = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("{asset_dir}: can't read the mods folder: {error}"));
            return (mods, errors);
        }
    };
    let mut folders: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    for folder in folders {
        let name = folder.file_name().unwrap_or_default().to_string_lossy();
        let dir = format!("{asset_dir}/{name}");
        match read_mod(&folder, dir.clone()) {
            Ok(mod_files) => mods.push(mod_files),
            Err(error) => errors.push(format!("{dir}: {error}")),
        }
    }
    (mods, errors)
}

fn read_mod(folder: &Path, dir: String) -> Result<ModFiles, String> {
    let manifest = fs::read_to_string(folder.join(MANIFEST))
        .map_err(|error| format!("can't read {MANIFEST}: {error}"))?;
    let mut mod_files = ModFiles {
        dir,
        manifest,
        folder: Some(folder.to_path_buf()),
        ..Default::default()
    };
    walk(folder, "", &mut mod_files)?;
    Ok(mod_files)
}

/// Collects the definition files, images and scripts under `folder`, which is at
/// `prefix` within the mod.
fn walk(folder: &Path, prefix: &str, mod_files: &mut ModFiles) -> Result<(), String> {
    let entries = fs::read_dir(folder).map_err(|error| format!("{prefix}: {error}"))?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}{name}");
        if path.is_dir() {
            walk(&path, &format!("{relative}/"), mod_files)?;
        } else if relative == MANIFEST {
            continue;
        } else if name.ends_with(".json5") {
            let text = fs::read_to_string(&path).map_err(|error| format!("{relative}: {error}"))?;
            mod_files.files.insert(relative, text);
        } else if name.ends_with(".png") {
            mod_files.images.insert(relative);
        } else if name.ends_with(".luau") {
            let text = fs::read_to_string(&path).map_err(|error| format!("{relative}: {error}"))?;
            mod_files.scripts.insert(relative, text);
        }
    }
    Ok(())
}

/// A mod whose manifest parsed.
#[derive(Debug, Clone)]
pub struct LoadedMod {
    pub manifest: Manifest,
    pub files: ModFiles,
}

/// Parses the manifests and puts the mods in load order: each after its
/// dependencies, otherwise by id. Mods with a bad manifest, a clashing id,
/// a dependency that's missing, skipped, circular or for another side, or
/// that break their side's rules ([`check_side`]), are reported and left
/// out.
pub fn order(mods: Vec<ModFiles>, errors: &mut Vec<String>) -> Vec<LoadedMod> {
    let mut by_id: BTreeMap<String, LoadedMod> = BTreeMap::new();
    for files in mods {
        let manifest: Manifest = match json5::from_str(&files.manifest) {
            Ok(manifest) => manifest,
            Err(error) => {
                errors.push(format!("{}/{MANIFEST}: {error}", files.dir));
                continue;
            }
        };
        if let Err(error) = super::check_namespace(&manifest.id) {
            errors.push(format!("{}/{MANIFEST}: id {error}", files.dir));
            continue;
        }
        if let Err(error) = check_side(&manifest, &files) {
            errors.push(format!("{}: not loaded: {error}", files.dir));
            continue;
        }
        if let Some(other) = by_id.get(&manifest.id) {
            errors.push(format!(
                "{}: the id `{}` is already taken by {}",
                files.dir, manifest.id, other.files.dir
            ));
            continue;
        }
        by_id.insert(manifest.id.clone(), LoadedMod { manifest, files });
    }

    // A mod that depends on one for another side would load where its
    // dependency doesn't: a `both` mod on a client without the server's
    // `server` mods, say.
    let crossed: Vec<(String, String)> = by_id
        .values()
        .filter_map(|m| {
            let side = m.manifest.side;
            let dep = m.manifest.dependencies.iter().find(|dep| {
                by_id
                    .get(*dep)
                    .is_some_and(|d| !side.may_depend_on(d.manifest.side))
            })?;
            Some((m.manifest.id.clone(), dep.clone()))
        })
        .collect();
    for (id, dep) in crossed {
        let Some(m) = by_id.remove(&id) else { continue };
        let theirs = by_id.get(&dep).map_or("other", |d| d.manifest.side.name());
        errors.push(format!(
            "{}: not loaded: a `{}` mod can't depend on `{dep}`, a `{theirs}` mod",
            m.files.dir,
            m.manifest.side.name(),
        ));
    }

    // Repeatedly take, in id order, the mods whose dependencies have all
    // been taken. Whatever's left at the end can never load.
    let mut ordered: Vec<LoadedMod> = Vec::new();
    loop {
        let ready: Vec<String> = by_id
            .values()
            .filter(|m| {
                m.manifest
                    .dependencies
                    .iter()
                    .all(|dep| ordered.iter().any(|o| &o.manifest.id == dep))
            })
            .map(|m| m.manifest.id.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for id in ready {
            ordered.extend(by_id.remove(&id));
        }
    }
    for stuck in by_id.values() {
        let missing: Vec<&str> = stuck
            .manifest
            .dependencies
            .iter()
            .filter(|dep| !ordered.iter().any(|o| &o.manifest.id == *dep))
            .map(String::as_str)
            .collect();
        errors.push(format!(
            "{}: not loaded, because these dependencies didn't load (missing, broken or circular): {}",
            stuck.files.dir,
            missing.join(", ")
        ));
    }
    ordered
}

/// Checks a mod keeps to its side's rules. A `server` or `client` mod
/// holds only scripts: a definition (or an image, which only definitions
/// use) would change what the two sides know, and a client never has a
/// server's `server` mods, nor a server its clients' `client` ones. What
/// the mods define is the `both` mods' alone, so both sides number every
/// block and item state the same.
pub fn check_side(manifest: &Manifest, files: &ModFiles) -> Result<(), String> {
    if manifest.side == ModSide::Both {
        return Ok(());
    }
    let defined = files.files.keys().chain(&files.images).next();
    match defined {
        Some(path) => Err(format!(
            "a `{}` mod holds only scripts, but it has {path}: what a mod defines must \
             be the same on the server and its clients, so it belongs in a `both` mod",
            manifest.side.name()
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, deps: &[&str]) -> String {
        let deps: Vec<String> = deps.iter().map(|d| format!("\"{d}\"")).collect();
        format!(
            "{{ id: \"{id}\", name: \"{id}\", dependencies: [{}] }}",
            deps.join(", ")
        )
    }

    fn ids(mods: &[LoadedMod]) -> Vec<&str> {
        mods.iter().map(|m| m.manifest.id.as_str()).collect()
    }

    #[test]
    fn mods_load_after_their_dependencies_then_by_id() {
        let mods = vec![
            ModFiles::new("mods/a", &manifest("zebra", &[]), &[]),
            ModFiles::new("mods/b", &manifest("apple", &["mango"]), &[]),
            ModFiles::new("mods/c", &manifest("mango", &["zebra"]), &[]),
            ModFiles::new("mods/d", &manifest("kiwi", &[]), &[]),
        ];
        let mut errors = Vec::new();
        let ordered = order(mods, &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(ids(&ordered), ["kiwi", "zebra", "mango", "apple"]);
    }

    #[test]
    fn missing_and_circular_dependencies_keep_a_mod_out() {
        let mods = vec![
            ModFiles::new("mods/a", &manifest("a", &["nowhere"]), &[]),
            ModFiles::new("mods/b", &manifest("b", &["c"]), &[]),
            ModFiles::new("mods/c", &manifest("c", &["b"]), &[]),
            // Depends on a mod that can't load, so can't load either.
            ModFiles::new("mods/d", &manifest("d", &["a"]), &[]),
            ModFiles::new("mods/e", &manifest("e", &[]), &[]),
        ];
        let mut errors = Vec::new();
        let ordered = order(mods, &mut errors);
        assert_eq!(ids(&ordered), ["e"]);
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors[0].contains("mods/a") && errors[0].contains("nowhere"));
    }

    #[test]
    fn bad_manifests_and_clashing_ids_are_reported() {
        let mods = vec![
            ModFiles::new("mods/one", &manifest("same", &[]), &[]),
            ModFiles::new("mods/two", &manifest("same", &[]), &[]),
            ModFiles::new("mods/three", "{ id: \"Bad Id\", name: \"x\" }", &[]),
            ModFiles::new("mods/four", "{ id: \"four\" }", &[]),
            ModFiles::new("mods/five", "{ id: \"five\", name: \"x\", nmae: 1 }", &[]),
        ];
        let mut errors = Vec::new();
        let ordered = order(mods, &mut errors);
        assert_eq!(ids(&ordered), ["same"]);
        assert_eq!(ordered[0].files.dir, "mods/one");
        let all = errors.join("\n");
        assert!(
            all.contains("mods/two") && all.contains("already taken"),
            "{all}"
        );
        assert!(all.contains("mods/three/mod.json5"), "{all}");
        assert!(
            all.contains("mods/four/mod.json5") && all.contains("name"),
            "{all}"
        );
        assert!(
            all.contains("mods/five/mod.json5") && all.contains("nmae"),
            "{all}"
        );
    }

    /// A `server` or `client` mod with a definition or an image isn't
    /// loaded; one with only scripts is.
    #[test]
    fn server_and_client_mods_hold_only_scripts() {
        let mods = vec![
            ModFiles::new(
                "mods/admin",
                r#"{ id: "admin", name: "x", side: "server" }"#,
                &[("scripts/kick.luau", "")],
            ),
            ModFiles::new(
                "mods/ore",
                r#"{ id: "ore", name: "x", side: "server" }"#,
                &[("blocks/ore.json5", "{}")],
            ),
            ModFiles::new(
                "mods/look",
                r#"{ id: "look", name: "x", side: "client" }"#,
                &[("images/a.png", "")],
            ),
            ModFiles::new(
                "mods/hud",
                r#"{ id: "hud", name: "x", side: "client" }"#,
                &[("scripts/hud.luau", "")],
            ),
            ModFiles::new("mods/odd", r#"{ id: "odd", name: "x", side: "left" }"#, &[]),
        ];
        let mut errors = Vec::new();
        let ordered = order(mods, &mut errors);
        assert_eq!(ids(&ordered), ["admin", "hud"]);
        assert_eq!(ordered[0].manifest.side, ModSide::Server);
        let all = errors.join("\n");
        assert!(
            all.contains("mods/ore") && all.contains("blocks/ore.json5"),
            "{all}"
        );
        assert!(
            all.contains("mods/look") && all.contains("images/a.png"),
            "{all}"
        );
        assert!(all.contains("mods/odd/mod.json5"), "{all}");
    }

    /// A `both` mod can't depend on a `server` or `client` mod (it would
    /// load where they don't), nor a `client` mod on a `server` one; any
    /// may depend on a `both` mod, and a side's mods on each other.
    #[test]
    fn mods_depend_only_on_mods_loaded_where_they_are() {
        let mod_on = |id: &str, side: &str, deps: &str| {
            ModFiles::new(
                &format!("mods/{id}"),
                &format!(r#"{{ id: "{id}", name: "x", side: "{side}", dependencies: [{deps}] }}"#),
                &[],
            )
        };
        let mods = vec![
            mod_on("base", "both", ""),
            mod_on("admin", "server", r#""base""#),
            mod_on("more_admin", "server", r#""admin""#),
            mod_on("hud", "client", r#""base""#),
            mod_on("needs_admin", "both", r#""admin""#),
            mod_on("needs_hud", "both", r#""hud""#),
            mod_on("crossed", "client", r#""admin""#),
        ];
        let mut errors = Vec::new();
        let ordered = order(mods, &mut errors);
        assert_eq!(ids(&ordered), ["base", "admin", "hud", "more_admin"]);
        assert_eq!(errors.len(), 3, "{errors:?}");
        let all = errors.join("\n");
        assert!(
            all.contains("mods/needs_admin") && all.contains("`admin`, a `server` mod"),
            "{all}"
        );
        assert!(all.contains("mods/needs_hud"), "{all}");
        assert!(all.contains("mods/crossed"), "{all}");
    }

    /// The disk reader finds the base game's manifest, definition files
    /// and images.
    #[test]
    fn reads_mod_folders_from_disk() {
        let (mods, errors) = read_mods(&crate::content::mods_dir(), "mods");
        assert!(errors.is_empty(), "{errors:?}");
        let base = mods.iter().find(|m| m.dir == "mods/defaria").unwrap();
        assert!(base.manifest.contains("defaria"));
        assert!(base.files.contains_key("blocks/stone.json5"));
        assert!(base.files.contains_key("models/cube.json5"));
        assert!(base.images.contains("images/stone.png"));
        assert!(base.images.contains("images/items/axe_head.png"));
    }
}
