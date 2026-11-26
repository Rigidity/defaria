use bevy::prelude::*;

use super::*;
use crate::content::{QUAD, bake::test_util::*, load::ModFiles, test_mod as content};

#[test]
fn textures_use_materials_or_inline_palettes() {
    let content = content(&[]);
    let [a, b] =
        ["test:a", "test:b"].map(|id| &content.textures[content.texture(id).unwrap() as usize]);
    assert_eq!(a.layers[0].image, "mods/test/images/a.png");
    assert_eq!(a.layers[0].palette, content.materials["test:grey"].palette);
    let red = b.layers[0].palette.color(0).to_u8_array();
    assert_eq!(red, [255, 0, 0, 255]);
}

/// An image file that isn't there is reported, but the texture still
/// loads, so blocks using it show the placeholder instead of vanishing.
#[test]
fn a_missing_image_is_reported_and_the_texture_kept() {
    let content = content(&[
        (
            "textures/x.json5",
            r#"{ layers: [{ image: "missing", material: "grey" }] }"#,
        ),
        (
            "blocks/x.json5",
            r#"{ model: "quad", textures: { all: "x" } }"#,
        ),
    ]);
    let all = errors(&content);
    assert_eq!(content.errors.len(), 1, "{all}");
    assert!(
        all.contains("mods/test/textures/x.json5")
            && all.contains("there's no mods/test/images/missing.png"),
        "{all}"
    );
    let texture = content.texture("test:x").unwrap();
    assert_eq!(
        content.textures[texture as usize].layers[0].image,
        "mods/test/images/missing.png"
    );
    assert!(content.block("test:x").is_some());
}

/// A base mod with soils `dirt` and `sand` (each with its own image)
/// and turfs `fern` and `moss`, textures `soil` and `soil_turf` with `#tag` inputs
/// and images named by them, and a block of any soil with optional
/// turf; plus `extra` files.
fn soil_mod(extra: &[(&str, &str)]) -> ModFiles {
    let mut files = vec![
        ("images/dirt.png", ""),
        ("images/sand.png", ""),
        ("images/overhang.png", ""),
        (
            "materials/dirt.json5",
            r##"{ tags: ["soil"], palette: ["#100000", "#200000"], hardness: 1, weight: 1 }"##,
        ),
        (
            "materials/sand.json5",
            r##"{ tags: ["soil"], palette: ["#001000", "#002000"], hardness: 1, weight: 1 }"##,
        ),
        (
            "materials/moss.json5",
            r##"{ tags: ["turf"], palette: ["#000010", "#000020"], hardness: 1, weight: 1 }"##,
        ),
        (
            "materials/fern.json5",
            r##"{ tags: ["turf"], palette: ["#001010", "#002020"], hardness: 1, weight: 1 }"##,
        ),
        (
            "textures/soil.json5",
            r##"{ layers: [{ image: "{soil}", material: "#soil" }] }"##,
        ),
        (
            "textures/soil_turf.json5",
            r##"{ layers: [
                { image: "{soil}", material: "#soil" },
                { image: "overhang", material: "#turf" },
            ] }"##,
        ),
        ("models/quad.json5", QUAD),
        (
            "blocks/ground.json5",
            r##"{
                properties: {
                    soil: { materials: "#soil" },
                    turf: { materials: "#turf", optional: true },
                },
                model: "quad",
                textures: { all: { texture: "soil", materials: { soil: "{soil}" } } },
                variants: [{
                    when: { turf: { not: "none" } },
                    textures: { all: {
                        texture: "soil_turf",
                        materials: { soil: "{soil}", turf: "{turf}" },
                    } },
                }],
            }"##,
        ),
    ];
    files.extend_from_slice(extra);
    ModFiles::new("mods/base", r##"{ id: "base", name: "Base" }"##, &files)
}

/// The images of the texture baked as `key`, by file name.
fn images_of(content: &Content, key: &str) -> Vec<String> {
    let layer = content
        .texture(key)
        .unwrap_or_else(|| panic!("no tile {key}"));
    content.textures[layer as usize]
        .layers
        .iter()
        .map(|layer| layer.image.rsplit('/').next().unwrap().to_owned())
        .collect()
}

