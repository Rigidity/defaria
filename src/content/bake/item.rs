//! Items: each block's, and those from their own files.

use std::collections::BTreeMap;

use bevy::platform::collections::HashMap;

use super::{
    Def, Defs,
    categories::category,
    fill::{fill_name, in_state, title_case},
    mix::{Attributes, Numbers},
    property::{check_count, combinations, properties},
    texture::Textures,
};
use crate::content::{
    Block, BlockState, Content, FULL_FLUID, Item, ItemStateInfo, Property, State, qualify,
    schema::{BlockDef, FluidContainerDef, ItemAction, ItemDef},
};

/// Gives each block an item, unless it opts out, then adds the item files.
/// An item file with a block's id changes that block's item.
pub(super) fn add_items(
    defs: &Defs,
    content: &mut Content,
    textures: &Textures,
    report: &mut impl FnMut(&str, Result<(), String>),
) {
    let overrides: HashMap<&str, &Def<ItemDef>> = defs
        .items
        .iter()
        .map(|def| (def.id.as_str(), def))
        .collect();
    for def in &defs.blocks {
        if !def.def.item || content.block(&def.id).is_none() {
            continue;
        }
        let file = overrides.get(def.id.as_str()).copied();
        let result = block_item(def, file, content, textures);
        // A mistake in the item file is that file's.
        report(file.map_or(&def.file, |file| &file.file), result);
    }
    for def in &defs.items {
        let is_block_item = defs
            .blocks
            .iter()
            .any(|block| block.id == def.id && block.def.item);
        if !is_block_item {
            report(&def.file, standalone_item(def, content, textures));
        } else if content.block(&def.id).is_none() {
            report(
                &def.file,
                Err(format!(
                    "block `{}` didn't load, so it has no item to change",
                    def.id
                )),
            );
        }
    }
}

/// Adds the item that places block `id`: one state per combination of the
/// block's item properties, each placing the block with those values and
/// the rest at their defaults. `file` is an item file changing it.
fn block_item(
    block_def: &Def<BlockDef>,
    file: Option<&Def<ItemDef>>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let id = block_def.id.as_str();
    let block = content.block(id).unwrap().clone();
    let category = match file.and_then(|file| file.def.category.as_ref()) {
        Some(reference) => category(Some(reference), &file.unwrap().namespace, content)?,
        None => category(
            block_def.def.category.as_ref(),
            &block_def.namespace,
            content,
        )?,
    };
    if let Some(file) = file {
        if !file.def.properties.is_empty() {
            return Err("a block's item has the block's properties; it can't add its own".into());
        }
        if file.def.block.is_some() {
            return Err("a block's item places that block; leave out `block`".into());
        }
    }
    let properties: Vec<Property> = block
        .properties
        .iter()
        .filter(|p| p.item)
        .cloned()
        .collect();
    let count = check_count(&properties)?;
    let no_stats = BTreeMap::new();
    let attributes = Attributes::new(
        file.and_then(|f| f.def.hardness.as_ref()),
        file.and_then(|f| f.def.weight.as_ref()),
        file.map_or(&no_stats, |f| &f.def.stats),
        &properties,
        Some(&block.properties),
        content,
    )?;
    let item = content.items.len();
    let mut states = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let info = (|| {
            let state = placed_state(&block, &values)?;
            let placed = content.state(state);
            let numbers = attributes.values(&placed_numbers(placed), &properties, &index, content);
            let mut info = ItemStateInfo {
                item,
                values: values.clone(),
                name: placed.name.clone(),
                texture: None,
                block: Some(state),
                hardness: numbers.hardness,
                weight: numbers.weight,
                stats: numbers.stats,
            };
            if let Some(file) = file {
                if let Some(name) = &file.def.name {
                    info.name = fill_name(name, &properties, &index, content)?;
                }
                if let Some(texture) = &file.def.texture {
                    info.texture =
                        Some(textures.bake_ref(texture, &values, &file.namespace, content)?);
                }
            }
            Ok::<_, String>(info)
        })()
        .map_err(|error| in_state(&values, error))?;
        states.push(info);
    }
    let unique = file.and_then(|file| file.def.unique).unwrap_or(false);
    let script = file
        .and_then(|file| Some((file.def.script.as_ref()?, &file.namespace)))
        .map(|(script, namespace)| qualify(script, namespace))
        .transpose()
        .map_err(|error| format!("`script`: {error}"))?;
    let action = file.and_then(|file| file.def.action);
    let fluid_capacity = fluid_capacity(file.and_then(|file| file.def.fluid_container))?;
    push_item(
        id,
        properties,
        unique,
        action,
        category,
        script,
        fluid_capacity,
        states,
        content,
    );
    Ok(())
}

