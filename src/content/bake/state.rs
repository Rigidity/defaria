//! Baking one block state from its look and parts.

use std::collections::BTreeMap;

use bevy::{platform::collections::HashMap, prelude::*};

use super::{
    block::Look, fluid::bake_fluid, mix::Numbers, model::rotation, parts::Part, texture::Textures,
};
use crate::content::{
    BakedFace, BlockBox, Content, Direction, MAX_LIGHT, Model, State, StatePart, Vertex,
    schema::{BlockDef, TextureRef},
};

/// Bakes one state of `def`, block number `block`, called `name`, with these
/// numbers: each part's faces rotated and textured, one after the other.
/// Textures for slots a part's model doesn't have are left out of that
/// part. A fluid has nothing to bump into, whatever its model's collision.
pub(super) fn bake_state(
    look: &Look,
    parts: &[Part],
    values: &[(String, String)],
    name: String,
    numbers: Numbers,
    namespace: &str,
    block: usize,
    def: &BlockDef,
    content: &mut Content,
    textures: &Textures,
) -> Result<State, String> {
    let light = look.light.unwrap_or_default();
    if let Some(level) = light.iter().find(|&&level| level > MAX_LIGHT) {
        return Err(format!(
            "light level {level} is too bright; the most is {MAX_LIGHT}"
        ));
    }
    let fluid = def.fluid.as_ref().map(bake_fluid).transpose()?;
    let mut state = State {
        block,
        name,
        values: values.to_vec(),
        light,
        render: look.render.unwrap_or_default(),
        double_sided: def.double_sided,
        hardness: numbers.hardness,
        weight: numbers.weight,
        stats: numbers.stats,
        ..default()
    };
    for part in parts {
        bake_part(
            part, look, values, namespace, def, &mut state, content, textures,
        )
        .map_err(|error| match part.textures {
            Some(_) => format!("part model `{}`: {error}", part.model.id),
            None => error,
        })?;
    }
    state.fluid = fluid;
    Ok(state)
}

/// Adds one part's faces, boxes and covered sides to `state`.
fn bake_part(
    part: &Part,
    look: &Look,
    values: &[(String, String)],
    namespace: &str,
    def: &BlockDef,
    state: &mut State,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let model = &part.model;
    let degrees = part.rotate.unwrap_or([0; 3]);
    let rotation = rotation(degrees)?;
    // Turns a point in the block about the block's center.
    let turn = |p: Vec3| rotation * (p - Vec3::splat(0.5)) + Vec3::splat(0.5);
    let rotate_direction = |d: Direction| {
        Direction::from_normal((rotation * d.normal().as_vec3()).round().as_ivec3()).unwrap()
    };

    // The state's textures, then the part's own.
    let mut slots: BTreeMap<&str, &TextureRef> = look.textures.clone();
    if let Some(own) = part.textures {
        slots.extend(own.iter().map(|(slot, texture)| (slot.as_str(), texture)));
    }
    let mut layers = HashMap::new();
    for (slot, texture) in slots {
        if !model.slots.contains_key(slot) {
            continue;
        }
        let layer = textures.bake_ref(texture, values, namespace, content)?;
        layers.insert(slot, layer);
    }

    let first_face = state.faces.len();
    for face in &model.faces {
        let layer = slot_texture(&face.slot, model, &layers)?;
        state.faces.push(BakedFace {
            // A quarter turn keeps it a unit vector, exactly.
            normal: rotation * face.normal,
            vertices: face
                .vertices
                .iter()
                .map(|v| Vertex {
                    pos: turn(v.pos),
                    uv: v.uv,
                })
                .collect(),
            layer,
            cull: face.cull.map(rotate_direction),
        });
    }
    let occludes = model
        .occludes
        .iter()
        .fold(0, |bits, &d| bits | rotate_direction(d).bit());
    state.occludes |= occludes;
    let first_box = state.collision.len();
    if def.fluid.is_none() {
        state.collision.extend(model.collision.iter().map(|b| {
            let [a, c] = [b.min, b.max].map(turn);
            BlockBox {
                min: a.min(c),
                max: a.max(c),
            }
        }));
    }
    state.parts.push(StatePart {
        model: model.id.clone(),
        rotate: degrees,
        faces: first_face..state.faces.len(),
        collision: first_box..state.collision.len(),
        occludes,
    });
    Ok(())
}

/// The texture for `slot`: the block's own, or its fallback's.
fn slot_texture(slot: &str, model: &Model, textures: &HashMap<&str, u32>) -> Result<u32, String> {
    let mut at = slot;
    loop {
        if let Some(&layer) = textures.get(at) {
            return Ok(layer);
        }
        // Fallbacks were checked to end when the model loaded.
        match model.slots.get(at).and_then(Option::as_ref) {
            Some(next) => at = next,
            None => return Err(format!("no texture for slot `{slot}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::content::{bake::test_util::*, test_mod as content};

    #[test]
    fn light_brighter_than_the_maximum_is_an_error() {
        let content = content(&[(
            "blocks/sun.json5",
            r#"{ model: "quad", textures: { all: "a" }, light: [16, 0, 0] }"#,
        )]);
        assert!(content.block("test:sun").is_none());
        assert!(
            errors(&content).contains("light level 16"),
            "{}",
            errors(&content)
        );
    }

    /// Textures for slots a state's model doesn't have aren't looked up, so
    /// a `{property}` texture only has to exist for the states whose model
    /// uses it.
    #[test]
    fn slots_the_model_lacks_are_skipped_for_that_state() {
        let content = content(&[(
            "blocks/thing.json5",
            r#"{
                properties: { p: ["a", "y"] },
                model: "quad",
                textures: { all: "{p}", top: "a", bottom: "a" },
                variants: [{ when: { p: "y" }, model: "pair" }],
            }"#,
        )]);
        // There's no texture `test:y`, but the `pair` state has no `all`.
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:thing").unwrap();
        let y = content.state(block.state(&[("p", "y")]).unwrap());
        assert_eq!(y.faces.len(), 2);
    }
}
