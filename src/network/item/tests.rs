use std::{path::Path, sync::OnceLock, time::Duration};

use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

use super::*;
use crate::{
    block_entity::{BlockEntities, BlockEntityPlugin},
    connections::ConnectionsPlugin,
    content::{
        ItemState, MODS_DIR,
        load::{ModFiles, read_mods},
    },
    light::LightPlugin,
    machine::MachinePlugin,
    network::NetworkPlugin,
    streaming::{ChunkLoader, StreamingPlugin, StreamingSettings, StreamingStatus},
    terrain::{TerrainPlugin, Voxels},
    voxel_material::BlockMaterial,
};

/// The base game, and mod `test` with filtered bins: `wood_bin` takes
/// only items made of wood, `planks_bin` only planks, `metal_out` offers
/// only metal items, and `bad_*` are definitions that must fail.
fn content() -> &'static Content {
    static CONTENT: OnceLock<Content> = OnceLock::new();
    CONTENT.get_or_init(|| {
        let (mut mods, errors) = read_mods(Path::new("assets/mods"), MODS_DIR);
        assert!(errors.is_empty(), "{errors:?}");
        let bin = |filter: &str, role: &str| {
            format!(
                r#"{{ model: "defaria:cube", textures: {{ all: "defaria:crate_side" }},
                     machine: {{
                       ports: [
                         {{ faces: ["up"], network: "defaria:item", role: "{role}", filter: {filter} }},
                         {{ faces: ["down"], network: "defaria:item", role: "output" }},
                       ],
                       slots: {{ items: 2 }},
                     }} }}"#
            )
        };
        let files = [
            (
                "blocks/wood_bin.json5",
                bin(r##"{ materials: ["#wood"] }"##, "input"),
            ),
            (
                "blocks/planks_bin.json5",
                bin(r#"{ items: ["defaria:planks"] }"#, "input"),
            ),
            (
                "blocks/metal_out.json5",
                bin(r##"{ materials: ["#metal"] }"##, "output"),
            ),
        ];
        let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
        mods.push(ModFiles::new(
            "mods/test",
            r#"{ id: "test", name: "Test", dependencies: ["defaria"] }"#,
            &files,
        ));
        let content = Content::from_mods(mods);
        assert!(content.errors.is_empty(), "{:#?}", content.errors);
        content
    })
}

fn item(id: &str, values: &[(&str, &str)]) -> ItemState {
    content().item(id).unwrap().state(values).unwrap()
}

fn planks() -> ItemState {
    item("defaria:planks", &[])
}

fn group(size: usize, items: &[(ItemState, u32)]) -> SlotGroup {
    let mut group = SlotGroup {
        name: "items".into(),
        slots: vec![None; size],
    };
    for &(item, count) in items {
        assert!(
            group
                .insert(content(), ItemStack::new(item, count))
                .is_none()
        );
    }
    group
}

fn count(groups: &[SlotGroup], item: ItemState) -> u64 {
    groups
        .iter()
        .flat_map(|g| g.slots.iter().flatten())
        .filter(|s| s.item == item)
        .map(|s| s.count as u64)
        .sum()
}

fn end(group: usize, role: PortRole) -> Endpoint<'static> {
    Endpoint {
        group,
        role,
        filter: None,
        machine: None,
    }
}

fn filter(block: &str) -> &'static ItemFilter {
    let state = content().default_state(block).unwrap();
    let machine = content().machine(state).unwrap();
    &machine.filters[0]
}

/// One output feeding two inputs sends them items in turn.
#[test]
fn one_source_feeds_two_machines_in_turn() {
    let mut groups = [group(9, &[(planks(), 10)]), group(9, &[]), group(9, &[])];
    let ends = [
        end(0, PortRole::Output),
        end(1, PortRole::Input),
        end(2, PortRole::Input),
    ];
    let mut cursor = Cursor::default();
    for moved in 1..=10 {
        assert_eq!(route(content(), &mut groups, &ends, &mut cursor, 1), 1);
        let (a, b) = (groups[1].total(), groups[2].total());
        assert!(a.abs_diff(b) <= 1, "after {moved}: {a} and {b}");
    }
    assert_eq!((groups[1].total(), groups[2].total()), (5, 5));
    // Nothing left to give.
    assert_eq!(route(content(), &mut groups, &ends, &mut cursor, 5), 0);
}