/// A texture whose inputs are `#tag`s is baked at load for every
/// combination of materials with those tags, whether or not anything
/// uses it, and `{input}` in an image picks the image named after each
/// material.
#[test]
fn tagged_inputs_bake_every_combination() {
    let content = Content::from_mods(vec![soil_mod(&[(
        "textures/unused.json5",
        r##"{ layers: [
            { image: "{soil}", material: "#soil" },
            { image: "overhang", material: "#turf" },
        ] }"##,
    )])]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    for key in [
        "base:soil(soil=base:dirt)",
        "base:soil(soil=base:sand)",
        "base:unused(soil=base:dirt,turf=base:fern)",
        "base:unused(soil=base:dirt,turf=base:moss)",
        "base:unused(soil=base:sand,turf=base:fern)",
        "base:unused(soil=base:sand,turf=base:moss)",
    ] {
        assert!(content.texture(key).is_some(), "no tile {key}");
    }
    assert_eq!(
        images_of(&content, "base:soil(soil=base:sand)"),
        ["sand.png"]
    );
    // Two soils and two turfs make four tiles.
    for (soil, turf) in [
        ("dirt", "fern"),
        ("dirt", "moss"),
        ("sand", "fern"),
        ("sand", "moss"),
    ] {
        let key = format!("base:soil_turf(soil=base:{soil},turf=base:{turf})");
        assert_eq!(
            images_of(&content, &key),
            [format!("{soil}.png"), "overhang.png".into()]
        );
    }
    // Each layer takes its own material's palette.
    let turfed = content
        .texture("base:soil_turf(soil=base:dirt,turf=base:moss)")
        .unwrap();
    let layers = &content.textures[turfed as usize].layers;
    assert_eq!(layers[0].palette, content.materials["base:dirt"].palette);
    assert_eq!(layers[1].palette, content.materials["base:moss"].palette);
    // Blocks use the tiles baked at load rather than making more.
    let ground = content.block("base:ground").unwrap();
    assert_eq!(ground.state_count, 2 * 3);
    let sand = content.state(ground.state(&[("soil", "base:sand")]).unwrap());
    assert_eq!(
        sand.faces[0].layer,
        content.texture("base:soil(soil=base:sand)").unwrap()
    );
    assert_eq!(content.textures.len(), 2 + 2 * 2 + 2 * 2);
}

/// Another mod adds a soil with only a material tagged `soil` and its
/// image: the base mod's textures and block take it in. If its image is
/// missing, that's reported and its tiles show the placeholder, and the
/// block still loads.
#[test]
fn a_mod_adds_a_soil_with_a_material_and_an_image() {
    let clay = |files: &[(&str, &str)]| {
        ModFiles::new(
            "mods/clay",
            r##"{ id: "clay", name: "Clay", dependencies: ["base"] }"##,
            files,
        )
    };
    let material = (
        "materials/clay.json5",
        r##"{ tags: ["soil"], palette: ["#101010", "#202020"], hardness: 1, weight: 1 }"##,
    );
    let content = Content::from_mods(vec![
        soil_mod(&[]),
        clay(&[("images/clay.png", ""), material]),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    assert_eq!(
        images_of(&content, "base:soil(soil=clay:clay)"),
        ["clay.png"]
    );
    let ground = content.block("base:ground").unwrap();
    assert!(
        ground
            .state(&[("soil", "clay:clay"), ("turf", "base:moss")])
            .is_some()
    );

    let content = Content::from_mods(vec![soil_mod(&[]), clay(&[material])]);
    let all = errors(&content);
    // Once for each texture that uses it, not once per tile.
    let missing = "there's no mods/clay/images/clay.png";
    assert_eq!(all.matches(missing).count(), 2, "{all}");
    assert!(all.contains("mods/base/textures/soil.json5"), "{all}");
    assert!(all.contains("mods/base/textures/soil_turf.json5"), "{all}");
    assert!(content.block("base:ground").is_some(), "{all}");
    assert!(
        content
            .image_paths()
            .contains(&"mods/clay/images/clay.png".to_owned())
    );
}

/// 46 materials tagged `a` and 46 tagged `b`: 2116 combinations, more
/// than [`MAX_TILES`].
fn many_materials() -> Vec<(String, String)> {
    (0..46)
        .flat_map(|i| {
            ["a", "b"].map(|tag| {
                (
                    format!("materials/{tag}{i}.json5"),
                    format!(
                        r##"{{ tags: ["{tag}"], palette: ["#000000", "#ffffff"], hardness: 1, weight: 1 }}"##
                    ),
                )
            })
        })
        .collect()
}

fn load_with(files: &[(String, String)]) -> Content {
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    Content::from_mods(vec![soil_mod(&refs)])
}

/// Tags whose combinations would make more tiles than the game has
/// room for are reported against the texture, which is left out, and
/// what uses it fails rather than making the tiles one at a time.
#[test]
fn too_many_combinations_are_reported() {
    let mut files = many_materials();
    files.push((
        "textures/x.json5".into(),
        r##"{ layers: [
            { image: "overhang", material: "#a" },
            { image: "overhang", material: "#b" },
        ] }"##
            .into(),
    ));
    files.push((
        "blocks/x.json5".into(),
        r##"{ model: "quad", textures: { all: {
            texture: "x", materials: { a: "a0", b: "b0" },
        } } }"##
            .into(),
    ));
    let content = load_with(&files);
    let all = errors(&content);
    assert!(
        all.contains("mods/base/textures/x.json5: its combinations make more tiles"),
        "{all}"
    );
    assert!(all.contains("mods/base/blocks/x.json5"), "{all}");
    assert!(content.textures.iter().all(|t| t.template != "base:x"));
    assert!(content.textures.len() < MAX_TILES);
}

