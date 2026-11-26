use super::*;
use crate::{content::base, testing::test_mods};

/// The base game and mod `test`, with `files`.
fn with(files: &[(&str, &str)]) -> Content {
    Content::from_mods(test_mods(files))
}

/// The errors loading `recipe` as `recipes/r.json5` gives.
fn errors(recipe: &str) -> String {
    with(&[("recipes/r.json5", recipe)]).errors.join("\n")
}

fn item(content: &Content, id: &str, values: &[(&str, &str)]) -> ItemState {
    content.item(id).unwrap().state(values).unwrap()
}

/// One file smelts every ore that has an ingot into it, whatever rock it's
/// in: coal, an ore with no ingot, makes no recipe.
#[test]
fn a_templated_recipe_smelts_each_ore_into_its_ingot() {
    let content = base();
    let ids: Vec<&str> = content
        .recipes_of("smelting")
        .map(|recipe| recipe.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "defaria:smelt_ore[ore=defaria:copper]",
            "defaria:smelt_ore[ore=defaria:gold]",
            "defaria:smelt_ore[ore=defaria:iron]",
            "defaria:smelt_sand",
        ]
    );
    let iron = content
        .recipe("defaria:smelt_ore[ore=defaria:iron]")
        .unwrap();
    let ore =
        |rock: &str, ore: &str| item(content, "defaria:stone", &[("rock", rock), ("ore", ore)]);
    assert!(iron.inputs[0].takes(ore("defaria:granite", "defaria:iron")));
    assert!(iron.inputs[0].takes(ore("defaria:stone", "defaria:iron")));
    assert!(!iron.inputs[0].takes(ore("defaria:stone", "defaria:copper")));
    assert!(!iron.inputs[0].takes(ore("defaria:stone", NONE)));
    assert_eq!(
        iron.outputs,
        [RecipeOutput::Item {
            item: item(content, "defaria:ingot", &[("metal", "defaria:iron")]),
            count: 1,
        }]
    );
    assert_eq!(
        content.item_name(item(content, "defaria:ingot", &[("metal", "defaria:gold")])),
        Some("Gold Ingot")
    );
    assert_eq!((iron.energy, iron.ticks), (2000, 80));
    assert!(
        content
            .recipe("defaria:smelt_ore[ore=defaria:coal]")
            .is_none()
    );
}

/// Fluids are counted in thousandths of a block, and a generator's recipe
/// makes energy.
#[test]
fn recipes_take_fluids_and_make_energy() {
    let content = base();
    let lava = content.recipe("defaria:burn_lava").unwrap();
    assert_eq!(
        lava.inputs,
        [RecipeInput::Fluid {
            fluid: "defaria:lava".into(),
            amount: 100,
        }]
    );
    assert_eq!((lava.energy, lava.generates(), lava.ticks), (0, 20000, 400));
    // A log of any wood, any way up.
    let log = content.recipe("defaria:burn_log").unwrap();
    let logs = content.item("defaria:log").unwrap();
    for offset in 0..logs.state_count {
        assert!(log.inputs[0].takes(ItemState(logs.first_state + offset)));
    }
}

/// A tick's share of an amount: the ticks add up to it exactly.
#[test]
fn shares_add_up_to_the_whole() {
    for (total, ticks) in [(0, 1), (2000, 80), (7, 3), (1, 20), (40000, 800), (5, 1)] {
        let sum: u64 = (0..ticks).map(|t| share(total, t, ticks)).sum();
        assert_eq!(sum, total, "{total} over {ticks}");
    }
    assert_eq!(share(2000, 0, 80), 25);
}

