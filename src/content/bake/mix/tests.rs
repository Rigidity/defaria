use super::*;
use crate::content::{BlockState, bake::test_util::*};

/// Every block and item state in the base game has a hardness and a
/// weight, worked out from its materials where it has them. Fluids,
/// which can't be mined, needn't have a hardness.
#[test]
fn base_states_have_hardness_and_weight() {
    let content = crate::content::base();
    let air = content.state(BlockState::AIR);
    assert_eq!((air.hardness, air.weight), (0.0, 0.0));
    for state in &content.states[1..] {
        assert!(
            state.hardness > 0.0 || state.is_fluid(),
            "{} hardness",
            state.name
        );
        assert!(state.weight > 0.0, "{} weight", state.name);
    }
    for info in &content.item_states {
        let fluid = info
            .block
            .is_some_and(|state| content.state(state).is_fluid());
        assert!(info.hardness > 0.0 || fluid, "{} hardness", info.name);
        assert!(info.weight > 0.0, "{} weight", info.name);
        if let Some(state) = info.block {
            let placed = content.state(state);
            assert_eq!(
                (info.hardness, info.weight),
                (placed.hardness, placed.weight)
            );
        }
    }
    let stone = content.block("defaria:stone").unwrap();
    let plain = content.state(stone.default_state());
    assert_eq!((plain.hardness, plain.weight), (1.5, 2600.0));
    // Rock counts three times as much as ore.
    let ore = content
        .state(stone.state(&[("ore", "defaria:iron")]).unwrap())
        .hardness;
    assert_eq!(ore, (3.0 * 1.5 + 3.0) / 4.0);
    let slab = content.state(content.default_state("defaria:stone_slab").unwrap());
    assert_eq!((slab.hardness, slab.weight), (1.5, 1300.0));
    let soil = content.block("defaria:soil").unwrap();
    let dirt = content.state(soil.default_state());
    assert_eq!(dirt.hardness, 0.5);
    // Soil counts three times as much as turf.
    let grass = content.state(soil.state(&[("turf", "defaria:grass")]).unwrap());
    assert_eq!(grass.hardness, (3.0 * 0.5 + 0.6) / 4.0);
    assert_eq!(grass.weight, (3.0 * 1500.0 + 1400.0) / 4.0);
    // An iron pickaxe on an oak handle weighs a kilogram or two.
    let pickaxe = content.default_item_info("defaria:pickaxe").unwrap();
    assert!((1.0..3.0).contains(&pickaxe.weight), "{}", pickaxe.weight);
}

