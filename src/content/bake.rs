//! Parses the definition files and resolves the references between them,
//! in dependency order: categories, network kinds, materials, textures,
//! models, blocks, items, recipes.
//!
//! Each definition that fails is reported once, naming its file, and left
//! out; definitions referring to it then fail with "no such …".
//!
//! Textures with `$inputs` are templates: each combination of materials a
//! block state or item uses is baked into its own tile, once, when the
//! first user asks for it. Textures with `#tag` inputs are baked for every
//! combination of tagged materials as they load.

mod block;
mod categories;
mod conduit;
mod fill;
mod fluid;
mod item;
mod look;
mod material;
mod mix;
mod model;
mod parse;
mod parts;
mod property;
mod state;
mod texture;
mod when;

#[cfg(test)]
mod test_util;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use bevy::{platform::collections::HashMap, prelude::*};

use super::{
    AIR, Block, Content, FULL_FLUID, ModInfo, State,
    load::LoadedMod,
    machine::{check_machine_items, check_tank_fluids},
    recipe::bake_recipe,
    schema::*,
    support,
};
use block::add_block;
use categories::add_categories;
use conduit::add_networks;
use item::add_items;
use material::material;
use model::model;
use parse::parse_file;
use texture::{Textures, template};

pub(crate) use fill::title_case;
pub(super) use mix::Mix;
pub(super) use model::rotation;
pub(super) use when::{When, pattern_matches};

/// A parsed definition and where it came from.
struct Def<T> {
    id: String,
    /// The mod's namespace, for its references.
    namespace: String,
    /// The file's asset path, for errors.
    file: String,
    def: T,
}

#[derive(Default)]
struct Defs {
    categories: Vec<Def<CategoryDef>>,
    networks: Vec<Def<NetworkDef>>,
    materials: Vec<Def<MaterialDef>>,
    textures: Vec<Def<TextureDef>>,
    models: Vec<Def<ModelDef>>,
    blocks: Vec<Def<BlockDef>>,
    items: Vec<Def<ItemDef>>,
    recipes: Vec<Def<RecipeDef>>,
}

pub fn bake(mods: Vec<LoadedMod>, errors: &mut Vec<String>) -> Content {
    let mut defs = Defs::default();
    // Every image, by asset path, and each mod's folder by namespace.
    let mut images = BTreeSet::new();
    let mut dirs = HashMap::new();
    for loaded in &mods {
        let dir = &loaded.files.dir;
        dirs.insert(loaded.manifest.id.clone(), dir.clone());
        images.extend(
            loaded
                .files
                .images
                .iter()
                .map(|path| format!("{dir}/{path}")),
        );
        for (path, text) in &loaded.files.files {
            if let Err(error) = parse_file(&loaded.manifest.id, dir, path, text, &mut defs) {
                errors.push(format!("{dir}/{path}: {error}"));
            }
        }
    }

    let mut content = Content {
        mods: mods
            .into_iter()
            .map(|loaded| ModInfo {
                id: loaded.manifest.id,
                name: loaded.manifest.name,
                version: loaded.manifest.version,
                description: loaded.manifest.description,
                authors: loaded.manifest.authors,
                dir: loaded.files.dir,
            })
            .collect(),
        materials: BTreeMap::new(),
        textures: Vec::new(),
        models: BTreeMap::new(),
        blocks: vec![Block {
            id: AIR.into(),
            properties: Vec::new(),
            first_state: 0,
            state_count: 1,
            connections: None,
            block_entity: None,
            script: None,
            support: None,
        }],
        states: vec![State {
            name: "Air".into(),
            fluid_room: FULL_FLUID,
            ..default()
        }],
        items: Vec::new(),
        item_states: Vec::new(),
        categories: Vec::new(),
        networks: Vec::new(),
        recipes: Vec::new(),
        errors: Vec::new(),
        category_index: HashMap::new(),
        network_index: HashMap::new(),
        texture_index: HashMap::new(),
        block_index: HashMap::new(),
        item_index: HashMap::new(),
        recipe_index: HashMap::new(),
        recipe_types: HashMap::new(),
        recipe_items: HashMap::new(),
    };
    content.block_index.insert(AIR.into(), 0);

    let mut report = |file: &str, result: Result<(), String>| {
        if let Err(error) = result {
            errors.push(format!("{file}: {error}"));
        }
    };
    add_categories(&defs.categories, &mut content);
    add_networks(&defs.networks, &mut content);
    for def in &defs.materials {
        let result = material(def).map(|material| {
            content.materials.insert(def.id.clone(), material);
        });
        report(&def.file, result);
    }
    let mut textures = Textures {
        images,
        dirs,
        ..default()
    };
    for def in &defs.textures {
        match template(def, &content, &textures) {
            Ok((template, problems)) => {
                for problem in problems {
                    report(&def.file, Err(problem));
                }
                // Textures without inputs, and those with `#tag` inputs in
                // every combination, are baked now, so every one is checked
                // and has a tile, used or not.
                textures.templates.insert(def.id.clone(), template);
                match textures.bake_all(&def.id, &mut content) {
                    Ok(missing) => {
                        for image in missing {
                            report(
                                &def.file,
                                Err(format!(
                                    "there's no {image}; the tiles that use it show a placeholder"
                                )),
                            );
                        }
                    }
                    Err(error) => {
                        textures.templates.remove(&def.id);
                        report(&def.file, Err(error));
                    }
                }
            }
            Err(error) => report(&def.file, Err(error)),
        }
    }
    for def in &defs.models {
        let result = model(def).map(|model| {
            content.models.insert(def.id.clone(), model);
        });
        report(&def.file, result);
    }
    for def in &defs.blocks {
        report(&def.file, add_block(def, &mut content, &textures));
    }
    for def in &defs.blocks {
        if let Some(machine) = &def.def.machine
            && content.block(&def.id).is_some()
        {
            report(
                &def.file,
                check_tank_fluids(machine, &def.namespace, &content),
            );
        }
    }
    for def in &defs.blocks {
        if let Some(rules) = &def.def.support
            && let Some(&index) = content.block_index.get(&def.id)
        {
            report(
                &def.file,
                support::bake(index, rules, &def.namespace, &mut content),
            );
        }
    }
    add_items(&defs, &mut content, &textures, &mut report);
    for def in &defs.recipes {
        match bake_recipe(&def.id, &def.namespace, &def.def, &content) {
            Ok(recipes) => content.add_recipes(recipes),
            Err(error) => report(&def.file, Err(error)),
        }
    }
    for def in &defs.blocks {
        if let Some(machine) = &def.def.machine
            && content.block(&def.id).is_some()
        {
            report(
                &def.file,
                check_machine_items(machine, &def.namespace, &content),
            );
        }
    }
    content
}
