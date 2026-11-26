//! A block's or item's properties, and the states their values make.

use std::collections::{BTreeMap, BTreeSet};

use crate::content::{
    Content, NONE, Placement, Property, Rule,
    connect::side_properties,
    index_of, qualify,
    schema::{ConnectionsDef, PropertyDef, PropertySpec},
    state_count,
};

/// Most states one block or item may have.
const MAX_STATES: usize = 4096;

/// Resolves a block's or item's properties, sorted by name. Materials are
/// listed by id, `#tag` by every material with the tag, in order of id.
pub(super) fn properties(
    defs: &BTreeMap<String, PropertyDef>,
    namespace: &str,
    content: &Content,
    for_block: bool,
) -> Result<Vec<Property>, String> {
    let mut out = Vec::new();
    for (name, def) in defs {
        check_state_text("its name", name)
            .map_err(|error| format!("property `{name}`: {error}"))?;
        let made = match def {
            PropertyDef::Full(spec) if spec.connections.is_some() => {
                connection_properties(spec, for_block)
            }
            _ => property(name, def, namespace, content, for_block).map(|p| vec![p]),
        };
        out.extend(made.map_err(|error| format!("property `{name}`: {error}"))?);
    }
    // A connections property's sides sort in among the others.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(pair) = out.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(format!(
            "there are two properties called `{}`: a `connections` property makes `north`, \
             `south`, `east`, `west`, `up` and `down`",
            pair[0].name
        ));
    }
    Ok(out)
}

/// The six side properties a `connections` property becomes.
fn connection_properties(spec: &PropertySpec, for_block: bool) -> Result<Vec<Property>, String> {
    if !for_block {
        return Err("`connections` is only for block properties".into());
    }
    let alone = spec.values.is_none()
        && spec.materials.is_none()
        && !spec.optional
        && spec.default.is_none()
        && spec.item.is_none()
        && spec.place.is_none();
    if !alone {
        return Err("`connections` goes alone: the engine sets its values".into());
    }
    Ok(side_properties().collect())
}

/// What a block's `connections` property connects by: a rule, with its
/// mod, or its conduit. A conduit must connect by its conduit, and only a
/// conduit can.
pub(super) fn connection_rule(
    defs: &BTreeMap<String, PropertyDef>,
    namespace: &str,
    conduit: bool,
) -> Result<Option<Rule>, String> {
    let found = defs.iter().find_map(|(name, def)| match def {
        PropertyDef::Full(spec) => spec.connections.as_ref().map(|rule| (name, rule)),
        PropertyDef::Values(_) => None,
    });
    match (found, conduit) {
        (None, false) => Ok(None),
        (None, true) => Err(
            "a conduit needs a property with `connections: true`, to join \
             the conduits next to it"
                .into(),
        ),
        (Some((_, ConnectionsDef::Conduit)), true) => Ok(Some(Rule::Conduit)),
        (Some((name, ConnectionsDef::Conduit)), false) => Err(format!(
            "property `{name}`: `connections: true` connects by the block's `conduit`, and it \
             has none"
        )),
        (Some((name, ConnectionsDef::Rule(_))), true) => Err(format!(
            "property `{name}`: a conduit connects by its network kind and group: write \
             `connections: true`"
        )),
        (Some((name, ConnectionsDef::Rule(rule))), false) => qualify(rule, namespace)
            .map(|rule| Some(Rule::Named(rule)))
            .map_err(|error| format!("property `{name}`: {error}")),
    }
}

fn property(
    name: &str,
    def: &PropertyDef,
    namespace: &str,
    content: &Content,
    for_block: bool,
) -> Result<Property, String> {
    let mut property = listed_property(name, def, namespace, content, for_block)?;
    let PropertyDef::Full(spec) = def else {
        return Ok(property);
    };
    if let Some(default) = &spec.default {
        let default = if property.materials && default != NONE {
            qualify(default, namespace)?
        } else {
            default.clone()
        };
        let at = property
            .values
            .iter()
            .position(|value| *value == default)
            .ok_or_else(|| format!("the default `{default}` isn't one of its values"))?;
        // The default is the first value: a block's default state has every
        // property at its first.
        let value = property.values.remove(at);
        property.values.insert(0, value);
    }
    // Placement refers to values by position, so it's worked out from the
    // final order.
    if let Some(place) = &spec.place {
        property.place = Some(Placement::new(
            place.from,
            &property.values,
            place.values.as_ref(),
        )?);
    }
    Ok(property)
}

