use crate::content::{bake::test_util::*, test_mod as content};

/// An item with a block's id changes the block's item; other items are
/// new. Blocks can opt out of an item.
#[test]
fn items_extend_or_replace_block_items() {
    let content = content(&[
        (
            "blocks/thing.json5",
            r#"{ name: "Oak thing", model: "quad", textures: { all: "a" } }"#,
        ),
        (
            "blocks/plain.json5",
            r#"{ name: "Plain thing", model: "quad", textures: { all: "a" } }"#,
        ),
        ("items/plain.json5", r#"{ unique: true }"#),
        (
            "blocks/hidden.json5",
            r#"{ model: "quad", textures: { all: "a" }, item: false }"#,
        ),
        ("items/thing.json5", r#"{ name: "Special thing" }"#),
        ("items/seeds.json5", r#"{ texture: "b", block: "hidden" }"#),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let thing = content.default_item_info("test:thing").unwrap();
    assert_eq!(thing.name, "Special thing");
    assert!(!content.item("test:thing").unwrap().unique);
    assert_eq!(thing.block, content.default_state("test:thing"));
    // What an item file leaves out comes from the block.
    let plain = content.default_item_info("test:plain").unwrap();
    assert_eq!(plain.name, "Plain thing");
    assert!(content.item("test:plain").unwrap().unique);
    assert_eq!(plain.block, content.default_state("test:plain"));
    assert!(content.item("test:hidden").is_none());
    let seeds = content.default_item_info("test:seeds").unwrap();
    assert_eq!(seeds.name, "Seeds");
    assert_eq!(seeds.block, content.default_state("test:hidden"));
    assert_eq!(seeds.texture, content.texture("test:b"));
    assert_eq!(content.items.len(), 3);
}

/// Items have properties of their own, fill texture inputs from them,
/// and pass the ones a block shares on to the state they place.
#[test]
fn items_carry_their_property_values() {
    let content = materials(&[
        (
            "blocks/plank.json5",
            r##"{ properties: { wood: { materials: "#wood" } }, model: "quad", textures: { all: "a" } }"##,
        ),
        (
            "items/tool.json5",
            r##"{
                name: "{head} Tool",
                properties: { head: { materials: "#metal" }, wood: { materials: "#wood" } },
                texture: { texture: "ore", materials: { rock: "{wood}", ore: "{head}" } },
                block: "plank",
                unique: true,
            }"##,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let tool = content.item("test:tool").unwrap();
    assert_eq!(tool.state_count, 2);
    // Oak, not the default (birch comes first by id), so the value
    // has to reach the block.
    let oak = tool.state(&[("wood", "test:oak")]).unwrap();
    assert_ne!(oak, tool.default_state());
    let look = content.item_state(oak).unwrap();
    assert_eq!(look.name, "Iron Tool");
    assert_eq!(
        look.values,
        [
            ("head".into(), "test:iron".into()),
            ("wood".into(), "test:oak".into())
        ]
    );
    let plank = content.block("test:plank").unwrap();
    assert_ne!(look.block, Some(plank.default_state()));
    assert_eq!(look.block, plank.state(&[("wood", "test:oak")]));
    assert_eq!(
        look.texture,
        content.texture("test:ore(ore=test:iron,rock=test:oak)")
    );

    // An item offering a value the block doesn't have is a mistake.
    let content = materials(&[
        (
            "blocks/plank.json5",
            r##"{ properties: { wood: { materials: "birch" } }, model: "quad", textures: { all: "a" } }"##,
        ),
        (
            "items/tool.json5",
            r##"{ properties: { wood: { materials: "#wood" } }, block: "plank" }"##,
        ),
    ]);
    let all = errors(&content);
    assert!(
        all.contains("items/tool.json5")
            && all.contains("block `test:plank` has no `wood` value `test:oak`"),
        "{all}"
    );
    assert!(content.item("test:tool").is_none());
}

/// `item: true` puts a plain property in the block's item and
/// `item: false` leaves a material one out; the item places the block
/// with its values and the rest at their defaults.
#[test]
fn the_item_flag_picks_the_block_items_properties() {
    let content = materials(&[(
        "blocks/x.json5",
        r##"{
            properties: {
                color: { values: ["red", "blue"], item: true },
                wood: { materials: "#wood", item: false },
                rock: { materials: "#rock" },
            },
            model: "quad",
            textures: { all: "a" },
        }"##,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let block = content.block("test:x").unwrap();
    let item = content.item("test:x").unwrap();
    let names: Vec<&str> = item.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["color", "rock"]);
    assert_eq!(item.state_count, 2);
    let blue = item.state(&[("color", "blue")]).unwrap();
    assert_eq!(
        content.item_state(blue).unwrap().block,
        block.state(&[("color", "blue")])
    );
}

/// A block's item file fills `{property}` in its name and texture from
/// each item state's values.
#[test]
fn a_block_items_file_fills_in_its_values() {
    let content = materials(&[
        (
            "blocks/log.json5",
            r##"{ properties: { wood: { materials: "#wood" } }, model: "quad", textures: { all: "a" } }"##,
        ),
        (
            "items/log.json5",
            r##"{ name: "{wood} Thing", texture: { texture: "bark", materials: { wood: "{wood}" } } }"##,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let item = content.item("test:log").unwrap();
    let oak = content
        .item_state(item.state(&[("wood", "test:oak")]).unwrap())
        .unwrap();
    assert_eq!(oak.name, "Oak Thing");
    assert_eq!(oak.texture, content.texture("test:bark(wood=test:oak)"));
    assert_eq!(
        content.textures[oak.texture.unwrap() as usize].template,
        "test:bark"
    );
}