/// Adds an item from its own file.
fn standalone_item(
    def: &Def<ItemDef>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let item_def = &def.def;
    let category = category(item_def.category.as_ref(), &def.namespace, content)?;
    let properties = properties(&item_def.properties, &def.namespace, content, false)?;
    let count = check_count(&properties)?;
    let block = match &item_def.block {
        Some(block) => {
            let id = qualify(block, &def.namespace)?;
            Some(
                content
                    .block(&id)
                    .ok_or_else(|| format!("no block `{id}`"))?
                    .clone(),
            )
        }
        None => None,
    };
    let attributes = Attributes::new(
        item_def.hardness.as_ref(),
        item_def.weight.as_ref(),
        &item_def.stats,
        &properties,
        None,
        content,
    )?;
    let item = content.items.len();
    let mut states = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let info = (|| {
            let name = match &item_def.name {
                Some(name) => fill_name(name, &properties, &index, content)?,
                None => title_case(&def.id),
            };
            let texture = match &item_def.texture {
                Some(texture) => {
                    Some(textures.bake_ref(texture, &values, &def.namespace, content)?)
                }
                None => None,
            };
            let block = match &block {
                Some(block) => Some(placed_state(block, &values)?),
                None => None,
            };
            // Left out, it's as hard and heavy as the block it places, with
            // its stats.
            let fallback = block.map_or_else(Numbers::default, |state| {
                placed_numbers(content.state(state))
            });
            let numbers = attributes.values(&fallback, &properties, &index, content);
            Ok::<_, String>(ItemStateInfo {
                item,
                values: values.clone(),
                name,
                texture,
                block,
                hardness: numbers.hardness,
                weight: numbers.weight,
                stats: numbers.stats,
            })
        })()
        .map_err(|error| in_state(&values, error))?;
        states.push(info);
    }
    let unique = item_def.unique.unwrap_or(false);
    let script = item_def
        .script
        .as_ref()
        .map(|script| qualify(script, &def.namespace))
        .transpose()
        .map_err(|error| format!("`script`: {error}"))?;
    push_item(
        &def.id,
        properties,
        unique,
        item_def.action,
        category,
        script,
        fluid_capacity(item_def.fluid_container)?,
        states,
        content,
    );
    Ok(())
}

/// The numbers of a block state, which an item placing it falls back to.
fn placed_numbers(state: &State) -> Numbers {
    Numbers {
        hardness: state.hardness,
        weight: state.weight,
        stats: state.stats.clone(),
    }
}

/// The state of `block` an item with these values places: its default,
/// with each property the item shares set to the item's value.
fn placed_state(block: &Block, values: &[(String, String)]) -> Result<BlockState, String> {
    let shared: Vec<(&str, &str)> = values
        .iter()
        .filter(|(name, _)| block.properties.iter().any(|p| &p.name == name))
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    block.state(&shared).ok_or_else(|| {
        let (name, value) = shared
            .iter()
            .find(|(n, v)| block.state(&[(n, v)]).is_none())
            .copied()
            .unwrap_or_default();
        format!("block `{}` has no `{name}` value `{value}`", block.id)
    })
}

/// A `fluid_container`'s capacity in [`FULL_FLUID`] units: at least one.
fn fluid_capacity(def: Option<FluidContainerDef>) -> Result<Option<u64>, String> {
    let Some(def) = def else {
        return Ok(None);
    };
    let units = (def.capacity * FULL_FLUID as f32).round();
    if !units.is_finite() || units < 1.0 {
        return Err(format!(
            "`fluid_container.capacity` must hold some fluid, not {}",
            def.capacity
        ));
    }
    Ok(Some(units as u64))
}

fn push_item(
    id: &str,
    properties: Vec<Property>,
    unique: bool,
    action: Option<ItemAction>,
    category: Option<usize>,
    script: Option<String>,
    fluid_capacity: Option<u64>,
    states: Vec<ItemStateInfo>,
    content: &mut Content,
) {
    content.item_index.insert(id.into(), content.items.len());
    content.items.push(Item {
        id: id.into(),
        properties,
        first_state: content.item_states.len() as u32,
        state_count: states.len() as u32,
        unique,
        category,
        action,
        script,
        fluid_capacity,
    });
    content.item_states.extend(states);
}

#[cfg(test)]
mod tests;