/// Tiles made as blocks use `$name` inputs stop at [`MAX_TILES`]: the
/// block that would go past it fails, and the tiles fit the GPU.
#[test]
fn tiles_made_on_use_stop_at_the_limit() {
    let mut files = many_materials();
    files.push((
        "textures/y.json5".into(),
        r##"{ layers: [
            { image: "overhang", material: "$a" },
            { image: "overhang", material: "$b" },
        ] }"##
            .into(),
    ));
    files.push((
        "blocks/y.json5".into(),
        r##"{
            properties: { a: { materials: "#a" }, b: { materials: "#b" } },
            model: "quad",
            textures: { all: { texture: "y", materials: { a: "{a}", b: "{b}" } } },
        }"##
        .into(),
    ));
    let content = load_with(&files);
    let all = errors(&content);
    assert!(all.contains("no room for more than 2048 tiles"), "{all}");
    assert!(content.block("base:y").is_none());
    assert_eq!(content.textures.len(), MAX_TILES);
}

/// Mistakes with `#tag` inputs are reported.
#[test]
fn tagged_input_mistakes_are_reported() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "textures/x.json5",
            r##"{ layers: [
                { image: "overhang", material: "#soil" },
                { image: "overhang", material: "$other" },
            ] }"##,
            "all `$name` or all `#tag`",
        ),
        (
            "textures/x.json5",
            r##"{ layers: [{ image: "{other}", material: "$other" }] }"##,
            "uses `{other}`, which isn't a `#tag` input",
        ),
        (
            "textures/x.json5",
            r##"{ layers: [
                { image: "overhang", material: "#soil" },
                { image: "overhang", material: "$soil" },
            ] }"##,
            "can't both be inputs",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: {
                texture: "soil", materials: { soil: "moss" },
            } } }"##,
            "takes `#soil` materials, and `base:moss` isn't one",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "soil" } }"##,
            "needs a material for `#soil`",
        ),
        (
            "textures/x.json5",
            r##"{ layers: [{ image: "{soil", material: "#soil" }] }"##,
            "has a `{` with no `}`",
        ),
        (
            "textures/x.json5",
            r##"{ layers: [{ image: "Big {soil}", material: "#soil" }] }"##,
            "Big dirt",
        ),
    ];
    for &(path, text, expected) in cases {
        let content = Content::from_mods(vec![soil_mod(&[(path, text)])]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{text}\n{all}");
        assert!(all.contains(&format!("mods/base/{path}")), "{all}");
        assert!(all.contains(expected), "expected `{expected}` in: {all}");
    }
}