/// Inputs fill before stores; stores give to inputs but not to each
/// other; nothing goes back into the group it came from.
#[test]
fn inputs_come_before_stores_and_stores_dont_trade() {
    let mut groups = [
        group(9, &[(planks(), 4)]),
        group(1, &[(planks(), 62)]),
        group(9, &[]),
    ];
    let ends = [
        end(0, PortRole::Output),
        end(1, PortRole::Input),
        end(2, PortRole::Store),
    ];
    let mut cursor = Cursor::default();
    assert_eq!(route(content(), &mut groups, &ends, &mut cursor, 4), 4);
    // The input took two (it was full then), the store the rest.
    assert_eq!(
        groups.iter().map(SlotGroup::total).collect::<Vec<_>>(),
        [0, 64, 2]
    );

    let mut groups = [group(9, &[(planks(), 5)]), group(9, &[])];
    let stores = [end(0, PortRole::Store), end(1, PortRole::Store)];
    assert_eq!(route(content(), &mut groups, &stores, &mut cursor, 5), 0);
    // A crate's own input and output, on one network.
    let own = [end(0, PortRole::Output), end(0, PortRole::Input)];
    assert_eq!(route(content(), &mut groups, &own, &mut cursor, 5), 0);
    // A store gives to an input.
    let ends = [end(0, PortRole::Store), end(1, PortRole::Input)];
    assert_eq!(route(content(), &mut groups, &ends, &mut cursor, 5), 5);
    assert_eq!(groups[1].total(), 5);
}

/// A full destination takes nothing more; the items go elsewhere, or
/// stay where they are.
#[test]
fn full_destinations_take_nothing() {
    let stone = item("defaria:stone", &[]);
    let mut groups = [
        group(9, &[(planks(), 100)]),
        group(1, &[(stone, 64)]),
        group(1, &[]),
    ];
    let ends = [
        end(0, PortRole::Output),
        end(1, PortRole::Input),
        end(2, PortRole::Input),
    ];
    let mut cursor = Cursor::default();
    assert_eq!(route(content(), &mut groups, &ends, &mut cursor, 64), 64);
    assert_eq!(groups[2].total(), 64);
    assert_eq!(route(content(), &mut groups, &ends, &mut cursor, 64), 0);
    assert_eq!(count(&groups, planks()), 100);
    assert_eq!(count(&groups, stone), 64);
    assert_eq!(groups[0].total(), 36);
}

/// Inputs with filters take only what passes; outputs with filters offer
/// only what passes.
#[test]
fn filters_pick_what_passes() {
    let content = content();
    let birch = item("defaria:planks", &[("wood", "defaria:birch")]);
    let log = item("defaria:log", &[]);
    let stone = item("defaria:stone", &[]);
    let pick = item("defaria:pickaxe", &[]);
    let wood = filter("test:wood_bin");
    let planks_only = filter("test:planks_bin");
    let metal = filter("test:metal_out");
    for (item, wood_passes, planks_passes, metal_passes) in [
        (planks(), true, true, false),
        (birch, true, true, false),
        (log, true, false, false),
        (stone, false, false, false),
        (pick, true, false, true),
    ] {
        let name = content.saved_item_name(item).unwrap();
        assert_eq!(wood.passes(content, item), wood_passes, "{name} wood");
        assert_eq!(
            planks_only.passes(content, item),
            planks_passes,
            "{name} planks"
        );
        assert_eq!(metal.passes(content, item), metal_passes, "{name} metal");
    }

    // Stone and logs to a planks-only input and a wood input: the logs go
    // to wood, the stone nowhere.
    let mut groups = [
        group(9, &[(stone, 3), (log, 3), (birch, 2)]),
        group(9, &[]),
        group(9, &[]),
    ];
    let ends = [
        end(0, PortRole::Output),
        Endpoint {
            filter: Some(planks_only),
            ..end(1, PortRole::Input)
        },
        Endpoint {
            filter: Some(wood),
            ..end(2, PortRole::Input)
        },
    ];
    let mut cursor = Cursor::default();
    assert_eq!(route(content, &mut groups, &ends, &mut cursor, 64), 5);
    assert_eq!(count(&groups[..1], stone), 3);
    assert_eq!(count(&groups[1..2], log), 0);
    assert_eq!(count(&groups[2..], log), 3);
    assert_eq!(count(&groups[1..], birch), 2);

    // An output that offers only metal: the pickaxe goes, the stone stays.
    let mut groups = [group(9, &[(stone, 3), (pick, 1)]), group(9, &[])];
    let ends = [
        Endpoint {
            filter: Some(metal),
            ..end(0, PortRole::Output)
        },
        end(1, PortRole::Input),
    ];
    assert_eq!(route(content, &mut groups, &ends, &mut cursor, 64), 1);
    assert_eq!(count(&groups[1..], pick), 1);
}