/// A hardness or weight is a number, or a base times the weighted
/// average of the materials' values, skipping `none`. Left out, it's 0,
/// and an item that places a block takes the block's.
#[test]
fn hardness_and_weight_mix_materials() {
    let content = materials(&[
        (
            "blocks/rock.json5",
            r##"{
                properties: { rock: { materials: "#rock" }, ore: { materials: "iron", optional: true } },
                model: "quad", textures: { all: "a" },
                hardness: { materials: { rock: 3, ore: 1 } },
                weight: { base: 0.5, materials: { rock: 1 } },
            }"##,
        ),
        (
            "blocks/plain.json5",
            r##"{ model: "quad", textures: { all: "a" }, hardness: 0.25, weight: 10 }"##,
        ),
        (
            "blocks/nothing.json5",
            r##"{ model: "quad", textures: { all: "a" } }"##,
        ),
        // Changes its block's item's weight only.
        ("items/plain.json5", r##"{ weight: 3 }"##),
        (
            "items/tool.json5",
            r##"{
                properties: { head: { materials: "iron" }, handle: { materials: "oak" } },
                hardness: { materials: { head: 1 } },
                weight: { base: 0.001, materials: { head: 1, handle: 3 } },
            }"##,
        ),
        ("items/brick.json5", r##"{ block: "plain" }"##),
        ("items/stick.json5", r##"{}"##),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let rock = content.block("test:rock").unwrap();
    let plain = content.state(rock.default_state());
    // `ore` is `none`, so only the rock counts.
    assert_eq!((plain.hardness, plain.weight), (2.0, 1000.0));
    let ore = content.state(rock.state(&[("ore", "test:iron")]).unwrap());
    assert_eq!(
        (ore.hardness, ore.weight),
        ((3.0 * 2.0 + 4.0) / 4.0, 1000.0)
    );

    let fixed = content.state(content.default_state("test:plain").unwrap());
    assert_eq!((fixed.hardness, fixed.weight), (0.25, 10.0));
    let nothing = content.state(content.default_state("test:nothing").unwrap());
    assert_eq!((nothing.hardness, nothing.weight), (0.0, 0.0));

    let info = |id: &str| content.default_item_info(id).unwrap();
    let both = |id: &str| (info(id).hardness, info(id).weight);
    // The block's item: the block's hardness, the file's weight.
    assert_eq!(both("test:plain"), (0.25, 3.0));
    let rock_item = content.item("test:rock").unwrap();
    let ore_item = rock_item.state(&[("ore", "test:iron")]).unwrap();
    assert_eq!(content.item_state(ore_item).unwrap().hardness, ore.hardness);
    let tool = info("test:tool");
    assert_eq!(tool.hardness, 4.0);
    assert!((tool.weight - 0.001 * (8000.0 + 3.0 * 700.0) / 4.0).abs() < 1e-4);
    // An item that places a block is as hard and heavy as it.
    assert_eq!(both("test:brick"), (0.25, 10.0));
    assert_eq!(both("test:stick"), (0.0, 0.0));
}

/// A block's item file counts the item's own properties: here only
/// `wood`, as the plain `axis` before it isn't the item's.
#[test]
fn a_block_items_file_mixes_the_items_materials() {
    let content = materials(&[
        (
            "blocks/log.json5",
            r##"{
                properties: { axis: ["x", "y", "z"], wood: { materials: "#wood" } },
                model: "quad", textures: { all: "a" }, hardness: 9, weight: 9,
            }"##,
        ),
        (
            "items/log.json5",
            r##"{
                hardness: { materials: { wood: 1 } },
                weight: { base: 0.5, materials: { wood: 1 } },
            }"##,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let item = content.item("test:log").unwrap();
    assert_eq!(item.state_count, 2);
    let both = |wood: &str| {
        let info = content
            .item_state(item.state(&[("wood", wood)]).unwrap())
            .unwrap();
        (info.hardness, info.weight)
    };
    assert_eq!(both("test:oak"), (1.0, 350.0));
    assert_eq!(both("test:birch"), (0.5, 300.0));
}

#[test]
fn hardness_and_weight_mistakes_are_reported() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, hardness: { materials: { wood: 1 } } }"##,
            "`hardness` counts `wood`, which isn't a property",
        ),
        (
            "blocks/x.json5",
            r##"{ properties: { axis: ["x"] }, model: "quad", textures: { all: "a" },
                 weight: { materials: { axis: 1 } } }"##,
            "`weight` counts `axis`, which isn't a material property",
        ),
        (
            "blocks/x.json5",
            r##"{ properties: { wood: { materials: "oak" } }, model: "quad", textures: { all: "a" },
                 hardness: { materials: { wood: 0 } } }"##,
            "a share must be more than 0",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, hardness: { materials: {} } }"##,
            "`hardness` has no `materials`",
        ),
        // `base` alone is a multiplier, not a weight.
        (
            "items/x.json5",
            r##"{ properties: { head: { materials: "iron", optional: true },
                                handle: { materials: "oak", optional: true } },
                 weight: { base: 0.0007, materials: { head: 1, handle: 3 } } }"##,
            "`weight`: in states with handle=none,head=none, none of its counted properties has a material",
        ),
        (
            "blocks/x.json5",
            r##"{ properties: { ore: { materials: "#ore", optional: true } },
                 model: "quad", textures: { all: "a" }, hardness: { materials: { ore: 1 } } }"##,
            "`hardness`: in states with ore=none, none of its counted properties has a material",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, weight: -1 }"##,
            "`weight` must be a number of at least 0",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, weight: "heavy" }"##,
            "a number, or `{ base, stat, materials, default }`",
        ),
        (
            "materials/x.json5",
            r##"{ palette: ["#000", "#fff"], hardness: -0.5, weight: 1 }"##,
            "`hardness` must be a number of at least 0",
        ),
        // Required, so the error is the material's, not its users'.
        (
            "materials/x.json5",
            r##"{ tags: ["rock"], palette: ["#000", "#fff"], weight: 1 }"##,
            "missing field `hardness`",
        ),
        (
            "materials/x.json5",
            r##"{ tags: ["rock"], palette: ["#000", "#fff"], hardness: 1 }"##,
            "missing field `weight`",
        ),
        // An integer where a number isn't one of the forms.
        (
            "blocks/x.json5",
            r##"{ properties: { axis: ["x", "y"] }, model: "quad", textures: { all: "a" },
                 variants: [{ when: { axis: 1 }, rotate: [90, 0, 0] }] }"##,
            "invalid type: integer `1`",
        ),
    ];
    for &(path, text, expected) in cases {
        let content = materials(&[(path, text)]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
        assert!(all.contains(&format!("mods/test/{path}")), "{all}");
        assert!(all.contains(expected), "expected `{expected}` in: {all}");
    }
}

