//! Models: their slots, faces and collision boxes, and rotating them.

use std::collections::BTreeMap;

use bevy::prelude::*;

use super::Def;
use crate::content::{BlockBox, Model, ModelFace, Vertex, schema::ModelDef};

pub(super) fn model(def: &Def<ModelDef>) -> Result<Model, String> {
    let model = &def.def;
    for (slot, fallback) in &model.slots {
        if let Some(fallback) = fallback
            && !model.slots.contains_key(fallback)
        {
            return Err(format!(
                "slot `{slot}` falls back to `{fallback}`, which isn't a slot"
            ));
        }
        // Following the fallbacks must end, not go round in a circle.
        let mut at = slot;
        let mut steps = 0;
        while let Some(Some(next)) = model.slots.get(at) {
            at = next;
            steps += 1;
            if steps > model.slots.len() {
                return Err(format!("slot `{slot}`'s fallbacks go round in a circle"));
            }
        }
    }
    let faces = model
        .faces
        .iter()
        .enumerate()
        .map(|(i, face)| {
            let vertices: Vec<Vertex> = face
                .vertices
                .iter()
                .map(|v| Vertex {
                    pos: Vec3::from(v.pos),
                    uv: Vec2::from(v.uv),
                })
                .collect();
            let normal = check_face(&face.texture, &vertices, &model.slots)
                .map_err(|error| format!("face {}: {error}", i + 1))?;
            Ok(ModelFace {
                slot: face.texture.clone(),
                cull: face.cull,
                vertices,
                normal,
            })
        })
        .collect::<Result<Vec<ModelFace>, String>>()?;
    let collision = match &model.collision {
        Some(boxes) => boxes
            .iter()
            .enumerate()
            .map(|(i, b)| {
                block_box(Vec3::from(b.from), Vec3::from(b.to))
                    .map_err(|error| format!("collision box {}: {error}", i + 1))
            })
            .collect::<Result<_, String>>()?,
        None => around_faces(&faces).into_iter().collect(),
    };
    Ok(Model {
        id: def.id.clone(),
        slots: model.slots.clone(),
        occludes: model.occludes.clone(),
        collision,
        faces,
    })
}

/// The box between two opposite corners, which must be inside the block
/// and apart on every axis.
fn block_box(from: Vec3, to: Vec3) -> Result<BlockBox, String> {
    let (min, max) = (from.min(to), from.max(to));
    if !(min.is_finite() && max.is_finite()) {
        return Err("a corner has a number that isn't finite".into());
    }
    if min.cmplt(Vec3::ZERO).any() || max.cmpgt(Vec3::ONE).any() {
        return Err("it must be inside the block, from [0, 0, 0] to [1, 1, 1]".into());
    }
    if min.cmpge(max).any() {
        return Err("it's flat: its corners must differ on every axis".into());
    }
    Ok(BlockBox { min, max })
}

/// The box around every face, if they have one that isn't flat and is
/// inside the block, for a model that doesn't give its `collision`.
fn around_faces(faces: &[ModelFace]) -> Option<BlockBox> {
    let mut points = faces
        .iter()
        .flat_map(|face| face.vertices.iter().map(|v| v.pos));
    let first = points.next()?;
    let (min, max) = points.fold((first, first), |(min, max), p| (min.min(p), max.max(p)));
    block_box(min.max(Vec3::ZERO), max.min(Vec3::ONE)).ok()
}

/// Checks a face and returns its normal: the face of a triangle, or of a
/// quad's first three corners, which run counter-clockwise around it.
fn check_face(
    slot: &str,
    vertices: &[Vertex],
    slots: &BTreeMap<String, Option<String>>,
) -> Result<Vec3, String> {
    if !slots.contains_key(slot) {
        return Err(format!("`{slot}` isn't one of the model's slots"));
    }
    if !(3..=4).contains(&vertices.len()) {
        return Err(format!(
            "a face has 3 or 4 vertices, not {}",
            vertices.len()
        ));
    }
    if let Some(i) = vertices
        .iter()
        .position(|v| !v.pos.is_finite() || !v.uv.is_finite())
    {
        return Err(format!("vertex {} has a number that isn't finite", i + 1));
    }
    let [a, b, c] = [0, 1, 2].map(|i| vertices[i].pos);
    let normal = (b - a).cross(c - a).normalize_or_zero();
    if normal == Vec3::ZERO {
        return Err("its first three vertices lie on a line".into());
    }
    Ok(normal)
}