/// A recipe's materials can be listed, and a combination that names no
/// state is skipped while the rest load.
#[test]
fn materials_can_be_listed_and_missing_combinations_skipped() {
    let content = with(&[(
        "recipes/press.json5",
        r##"{
            type: "pressing",
            materials: { metal: ["defaria:iron", "defaria:coal"], wood: "#wood" },
            inputs: [
                { item: "defaria:ingot", when: { metal: "{metal}" }, count: 2 },
                { item: "defaria:planks", when: { wood: "{wood}" } },
            ],
            outputs: [{ item: "defaria:ingot", properties: { metal: "{metal}" }, count: 3 }],
            time: 1,
        }"##,
    )]);
    assert!(content.errors.is_empty(), "{:#?}", content.errors);
    let ids: Vec<&str> = content
        .recipes_of("pressing")
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "test:press[metal=defaria:iron,wood=defaria:birch]",
            "test:press[metal=defaria:iron,wood=defaria:oak]",
            "test:press[metal=defaria:iron,wood=defaria:spruce]",
        ]
    );
    let recipe = &content.recipe(ids[1]).unwrap();
    let RecipeInput::Item { items, count } = &recipe.inputs[1] else {
        panic!()
    };
    assert_eq!(
        (items.as_slice(), *count),
        (
            &[item(&content, "defaria:planks", &[("wood", "defaria:oak")])][..],
            1
        )
    );
    assert_eq!(recipe.ticks, 20);
}

#[test]
fn bad_recipes_are_errors() {
    let cases = [
        (
            r#"{ type: "s", inputs: [], time: 1 }"#,
            "at least one input",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:glass" }], time: 0 }"#,
            "must be over 0",
        ),
        (
            r#"{ type: "S", inputs: [{ item: "defaria:glass" }], time: 1 }"#,
            "recipe type",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "nope" }], time: 1 }"#,
            "input 1: no item `test:nope`",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:glass", fluid: "defaria:water" }], time: 1 }"#,
            "an `item` or a `fluid`",
        ),
        (
            r#"{ type: "s", inputs: [{ fluid: "defaria:stone", amount: 1 }], time: 1 }"#,
            "`defaria:stone` isn't a fluid",
        ),
        (
            r#"{ type: "s", inputs: [{ fluid: "defaria:water" }], time: 1 }"#,
            "needs an `amount`",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:glass", count: 65 }], time: 1 }"#,
            "`count` is 65",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:log", when: { colour: "red" } }], time: 1 }"#,
            "no property `colour`",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:glass" }], outputs: [{ energy: 0 }], time: 1 }"#,
            "`energy` is 0",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:glass" }], outputs: [{ item: "defaria:ingot", properties: { metal: "defaria:oak" } }], time: 1 }"#,
            "output 1: item `defaria:ingot` has no `metal` of `defaria:oak`",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:{x}" }], time: 1 }"#,
            "isn't one of the recipe's `materials`",
        ),
        (
            r##"{ type: "s", materials: { m: "#rock" }, inputs: [{ item: "defaria:ingot", when: { metal: "{m}" } }], time: 1 }"##,
            "no combination of its materials makes a recipe",
        ),
        (
            r##"{ type: "s", materials: { m: "#nothing" }, inputs: [{ item: "defaria:glass" }], time: 1 }"##,
            "names no materials",
        ),
        (
            r#"{ type: "s", inputs: [{ item: "defaria:glass" }], time: 1, speed: 2 }"#,
            "unknown field",
        ),
    ];
    for (recipe, error) in cases {
        let errors = errors(recipe);
        assert!(errors.contains(error), "{recipe}: {errors}");
    }
}

/// A machine's `recipes` finds its input and output slot groups and tanks
/// by name, or by being called `input` and `output`.
#[test]
fn machines_find_their_recipe_slots_and_tanks() {
    let content = base();
    let furnace = content
        .machine(content.default_state("defaria:electric_furnace").unwrap())
        .unwrap();
    let recipes = furnace.recipes.as_ref().unwrap();
    assert_eq!(recipes.kind, "smelting");
    assert_eq!(recipes.input_slots, [furnace.slot_group("input").unwrap()]);
    assert_eq!(
        recipes.output_slots,
        [furnace.slot_group("output").unwrap()]
    );
    assert!(recipes.input_tanks.is_empty() && recipes.output_tanks.is_empty());
    let generator = content
        .machine(
            content
                .default_state("defaria:combustion_generator")
                .unwrap(),
        )
        .unwrap();
    let recipes = generator.recipes.as_ref().unwrap();
    assert_eq!(
        (
            recipes.input_slots.as_slice(),
            recipes.input_tanks.as_slice()
        ),
        (&[0][..], &[0][..])
    );
    assert!(recipes.output_slots.is_empty() && recipes.output_tanks.is_empty());
}