/// A block's item file counting a property the block keeps off the
/// item says so, rather than that it isn't a property at all.
#[test]
fn an_item_counting_a_block_only_property_is_told_why() {
    let content = materials(&[
        (
            "blocks/plank.json5",
            r##"{ properties: { wood: { materials: "#wood", item: false } },
                 model: "quad", textures: { all: "a" } }"##,
        ),
        (
            "items/plank.json5",
            r##"{ hardness: { materials: { wood: 1 } } }"##,
        ),
    ]);
    let all = errors(&content);
    assert_eq!(content.errors.len(), 1, "{all}");
    assert!(all.contains("mods/test/items/plank.json5"), "{all}");
    assert!(
        all.contains(
            "`hardness` counts `wood`, which isn't one of the item's properties: \
             the block marks it `item: false`"
        ),
        "{all}"
    );
}

/// Materials with stats, on top of [`materials`]: copper and gold
/// conduct, iron (from `materials`) doesn't say.
fn stat_materials(files: &[(&'static str, &'static str)]) -> Content {
    let mut all = vec![
        (
            "materials/copper.json5",
            r##"{ tags: ["metal"], palette: ["#300000", "#400000"], hardness: 2, weight: 9000,
                 stats: { conductivity: 1, max_temperature: 1358 } }"##,
        ),
        (
            "materials/gold.json5",
            r##"{ tags: ["metal"], palette: ["#303000", "#404000"], hardness: 1, weight: 19000,
                 stats: { conductivity: 0.6, max_temperature: 1337 } }"##,
        ),
    ];
    all.extend_from_slice(files);
    materials(&all)
}