/// However items are routed, every item is somewhere, as many as before.
#[test]
fn routing_never_makes_or_loses_items() {
    let content = content();
    let kinds = [
        planks(),
        item("defaria:log", &[]),
        item("defaria:stone", &[]),
        item("defaria:pickaxe", &[]),
    ];
    let mut groups = [
        group(9, &[(kinds[0], 300), (kinds[1], 70), (kinds[3], 1)]),
        group(3, &[(kinds[2], 100)]),
        group(2, &[(kinds[1], 10)]),
        group(4, &[]),
        group(1, &[]),
    ];
    let before: Vec<u64> = kinds.iter().map(|&k| count(&groups, k)).collect();
    let roles = [
        PortRole::Output,
        PortRole::Store,
        PortRole::Input,
        PortRole::Store,
        PortRole::Input,
    ];
    let wood = filter("test:wood_bin");
    let mut ends: Vec<Endpoint> = roles
        .iter()
        .enumerate()
        .map(|(i, &role)| end(i, role))
        .collect();
    ends[4].filter = Some(wood);
    // The store's group, also as an output on another face.
    ends.push(end(1, PortRole::Output));
    let mut cursor = Cursor::default();
    let mut total = 0;
    for budget in [1, 2, 3, 5, 8, 13, 21, 34, 55, 64, 64, 64] {
        total += route(content, &mut groups, &ends, &mut cursor, budget);
        let now: Vec<u64> = kinds.iter().map(|&k| count(&groups, k)).collect();
        assert_eq!(now, before);
    }
    assert!(total > 0);
}

/// Merging keeps a network's round-robin; splitting starts afresh, as
/// nothing in it is an item.
#[test]
fn routing_state_merges_and_splits() {
    let mut a = ItemRouting {
        credit: 500,
        cursor: Cursor { giver: 1, taker: 2 },
    };
    a.merge(ItemRouting {
        credit: 750,
        ..default()
    });
    assert_eq!(a.credit, 750);
    assert_eq!(a.cursor.taker, 2);
    let share = Share {
        conduits: 1,
        capacity: 1.0,
    };
    assert_eq!(
        a.split(&[share, share], share),
        vec![ItemRouting::default(); 2]
    );
}

/// Every filter mistake is an error naming it.
#[test]
fn filter_mistakes_are_reported() {
    let base = || read_mods(Path::new("assets/mods"), MODS_DIR).0;
    for (machine, error) in [
        (
            r#"{ ports: [{ faces: ["up"], network: "defaria:item", role: "input", filter: {} }], slots: { a: 1 } }"#,
            "`filter` is empty",
        ),
        (
            r##"{ ports: [{ faces: ["up"], network: "defaria:item", role: "input", filter: { materials: ["#nope"] } }], slots: { a: 1 } }"##,
            "no material has tag `nope`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "defaria:item", role: "input", filter: { materials: ["defaria:cheese"] } }], slots: { a: 1 } }"#,
            "no material `defaria:cheese`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "defaria:fluid", role: "input", filter: { items: ["defaria:planks"] } }], tanks: { a: { capacity: 1 } } }"#,
            "only a port on an item network has a `filter`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "defaria:item", role: "input", filter: { items: ["defaria:cheese"] } }], slots: { a: 1 } }"#,
            "`filter.items` names `defaria:cheese`, which isn't an item",
        ),
        (
            r#"{ slots: { a: 1 }, emits: { item: "defaria:cheese" } }"#,
            "`emits.item` names `defaria:cheese`",
        ),
        (
            r#"{ slots: { a: 1, b: 1 }, emits: { item: "defaria:planks" } }"#,
            "say which with `slots`",
        ),
    ] {
        let mut mods = base();
        let block = format!(
            r#"{{ model: "defaria:cube", textures: {{ all: "defaria:crate_side" }}, machine: {machine} }}"#
        );
        mods.push(ModFiles::new(
            "mods/test",
            r#"{ id: "test", name: "Test", dependencies: ["defaria"] }"#,
            &[("blocks/bad.json5", &block)],
        ));
        let content = Content::from_mods(mods);
        assert!(
            content.errors.iter().any(|e| e.contains(error)),
            "{machine}: {:#?}",
            content.errors
        );
    }
}