/// A recipe machine's input groups take only what one of its type's
/// recipes takes; its other groups, and other machines', take anything.
#[test]
fn input_groups_take_only_what_a_recipe_takes() {
    let content = base();
    let furnace = content
        .machine(content.default_state("defaria:electric_furnace").unwrap())
        .unwrap();
    let (input, output) = (
        furnace.slot_group("input").unwrap(),
        furnace.slot_group("output").unwrap(),
    );
    let iron_ore = item(content, "defaria:stone", &[("ore", "defaria:iron")]);
    let sand = item(content, "defaria:soil", &[("soil", "defaria:sand")]);
    let glass = content.default_item("defaria:glass").unwrap();
    let coal_ore = item(content, "defaria:stone", &[("ore", "defaria:coal")]);
    assert!(content.slot_group_takes(furnace, input, iron_ore));
    assert!(content.slot_group_takes(furnace, input, sand));
    assert!(!content.slot_group_takes(furnace, input, glass));
    // Coal ore has no ingot to smelt into; it burns in a generator.
    assert!(!content.slot_group_takes(furnace, input, coal_ore));
    assert!(content.recipes_take("combustion", coal_ore));
    assert!(content.slot_group_takes(furnace, output, glass));
    let crate_box = content
        .machine(content.default_state("defaria:crate").unwrap())
        .unwrap();
    assert!(content.slot_group_takes(crate_box, 0, glass));
}