/// A property with its values in the order they're listed, and no
/// placement yet.
fn listed_property(
    name: &str,
    def: &PropertyDef,
    namespace: &str,
    content: &Content,
    for_block: bool,
) -> Result<Property, String> {
    let spec = match def {
        PropertyDef::Values(values) => {
            return plain_property(name, values, false);
        }
        PropertyDef::Full(spec) => spec,
    };
    if spec.item.is_some() && !for_block {
        return Err("`item` is only for block properties".into());
    }
    if spec.place.is_some() && !for_block {
        return Err("`place` is only for block properties".into());
    }
    match (&spec.values, &spec.materials) {
        (Some(values), None) if !spec.optional => {
            let property = plain_property(name, values, spec.item.unwrap_or(false))?;
            if spec.place.is_some() && property.item {
                return Err("a property set by `place` can't be part of the item".into());
            }
            Ok(property)
        }
        (Some(_), None) => Err("`optional` is for material properties".into()),
        (None, Some(_)) if spec.place.is_some() => {
            Err("`place` is only for plain values, not materials".into())
        }
        (None, Some(materials)) => {
            let mut values: Vec<String> = Vec::new();
            if spec.optional {
                values.push(NONE.into());
            }
            for entry in materials.values() {
                let found: Vec<String> = match entry.strip_prefix('#') {
                    Some(tag) => {
                        let tagged: Vec<String> = content
                            .materials
                            .values()
                            .filter(|m| m.has_tag(tag))
                            .map(|m| m.id.clone())
                            .collect();
                        if tagged.is_empty() {
                            return Err(format!("no material has tag `{tag}`"));
                        }
                        tagged
                    }
                    None => {
                        let id = qualify(entry, namespace)?;
                        if !content.materials.contains_key(&id) {
                            return Err(format!("no material `{id}`"));
                        }
                        vec![id]
                    }
                };
                for id in found {
                    if !values.contains(&id) {
                        values.push(id);
                    }
                }
            }
            if values.iter().all(|v| v == NONE) {
                return Err("it lists no materials".into());
            }
            Ok(Property {
                name: name.into(),
                values,
                materials: true,
                item: spec.item.unwrap_or(true),
                place: None,
            })
        }
        _ => Err("give either `values` or `materials`".into()),
    }
}

fn plain_property(name: &str, values: &[String], item: bool) -> Result<Property, String> {
    if values.is_empty() {
        return Err("it has no values".into());
    }
    if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return Err("it lists a value twice".into());
    }
    for value in values {
        check_state_text(&format!("value `{value}`"), value)?;
    }
    Ok(Property {
        name: name.into(),
        values: values.to_vec(),
        materials: false,
        item,
        place: None,
    })
}

/// Checks that a property's name or value can be written in a state's
/// name (`id[name=value,…]`, see `StateName`) and read back.
fn check_state_text(what: &str, text: &str) -> Result<(), String> {
    match text.chars().find(|c| matches!(c, '[' | ']' | '=' | ',')) {
        Some(c) => Err(format!("{what} has a `{c}`, which state names use")),
        None if text.is_empty() => Err(format!("{what} is empty")),
        None => Ok(()),
    }
}

/// Checks the number of states properties make.
pub(super) fn check_count(properties: &[Property]) -> Result<usize, String> {
    let count = state_count(properties);
    if count > MAX_STATES {
        return Err(format!(
            "{count} states is too many; a block or item may have {MAX_STATES}"
        ));
    }
    Ok(count)
}

/// Every combination of the properties' values, in state order (the last
/// property fastest): each one's value indices, and names and values.
pub(super) fn combinations(
    properties: &[Property],
) -> impl Iterator<Item = (Vec<usize>, Vec<(String, String)>)> + '_ {
    (0..state_count(properties)).map(|offset| {
        let index = index_of(properties, offset);
        let values = named_values(properties, &index);
        (index, values)
    })
}

/// Each state's property names and values, by value index.
fn named_values(properties: &[Property], index: &[usize]) -> Vec<(String, String)> {
    properties
        .iter()
        .zip(index)
        .map(|(p, &i)| (p.name.clone(), p.values[i].clone()))
        .collect()
}

#[cfg(test)]
mod tests;
