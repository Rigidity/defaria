use bevy::prelude::*;

use crate::content::{Direction, bake::test_util::*, test_mod as content};

/// `default` picks a property's default value, which comes first so the
/// block's default state has it; the other values keep their order.
/// Without it, the default is the first value. Placement still gives
/// the values it names.
#[test]
fn default_names_the_default_value() {
    let content = materials(&[
        (
            "blocks/plank.json5",
            r##"{
                properties: {
                    wood: { materials: "#wood", default: "oak" },
                    ore: { materials: "#ore", optional: true, default: "coal" },
                    axis: { values: ["x", "y", "z"], default: "y", place: "face" },
                    size: ["small", "big"],
                },
                model: "quad",
                textures: { all: "a" },
            }"##,
        ),
        (
            "blocks/plain.json5",
            r##"{
                properties: { ore: { materials: "#ore", optional: true } },
                model: "quad",
                textures: { all: "a" },
            }"##,
        ),
    ]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let plank = content.block("test:plank").unwrap();
    let values = |name: &str| {
        plank
            .properties
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .values
            .clone()
    };
    // By id, birch would come first.
    assert_eq!(values("wood"), ["test:oak", "test:birch"]);
    assert_eq!(values("ore"), ["test:coal", "none", "test:iron"]);
    assert_eq!(values("axis"), ["y", "x", "z"]);
    assert_eq!(values("size"), ["small", "big"]);
    let default = content.state(plank.default_state());
    for (name, value) in [
        ("axis", "y"),
        ("ore", "test:coal"),
        ("size", "small"),
        ("wood", "test:oak"),
    ] {
        assert!(
            default.values.contains(&(name.into(), value.into())),
            "{:?}",
            default.values
        );
    }
    // Placing on a block's east side still sets `axis` to `x`.
    let placed = content.placed(plank.default_state(), Direction::East, Vec3::NEG_X);
    assert!(
        content
            .state(placed)
            .values
            .contains(&("axis".into(), "x".into()))
    );
    // An optional material defaults to `none`.
    let plain = content.block("test:plain").unwrap();
    assert_eq!(
        content.state(plain.default_state()).values,
        [("ore".to_owned(), "none".to_owned())]
    );
}

#[test]
fn a_default_that_isnt_a_value_is_an_error() {
    for (property, expected) in [
        (
            r##"{ materials: "#wood", default: "rock" }"##,
            "`test:rock` isn't one",
        ),
        (r##"{ values: ["a", "b"], default: "c" }"##, "`c` isn't one"),
        (
            r##"{ materials: "#wood", default: "none" }"##,
            "`none` isn't one",
        ),
    ] {
        let block = format!(
            r#"{{ properties: {{ p: {property} }}, model: "quad", textures: {{ all: "a" }} }}"#
        );
        let content = materials(&[("blocks/x.json5", Box::leak(block.into_boxed_str()))]);
        let all = errors(&content);
        assert!(all.contains(expected), "{property}: {all}");
        assert!(content.block("test:x").is_none());
    }
}

/// A material property takes its values from tags and lists, in
/// order, without repeats; `{property}` gives the material's id in
/// texture bindings and its name in names.
#[test]
fn material_properties_list_materials_and_name_states() {
    let content = materials(&[(
        "blocks/log.json5",
        r##"{
            name: "{wood} Log",
            properties: { wood: { materials: ["oak", "#wood"] } },
            model: "quad",
            textures: { all: { texture: "bark", materials: { wood: "{wood}" } } },
        }"##,
    )]);
    assert!(content.errors.is_empty(), "{}", errors(&content));
    let log = content.block("test:log").unwrap();
    assert_eq!(log.properties[0].values, ["test:oak", "test:birch"]);
    assert!(log.properties[0].materials && log.properties[0].item);
    let birch = log.state(&[("wood", "test:birch")]).unwrap();
    assert_eq!(content.state(log.default_state()).name, "Oak Log");
    // A material with no name is named after its id.
    assert_eq!(content.state(birch).name, "Birch Log");
    // Each wood bakes its own tile, the oak one in its bark palette and
    // birch (with no bark palette) in its main one.
    let tile = |state| &content.textures[content.state(state).faces[0].layer as usize];
    let oak_tile = tile(log.default_state());
    assert_eq!(oak_tile.id, "test:bark(wood=test:oak)");
    assert_eq!(
        oak_tile.layers[0].palette,
        content.materials["test:oak"].palettes["bark"]
    );
    assert_eq!(
        tile(birch).layers[0].palette,
        content.materials["test:birch"].palette
    );
    // One item per wood, each placing its own.
    let item = content.item("test:log").unwrap();
    assert_eq!(item.state_count, 2);
    let birch_item = item.state(&[("wood", "test:birch")]).unwrap();
    let look = content.item_state(birch_item).unwrap();
    assert_eq!((look.name.as_str(), look.block), ("Birch Log", Some(birch)));
    assert_eq!(content.item_of(birch_item).unwrap().id, "test:log");
}

#[test]
fn a_block_with_too_many_states_is_refused() {
    let values: Vec<String> = (0..17).map(|i| format!("\"v{i}\"")).collect();
    let values = values.join(", ");
    let block = format!(
        r#"{{ properties: {{ a: [{values}], b: [{values}], c: [{values}] }}, model: "quad", textures: {{ all: "a" }} }}"#
    );
    let content = content(&[("blocks/x.json5", &block)]);
    assert!(
        errors(&content).contains("too many"),
        "{}",
        errors(&content)
    );
}

/// A property's name and values go into state names (`id[name=value]`),
/// so they can't hold the characters those use.
#[test]
fn names_and_values_that_break_state_names_are_errors() {
    for (properties, expected) in [
        (r#"{ p: ["a,b", "c"] }"#, "value `a,b` has a `,`"),
        (r#"{ p: ["a", "b=c"] }"#, "value `b=c` has a `=`"),
        (r#"{ p: ["", "c"] }"#, "value `` is empty"),
        (
            r#"{ "x=y": ["a", "b"] }"#,
            "property `x=y`: its name has a `=`",
        ),
        (
            r#"{ "x]": ["a", "b"] }"#,
            "property `x]`: its name has a `]`",
        ),
    ] {
        let block =
            format!(r#"{{ properties: {properties}, model: "quad", textures: {{ all: "a" }} }}"#);
        let content = materials(&[("blocks/x.json5", Box::leak(block.into_boxed_str()))]);
        let all = errors(&content);
        assert!(all.contains(expected), "{properties}: {all}");
        assert!(content.block("test:x").is_none());
    }
}
