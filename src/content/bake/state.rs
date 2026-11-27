//! Baking one block state from its look and parts.

use std::collections::BTreeMap;

use bevy::{platform::collections::HashMap, prelude::*};

use super::{
    fluid::{bake_fluid, fluid_room},
    look::Look,
    mix::Numbers,
    model::rotation,
    parts::Part,
    texture::Textures,
};
use crate::content::{
    BakedFace, BlockBox, Content, Direction, MAX_LIGHT, Model, SideTexture, State, StatePart,
    Vertex,
    schema::{BlockDef, TextureRef},
};

/// Bakes one state of `def`, block number `block`, called `name`, with these
/// numbers: each part's faces rotated and textured, one after the other.
/// Textures for slots a part's model doesn't have are left out of that
/// part. A fluid has nothing to bump into or aim at, whatever its model's
/// collision and target.
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
    let light_filter = def.light_filter;
    if light_filter > MAX_LIGHT {
        return Err(format!(
            "light_filter {light_filter} is too much; the most is {MAX_LIGHT}"
        ));
    }
    let fluid = def.fluid.as_ref().map(bake_fluid).transpose()?;
    let fluid_room = fluid_room(def)?;
    let mut state = State {
        block,
        name,
        values: values.to_vec(),
        light,
        light_filter,
        render: look.render.unwrap_or_default(),
        double_sided: def.double_sided,
        hardness: numbers.hardness,
        weight: numbers.weight,
        stats: numbers.stats,
        fluid_room,
        replaceable: def.replaceable,
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
    // Something to aim at, unless it's a fluid, which the crosshair
    // passes through.
    if state.target.is_empty() && fluid.is_none() {
        state.target.push(BlockBox::FULL);
    }
    state.fluid = fluid;
    for side in Direction::ALL {
        state.side_textures[side as usize] = side_texture(&state.faces, side);
    }
    Ok(state)
}

/// The textures of the faces culled against `side`.
pub(in crate::content) fn side_texture(faces: &[BakedFace], side: Direction) -> SideTexture {
    let mut layers = faces
        .iter()
        .filter(|face| face.cull == Some(side))
        .map(|face| face.layer);
    match layers.next() {
        None => SideTexture::None,
        Some(first) if layers.all(|layer| layer == first) => SideTexture::One(first),
        Some(_) => SideTexture::Mixed,
    }
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
    let turn_box = |b: &BlockBox| {
        let [a, c] = [b.min, b.max].map(turn);
        BlockBox {
            min: a.min(c),
            max: a.max(c),
        }
    };
    let first_box = state.collision.len();
    if def.fluid.is_none() {
        state.collision.extend(model.collision.iter().map(turn_box));
        state.target.extend(model.target.iter().map(turn_box));
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

    /// `light_filter` is the block's, on every state, 0 if left out, and
    /// at most the brightest light, 15.
    #[test]
    fn blocks_filter_the_light_they_say() {
        let content = content(&[
            (
                "blocks/murk.json5",
                r#"{
                    properties: { shade: ["a", "b"] },
                    model: "quad",
                    textures: { all: "a" },
                    light_filter: 3,
                }"#,
            ),
            (
                "blocks/plain.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/void.json5",
                r#"{ model: "quad", textures: { all: "a" }, light_filter: 16 }"#,
            ),
            (
                "blocks/ink.json5",
                r#"{ model: "quad", textures: { all: "a" }, light_filter: 15 }"#,
            ),
        ]);
        let murk = content.block("test:murk").unwrap();
        for shade in ["a", "b"] {
            let state = content.state(murk.state(&[("shade", shade)]).unwrap());
            assert_eq!(state.light_filter, 3, "{shade}");
        }
        let plain = content.state(content.default_state("test:plain").unwrap());
        assert_eq!(plain.light_filter, 0);
        let ink = content.state(content.default_state("test:ink").unwrap());
        assert_eq!(ink.light_filter, 15);
        assert!(content.block("test:void").is_none());
        assert!(
            errors(&content).contains("light_filter 16"),
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