/// An attribute reads the stat it names, or the stat named like its
/// field, averaged over its materials by share. Block states and their
/// items get their `stats` worked out at load.
#[test]
fn attributes_read_material_stats() {
    let content = stat_materials(&[(
        "blocks/wire.json5",
        r##"{
                properties: { core: { materials: ["copper", "gold"] }, coat: { materials: ["gold", "copper"] } },
                model: "quad", textures: { all: "a" },
                // Hardness from another stat, to show any field can.
                hardness: { base: 2, stat: "conductivity", materials: { core: 1 } },
                stats: {
                    capacity: { base: 500, stat: "conductivity", materials: { core: 3, coat: 1 } },
                    max_temperature: { materials: { core: 1, coat: 1 } },
                    size: 0.25,
                },
            }"##,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let wire = content.block("test:wire").unwrap();
    let state = |core: &str, coat: &str| {
        content.state(wire.state(&[("core", core), ("coat", coat)]).unwrap())
    };
    let copper = state("test:copper", "test:copper");
    assert_eq!(copper.hardness, 2.0);
    assert_eq!(copper.stat("capacity"), Some(500.0));
    assert_eq!(copper.stat("max_temperature"), Some(1358.0));
    assert_eq!(copper.stat("size"), Some(0.25));
    assert_eq!(copper.stat("conductivity"), None);
    // Three parts copper to one of gold.
    let mixed = state("test:copper", "test:gold");
    let capacity = mixed.stat("capacity").unwrap();
    assert!(
        (capacity - 500.0 * (3.0 + 0.6) / 4.0).abs() < 1e-3,
        "{capacity}"
    );
    assert_eq!(mixed.stat("max_temperature"), Some((1358.0 + 1337.0) / 2.0));
    assert!((state("test:gold", "test:copper").hardness - 1.2).abs() < 1e-6);

    // The block's item has the stats of the state it places.
    let item = content.item("test:wire").unwrap();
    let info = content
        .item_state(
            item.state(&[("core", "test:copper"), ("coat", "test:gold")])
                .unwrap(),
        )
        .unwrap();
    assert_eq!(info.stat("capacity"), Some(capacity));
}

/// An item's own `stats` add to and replace those of the block it
/// places.
#[test]
fn item_stats_replace_the_blocks() {
    let content = stat_materials(&[
        (
            "blocks/wire.json5",
            r##"{ properties: { metal: { materials: ["copper", "gold"] } },
                 model: "quad", textures: { all: "a" },
                 stats: { capacity: 10, length: 1 } }"##,
        ),
        (
            "items/wire.json5",
            r##"{ stats: { capacity: { base: 5, stat: "conductivity", materials: { metal: 1 } } } }"##,
        ),
        (
            "items/spool.json5",
            r##"{ block: "wire", stats: { length: 50 } }"##,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let item = content.item("test:wire").unwrap();
    let gold = content
        .item_state(item.state(&[("metal", "test:gold")]).unwrap())
        .unwrap();
    assert_eq!(gold.stat("capacity"), Some(3.0));
    assert_eq!(gold.stat("length"), Some(1.0));
    let spool = content.default_item_info("test:spool").unwrap();
    assert_eq!(
        (spool.stat("capacity"), spool.stat("length")),
        (Some(10.0), Some(50.0))
    );
}

/// `hardness` and `weight` read as stats of blocks and items too, as
/// they do of materials.
#[test]
fn hardness_and_weight_are_stats() {
    let content = crate::content::base();
    let stone = content.state(content.default_state("defaria:stone").unwrap());
    assert_eq!(stone.stat("hardness"), Some(stone.hardness));
    assert_eq!(stone.stat("weight"), Some(stone.weight));
    let item = content.default_item_info("defaria:stone").unwrap();
    assert_eq!(item.stat("hardness"), Some(item.hardness));
    assert_eq!(item.stat("weight"), Some(item.weight));
}

/// A material without the stat counts as the attribute's `default`;
/// without one, it's an error naming the material and the stat.
#[test]
fn a_missing_stat_takes_the_default() {
    let content = stat_materials(&[(
        "blocks/wire.json5",
        r##"{ properties: { metal: { materials: "#metal" } }, model: "quad", textures: { all: "a" },
             stats: { conductivity: { materials: { metal: 1 }, default: 0.1 } } }"##,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let wire = content.block("test:wire").unwrap();
    let conductivity = |metal: &str| {
        content
            .state(wire.state(&[("metal", metal)]).unwrap())
            .stat("conductivity")
    };
    assert_eq!(conductivity("test:iron"), Some(0.1));
    assert_eq!(conductivity("test:gold"), Some(0.6));

    let content = stat_materials(&[(
        "blocks/wire.json5",
        r##"{ properties: { metal: { materials: "#metal" } }, model: "quad", textures: { all: "a" },
             stats: { conductivity: { materials: { metal: 1 } } } }"##,
    )]);
    let all = errors(&content);
    assert_eq!(content.errors.len(), 1, "{all}");
    assert!(all.contains("mods/test/blocks/wire.json5"), "{all}");
    assert!(
        all.contains(
            "`stats.conductivity` averages stat `conductivity` of `metal`, but material \
             `test:iron` has no `conductivity`"
        ),
        "{all}"
    );
    assert!(content.block("test:wire").is_none());
}

#[test]
fn stat_mistakes_are_reported() {
    let cases: &[(&str, &str, &str)] = &[
        // No material has it: a misspelling, most likely.
        (
            "blocks/x.json5",
            r##"{ properties: { metal: { materials: "copper" } }, model: "quad", textures: { all: "a" },
                 stats: { capacity: { stat: "conductivty", materials: { metal: 1 } } } }"##,
            "`stats.capacity` averages stat `conductivty`, which no material has",
        ),
        // Named like the field, by default.
        (
            "items/x.json5",
            r##"{ properties: { metal: { materials: "copper" } },
                 stats: { capacity: { materials: { metal: 1 } } } }"##,
            "`stats.capacity` averages stat `capacity`, which no material has",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, stats: { Capacity: 1 } }"##,
            "stat `Capacity` must be lowercase letters, digits and `_` only",
        ),
        (
            "blocks/x.json5",
            r##"{ model: "quad", textures: { all: "a" }, stats: { weight: 1 } }"##,
            "`weight` is a field of its own, not one of the `stats`",
        ),
        (
            "blocks/x.json5",
            r##"{ properties: { metal: { materials: "copper" } }, model: "quad", textures: { all: "a" },
                 stats: { conductivity: { materials: { metal: 1 }, default: -1 } } }"##,
            "`stats.conductivity` `default` must be a number of at least 0",
        ),
        (
            "materials/x.json5",
            r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1, stats: { hardness: 2 } }"##,
            "`hardness` is a field of its own, not one of the `stats`",
        ),
        (
            "materials/x.json5",
            r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1, stats: { "max-temp": 2 } }"##,
            "stat `max-temp` must be lowercase letters, digits and `_` only",
        ),
        (
            "materials/x.json5",
            r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1, stats: { conductivity: -2 } }"##,
            "`conductivity` must be a number of at least 0",
        ),
    ];
    for &(path, text, expected) in cases {
        let content = stat_materials(&[(path, text)]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
        assert!(all.contains(&format!("mods/test/{path}")), "{all}");
        assert!(all.contains(expected), "expected `{expected}` in: {all}");
    }
}
