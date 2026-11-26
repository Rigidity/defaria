//! Tests of the bake as a whole: how definitions refer to each other,
//! and how mistakes are reported.

use super::test_util::*;
use crate::content::{Content, QUAD, load::ModFiles, test_mod as content};

/// A broken texture leaves out the block that uses it, and that block's
/// item: each is reported against its own file.
#[test]
fn a_broken_definition_takes_its_dependents_with_it() {
    let content = content(&[
        (
            "textures/broken.json5",
            r#"{ layers: [{ image: "a", material: "missing" }] }"#,
        ),
        (
            "blocks/user.json5",
            r#"{ model: "quad", textures: { all: "broken" } }"#,
        ),
        (
            "blocks/fine.json5",
            r#"{ model: "quad", textures: { all: "a" } }"#,
        ),
    ]);
    assert_eq!(content.errors.len(), 2, "{}", errors(&content));
    assert!(content.errors[0].contains("textures/broken.json5"));
    assert!(content.errors[1].contains("blocks/user.json5"));
    assert!(content.errors[1].contains("no texture `test:broken`"));
    assert!(content.block("test:user").is_none());
    assert!(content.item("test:user").is_none());
    assert!(content.block("test:fine").is_some());

    // An item file changing a block that didn't load says so.
    let content = crate::content::test_mod(&[
        (
            "blocks/sun.json5",
            r#"{ model: "quad", textures: { all: "a" }, light: [16, 0, 0] }"#,
        ),
        ("items/sun.json5", r#"{ unique: true }"#),
    ]);
    let all = errors(&content);
    assert_eq!(content.errors.len(), 2, "{all}");
    assert!(
        content.errors[1].contains("items/sun.json5")
            && content.errors[1].contains("block `test:sun` didn't load"),
        "{all}"
    );
    assert!(content.item("test:sun").is_none());
}

/// Mods refer to each other's definitions with `mod:name`, and images
/// load from the mod that owns them.
#[test]
fn mods_use_each_others_definitions() {
    let base = ModFiles::new(
        "mods/base",
        r#"{ id: "base", name: "Base" }"#,
        &[
            ("images/rock.png", ""),
            (
                "materials/rock.json5",
                r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1 }"##,
            ),
            ("models/quad.json5", QUAD),
        ],
    );
    let addon = ModFiles::new(
        "mods/addon",
        r#"{ id: "addon", name: "Addon", dependencies: ["base"] }"#,
        &[
            (
                "textures/rock.json5",
                r#"{ layers: [{ image: "base:rock", material: "base:rock" }] }"#,
            ),
            (
                "blocks/rock.json5",
                r#"{ model: "base:quad", textures: { all: "rock" } }"#,
            ),
        ],
    );
    let content = Content::from_mods(vec![addon, base]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    assert_eq!(content.mods[0].id, "base");
    let texture = &content.textures[content.texture("addon:rock").unwrap() as usize];
    assert_eq!(texture.layers[0].image, "mods/base/images/rock.png");
    assert!(content.block("addon:rock").is_some());
}

/// Each mistake is reported against its file, and only the broken
/// definition (and what uses it) is left out.
#[test]
fn mistakes_name_the_file_and_skip_only_that_definition() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "blocks/x.json5",
            r#"{ model: "quad", textures: { all: "a" }, colour: 1 }"#,
            "colour",
        ),
        (
            "blocks/x.json5",
            r#"{ model: "quad", textures: { all: "a" "#,
            "line 1",
        ),
        (
            "blocks/x.json5",
            r#"{ model: "nope", textures: { all: "a" } }"#,
            "no model `test:nope`",
        ),
        (
            "blocks/x.json5",
            r#"{ model: "quad", textures: { al: "a" } }"#,
            "has a slot `al`",
        ),
        (
            "blocks/x.json5",
            r#"{ model: "quad" }"#,
            "no texture for slot `side`",
        ),
        (
            "blocks/x.json5",
            r#"{ textures: { all: "a" } }"#,
            "no `model`",
        ),
        (
            "blocks/x.json5",
            r#"{ model: "quad", textures: { all: "zz" } }"#,
            "no texture `test:zz`",
        ),
        (
            "blocks/x.json5",
            r#"{ properties: { p: ["a", "b"] }, model: "quad", textures: { all: "{p}x" } }"#,
            "state p=a: no texture `test:ax`",
        ),
        (
            "blocks/x.json5",
            r#"{ properties: { p: ["a"] }, model: "quad", textures: { all: "a" }, variants: [{ when: { p: "c" } }] }"#,
            "`p` has no value `c`",
        ),
        (
            "blocks/x.json5",
            r#"{ properties: { p: ["a"] }, model: "quad", textures: { all: "a" }, variants: [{ when: { q: "a" } }] }"#,
            "variant 1: no property `q`",
        ),
        (
            "blocks/x.json5",
            r#"{ properties: { p: ["a", "a"] }, model: "quad", textures: { all: "a" } }"#,
            "lists a value twice",
        ),
        (
            "blocks/x.json5",
            r#"{ properties: { p: ["a", "b"] }, model: "quad", textures: { all: "a" }, variants: [{ when: { p: "b" }, textures: { al: "a" } }] }"#,
            "has a slot `al`",
        ),
        ("blocks/defaria:x.json5", "{}", "must be"),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
            "a fluid has no `model`",
        ),
        (
            "blocks/x.json5",
            r##"{ rotate: [0, 90, 0], textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
            "a fluid can't be rotated",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 0, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
            "fluid `rate` is 0; it must be above 0",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1.5, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
            "fluid `fall` is 1.5; it must be from 0 to 1 block",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: -0.1, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
            "fluid `film` is -0.1",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.001, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
            "fluid `ripple` is 0.001; it must be at least 0.002",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 2, view: { color: "#203060", distance: 12 } } }"##,
            "fluid `move_speed` is 2",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 0 } } }"##,
            "fluid view `distance` is 0",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "blue", distance: 12 } } }"##,
            "`blue` isn't a hex color",
        ),
        (
            "blocks/x.json5",
            r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 }, depth: 3 } }"##,
            "unknown field `depth`",
        ),
        (
            "models/x.json5",
            r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [
                { pos: [0, 0, 0], uv: [0, 0] }, { pos: [1, 0, 0], uv: [0, 0] },
                { pos: [1, 1, 0], uv: [0, 0] }, { pos: [Infinity, 1, 0], uv: [0, 0] },
            ] }] }"#,
            "vertex 4 has a number that isn't finite",
        ),
        (
            "models/x.json5",
            r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [
                { pos: [0, 0, 0], uv: [0, 0] }, { pos: [1, 0, 0], uv: [NaN, 0] },
                { pos: [1, 1, 0], uv: [0, 0] },
            ] }] }"#,
            "vertex 2 has a number that isn't finite",
        ),
        (
            "blocks/x.json5",
            r#"{ properties: { p: [] }, model: "quad", textures: { all: "a" } }"#,
            "property `p`: it has no values",
        ),
        (
            "models/x.json5",
            r#"{ collision: [{ from: [0, 0, 0], to: [1, 1.5, 1] }], faces: [] }"#,
            "collision box 1: it must be inside the block",
        ),
        (
            "models/x.json5",
            r#"{ collision: [{ from: [0, 0, 0], to: [1, 1, 1] }, { from: [0, 0.5, 0], to: [1, 0.5, 1] }], faces: [] }"#,
            "collision box 2: it's flat",
        ),
        (
            "blocks/x.json5",
            r#"{ model: "quad", textures: { all: "a" }, rotate: [10, 0, 0] }"#,
            "multiples of 90",
        ),
        (
            "textures/x.json5",
            r#"{ layers: [{ image: "a" }] }"#,
            "either a `material` or a `palette`",
        ),
        (
            "textures/x.json5",
            r#"{ layers: [{ image: "a", material: "nope" }] }"#,
            "no material `test:nope`",
        ),
        (
            "textures/x.json5",
            r#"{ layers: [] }"#,
            "at least one layer",
        ),
        (
            "materials/x.json5",
            r##"{ palette: ["#000"], hardness: 1, weight: 1 }"##,
            "2 to 8 colors",
        ),
        (
            "materials/x.json5",
            r#"{ palette: ["black", "white"], hardness: 1, weight: 1 }"#,
            "`black` isn't a hex color",
        ),
        (
            "models/x.json5",
            r#"{ slots: { a: "b", b: "a" }, faces: [] }"#,
            "go round in a circle",
        ),
        (
            "models/x.json5",
            r#"{ slots: { a: "z" }, faces: [] }"#,
            "`z`, which isn't a slot",
        ),
        (
            "models/x.json5",
            r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [{ pos: [0, 0, 0], uv: [0, 0] }] }] }"#,
            "face 1: a face has 3 or 4 vertices",
        ),
        (
            "models/x.json5",
            r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [
                { pos: [0, 0, 0], uv: [0, 0] }, { pos: [1, 0, 0], uv: [0, 0] }, { pos: [2, 0, 0], uv: [0, 0] },
            ] }] }"#,
            "lie on a line",
        ),
        (
            "models/x.json5",
            r#"{ faces: [{ texture: "b", vertices: [] }] }"#,
            "isn't one of the model's slots",
        ),
        (
            "items/x.json5",
            r#"{ block: "nope" }"#,
            "no block `test:nope`",
        ),
        ("blocks/Bad Name.json5", "{}", "must be"),
        ("stuff/x.json5", "{}", "not a definition"),
    ];
    for &(path, text, expected) in cases {
        let content = content(&[(path, text)]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
        assert!(all.contains(&format!("mods/test/{path}")), "{all}");
        assert!(all.contains(expected), "expected `{expected}` in: {all}");
        // The good definitions around it still loaded.
        assert!(content.texture("test:a").is_some(), "{all}");
        assert!(content.models.contains_key("test:quad"), "{all}");
        assert!(content.block("test:x").is_none());
    }
}