/// The base game's creative item source emits planks into its only group.
#[test]
fn the_item_source_emits_planks() {
    let content = crate::content::base();
    let state = content.default_state("defaria:item_source").unwrap();
    let source = ItemSource::new(state, content).unwrap();
    assert_eq!(source.item, "defaria:planks");
    assert_eq!(source.slots, 0);
}

// The solver in a world: tubes, crates and the item source, placed
// near the loaded world's middle.

const HOME: Vec3 = Vec3::new(8.0, 40.0, 8.0);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .insert_resource(content().clone())
        .add_plugins((
            TerrainPlugin,
            LightPlugin,
            StreamingPlugin,
            BlockEntityPlugin,
            ConnectionsPlugin,
            NetworkPlugin,
            MachinePlugin,
            crate::network::energy::EnergyPlugin,
            ItemNetworkPlugin,
            crate::recipe::RecipePlugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(BlockMaterial::unloaded(Handle::default()))
        .insert_resource(StreamingSettings {
            radius: 2,
            above: 1,
            below: 1,
            ..default()
        });
    app.world_mut()
        .spawn((ChunkLoader, Transform::from_translation(HOME)));
    app.update();
    app.update();
    for _ in 0..2000 {
        app.update();
        if app.world().resource::<StreamingStatus>().settled {
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                50,
            )));
            return app;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("never settled");
}

fn state(id: &str) -> BlockState {
    content().default_state(id).unwrap()
}

fn set(app: &mut App, pos: IVec3, state: BlockState) {
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| assert!(voxels.set(pos, state)))
        .unwrap();
    app.update();
}

fn slots(app: &App, pos: IVec3) -> Slots {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world().get::<Slots>(entity).unwrap().clone()
}

fn fill(app: &mut App, pos: IVec3, stack: ItemStack) {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    let mut slots = app.world_mut().get_mut::<Slots>(entity).unwrap();
    assert!(slots.0[0].insert(content(), stack).is_none());
}

/// Runs frames until `n` more network ticks have run.
fn run_ticks(app: &mut App, n: u64) {
    let clock = |app: &App| {
        app.world()
            .resource::<crate::network::energy::EnergyClock>()
            .ticks
    };
    let until = clock(app) + n;
    while clock(app) < until {
        app.update();
    }
    assert_eq!(clock(app), until);
}

/// How many of `item` the crates at `at` hold, in all.
fn held(app: &App, at: &[IVec3], item: ItemState) -> u64 {
    let groups: Vec<SlotGroup> = at.iter().flat_map(|&pos| slots(app, pos).0).collect();
    count(&groups, item)
}

/// A source on a tube that runs to two crates fills both, evenly, at the
/// tube's rate, exactly: iron tubes 4 items a second, gold ones 6.
#[test]
fn a_source_fills_two_crates_through_tubes() {
    for (frame, rate) in [("defaria:iron", 4), ("defaria:gold", 6)] {
        fills_two_crates(frame, rate);
    }
}

fn fills_two_crates(frame: &str, rate: u64) {
    let mut app = app();
    let tube = content()
        .block_state("defaria:tube", &[("frame", frame)])
        .unwrap();
    let source = IVec3::new(3, 54, -5);
    let (a, b) = (IVec3::new(2, 51, -5), IVec3::new(4, 51, -5));
    set(&mut app, source, state("defaria:item_source"));
    for pos in [
        IVec3::new(3, 53, -5),
        IVec3::new(3, 52, -5),
        IVec3::new(2, 52, -5),
        IVec3::new(4, 52, -5),
    ] {
        set(&mut app, pos, tube);
    }
    set(&mut app, a, state("defaria:crate"));
    set(&mut app, b, state("defaria:crate"));
    let id = app
        .world()
        .resource::<Networks>()
        .at(source - IVec3::Y)
        .unwrap();
    assert_eq!(
        app.world().resource::<ItemRates>().0.get(&id),
        Some(&(rate as f32))
    );
    let start = held(&app, &[a, b], planks());
    run_ticks(&mut app, 60);
    // Three seconds: three seconds' worth exactly, half to each.
    let (in_a, in_b) = (held(&app, &[a], planks()), held(&app, &[b], planks()));
    assert_eq!(in_a + in_b - start, 3 * rate, "{frame}: {in_a} {in_b}");
    assert!(in_a.abs_diff(in_b) <= 1, "{frame}: {in_a} {in_b}");
    // The source is still full.
    assert_eq!(held(&app, &[source], planks()), 9 * 64);
}

