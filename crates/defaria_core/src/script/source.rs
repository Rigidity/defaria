//! Which scripts the mods have: their ids, files and text, gathered from
//! the mod folders the content loader reads (`content::load`).

use std::{collections::BTreeMap, path::Path};

use crate::content::{
    self,
    load::{self, LoadedMod},
};

/// The folder in a mod that scripts go in.
pub const SCRIPTS_DIR: &str = "scripts";
/// Scripts' file extension.
pub const EXTENSION: &str = "luau";

/// One script file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `mod:name`: `scripts/machines/furnace.luau` in mod `m` is
    /// `m:machines/furnace`.
    pub id: String,
    /// The mod it's in.
    pub mod_id: String,
    /// Its asset path (`mods/<folder>/scripts/<name>.luau`), which errors
    /// name it by and hot reloading watches.
    pub file: String,
    pub text: String,
    /// Whether it was read from a mod folder on disk (so hot reloading
    /// can watch it), rather than made in memory or sent by a server.
    pub on_disk: bool,
}

/// A mod that scripts can run for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModScripts {
    pub id: String,
    /// The mods its scripts may `require` from, besides its own.
    pub dependencies: Vec<String>,
    /// Its scripts' ids, in path order.
    pub scripts: Vec<String>,
}

/// Every mod's scripts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptSources {
    /// In load order: each after its dependencies.
    pub mods: Vec<ModScripts>,
    pub sources: BTreeMap<String, Source>,
    /// Files that couldn't be scripts, one message each.
    pub errors: Vec<String>,
}

impl ScriptSources {
    /// The scripts of mods already read and ordered.
    pub fn from_mods(mods: &[LoadedMod]) -> Self {
        let mut sources = Self::default();
        for loaded in mods {
            let mod_id = &loaded.manifest.id;
            let mut scripts = Vec::new();
            for (path, text) in &loaded.files.scripts {
                let file = format!("{}/{path}", loaded.files.dir);
                let name = path
                    .strip_prefix(&format!("{SCRIPTS_DIR}/"))
                    .and_then(|name| name.strip_suffix(&format!(".{EXTENSION}")));
                let Some(name) = name else {
                    sources.errors.push(format!(
                        "{file}: scripts go in the mod's `{SCRIPTS_DIR}/` folder"
                    ));
                    continue;
                };
                let id = match content::qualify(name, mod_id) {
                    Ok(id) => id,
                    Err(error) => {
                        sources.errors.push(format!("{file}: the name {error}"));
                        continue;
                    }
                };
                scripts.push(id.clone());
                sources.sources.insert(
                    id.clone(),
                    Source {
                        id,
                        mod_id: mod_id.clone(),
                        file,
                        text: text.clone(),
                        on_disk: loaded.files.folder.is_some(),
                    },
                );
            }
            sources.mods.push(ModScripts {
                id: mod_id.clone(),
                dependencies: loaded.manifest.dependencies.clone(),
                scripts,
            });
        }
        sources
    }

    /// Reads the scripts of the mods in `root` (the `assets/mods` folder),
    /// keeping only the mods named in `loaded` (the ones the content
    /// loaded). The folders' other problems are the content loader's to
    /// report, so they're left out here.
    pub fn read(root: &Path, loaded: &[String]) -> Self {
        let (mods, _) = load::read_mods(root, content::MODS_DIR);
        let mut ordered = load::order(mods, &mut Vec::new());
        ordered.retain(|m| loaded.contains(&m.manifest.id));
        Self::from_mods(&ordered)
    }

    pub fn get(&self, id: &str) -> Option<&Source> {
        self.sources.get(id)
    }

    /// Replaces the text of the script whose asset path is `file`. False
    /// if there's no such script.
    pub fn set_text(&mut self, file: &str, text: String) -> bool {
        match self.sources.values_mut().find(|source| source.file == file) {
            Some(source) => {
                source.text = text;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load::ModFiles;

    fn sources(mods: Vec<ModFiles>) -> ScriptSources {
        let mut errors = Vec::new();
        let ordered = load::order(mods, &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        ScriptSources::from_mods(&ordered)
    }

    #[test]
    fn scripts_are_named_by_mod_and_path() {
        let sources = sources(vec![ModFiles::new(
            "mods/folder",
            "{ id: \"m\", name: \"M\" }",
            &[
                ("scripts/furnace.luau", "-- a"),
                ("scripts/machines/pump.luau", "-- b"),
                ("blocks/x.json5", "{}"),
            ],
        )]);
        assert!(sources.errors.is_empty(), "{:?}", sources.errors);
        assert_eq!(sources.mods[0].scripts, ["m:furnace", "m:machines/pump"]);
        let pump = sources.get("m:machines/pump").unwrap();
        assert_eq!(pump.file, "mods/folder/scripts/machines/pump.luau");
        assert_eq!(pump.mod_id, "m");
        assert_eq!(pump.text, "-- b");
    }

    #[test]
    fn scripts_outside_the_folder_or_badly_named_are_reported() {
        let sources = sources(vec![ModFiles::new(
            "mods/m",
            "{ id: \"m\", name: \"M\" }",
            &[("loose.luau", ""), ("scripts/Bad Name.luau", "")],
        )]);
        assert!(sources.sources.is_empty());
        let all = sources.errors.join("\n");
        assert!(all.contains("mods/m/loose.luau"), "{all}");
        assert!(all.contains("mods/m/scripts/Bad Name.luau"), "{all}");
    }

    #[test]
    fn mods_keep_their_load_order_and_dependencies() {
        let sources = sources(vec![
            ModFiles::new(
                "mods/b",
                "{ id: \"b\", name: \"B\", dependencies: [\"a\"] }",
                &[],
            ),
            ModFiles::new("mods/a", "{ id: \"a\", name: \"A\" }", &[]),
        ]);
        let ids: Vec<_> = sources.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(sources.mods[1].dependencies, ["a"]);
    }

    #[test]
    fn hot_reload_replaces_a_scripts_text_by_file() {
        let mut sources = sources(vec![ModFiles::new(
            "mods/m",
            "{ id: \"m\", name: \"M\" }",
            &[("scripts/a.luau", "old")],
        )]);
        assert!(sources.set_text("mods/m/scripts/a.luau", "new".into()));
        assert!(!sources.set_text("mods/m/scripts/nope.luau", "x".into()));
        assert_eq!(sources.get("m:a").unwrap().text, "new");
    }
}
