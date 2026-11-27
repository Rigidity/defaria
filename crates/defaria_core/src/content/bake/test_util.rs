//! Helpers the bake's tests share.

use crate::content::{Content, test_mod as content};

pub(super) fn errors(content: &Content) -> String {
    content.errors.join("\n")
}

/// Materials for the material tests: two woods (one with a `bark`
/// palette), a rock and two ores, plus a texture with inputs.
pub(super) fn materials(files: &[(&'static str, &'static str)]) -> Content {
    let mut all = vec![
        (
            "materials/oak.json5",
            r##"{ name: "Oak", tags: ["wood"], palette: ["#100000", "#200000"],
                 palettes: { bark: ["#300000", "#400000"] }, hardness: 1, weight: 700 }"##,
        ),
        (
            "materials/birch.json5",
            r##"{ tags: ["wood", "pale"], palette: ["#001000", "#002000"], hardness: 0.5,
                 weight: 600 }"##,
        ),
        (
            "materials/rock.json5",
            r##"{ tags: ["rock"], palette: ["#000010", "#000020"], hardness: 2, weight: 2000 }"##,
        ),
        (
            "materials/iron.json5",
            r##"{ tags: ["ore", "metal"], palette: ["#101010", "#202020"], hardness: 4,
                 weight: 8000 }"##,
        ),
        (
            "materials/coal.json5",
            r##"{ tags: ["ore"], palette: ["#050505", "#060606"], hardness: 1, weight: 1400 }"##,
        ),
        (
            "textures/bark.json5",
            r##"{ layers: [{ image: "a", material: "$wood", role: "bark" }] }"##,
        ),
        (
            "textures/ore.json5",
            r##"{ layers: [{ image: "a", material: "$rock" }, { image: "a", material: "$ore" }] }"##,
        ),
    ];
    all.extend_from_slice(files);
    content(&all)
}

/// A block's `fluid`, for tests that only need it to be a fluid.
pub(super) const TEST_FLUID: &str = r##"fluid: {
    rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5,
    view: { color: "#203060", distance: 12 },
}"##;
