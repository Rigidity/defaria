//! Reading mod folders and putting the mods in load order.
//!
//! Reading is separate from everything else so tests can build mods in
//! memory: [`ModFiles`] is a mod folder's contents, and [`read_mods`] is the
//! only part that touches the disk.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use super::schema::Manifest;

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
}

impl ModFiles {
    /// An in-memory mod with `files` (path, text). `.png` paths become
    /// images; their text is ignored.
    #[cfg(test)]
    pub fn new(dir: &str, manifest: &str, files: &[(&str, &str)]) -> Self {
        let mut mod_files = Self {
            dir: dir.into(),
            manifest: manifest.into(),
            ..Default::default()
        };
        for &(path, text) in files {
            if path.ends_with(".png") {
                mod_files.images.insert(path.into());
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
        ..Default::default()
    };
    walk(folder, "", &mut mod_files)?;
    Ok(mod_files)
}

/// Collects the definition files and images under `folder`, which is at
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
/// or a dependency that's missing, skipped or circular are reported and
/// left out.
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
        if let Some(other) = by_id.get(&manifest.id) {
            errors.push(format!(
                "{}: the id `{}` is already taken by {}",
                files.dir, manifest.id, other.files.dir
            ));
            continue;
        }
        by_id.insert(manifest.id.clone(), LoadedMod { manifest, files });
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

    /// The disk reader finds the base game's manifest, definition files
    /// and images.
    #[test]
    fn reads_mod_folders_from_disk() {
        let (mods, errors) = read_mods(Path::new("assets/mods"), "mods");
        assert!(errors.is_empty(), "{errors:?}");
        let base = mods.iter().find(|m| m.dir == "mods/defaria").unwrap();
        assert!(base.manifest.contains("defaria"));
        assert!(base.files.contains_key("blocks/stone.json5"));
        assert!(base.files.contains_key("models/cube.json5"));
        assert!(base.images.contains("images/stone.png"));
        assert!(base.images.contains("images/items/axe_head.png"));
    }
}