/// Through moves and a network splitting in two, every item is in one of
/// the crates.
#[test]
fn items_are_kept_through_a_split() {
    let mut app = app();
    let tube = state("defaria:tube");
    let from = IVec3::new(3, 52, -5);
    let (b, c) = (IVec3::new(3, 49, -4), IVec3::new(3, 49, -6));
    set(&mut app, from, state("defaria:crate"));
    set(&mut app, b, state("defaria:crate"));
    set(&mut app, c, state("defaria:crate"));
    let crates = [from, b, c];
    for pos in [
        IVec3::new(3, 51, -5),
        IVec3::new(3, 50, -5),
        IVec3::new(3, 50, -4),
        IVec3::new(3, 50, -6),
    ] {
        set(&mut app, pos, tube);
    }
    let log = item("defaria:log", &[]);
    fill(&mut app, from, ItemStack::new(planks(), 40));
    fill(&mut app, from, ItemStack::new(log, 30));
    for _ in 0..40 {
        app.update();
        assert_eq!(held(&app, &crates, planks()), 40);
        assert_eq!(held(&app, &crates, log), 30);
    }
    let at_c = slots(&app, c).total();
    assert!(at_c > 0);
    // Break the tube to `c`: it's off the network, and gets no more once
    // the networks have caught up (a network tick can still fall in the
    // frame of the break, before they do).
    set(&mut app, IVec3::new(3, 50, -6), BlockState::AIR);
    app.update();
    let at_c = slots(&app, c).total();
    for _ in 0..40 {
        app.update();
        assert_eq!(held(&app, &crates, planks()), 40);
        assert_eq!(held(&app, &crates, log), 30);
    }
    assert_eq!(slots(&app, c).total(), at_c);
    assert!(slots(&app, b).total() > at_c);
}

/// A crate of ore feeds an electric furnace (#115) through a tube into its
/// top, and a tube from its bottom takes the ingots on to another crate.
#[test]
fn a_crate_feeds_a_furnace_whose_ingots_go_on_to_a_crate() {
    let mut app = app();
    let tube = state("defaria:tube");
    let ore_crate = IVec3::new(3, 54, -5);
    let furnace = IVec3::new(3, 52, -5);
    let ingot_crate = IVec3::new(4, 50, -5);
    let facing_south = |id: &str| content().block_state(id, &[("facing", "south")]).unwrap();
    set(&mut app, ore_crate, state("defaria:crate"));
    set(&mut app, IVec3::new(3, 53, -5), tube);
    set(&mut app, furnace, facing_south("defaria:electric_furnace"));
    // Its back, north, to the creative generator's side.
    set(&mut app, IVec3::new(3, 52, -6), state("defaria:wire"));
    set(
        &mut app,
        IVec3::new(4, 52, -6),
        facing_south("defaria:creative_generator"),
    );
    set(&mut app, IVec3::new(3, 51, -5), tube);
    set(&mut app, IVec3::new(4, 51, -5), tube);
    set(&mut app, ingot_crate, state("defaria:crate"));
    let ore = item("defaria:stone", &[("ore", "defaria:iron")]);
    let ingot = item("defaria:ingot", &[("metal", "defaria:iron")]);
    fill(&mut app, ore_crate, ItemStack::new(ore, 4));
    // 4 s each, and a tick or two between them.
    for _ in 0..400 {
        app.update();
    }
    assert_eq!(held(&app, &[ore_crate, furnace], ore), 0);
    assert_eq!(held(&app, &[ingot_crate], ingot), 4);
    assert_eq!(held(&app, &[furnace], ingot), 0);
}

/// A crate of glass and ore feeds a furnace: only the ore goes in, as no
/// smelting recipe takes glass, and the glass ahead of it doesn't hold it
/// up.
#[test]
fn a_furnace_takes_only_what_it_smelts() {
    let mut app = app();
    let ore_crate = IVec3::new(3, 54, -5);
    let furnace = IVec3::new(3, 52, -5);
    set(&mut app, ore_crate, state("defaria:crate"));
    set(&mut app, IVec3::new(3, 53, -5), state("defaria:tube"));
    set(
        &mut app,
        furnace,
        content()
            .block_state("defaria:electric_furnace", &[("facing", "south")])
            .unwrap(),
    );
    let glass = item("defaria:glass", &[]);
    let ore = item("defaria:stone", &[("ore", "defaria:iron")]);
    fill(&mut app, ore_crate, ItemStack::new(glass, 3));
    fill(&mut app, ore_crate, ItemStack::new(ore, 2));
    run_ticks(&mut app, 40);
    assert_eq!(held(&app, &[ore_crate], glass), 3);
    assert_eq!(held(&app, &[furnace], glass), 0);
    assert_eq!(held(&app, &[furnace], ore), 2);
}

