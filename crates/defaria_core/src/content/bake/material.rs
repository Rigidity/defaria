//! Materials and their palettes, and checking the numbers they give.

use bevy::prelude::*;

use super::{Def, fill::title_case};
use crate::{
    content::palette::Palette,
    content::{BUILT_IN_STATS, Material, check_name, check_namespace, schema::MaterialDef},
};

/// A palette from 2 to 8 hex colors, darkest first.
pub(super) fn palette(colors: &[String]) -> Result<Palette, String> {
    if !(2..=8).contains(&colors.len()) {
        return Err(format!(
            "a palette needs 2 to 8 colors, not {}",
            colors.len()
        ));
    }
    let stops = colors
        .iter()
        .map(|color| Srgba::hex(color).map_err(|_| format!("`{color}` isn't a hex color")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Palette::ramp(&stops))
}

pub(super) fn material(def: &Def<MaterialDef>) -> Result<Material, String> {
    let material = &def.def;
    for tag in &material.tags {
        check_name(tag).map_err(|error| format!("tag {error}"))?;
    }
    let palettes = material
        .palettes
        .iter()
        .map(|(role, colors)| {
            palette(colors)
                .map(|palette| (role.clone(), palette))
                .map_err(|error| format!("palette `{role}`: {error}"))
        })
        .collect::<Result<_, String>>()?;
    check_amount("hardness", material.hardness)?;
    check_amount("weight", material.weight)?;
    for (name, &value) in &material.stats {
        check_stat_name(name)?;
        check_amount(name, value)?;
    }
    Ok(Material {
        id: def.id.clone(),
        name: material.name.clone().unwrap_or_else(|| title_case(&def.id)),
        tags: material.tags.clone(),
        palette: palette(&material.palette)?,
        palettes,
        hardness: material.hardness,
        weight: material.weight,
        stats: material.stats.clone(),
    })
}

/// Checks a hardness, weight, stat or share: a number, not below 0.
pub(super) fn check_amount(what: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(format!(
            "`{what}` must be a number of at least 0, not {value}"
        ))
    }
}

/// Checks a stat's name: lowercase letters, digits and `_`, and not one a
/// material has as a field of its own.
pub(super) fn check_stat_name(name: &str) -> Result<(), String> {
    check_namespace(name).map_err(|error| format!("stat {error}"))?;
    if BUILT_IN_STATS.contains(&name) {
        return Err(format!(
            "`{name}` is a field of its own, not one of the `stats`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::bake::test_util::*;

    #[test]
    fn material_mistakes_are_reported() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#nothing" } }, model: "quad", textures: { all: "a" } }"##,
                "no material has tag `nothing`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "teak" } }, model: "quad", textures: { all: "a" } }"##,
                "no material `test:teak`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { values: ["a"], optional: true } }, model: "quad", textures: { all: "a" } }"##,
                "`optional` is for material properties",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#wood" } }, model: "quad", textures: { all: "bark" } }"##,
                "texture `test:bark` needs a material for `$wood`",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: { texture: "a", materials: { wood: "oak" } } } }"##,
                "texture `test:a` has no input `wood`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#wood" } }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: "#rock" } }] }"##,
                "`w` has no value `#rock`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: ["a"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: { not: "b" } } }] }"##,
                "`w` has no value `b`",
            ),
            (
                "items/x.json5",
                r##"{ properties: { w: { materials: "#wood", item: true } } }"##,
                "`item` is only for block properties",
            ),
            (
                "textures/x.json5",
                r##"{ layers: [{ image: "a", palette: ["#000", "#fff"], role: "bark" }] }"##,
                "`role` is for layers with a `material`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: ["a"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: [] } }] }"##,
                "`w` lists no values",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: [] } }, model: "quad", textures: { all: "a" } }"##,
                "property `w`: it lists no materials",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#wood", optinal: true } }, model: "quad", textures: { all: "a" } }"##,
                "unknown field `optinal`",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: { texture: "a", material: {} } } }"##,
                "unknown field `material`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: ["a", "b"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: { not: "a", also: "b" } } }] }"##,
                "unknown field `also`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: "a" }, model: "quad", textures: { all: "a" } }"##,
                "a list of values, or `{ values }` or `{ materials }`",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000", "#fff"], palettes: { bark: ["#000"] }, hardness: 1,
                     weight: 1 }"##,
                "palette `bark`: a palette needs 2 to 8 colors",
            ),
        ];
        for &(path, text, expected) in cases {
            let content = materials(&[(path, text)]);
            let all = errors(&content);
            assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
            assert!(all.contains(&format!("mods/test/{path}")), "{all}");
            assert!(all.contains(expected), "expected `{expected}` in: {all}");
        }
        // A block's item takes the block's properties, not its own.
        let content = materials(&[
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" } }"##,
            ),
            ("items/x.json5", r##"{ properties: { p: ["a"] } }"##),
        ]);
        assert!(
            errors(&content).contains("items/x.json5"),
            "{}",
            errors(&content)
        );
        assert!(content.item("test:x").is_none());
    }

    /// A material without a hardness or weight is its own file's error;
    /// blocks counting its tag load without it.
    #[test]
    fn a_material_missing_hardness_fails_alone() {
        let content = materials(&[
            (
                "materials/marble.json5",
                r##"{ tags: ["rock"], palette: ["#000", "#fff"], weight: 2700 }"##,
            ),
            (
                "blocks/stone.json5",
                r##"{ properties: { rock: { materials: "#rock" } }, model: "quad", textures: { all: "a" },
                     hardness: { materials: { rock: 1 } } }"##,
            ),
        ]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{all}");
        assert!(all.contains("mods/test/materials/marble.json5"), "{all}");
        assert!(all.contains("missing field `hardness`"), "{all}");
        let stone = content.block("test:stone").unwrap();
        assert_eq!(stone.state_count, 1);
        assert_eq!(content.state(stone.default_state()).hardness, 2.0);
    }

    /// The base game's metals and woods have the stats wires and pipes
    /// read, and they compare as they should.
    #[test]
    fn base_materials_have_conduit_stats() {
        let content = crate::content::base();
        let stat = |material: &str, name: &str| {
            content.materials[&format!("defaria:{material}")]
                .stat(name)
                .unwrap_or_else(|| panic!("{material} has no {name}"))
        };
        for metal in ["copper", "gold", "iron"] {
            assert!(stat(metal, "conductivity") > 0.0, "{metal}");
        }
        for material in ["copper", "gold", "iron", "oak", "birch", "spruce"] {
            assert!(stat(material, "throughput") > 0.0, "{material}");
            assert!(stat(material, "max_temperature") > 0.0, "{material}");
        }
        assert!(stat("copper", "conductivity") > stat("gold", "conductivity"));
        assert!(stat("gold", "conductivity") > stat("iron", "conductivity"));
        // Iron pipes carry what melts copper; wood burns well below either.
        assert!(stat("iron", "max_temperature") > stat("copper", "max_temperature"));
        assert!(stat("oak", "max_temperature") < stat("copper", "max_temperature"));
        assert!(stat("oak", "throughput") < stat("iron", "throughput"));
    }
}