#[test]
fn bad_recipe_machines_are_errors() {
    let block = |machine: &str| {
        let text = format!(
            r#"{{ model: "defaria:cube", textures: {{ all: "defaria:machine_side" }}, machine: {machine} }}"#
        );
        with(&[("blocks/m.json5", &text)]).errors.join("\n")
    };
    let errors = block(r#"{ slots: { input: 1 }, recipes: { type: "s", inputs: ["nope"] } }"#);
    assert!(
        errors.contains("names `nope`, which isn't one of its slot groups or tanks"),
        "{errors}"
    );
    let errors = block(
        r#"{ slots: { input: 1, output: 1 }, recipes: { type: "s", inputs: ["input", "input"] } }"#,
    );
    assert!(
        errors.contains("machine `recipes` names `input` twice"),
        "{errors}"
    );
    let errors = block(r#"{ energy: { capacity: 10, use: 5 }, recipes: "s" }"#);
    assert!(errors.contains("leave out `energy.use`"), "{errors}");
    let errors = block(r#"{ recipes: { type: "s", speed: 2 } }"#);
    assert!(errors.contains("unknown field"), "{errors}");
}

/// Recipe machines that can never run are errors once everything has
/// loaded, naming the block and the recipes; one that can run some of its
/// type's recipes only gets a warning, and the base game has none.
#[test]
fn recipe_machines_that_cant_run_are_errors() {
    assert_eq!(check_recipe_machines(base()), (vec![], vec![]));
    let block = |machine: &str, recipe: &str| {
        let text = format!(
            r#"{{ model: "defaria:cube", textures: {{ all: "defaria:machine_side" }}, machine: {machine} }}"#
        );
        let content = with(&[("blocks/m.json5", &text), ("recipes/r.json5", recipe)]);
        let (_, warnings) = check_recipe_machines(&content);
        (content.errors.join("\n"), warnings.join("\n"))
    };
    let ore = r#"{ type: "smelting", inputs: [{ item: "defaria:planks" }], outputs: [{ item: "defaria:glass" }], energy: 100, time: 1 }"#;
    // No recipes of its type at all.
    let (errors, _) = block(r#"{ slots: { input: 1 }, recipes: "nothing" }"#, ore);
    assert!(
        errors.contains("block `test:m` runs `nothing` recipes, and there are none"),
        "{errors}"
    );
    // No input slots, no output slots, no energy buffer: it can run none
    // of the smelting recipes, the base game's or the test's.
    let (errors, _) = block(
        r#"{ tanks: { input: { capacity: 1 } }, recipes: "smelting" }"#,
        ore,
    );
    assert!(
        errors.contains("block `test:m` can't run any of its `smelting` recipes"),
        "{errors}"
    );
    assert!(
        errors.contains("`test:r` takes items, and the machine has no input slot group"),
        "{errors}"
    );
    let (errors, _) = block(r#"{ slots: { input: 1 }, recipes: "smelting" }"#, ore);
    assert!(
        errors.contains("gives items, and the machine has no output slot group"),
        "{errors}"
    );
    let (errors, _) = block(
        r#"{ slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
        ore,
    );
    assert!(
        errors.contains("uses energy, and the machine has no energy buffer"),
        "{errors}"
    );
    // A generator making more in a tick than its buffer holds.
    let burn = r#"{ type: "burning", inputs: [{ item: "defaria:planks" }], outputs: [{ energy: 1000 }], time: 1 }"#;
    let (errors, _) = block(
        r#"{ energy: { capacity: 10, max_out: 10 }, slots: { input: 1 }, recipes: "burning" }"#,
        burn,
    );
    assert!(
        errors.contains("`test:r` makes 50 J a tick, more than the machine's buffer holds (10 J)"),
        "{errors}"
    );
    // Fluids its tanks don't take.
    let melt = r#"{ type: "melting", inputs: [{ fluid: "defaria:lava", amount: 1 }], outputs: [{ fluid: "defaria:water", amount: 1 }], time: 1 }"#;
    let (errors, _) = block(
        r#"{ tanks: { input: { capacity: 1, accepts: ["defaria:water"] }, output: { capacity: 1 } }, recipes: "melting" }"#,
        melt,
    );
    assert!(
        errors.contains("takes `defaria:lava`, and no input tank of the machine holds it"),
        "{errors}"
    );
    // Able to run the base game's smelting but not the test's: a warning.
    let (errors, warnings) = block(
        r#"{ ports: [{ faces: ["up"], network: "defaria:electric", role: "consume" }], energy: { capacity: 100, max_in: 100 }, slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
        r#"{ type: "smelting", inputs: [{ item: "defaria:planks" }], outputs: [{ fluid: "defaria:water", amount: 1 }], time: 1 }"#,
    );
    assert_eq!(errors, "");
    assert!(
        warnings.contains("block `test:m` can't run some of its `smelting` recipes: `test:r` gives `defaria:water`"),
        "{warnings}"
    );
    // Fluid amounts larger than the tanks that take that fluid hold, in
    // or out.
    let big = r#"{ type: "melting", inputs: [{ fluid: "defaria:lava", amount: 3 }], outputs: [{ fluid: "defaria:water", amount: 1 }], time: 1 }"#;
    let (errors, _) = block(
        r#"{ tanks: { a: { capacity: 2 }, b: { capacity: 4, accepts: ["defaria:water"] }, output: { capacity: 1 } }, recipes: { type: "melting", inputs: ["a", "b"], outputs: ["output"] } }"#,
        big,
    );
    assert!(
        errors.contains("`test:r` takes 3 blocks of `defaria:lava`, more than the machine's input tanks hold (2)"),
        "{errors}"
    );
    let (errors, _) = block(
        r#"{ tanks: { input: { capacity: 1 }, output: { capacity: 1 } }, recipes: "melting" }"#,
        r#"{ type: "melting", inputs: [{ fluid: "defaria:lava", amount: 1 }], outputs: [{ fluid: "defaria:water", amount: 2 }], time: 1 }"#,
    );
    assert!(
        errors.contains("`test:r` gives 2 blocks of `defaria:water`, more than the machine's output tanks hold (1)"),
        "{errors}"
    );
    // More unique items than slots: each takes a slot of its own.
    let unique = |recipe: &str| {
        let machine = r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { input: 2, output: 2 }, recipes: "bench" } }"#;
        let content = with(&[
            ("blocks/m.json5", machine),
            ("items/gem.json5", r#"{ unique: true }"#),
            ("recipes/r.json5", recipe),
        ]);
        content.errors.join("\n")
    };
    let errors = unique(
        r#"{ type: "bench", inputs: [{ item: "test:gem", count: 3 }], outputs: [{ item: "defaria:glass" }], time: 1 }"#,
    );
    assert!(
        errors.contains("`test:r` takes 3 unique items, more than the machine's input slots (2)"),
        "{errors}"
    );
    let errors = unique(
        r#"{ type: "bench", inputs: [{ item: "defaria:glass" }], outputs: [{ item: "test:gem", count: 3 }], time: 1 }"#,
    );
    assert!(
        errors.contains("`test:r` gives 3 unique items, more than the machine's output slots (2)"),
        "{errors}"
    );
    // More kinds of item, or of fluid, than slots or tanks to hold them
    // apart.
    let (errors, _) = block(
        r#"{ slots: { input: 1, output: 2 }, recipes: "mixing" }"#,
        r#"{ type: "mixing", inputs: [{ item: "defaria:planks" }, { item: "defaria:glass" }], outputs: [{ item: "defaria:log" }], time: 1 }"#,
    );
    assert!(
        errors.contains(
            "`test:r` takes items needing 2 slots, more than the machine's input slots (1)"
        ),
        "{errors}"
    );
    let (errors, _) = block(
        r#"{ slots: { input: 2, output: 1 }, recipes: "mixing" }"#,
        r#"{ type: "mixing", inputs: [{ item: "defaria:planks" }, { item: "defaria:glass" }], outputs: [{ item: "defaria:log" }, { item: "defaria:planks", count: 2 }], time: 1 }"#,
    );
    assert!(
        errors.contains(
            "`test:r` gives items needing 2 slots, more than the machine's output slots (1)"
        ),
        "{errors}"
    );
    let (errors, _) = block(
        r#"{ tanks: { input: { capacity: 1 }, output: { capacity: 1 } }, recipes: "mixing" }"#,
        r#"{ type: "mixing", inputs: [{ fluid: "defaria:lava", amount: 1 }, { fluid: "defaria:water", amount: 1 }], outputs: [{ fluid: "defaria:water", amount: 1 }], time: 1 }"#,
    );
    assert!(
        errors.contains(
            "`test:r` takes 2 fluids, more than the machine's input tanks that hold them (1)"
        ),
        "{errors}"
    );
    // Two of the same item, or inputs that could share a slot, are fine.
    let (errors, _) = block(
        r#"{ slots: { input: 1, output: 1 }, recipes: "mixing" }"#,
        r#"{ type: "mixing", inputs: [{ item: "defaria:log" }, { item: "defaria:log", when: { wood: "defaria:oak" } }], outputs: [{ item: "defaria:planks" }, { item: "defaria:planks", count: 3 }], time: 1 }"#,
    );
    assert_eq!(errors, "");
    // Uses energy, and nothing fills its buffer: no port that takes it, or
    // one with no `max_in`.
    for machine in [
        r#"{ energy: { capacity: 100, max_in: 100 }, slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
        r#"{ ports: [{ faces: ["up"], network: "defaria:electric", role: "consume" }], energy: { capacity: 100 }, slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
        r#"{ ports: [{ faces: ["up"], network: "defaria:electric", role: "produce" }], energy: { capacity: 100, max_in: 100, max_out: 100 }, slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
    ] {
        let (errors, _) = block(machine, ore);
        assert!(
            errors.contains("`test:r` uses energy, and nothing can fill the machine's buffer"),
            "{machine}: {errors}"
        );
    }
    // A script may fill it, up to its `max_generate`.
    let (errors, _) = block(
        r#"{ energy: { capacity: 100, max_generate: 100 }, slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
        ore,
    );
    assert_eq!(errors, "");
    // A recipe type no machine runs.
    let (_, warnings) = block(
        r#"{ slots: { input: 1, output: 1 }, recipes: "smelting" }"#,
        burn,
    );
    assert!(
        warnings.contains("no machine runs `burning` recipes"),
        "{warnings}"
    );
}