/// A rotation of `degrees` about X, then Y, then Z, each a multiple of 90.
/// Built from exact quarter turns, so whole and half coordinates stay
/// exact.
pub(in crate::content) fn rotation(degrees: [i32; 3]) -> Result<Mat3, String> {
    let quarter_turns = [
        // About X: Y goes to Z, Z to -Y.
        Mat3::from_cols(Vec3::X, Vec3::Z, Vec3::NEG_Y),
        // About Y: Z goes to X, X to -Z.
        Mat3::from_cols(Vec3::NEG_Z, Vec3::Y, Vec3::X),
        // About Z: X goes to Y, Y to -X.
        Mat3::from_cols(Vec3::Y, Vec3::NEG_X, Vec3::Z),
    ];
    let mut out = Mat3::IDENTITY;
    for (axis, d) in degrees.into_iter().enumerate() {
        if d % 90 != 0 {
            return Err(format!("`rotate` takes multiples of 90 degrees, not {d}"));
        }
        for _ in 0..d.rem_euclid(360) / 90 {
            out = quarter_turns[axis] * out;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{bake::test_util::*, test_mod as content};

    /// A model's collision is the box around its faces unless it gives its
    /// own boxes, and it turns with the block.
    #[test]
    fn collision_is_the_box_around_the_faces_or_given() {
        let content = content(&[
            // QUAD is flat, so there's nothing to bump into by default.
            (
                "blocks/flat.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "models/post.json5",
                r#"{ collision: [{ from: [0.75, 0, 0.25], to: [0.25, 1, 0.75] }], faces: [] }"#,
            ),
            ("models/ghost.json5", r#"{ collision: [], faces: [] }"#),
            ("blocks/post.json5", r#"{ model: "post" }"#),
            (
                "blocks/lying_post.json5",
                r#"{ model: "post", rotate: [0, 0, 90] }"#,
            ),
            ("blocks/ghost.json5", r#"{ model: "ghost" }"#),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let collision = |id: &str| {
            let state = content.default_state(&format!("test:{id}")).unwrap();
            content.state(state).collision.clone()
        };
        assert_eq!(collision("flat"), []);
        assert_eq!(collision("ghost"), []);
        // Corners in either order.
        let post = BlockBox {
            min: Vec3::new(0.25, 0.0, 0.25),
            max: Vec3::new(0.75, 1.0, 0.75),
        };
        assert_eq!(collision("post"), [post]);
        assert_eq!(
            collision("lying_post"),
            [BlockBox {
                min: Vec3::new(0.0, 0.25, 0.25),
                max: Vec3::new(1.0, 0.75, 0.75),
            }]
        );
        let base = crate::content::base();
        let slab = base.default_state("defaria:stone_slab").unwrap();
        assert_eq!(
            base.state(slab).collision,
            [BlockBox {
                min: Vec3::ZERO,
                max: Vec3::new(1.0, 0.5, 1.0),
            }]
        );
        let stone = base.default_state("defaria:stone").unwrap();
        assert_eq!(base.state(stone).collision, [BlockBox::FULL]);
    }

    #[test]
    fn rotations_are_exact_quarter_turns() {
        let turn = rotation([90, 0, 0]).unwrap();
        assert_eq!(turn * Vec3::Y, Vec3::Z);
        assert_eq!(rotation([0, 90, 0]).unwrap() * Vec3::Z, Vec3::X);
        assert_eq!(rotation([0, 0, 90]).unwrap() * Vec3::X, Vec3::Y);
        assert_eq!(rotation([0, 0, -90]).unwrap() * Vec3::Y, Vec3::X);
        // X first, then Z: Y goes to Z, which Z leaves alone.
        assert_eq!(rotation([90, 0, 90]).unwrap() * Vec3::Y, Vec3::Z);
        assert_eq!(rotation([0, 360, 0]).unwrap(), Mat3::IDENTITY);
        assert!(rotation([45, 0, 0]).is_err());
    }
}
