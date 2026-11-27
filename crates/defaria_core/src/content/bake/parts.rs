//! The models a block state is made of: its own, and its `parts`.

use std::collections::BTreeMap;

use super::{fill::fill, look::Look, when::When};
use crate::content::{
    Content, Model, qualify,
    schema::{BlockDef, TextureRef},
};

/// One model of a block state: the state's own (from its look), or one of
/// the block's `parts`.
pub(super) struct Part<'d> {
    pub(super) model: Model,
    pub(super) rotate: Option<[i32; 3]>,
    /// The part's own textures, over the state's. `None` for the state's
    /// own model.
    pub(super) textures: Option<&'d BTreeMap<String, TextureRef>>,
}

/// The model `id` names, with `{property}` filled in.
fn find_model<'a>(
    id: &str,
    values: &[(String, String)],
    namespace: &str,
    content: &'a Content,
) -> Result<&'a Model, String> {
    let id = qualify(&fill(id, values)?, namespace)?;
    content
        .models
        .get(&id)
        .ok_or_else(|| format!("no model `{id}`"))
}

/// A state's parts: the model its look chose, if any, then every one of
/// the block's `parts` that matches it. A block with no `parts` must
/// have a model.
pub(super) fn parts<'d>(
    block: &'d BlockDef,
    look: &Look,
    whens: &[When],
    index: &[usize],
    values: &[(String, String)],
    namespace: &str,
    content: &Content,
) -> Result<Vec<Part<'d>>, String> {
    let mut parts = Vec::new();
    match look.model {
        Some(id) => parts.push(Part {
            model: find_model(id, values, namespace, content)?.clone(),
            rotate: look.rotate,
            textures: None,
        }),
        None if block.parts.is_empty() => return Err("no `model`".into()),
        None => {}
    }
    for (i, (part, when)) in block.parts.iter().zip(whens).enumerate() {
        if !when.matches(index) {
            continue;
        }
        let model = find_model(&part.model, values, namespace, content)
            .map_err(|error| format!("part {}: {error}", i + 1))?;
        if let Some(slot) = part
            .textures
            .keys()
            .find(|slot| !model.slots.contains_key(slot.as_str()))
        {
            return Err(format!(
                "part {}: model `{}` has no slot `{slot}`",
                i + 1,
                model.id
            ));
        }
        parts.push(Part {
            model: model.clone(),
            rotate: part.rotate,
            textures: Some(&part.textures),
        });
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;
    use crate::content::{
        BlockBox, BlockState, Direction, State, bake::test_util::*, test_mod as content,
    };

    /// A plate over the block's north side, covering it, with a box to
    /// bump into.
    const PLATE: &str = r#"{
        slots: { all: null },
        occludes: ["north"],
        collision: [{ from: [0, 0, 0], to: [1, 1, 0.125] }],
        faces: [{
            texture: "all", cull: "north",
            vertices: [
                { pos: [1, 0, 0], uv: [0, 1] },
                { pos: [0, 0, 0], uv: [1, 1] },
                { pos: [0, 1, 0], uv: [1, 0] },
                { pos: [1, 1, 0], uv: [0, 0] },
            ],
        }],
    }"#;

    /// A small box in the middle, drawn as one face on its top.
    const CORE: &str = r#"{
        slots: { all: null },
        collision: [{ from: [0.25, 0.25, 0.25], to: [0.75, 0.75, 0.75] }],
        faces: [{
            texture: "all",
            vertices: [
                { pos: [0.25, 0.75, 0.75], uv: [0, 1] },
                { pos: [0.75, 0.75, 0.75], uv: [1, 1] },
                { pos: [0.75, 0.75, 0.25], uv: [1, 0] },
                { pos: [0.25, 0.75, 0.25], uv: [0, 0] },
            ],
        }],
    }"#;

    /// A block of a core and a plate on each side its properties say.
    fn parts_mod(extra: &[(&str, &str)]) -> Content {
        let mut files = vec![
            ("models/plate.json5", PLATE),
            ("models/core.json5", CORE),
            (
                "blocks/boxy.json5",
                r#"{
                    properties: { north: ["false", "true"], south: ["false", "true"] },
                    textures: { all: "a" },
                    parts: [
                        { model: "core" },
                        { when: { north: "true" }, model: "plate" },
                        { when: { south: "true" }, model: "plate", rotate: [0, 180, 0] },
                    ],
                }"#,
            ),
        ];
        files.extend_from_slice(extra);
        content(&files)
    }

    fn boxy<'c>(content: &'c Content, north: &str, south: &str) -> (BlockState, &'c State) {
        let block = content.block("test:boxy").unwrap();
        let state = block.state(&[("north", north), ("south", south)]).unwrap();
        (state, content.state(state))
    }

    #[test]
    fn parts_add_to_the_states_own_model_only_where_they_match() {
        let content = parts_mod(&[(
            "blocks/flag.json5",
            r#"{
                properties: { flag: ["false", "true"] },
                model: "quad",
                textures: { all: "a" },
                parts: [{ when: { flag: "true" }, model: "plate" }],
            }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:flag").unwrap();
        // No part matches: just the block's own model.
        let bare = content.state(block.state(&[("flag", "false")]).unwrap());
        assert_eq!(bare.parts.len(), 1);
        assert_eq!(bare.parts[0].model, "test:quad");
        assert_eq!(bare.faces.len(), 1);
        assert_eq!(bare.faces[0].cull, Some(Direction::Up));
        // The block's own model comes first, then the part.
        let flagged = content.state(block.state(&[("flag", "true")]).unwrap());
        let models: Vec<&str> = flagged.parts.iter().map(|p| p.model.as_str()).collect();
        assert_eq!(models, ["test:quad", "test:plate"]);
        assert_eq!(flagged.faces.len(), 2);
        assert_eq!(flagged.faces[0], bare.faces[0]);
        assert_eq!(flagged.parts[1].faces, 1..2);
        assert_eq!(flagged.faces[1].cull, Some(Direction::North));
    }

    #[test]
    fn every_matching_part_is_added_turned_as_it_says() {
        let content = parts_mod(&[]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let (_, core) = boxy(&content, "false", "false");
        assert_eq!(core.parts.len(), 1);
        assert_eq!(core.faces.len(), 1);

        let (_, both) = boxy(&content, "true", "true");
        let parts: Vec<(&str, [i32; 3])> = both
            .parts
            .iter()
            .map(|p| (p.model.as_str(), p.rotate))
            .collect();
        assert_eq!(
            parts,
            [
                ("test:core", [0, 0, 0]),
                ("test:plate", [0, 0, 0]),
                ("test:plate", [0, 180, 0]),
            ]
        );
        assert_eq!(both.faces.len(), 3);
        let south = &both.faces[both.parts[2].faces.clone()][0];
        assert_eq!(south.cull, Some(Direction::South));
        assert_eq!(south.normal, Vec3::Z);
        assert!(south.vertices.iter().all(|v| v.pos.z == 1.0));
    }

    #[test]
    fn covered_sides_and_culling_come_from_the_parts() {
        let content = parts_mod(&[]);
        let (core_state, core) = boxy(&content, "false", "false");
        assert!(Direction::ALL.iter().all(|&d| !core.occludes(d)));
        let (north_state, north) = boxy(&content, "true", "false");
        assert!(north.occludes(Direction::North));
        assert!(!north.occludes(Direction::South));
        assert_eq!(north.parts[1].occludes, Direction::North.bit());
        let (both_state, both) = boxy(&content, "true", "true");
        assert!(both.occludes(Direction::North) && both.occludes(Direction::South));
        assert!(!both.occludes(Direction::Up));
        assert!(!both.is_opaque());

        // A neighbour to the north hides a face against it only when one of
        // its parts covers its south side.
        assert!(!content.hides(core_state, north_state, Direction::North));
        assert!(content.hides(core_state, both_state, Direction::North));
        // The plate's face is hidden by a neighbour covering that side.
        assert!(content.hides(north_state, both_state, Direction::North));
        assert!(!content.hides(north_state, core_state, Direction::North));
    }

    #[test]
    fn collision_is_every_parts_boxes() {
        let content = parts_mod(&[]);
        let core = BlockBox {
            min: Vec3::splat(0.25),
            max: Vec3::splat(0.75),
        };
        let north = BlockBox {
            min: Vec3::ZERO,
            max: Vec3::new(1.0, 1.0, 0.125),
        };
        let south = BlockBox {
            min: Vec3::new(0.0, 0.0, 0.875),
            max: Vec3::ONE,
        };
        assert_eq!(boxy(&content, "false", "false").1.collision, [core]);
        assert_eq!(boxy(&content, "false", "true").1.collision, [core, south]);
        let (_, both) = boxy(&content, "true", "true");
        assert_eq!(both.collision, [core, north, south]);
        assert_eq!(both.parts[2].collision, 2..3);
    }

    #[test]
    fn a_parts_textures_go_over_the_blocks_for_that_part() {
        let content = parts_mod(&[(
            "blocks/two_tone.json5",
            r#"{
                textures: { all: "a" },
                parts: [{ model: "core" }, { model: "plate", textures: { all: "b" } }],
            }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let state = content.state(content.default_state("test:two_tone").unwrap());
        let layer = |id: &str| content.texture(id).unwrap();
        assert_eq!(state.faces[0].layer, layer("test:a"));
        assert_eq!(state.faces[1].layer, layer("test:b"));
    }

    #[test]
    fn part_mistakes_are_reported() {
        let cases = [
            (
                r#"{ textures: { all: "a" }, parts: [{ when: { open: "true" }, model: "core" }] }"#,
                "part 1: no property `open`",
            ),
            (
                r#"{ textures: { all: "a" }, parts: [{ model: "core" }, { model: "nothing" }] }"#,
                "part 2: no model `test:nothing`",
            ),
            (
                r#"{ textures: { all: "a" }, parts: [{ model: "core", textures: { side: "b" } }] }"#,
                "part 1: model `test:core` has no slot `side`",
            ),
            (
                r#"{ textures: { all: "a" }, parts: [{ model: "core", rotate: [45, 0, 0] }] }"#,
                "multiples of 90",
            ),
            (r#"{ textures: { all: "a" } }"#, "no `model`"),
        ];
        for (block, error) in cases {
            let content = parts_mod(&[("blocks/broken.json5", block)]);
            let errors = errors(&content);
            assert!(
                errors.contains("broken.json5") && errors.contains(error),
                "{block}: expected `{error}`, got:\n{errors}"
            );
            assert!(content.default_state("test:broken").is_none());
        }
    }

    #[test]
    fn the_base_pipe_has_an_arm_for_each_side_it_says() {
        let content = crate::content::base();
        let pipe = content.block("defaria:pipe").unwrap();
        let core = content.state(pipe.default_state());
        let models: Vec<&str> = core.parts.iter().map(|p| p.model.as_str()).collect();
        assert_eq!(models, ["defaria:pipe_core"]);
        assert!(Direction::ALL.iter().all(|&d| !core.occludes(d)));

        for side in Direction::ALL {
            let state = pipe.state(&[(side.name(), "true")]).unwrap();
            let state = content.state(state);
            assert_eq!(state.parts.len(), 2, "{side:?}");
            let arm = &state.parts[1];
            assert_eq!(arm.model, "defaria:pipe_arm");
            // The arm's end is on that side, and hidden by what covers it.
            let ends: Vec<_> = state.faces[arm.faces.clone()]
                .iter()
                .filter_map(|f| f.cull)
                .collect();
            assert_eq!(ends, [side], "{side:?}");
            // Its box reaches that side of the block from the core.
            let reach = state.collision[arm.collision.clone()][0];
            let n = side.normal().as_vec3();
            let far = if n.max_element() > 0.0 {
                reach.max
            } else {
                reach.min
            };
            assert_eq!((far - Vec3::splat(0.5)).dot(n), 0.5, "{side:?}");
        }

        let all: Vec<(&str, &str)> = Direction::ALL.iter().map(|d| (d.name(), "true")).collect();
        let all = content.state(pipe.state(&all).unwrap());
        assert_eq!(all.parts.len(), 7);
        assert_eq!(all.collision.len(), 7);
    }
}
