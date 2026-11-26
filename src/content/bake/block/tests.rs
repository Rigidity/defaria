use super::*;
use crate::content::{BlockBox, Direction, bake::test_util::*, test_mod as content};

#[test]
fn a_minimal_block_loads_with_an_item() {
    let content = content(&[(
        "blocks/thing.json5",
        r#"{ model: "quad", textures: { all: "a" } }"#,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let block = content.block("test:thing").unwrap();
    assert_eq!(content.state(block.default_state()).name, "Thing");
    let state = content.state(block.default_state());
    assert_eq!(state.faces.len(), 1);
    assert_eq!(state.faces[0].layer, content.texture("test:a").unwrap());
    assert_eq!(state.faces[0].normal, Vec3::Y);
    assert!(state.occludes(Direction::Up));
    assert!(!state.occludes(Direction::Down));
    let item = content.default_item_info("test:thing").unwrap();
    assert_eq!(item.block, Some(block.default_state()));
    assert_eq!(item.name, "Thing");
    assert!(!content.item("test:thing").unwrap().unique);
}

/// `light` gives a block's light per colour channel; variants replace
/// it. Cubes are opaque to light, other shapes aren't.
#[test]
fn blocks_give_off_the_light_they_say() {
    let content = content(&[
        (
            "blocks/lamp.json5",
            r#"{
                properties: { color: ["red", "off"] },
                model: "quad",
                textures: { all: "a" },
                light: [15, 2, 0],
                variants: [{ when: { color: "off" }, light: [0, 0, 0] }],
            }"#,
        ),
        (
            "blocks/plain.json5",
            r#"{ model: "quad", textures: { all: "a" } }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let lamp = content.block("test:lamp").unwrap();
    let red = content.state(lamp.state(&[("color", "red")]).unwrap());
    assert_eq!(red.light, [15, 2, 0]);
    assert!(red.emits_light());
    let off = content.state(lamp.state(&[("color", "off")]).unwrap());
    assert!(!off.emits_light());
    let plain = content.state(content.default_state("test:plain").unwrap());
    assert_eq!(plain.light, [0; 3]);
    assert!(!plain.is_opaque(), "a quad lets light through");
    assert!(
        crate::content::base()
            .state(crate::content::block("stone"))
            .is_opaque()
    );
    assert!(!content.state(crate::content::BlockState::AIR).is_opaque());
}

/// `render` says how a block's faces are drawn (opaque unless it says
/// otherwise), and variants replace it. A see-through block lets light
/// through its covered sides, and its covered sides hide only the
/// faces they should. `fluid` is off unless set.
#[test]
fn blocks_are_drawn_as_their_render_says() {
    // The quad covers its top.
    let content = content(&[
        (
            "blocks/pane.json5",
            r#"{
                properties: { kind: ["clear", "leafy", "solid"] },
                model: "quad",
                textures: { all: "a" },
                render: "translucent",
                variants: [
                    { when: { kind: "leafy" }, render: "cutout" },
                    { when: { kind: "solid" }, render: "opaque" },
                ],
            }"#,
        ),
        (
            "blocks/pool.json5",
            &format!(r#"{{ textures: {{ all: "a" }}, {TEST_FLUID} }}"#),
        ),
        (
            "blocks/tar.json5",
            &format!(r#"{{ textures: {{ all: "a" }}, {TEST_FLUID} }}"#),
        ),
        (
            "blocks/deep.json5",
            &format!(r#"{{ textures: {{ all: "a" }}, {TEST_FLUID} }}"#),
        ),
        (
            "blocks/box.json5",
            r#"{ model: "pair", textures: { top: "a", bottom: "a" } }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let pane = content.block("test:pane").unwrap();
    let state = |kind| pane.state(&[("kind", kind)]).unwrap();
    let [clear, leafy, solid] = ["clear", "leafy", "solid"].map(state);
    let render = |s| content.state(s).render;
    assert_eq!(
        [clear, leafy, solid].map(render),
        [Render::Translucent, Render::Cutout, Render::Opaque]
    );
    for see_through in [clear, leafy] {
        let state = content.state(see_through);
        assert!(state.occludes(Direction::Up) && !state.blocks_light(Direction::Up));
    }
    assert!(content.state(solid).blocks_light(Direction::Up));
    let pool = content.default_state("test:pool").unwrap();
    assert!(content.state(pool).is_fluid() && !content.state(solid).is_fluid());
    assert_eq!(render(pool), Render::Opaque);
    // A fluid has no collision.
    let collision = |id| &content.state(content.default_state(id).unwrap()).collision;
    assert_eq!(collision("test:box"), &[BlockBox::FULL]);
    assert!(collision("test:deep").is_empty());

    // What hides a block's bottom face, from below.
    let hides = |block, neighbour| content.hides(block, neighbour, Direction::Down);
    assert!(hides(clear, solid) && hides(leafy, solid));
    assert!(
        hides(clear, pool),
        "an opaque fluid hides see-through faces"
    );
    // Two different opaque fluids: exactly one face between them.
    let tar = content.default_state("test:tar").unwrap();
    assert!(
        hides(pool, tar) != hides(tar, pool),
        "one face between two fluids"
    );
    assert!(!hides(solid, pool), "but not a solid opaque block's");
    assert!(hides(pool, pool), "an opaque fluid hides itself");
    assert!(
        !hides(solid, clear),
        "only the same block's translucent faces"
    );
    assert!(!hides(pool, clear), "another translucent block");
    assert!(
        !hides(pool, leafy) && !hides(leafy, leafy),
        "cut-outs hide nothing"
    );
}

/// A translucent block hides its own faces against it only where the
/// two faces that meet have the same texture, so a seam shows between
/// two colors of one block. Other properties don't split it, nor does
/// how a face is culled on another side.
#[test]
fn translucent_faces_hide_only_faces_with_their_texture() {
    // A top face culled up and a bottom face culled down, covering both.
    let layer = r#"{
        slots: { all: null, top: "all", bottom: "all" },
        occludes: ["up", "down"],
        faces: [
            { texture: "top", cull: "up", vertices: [
                { pos: [0, 1, 1], uv: [0, 1] }, { pos: [1, 1, 1], uv: [1, 1] },
                { pos: [1, 1, 0], uv: [1, 0] }, { pos: [0, 1, 0], uv: [0, 0] },
            ] },
            { texture: "bottom", cull: "down", vertices: [
                { pos: [0, 0, 0], uv: [0, 1] }, { pos: [1, 0, 0], uv: [1, 1] },
                { pos: [1, 0, 1], uv: [1, 0] }, { pos: [0, 0, 1], uv: [0, 0] },
            ] },
        ],
    }"#;
    let content = content(&[
        ("models/layer.json5", layer),
        (
            "blocks/pane.json5",
            r#"{
                properties: {
                    color: ["grey", "red", "red_under"],
                    lit: ["no", "yes"],
                },
                model: "layer",
                render: "translucent",
                textures: { all: "a" },
                variants: [
                    { when: { color: "red" }, textures: { all: "b" } },
                    { when: { color: "red_under" }, textures: { all: "a", bottom: "b" } },
                ],
            }"#,
        ),
        (
            "blocks/other.json5",
            r#"{ model: "layer", render: "translucent", textures: { all: "a" } }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let pane = content.block("test:pane").unwrap();
    let state = |color, lit| pane.state(&[("color", color), ("lit", lit)]).unwrap();
    let (grey, red, under) = (
        state("grey", "no"),
        state("red", "no"),
        state("red_under", "no"),
    );
    // `block`'s top against `above`'s bottom.
    let hides = |block, above| content.hides(block, above, Direction::Up);
    assert!(hides(grey, grey) && hides(red, red), "one color");
    assert!(
        hides(grey, state("grey", "yes")),
        "another state, the same texture"
    );
    assert!(
        !hides(grey, red) && !hides(red, grey),
        "a seam between colors"
    );
    // `under`'s top is grey and its bottom red: what meets decides.
    assert!(!hides(grey, under) && hides(under, grey));
    assert!(hides(red, under) && !hides(under, red));
    let other = content.default_state("test:other").unwrap();
    assert!(
        !hides(grey, other),
        "another block, even with the same texture"
    );
}

#[test]
fn an_unknown_render_is_an_error() {
    let content = content(&[(
        "blocks/odd.json5",
        r#"{ model: "quad", textures: { all: "a" }, render: "glowing" }"#,
    )]);
    assert!(content.block("test:odd").is_none());
    assert!(errors(&content).contains("glowing"), "{}", errors(&content));
}

/// Variants apply in order where they match, textures merge, and
/// `{property}` picks a texture per state.
#[test]
fn variants_and_property_names_pick_the_look() {
    let content = content(&[(
        "blocks/thing.json5",
        r#"{
            properties: { tex: ["a", "b"], turn: ["no", "yes"] },
            model: "quad",
            textures: { all: "{tex}" },
            variants: [
                { when: { turn: "yes" }, rotate: [0, 0, 90] },
                { when: { tex: "b", turn: ["yes"] }, textures: { side: "a" } },
            ],
        }"#,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let block = content.block("test:thing").unwrap();
    assert_eq!(block.state_count, 4);
    let [a, b] = ["test:a", "test:b"].map(|id| content.texture(id).unwrap());
    let face =
        |values: &[(&str, &str)]| content.state(block.state(values).unwrap()).faces[0].clone();
    assert_eq!(face(&[("tex", "a")]).layer, a);
    assert_eq!(face(&[("tex", "b")]).layer, b);
    // Turned a quarter about Z, the top face faces west.
    let turned = face(&[("tex", "b"), ("turn", "yes")]);
    assert_eq!(turned.normal, Vec3::NEG_X);
    assert_eq!(turned.cull, Some(Direction::West));
    // About the block's centre: the quad on the top moves to the west
    // side, still inside the block.
    for vertex in &turned.vertices {
        assert_eq!(vertex.pos.x, 0.0, "{vertex:?}");
        assert!(vertex.pos.cmpge(Vec3::ZERO).all() && vertex.pos.cmple(Vec3::ONE).all());
    }
    // The second variant's `side` beats the block's `all`.
    assert_eq!(turned.layer, a);
    let state = content.state(block.state(&[("turn", "yes")]).unwrap());
    assert!(state.occludes(Direction::West) && !state.occludes(Direction::Up));
}

/// When several matching variants set the same thing, the last wins:
/// the model and rotation are replaced, and textures are merged slot by
/// slot, so a slot only the block sets survives. A block can set slots
/// for one model and switch to another with different slots.
#[test]
fn later_variants_win_and_textures_merge_by_slot() {
    let content = content(&[(
        "blocks/thing.json5",
        r#"{
            properties: { p: ["x", "y"] },
            model: "quad",
            rotate: [0, 0, 90],
            textures: { all: "a", top: "a", bottom: "b" },
            variants: [
                { when: { p: "y" }, rotate: [0, 0, 180], textures: { top: "b" } },
                { when: { p: "y" }, model: "pair", rotate: [0, 0, 0], textures: { top: "c" } },
            ],
        }"#,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let [a, b, c] = ["test:a", "test:b", "test:c"].map(|id| content.texture(id).unwrap());
    let block = content.block("test:thing").unwrap();

    let x = content.state(block.state(&[("p", "x")]).unwrap());
    assert_eq!(x.faces.len(), 1);
    assert_eq!(x.faces[0].normal, Vec3::NEG_X);
    assert_eq!(x.faces[0].layer, a);

    let y = content.state(block.state(&[("p", "y")]).unwrap());
    let faces: Vec<(Vec3, u32)> = y.faces.iter().map(|f| (f.normal, f.layer)).collect();
    assert_eq!(faces, [(Vec3::Y, c), (Vec3::NEG_Y, b)]);
}
