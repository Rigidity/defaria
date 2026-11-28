//! Blocks: checking one, and baking and adding all its states.

use std::collections::BTreeSet;

use bevy::prelude::*;

use super::{
    Def,
    conduit::ConduitMix,
    fill::{fill_name, in_state, title_case},
    fluid::fluid_box,
    look::look,
    mix::{Attributes, Numbers},
    parts::{Part, parts},
    property::{check_count, combinations, connection_rule, properties},
    state::bake_state,
    texture::Textures,
    when::all_whens,
};
use crate::content::{
    Block, Connections, Content,
    machine::MachineMix,
    qualify,
    schema::{BlockDef, Render},
};

/// Checks a block, bakes every one of its states, and adds them all, or
/// nothing if any state fails.
pub(super) fn add_block(
    def: &Def<BlockDef>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let block = &def.def;
    let properties = properties(&block.properties, &def.namespace, content, true)?;
    let connections = connection_rule(&block.properties, &def.namespace, block.conduit.is_some())?
        .and_then(|rule| Connections::new(rule, &properties));
    let conduit = block
        .conduit
        .as_ref()
        .map(|conduit| ConduitMix::new(conduit, &def.namespace, &properties, content))
        .transpose()?;
    let machine = block
        .machine
        .as_ref()
        .map(|machine| MachineMix::new(machine, &def.namespace, &properties, content))
        .transpose()?;
    let script = block
        .script
        .as_ref()
        .map(|script| qualify(script, &def.namespace))
        .transpose()
        .map_err(|error| format!("`script`: {error}"))?;
    let count = check_count(&properties)?;
    if content.states.len() + count > u16::MAX as usize + 1 {
        return Err("out of block states: there are too many blocks".into());
    }
    let whens = all_whens(
        "variant",
        block.variants.iter().map(|variant| &variant.when),
        &properties,
        &def.namespace,
        content,
    )?;
    let part_whens = all_whens(
        "part",
        block.parts.iter().map(|part| &part.when),
        &properties,
        &def.namespace,
        content,
    )?;
    let attributes = Attributes::new(
        block.hardness.as_ref(),
        block.weight.as_ref(),
        &block.stats,
        &properties,
        None,
        content,
    )?;

    // A fluid is drawn as a box of its height, by the engine.
    if block.fluid.is_some() {
        let named = block.model.is_some() || block.variants.iter().any(|v| v.model.is_some());
        if named {
            return Err("a fluid has no `model`: it's drawn as a box as high as it's full".into());
        }
        if block.rotate.is_some() || block.variants.iter().any(|v| v.rotate.is_some()) {
            return Err("a fluid can't be rotated".into());
        }
        if !block.parts.is_empty() {
            return Err("a fluid has no `parts`: it's drawn as a box as high as it's full".into());
        }
    }

    // Each state's property values, look and parts, last property fastest.
    let mut looks = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let look = look(block, &whens, &index);
        let parts = if block.fluid.is_some() {
            vec![Part {
                model: fluid_box().clone(),
                rotate: None,
                textures: None,
                next: None,
                reaches: None,
            }]
        } else {
            parts(
                block,
                &look,
                &part_whens,
                &index,
                &values,
                &def.namespace,
                content,
            )
            .map_err(|error| in_state(&values, error))?
        };
        looks.push((index, values, look, parts));
    }
    // A slot is skipped where the state's models don't have it, so a
    // variant can switch to a model with other slots; a slot no model has
    // is a mistake.
    let slots: BTreeSet<&str> = looks
        .iter()
        .flat_map(|(_, _, _, parts)| parts.iter())
        .flat_map(|part| part.model.slots.keys().map(String::as_str))
        .collect();
    let named = block
        .textures
        .keys()
        .chain(block.variants.iter().flat_map(|v| v.textures.keys()));
    if let Some(slot) = named
        .into_iter()
        .find(|slot| !slots.contains(slot.as_str()))
    {
        return Err(format!("no model this block uses has a slot `{slot}`"));
    }
    let index = content.blocks.len();
    let first_state = content.states.len() as u16;
    let mut states = Vec::with_capacity(count);
    for (value_index, values, look, parts) in &looks {
        let name = match look.name.or(block.name.as_deref()) {
            Some(name) => fill_name(name, &properties, value_index, content),
            None => Ok(title_case(&def.id)),
        };
        let state = name
            .and_then(|name| {
                // Left out, they're 0.
                let numbers =
                    attributes.values(&Numbers::default(), &properties, value_index, content);
                bake_state(
                    look,
                    parts,
                    values,
                    name,
                    numbers,
                    &def.namespace,
                    index,
                    block,
                    content,
                    textures,
                )
            })
            .and_then(|mut state| {
                state.conduit = conduit
                    .as_ref()
                    .map(|conduit| conduit.value(&properties, value_index, values, content))
                    .transpose()?;
                state.machine = machine
                    .as_ref()
                    .map(|machine| {
                        machine.value(&properties, value_index, look.rotate, first_state, content)
                    })
                    .transpose()?
                    .map(Box::new);
                Ok(state)
            })
            .map_err(|error| in_state(values, error))?;
        states.push(state);
    }

    content.blocks.push(Block {
        id: def.id.clone(),
        properties,
        first_state,
        state_count: count as u16,
        connections,
        // A machine keeps what it holds in its block entity, and a conduit
        // what it carries.
        block_entity: block
            .block_entity
            .clone()
            .or_else(|| (block.machine.is_some() || block.conduit.is_some()).then(default)),
        script,
        // Filled in once every block is in, as it names other blocks.
        support: None,
        next: Vec::new(),
    });
    content.block_index.insert(def.id.clone(), index);
    for state in states.iter().filter(|s| s.render == Render::Cutout) {
        for face in state.faces.iter().chain(&state.next_faces) {
            content.textures[face.layer as usize].cutout = true;
        }
    }
    content.states.extend(states);
    Ok(())
}

#[cfg(test)]
mod tests;