/// Two furnaces on one line, the second under the first's output tube,
/// which also runs on to a crate: a mix of sand and ore all ends up in
/// the crate as glass and ingots, and none of it in the second furnace,
/// which has no recipe for either.
#[test]
fn chained_furnaces_pass_on_what_the_second_cant_use() {
    let mut app = app();
    let tube = state("defaria:tube");
    let top_crate = IVec3::new(3, 54, -5);
    let first = IVec3::new(3, 52, -5);
    let second = IVec3::new(3, 50, -5);
    let bottom_crate = IVec3::new(4, 50, -5);
    let facing_south = |id: &str| content().block_state(id, &[("facing", "south")]).unwrap();
    set(&mut app, top_crate, state("defaria:crate"));
    set(&mut app, IVec3::new(3, 53, -5), tube);
    set(&mut app, first, facing_south("defaria:electric_furnace"));
    set(&mut app, IVec3::new(3, 52, -6), state("defaria:wire"));
    set(
        &mut app,
        IVec3::new(4, 52, -6),
        facing_south("defaria:creative_generator"),
    );
    set(&mut app, IVec3::new(3, 51, -5), tube);
    set(&mut app, IVec3::new(4, 51, -5), tube);
    set(&mut app, second, facing_south("defaria:electric_furnace"));
    set(&mut app, bottom_crate, state("defaria:crate"));
    let sand = item("defaria:soil", &[("soil", "defaria:sand")]);
    let ore = item("defaria:stone", &[("ore", "defaria:iron")]);
    let glass = item("defaria:glass", &[]);
    let ingot = item("defaria:ingot", &[("metal", "defaria:iron")]);
    fill(&mut app, top_crate, ItemStack::new(sand, 2));
    fill(&mut app, top_crate, ItemStack::new(ore, 2));
    // 3 s for each sand, 4 s for each ore, and a tick or two between.
    run_ticks(&mut app, 320);
    assert_eq!(held(&app, &[bottom_crate], glass), 2);
    assert_eq!(held(&app, &[bottom_crate], ingot), 2);
    assert_eq!(slots(&app, second).total(), 0);
    assert_eq!(slots(&app, first).total(), 0);
}

/// A furnace fed by a tube starts smelting on the tick the ore arrives:
/// items move before recipe machines run.
#[test]
fn a_tube_fed_furnace_starts_on_the_tick_the_ore_arrives() {
    let mut app = app();
    let tube = state("defaria:tube");
    let ore_crate = IVec3::new(3, 54, -5);
    let furnace = IVec3::new(3, 52, -5);
    let facing_south = |id: &str| content().block_state(id, &[("facing", "south")]).unwrap();
    set(&mut app, ore_crate, state("defaria:crate"));
    set(&mut app, IVec3::new(3, 53, -5), tube);
    set(&mut app, furnace, facing_south("defaria:electric_furnace"));
    set(&mut app, IVec3::new(3, 52, -6), state("defaria:wire"));
    set(
        &mut app,
        IVec3::new(4, 52, -6),
        facing_south("defaria:creative_generator"),
    );
    // A second to charge the furnace's buffer.
    for _ in 0..20 {
        app.update();
    }
    let ore = item("defaria:stone", &[("ore", "defaria:iron")]);
    fill(&mut app, ore_crate, ItemStack::new(ore, 1));
    let entity = app
        .world()
        .resource::<BlockEntities>()
        .get(furnace)
        .unwrap();
    for _ in 0..20 {
        app.update();
        if held(&app, &[ore_crate], ore) == 0 {
            // The tick it left the crate, the furnace took it in.
            let progress = app.world().get::<crate::recipe::RecipeProgress>(entity);
            assert!(progress.unwrap().recipe.is_some());
            assert_eq!(held(&app, &[furnace], ore), 0);
            return;
        }
    }
    panic!("the ore never left the crate");
}
