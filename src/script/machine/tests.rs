//! Machines' scripts: test machines in a test mod, over chunks spawned by
//! hand, with the energy solver and recipes but no rendering. The base
//! game's solar panel is tested in the whole game, in `solar_panel`.

use std::{collections::BTreeMap, path::Path, sync::OnceLock, time::Duration};

use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

use super::*;
use crate::{
    block_entity::{BlockData, BlockEntities, BlockEntityPlugin, Value},
    connections::ConnectionsPlugin,
    content::{
        BlockState, FULL_FLUID, ItemState, MODS_DIR,
        load::{ModFiles, order, read_mods},
    },
    inventory::ItemStack,
    light::{ChunkLight, Light},
    machine::{EnergyBuffer, MachineCorePlugin, Slots, Tanks},
    network::{NetworkCorePlugin, energy::EnergyPlugin, energy::EnergyReport},
    recipe::RecipePlugin,
    script::{BlockUsed, ScriptPlugin, ScriptSettings, source::ScriptSources},
    world::{Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated, VoxelChanged, Voxels},
};

const COUNTER: &str = r#"
-- Counts its ticks; the one at x = 9 fails on its third.
function on_tick(m, dt)
    m.data.ticks += 1
    m.data.dt = dt
    if m.pos.x == 9 and m.data.ticks == 3 then
        error("boom")
    end
end
"#;

const THIEF: &str = r#"
-- Has no `max_generate`, and tries to make energy anyway.
function on_tick(m)
    m.data.made += m.energy:generate(1000)
    m.data.taken += m.energy:take(1)
end
"#;

const FORGER: &str = r#"
-- Tries to write its stored energy.
function on_tick(m)
    m.energy.stored = 1000000
end
"#;

const DYNAMO: &str = r#"
-- Makes as much as it's let, every tick.
function on_tick(m)
    m.data.made += m.energy:generate(1e9)
end

-- Making energy outside `on_tick` makes none.
function on_used(m)
    m.data.used = m.energy:generate(1e9)
    m.data.max = m.energy.max_generate
end
"#;

const MOVER: &str = r#"
-- Draws water from below into `a`, moves it on to `b`, and pours it out
-- on top; and moves items from `input` to `output`.
function on_tick(m)
    m.data.drawn += m.tanks.a:draw("down", 300)
    m.data.moved += m.tanks.a:move_to(m.tanks.b, 200)
    m.data.poured += m.tanks.b:pour("up", 100)
    m.data.items += m.slots.input:move(1, "output", 5)
    local first = m.slots.output:get(1)
    m.data.first = if first then first.id else ""
    m.data.slots = #m.slots.output
    m.data.ports = #m.ports
end
"#;

const REACHER: &str = r#"
-- Pours two blocks away, further than a machine may.
function on_tick(m)
    m.tanks.a:pour(world.block(m.pos + vector.create(2, 0, 0)), 1)
end
"#;

const STRETCHER: &str = r#"
-- Pours at a position no world reaches, or (the one at x = 8) at the
-- corner of the ones it can, without catching the error: `pcall` would
-- catch a crash too.
function on_tick(m)
    m.data.ticks += 1
    local corner = 2 ^ 30
    local at = if m.pos.x == 8
        then vector.create(corner, corner, corner)
        else vector.create(-3e9, 0, 0)
    m.tanks.a:pour(at, 1)
end
"#;

const METER: &str = r#"
-- Reads the light above it.
function on_tick(m)
    local light = m:neighbour("up").light
    m.data.sky = light.sky
    m.data.red = light.red
    m:set_active(light.sky > 10)
    local iron = world.material_def("defaria:iron")
    m.data.iron = iron.name
    m.data.metal = table.find(iron.tags, "metal") ~= nil
    m.data.nothing = world.material_def("nowhere") == nil
end
"#;

const OVEN: &str = r#"
-- Counts the recipes it finishes, and tries to put what it made back in
-- its input, where no recipe takes it.
function on_recipe_finished(m, recipe)
    m.data.done += 1
    m.data.last = recipe.id
    m.data.kind = recipe.type
    m.data.back += m.slots.output:move(1, "input")
end
"#;

fn machine_file(script: &str, machine: &str, data: &str) -> String {
    format!(
        r#"{{ model: "defaria:cube", textures: {{ all: "defaria:lamp" }}, script: "{script}",
            block_entity: {{ data: {{ {data} }} }}, machine: {{ {machine} }} }}"#
    )
}

const PORTS: &str =
    r#"ports: [{ faces: ["east", "west"], network: "defaria:electric", role: "consume" }]"#;

/// The base game and mod `test`, with a machine for each script.
fn mods() -> Vec<ModFiles> {
    let (mut mods, errors) = read_mods(Path::new("assets/mods"), MODS_DIR);
    assert!(errors.is_empty(), "{errors:?}");
    let counter = machine_file(
        "counter",
        &format!("{PORTS}, energy: {{ capacity: 100, max_in: 10 }}"),
        "ticks: 0",
    );
    let thief = machine_file("thief", "energy: { capacity: 1000 }", "made: 0, taken: 0");
    let forger = machine_file("forger", "energy: { capacity: 1000 }", "");
    let dynamo = machine_file(
        "dynamo",
        "energy: { capacity: 1000, max_generate: 100 }",
        "made: 0",
    );
    let mover = machine_file(
        "mover",
        &format!(
            "{PORTS}, energy: {{ capacity: 10, max_in: 10 }}, tanks: {{ a: {{ capacity: 4 }}, b: {{ capacity: 4, accepts: [\"defaria:water\"] }} }}, slots: {{ input: 1, output: 2 }}"
        ),
        "drawn: 0, moved: 0, poured: 0, items: 0",
    );
    let reacher = machine_file("reacher", "tanks: { a: { capacity: 4 } }", "");
    let stretcher = machine_file("stretcher", "tanks: { a: { capacity: 4 } }", "ticks: 0");
    let meter = machine_file("meter", "", "");
    let oven = machine_file(
        "oven",
        &format!(
            r#"{PORTS}, energy: {{ capacity: 100000, max_in: 1000 }}, slots: {{ input: 1, output: 1 }}, recipes: "smelting""#
        ),
        "done: 0, back: 0",
    );
    mods.push(ModFiles::new(
        "mods/test",
        r#"{ id: "test", name: "Test", dependencies: ["defaria"] }"#,
        &[
            ("blocks/counter.json5", &counter),
            ("blocks/thief.json5", &thief),
            ("blocks/forger.json5", &forger),
            ("blocks/dynamo.json5", &dynamo),
            ("blocks/mover.json5", &mover),
            ("blocks/reacher.json5", &reacher),
            ("blocks/stretcher.json5", &stretcher),
            ("blocks/meter.json5", &meter),
            ("blocks/oven.json5", &oven),
            ("scripts/counter.luau", COUNTER),
            ("scripts/thief.luau", THIEF),
            ("scripts/forger.luau", FORGER),
            ("scripts/dynamo.luau", DYNAMO),
            ("scripts/mover.luau", MOVER),
            ("scripts/reacher.luau", REACHER),
            ("scripts/stretcher.luau", STRETCHER),
            ("scripts/meter.luau", METER),
            ("scripts/oven.luau", OVEN),
        ],
    ));
    mods
}

fn content() -> &'static Content {
    static CONTENT: OnceLock<Content> = OnceLock::new();
    CONTENT.get_or_init(|| {
        let content = Content::from_mods(mods());
        assert!(content.errors.is_empty(), "{:#?}", content.errors);
        content
    })
}

fn runtime() -> ScriptRuntime {
    let sources = ScriptSources::from_mods(&order(mods(), &mut Vec::new()));
    assert!(sources.errors.is_empty(), "{:?}", sources.errors);
    let runtime = ScriptRuntime::new(sources, ScriptSettings::default());
    assert!(
        runtime.load_errors().is_empty(),
        "{:?}",
        runtime.load_errors()
    );
    runtime
}

fn block(id: &str) -> BlockState {
    content().default_state(id).unwrap()
}

/// Machines, networks, energy, recipes and scripts over chunk (0, 0, 0),
/// dark but for what a test lights, stepping 10 ms a frame.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            10,
        )))
        .insert_resource(content().clone())
        .insert_resource(runtime())
        .init_resource::<ChunkMap>()
        .add_message::<VoxelChanged>()
        .add_message::<FluidChanged>()
        .add_message::<PropertiesUpdated>()
        .add_plugins((
            ConnectionsPlugin,
            NetworkCorePlugin,
            BlockEntityPlugin,
            MachineCorePlugin,
            EnergyPlugin,
            RecipePlugin,
            ScriptPlugin,
            MachineScriptPlugin,
        ));
    spawn(&mut app, IVec3::ZERO, Chunk::default());
    app.update();
    app
}

fn spawn(app: &mut App, coord: IVec3, chunk: Chunk) {
    let world = app.world_mut();
    let entity = world
        .spawn((ChunkCoord(coord), chunk, ChunkLight::default()))
        .id();
    world
        .resource_mut::<ChunkMap>()
        .insert(ChunkCoord(coord), entity);
}

fn unload(app: &mut App, coord: IVec3) -> Chunk {
    let entity = app
        .world_mut()
        .resource_mut::<ChunkMap>()
        .remove(ChunkCoord(coord))
        .unwrap();
    let chunk = app.world().get::<Chunk>(entity).unwrap().clone();
    app.world_mut().despawn(entity);
    app.update();
    chunk
}

fn set(app: &mut App, voxels: &[(IVec3, BlockState)]) {
    let voxels = voxels.to_vec();
    app.world_mut()
        .run_system_once(move |mut world: Voxels| {
            for &(pos, state) in &voxels {
                assert!(world.set(pos, state), "{pos} isn't loaded");
            }
        })
        .unwrap();
    app.update();
    app.update();
}

/// Runs frames until `n` more network ticks have run, with every
/// machine's `on_tick` for the last of them (they're spread over the
/// fixed steps after it). A frame is less than a fixed step, so it stops
/// on the step it means to.
fn ticks(app: &mut App, n: u64) {
    let until = app.world().resource::<NetworkClock>().ticks + n;
    while app.world().resource::<NetworkClock>().ticks < until
        || !app.world().resource::<MachineTickPhase>().tick_done()
    {
        app.update();
    }
    assert_eq!(app.world().resource::<NetworkClock>().ticks, until);
}

/// Runs frames until network tick `n` more is due, stopping on its own
/// fixed step: only the machines in its first slot have ticked.
fn to_network_tick(app: &mut App, n: u64) {
    let until = app.world().resource::<NetworkClock>().ticks + n;
    while app.world().resource::<NetworkClock>().ticks < until {
        app.update();
    }
}

/// `n` positions two apart in x and z from `from` (so machines there
/// don't touch), in chunk (0, 0, 0), whose machines tick in `slot`.
fn in_slot(from: IVec3, slot: u32, n: usize) -> Vec<IVec3> {
    let found: Vec<IVec3> = (0..64)
        .map(|i| from + IVec3::new(i % 8, 0, i / 8) * 2)
        .filter(|&pos| tick_slot(pos, 3) == slot)
        .take(n)
        .collect();
    assert_eq!(found.len(), n, "too few in slot {slot} from {from}");
    found
}

fn entity(app: &App, pos: IVec3) -> Entity {
    app.world()
        .resource::<BlockEntities>()
        .get(pos)
        .unwrap_or_else(|| panic!("no block entity at {pos}"))
}

fn data(app: &App, pos: IVec3) -> BTreeMap<String, Value> {
    app.world()
        .get::<BlockData>(entity(app, pos))
        .unwrap()
        .0
        .clone()
}

fn int(app: &App, pos: IVec3, key: &str) -> i64 {
    match data(app, pos).get(key) {
        Some(Value::Int(n)) => *n,
        Some(Value::Float(n)) => *n as i64,
        other => panic!("{key} at {pos} is {other:?}"),
    }
}

fn stored(app: &App, pos: IVec3) -> u64 {
    app.world()
        .get::<EnergyBuffer>(entity(app, pos))
        .unwrap()
        .stored
}

fn disabled(app: &App, pos: IVec3) -> bool {
    app.world()
        .resource::<ScriptRuntime>()
        .is_disabled(Caller::Block(pos))
}

fn v(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, y, z)
}

#[test]
fn the_index_knows_machines_hooks() {
    let index = ScriptIndex::new(content(), &runtime());
    let counter = index.block(content(), block("test:counter")).unwrap();
    assert!(counter.tick && !counter.recipe_finished);
    let oven = index.block(content(), block("test:oven")).unwrap();
    assert!(oven.recipe_finished && !oven.tick);
    assert!(index.any_tick());
    // With no scripts, no machine ticks, and none are looked through.
    let none = ScriptIndex::new(
        content(),
        &ScriptRuntime::new(Default::default(), default()),
    );
    assert!(!none.any_tick());
}

/// `on_tick` runs once a network tick, with the tick's length, and an
/// error in one machine's disables only that machine.
#[test]
fn machines_tick_and_an_error_disables_only_its_own() {
    let mut app = app();
    let (good, bad) = (v(4, 4, 4), v(9, 4, 4));
    set(
        &mut app,
        &[(good, block("test:counter")), (bad, block("test:counter"))],
    );
    let start = int(&app, good, "ticks");
    ticks(&mut app, 10);
    assert_eq!(int(&app, good, "ticks") - start, 10);
    assert_eq!(data(&app, good)["dt"], Value::Float(0.05));
    // It failed on its third tick, which left its data as it was, and it
    // runs no more.
    assert!(disabled(&app, bad));
    assert!(!disabled(&app, good));
    assert_eq!(int(&app, bad, "ticks"), 2);
    ticks(&mut app, 5);
    assert_eq!(int(&app, bad, "ticks"), 2);
    assert_eq!(int(&app, good, "ticks") - start, 15);
}

/// A machine with no `max_generate` makes nothing however it asks, and
/// one that writes its stored energy fails; neither has any energy, and
/// the books balance.
#[test]
fn a_script_cannot_make_energy_from_nothing() {
    let mut app = app();
    let (thief, forger) = (v(2, 4, 4), v(6, 4, 4));
    set(
        &mut app,
        &[(thief, block("test:thief")), (forger, block("test:forger"))],
    );
    ticks(&mut app, 20);
    assert_eq!(stored(&app, thief), 0);
    assert_eq!(int(&app, thief, "made"), 0);
    assert_eq!(int(&app, thief, "taken"), 0);
    assert!(!disabled(&app, thief));
    assert_eq!(stored(&app, forger), 0);
    assert!(disabled(&app, forger));
    assert_eq!(app.world().resource::<EnergyReport>().generated, 0);
}

/// A machine with `max_generate` makes no more than that a second, however
/// much it asks for, and nothing outside `on_tick`; and taking spends what
/// it holds.
#[test]
fn generating_is_capped_by_max_generate_and_only_in_on_tick() {
    let mut app = app();
    let dynamo = v(4, 4, 4);
    set(&mut app, &[(dynamo, block("test:dynamo"))]);
    let before = stored(&app, dynamo);
    ticks(&mut app, 20);
    // 100 W for a second.
    assert_eq!(stored(&app, dynamo) - before, 100);
    let report = app.world().resource::<EnergyReport>();
    assert_eq!(report.generated, stored(&app, dynamo));
    assert_eq!(int(&app, dynamo, "made") as u64, stored(&app, dynamo));

    app.world_mut().write_message(BlockUsed {
        pos: dynamo,
        player: None,
    });
    app.update();
    app.update();
    assert_eq!(int(&app, dynamo, "used"), 0);
    assert_eq!(int(&app, dynamo, "max"), 100);

    // Full, it makes nothing more.
    ticks(&mut app, 300);
    assert_eq!(stored(&app, dynamo), 1000);
    assert_eq!(app.world().resource::<EnergyReport>().generated, 1000);
}

fn fluid_at(app: &mut App, pos: IVec3) -> u64 {
    app.world_mut()
        .run_system_once(move |voxels: Voxels, content: Res<Content>| {
            let (state, partial) = voxels.get_with_fluid(pos).unwrap();
            if content.state(state).is_fluid() {
                partial.unwrap_or(FULL_FLUID) as u64
            } else {
                0
            }
        })
        .unwrap()
}

fn stone() -> ItemState {
    content().default_item("defaria:stone").unwrap()
}

/// Fluid drawn from the world, moved between tanks and poured back out,
/// and items moved between slot groups, are never more or less than
/// there were.
#[test]
fn tanks_and_slots_move_what_they_hold_and_nothing_more() {
    let mut app = app();
    let at = v(4, 4, 4);
    let (below, above) = (at - IVec3::Y, at + IVec3::Y);
    set(
        &mut app,
        &[(at, block("test:mover")), (below, block("defaria:water"))],
    );
    let e = entity(&app, at);
    app.world_mut()
        .get_mut::<Slots>(e)
        .unwrap()
        .group_mut("input")
        .unwrap()
        .insert(content(), ItemStack::new(stone(), 12));
    let total = |app: &mut App| {
        let tanks = app.world().get::<Tanks>(entity(app, at)).unwrap().clone();
        fluid_at(app, below) + fluid_at(app, above) + tanks.total("defaria:water")
    };
    assert_eq!(total(&mut app), FULL_FLUID as u64);
    for _ in 0..12 {
        ticks(&mut app, 1);
        assert_eq!(total(&mut app), FULL_FLUID as u64);
    }
    // It drew 300 a tick until the water was gone, moved 200 a tick and
    // poured 100: 1,200 poured would be more than there was.
    assert_eq!(int(&app, at, "drawn"), 1000);
    assert_eq!(int(&app, at, "moved"), 1000);
    assert_eq!(int(&app, at, "poured"), 1000);
    assert_eq!(fluid_at(&mut app, below), 0);
    assert_eq!(fluid_at(&mut app, above), 1000);

    let slots = app.world().get::<Slots>(entity(&app, at)).unwrap();
    assert_eq!(slots.group("input").unwrap().total(), 0);
    assert_eq!(
        slots.group("output").unwrap().slots[0],
        Some(ItemStack::new(stone(), 12))
    );
    assert_eq!(int(&app, at, "items"), 12);
    assert_eq!(data(&app, at)["first"], Value::Text("defaria:stone".into()));
    assert_eq!(int(&app, at, "slots"), 2);
    assert_eq!(int(&app, at, "ports"), 2);
}

/// A tank pouring further than one step from its machine fails.
#[test]
fn a_machine_reaches_only_the_blocks_next_to_it() {
    let mut app = app();
    let at = v(4, 4, 4);
    set(&mut app, &[(at, block("test:reacher"))]);
    ticks(&mut app, 2);
    assert!(disabled(&app, at));
}

/// Once a network tick's machines have used its steps, the rest skip
/// it, and the next tick starts with them, so every machine gets its
/// turn.
#[test]
fn machine_ticks_share_the_ticks_steps_in_turn() {
    let mut app = app();
    // In the same slot, so they share its steps.
    let counters: [IVec3; 3] = in_slot(v(0, 4, 0), 1, 3).try_into().unwrap();
    let counter = block("test:counter");
    set(&mut app, &counters.map(|pos| (pos, counter)));
    // Any call spends this, so one machine ticks a tick.
    app.world_mut()
        .resource_mut::<HookSettings>()
        .steps_per_tick = 1;
    let start = counters.map(|pos| int(&app, pos, "ticks"));
    ticks(&mut app, 6);
    for (pos, start) in counters.into_iter().zip(start) {
        assert_eq!(int(&app, pos, "ticks") - start, 2, "{pos}");
    }
}

/// Machines in one slot share its time too: once it's spent only the
/// first of them ticks, and the next tick starts with the rest.
#[test]
fn machine_ticks_share_the_ticks_time_in_turn() {
    let mut app = app();
    let counters: [IVec3; 3] = in_slot(v(0, 4, 0), 1, 3).try_into().unwrap();
    let counter = block("test:counter");
    set(&mut app, &counters.map(|pos| (pos, counter)));
    app.world_mut().resource_mut::<HookSettings>().time_per_tick = Duration::ZERO;
    let start = counters.map(|pos| int(&app, pos, "ticks"));
    ticks(&mut app, 6);
    for (pos, start) in counters.into_iter().zip(start) {
        assert_eq!(int(&app, pos, "ticks") - start, 2, "{pos}");
    }
}

/// Machines' `on_tick`s are spread over the fixed steps between network
/// ticks, so no one step runs them all; each still runs once a tick.
#[test]
fn machine_ticks_are_spread_over_the_steps_between_network_ticks() {
    let mut app = app();
    let mut counters = in_slot(v(0, 4, 0), 0, 2);
    counters.extend(in_slot(v(0, 6, 0), 1, 2));
    counters.extend(in_slot(v(0, 8, 0), 2, 2));
    let counter = block("test:counter");
    let placed: Vec<_> = counters.iter().map(|&pos| (pos, counter)).collect();
    set(&mut app, &placed);
    ticks(&mut app, 1);
    let start: Vec<i64> = counters
        .iter()
        .map(|&pos| int(&app, pos, "ticks"))
        .collect();
    let ticked = |app: &App| -> Vec<i64> {
        counters
            .iter()
            .zip(&start)
            .map(|(&pos, start)| int(app, pos, "ticks") - start)
            .collect()
    };
    // On the network tick's own step, only the first slot's.
    to_network_tick(&mut app, 1);
    assert_eq!(ticked(&app), [1, 1, 0, 0, 0, 0]);
    ticks(&mut app, 0);
    assert_eq!(ticked(&app), [1; 6]);
    ticks(&mut app, 4);
    assert_eq!(ticked(&app), [5; 6]);
}

/// A tank pouring at a position no world reaches, or so far that the
/// distance overflows, gets an error, not a crash.
#[test]
fn pouring_out_of_the_world_is_an_error() {
    let mut app = app();
    let (far, corner) = (v(4, 4, 4), v(8, 4, 4));
    let stretcher = block("test:stretcher");
    set(&mut app, &[(far, stretcher), (corner, stretcher)]);
    ticks(&mut app, 2);
    // Each failed (so its count wasn't saved), and is disabled.
    for at in [far, corner] {
        assert_eq!(int(&app, at, "ticks"), 0);
        assert!(disabled(&app, at));
    }
}

/// A machine's script reads the light, and sets it active.
#[test]
fn a_machine_reads_the_light_and_sets_itself_active() {
    let mut app = app();
    let at = v(4, 4, 4);
    let chunk = app
        .world()
        .resource::<ChunkMap>()
        .get(ChunkCoord(IVec3::ZERO))
        .unwrap();
    app.world_mut()
        .get_mut::<ChunkLight>(chunk)
        .unwrap()
        .set_at(at + IVec3::Y, Light::new(12, [3, 0, 0]));
    set(&mut app, &[(at, block("test:meter"))]);
    ticks(&mut app, 1);
    assert_eq!(int(&app, at, "sky"), 12);
    assert_eq!(int(&app, at, "red"), 3);
    let data = data(&app, at);
    assert_eq!(data["iron"], Value::Text("Iron".into()));
    assert_eq!(data["metal"], Value::Bool(true));
    assert_eq!(data["nothing"], Value::Bool(true));
    let active = app
        .world()
        .get::<crate::machine::MachineActive>(entity(&app, at))
        .unwrap();
    assert!(active.0);
}

/// A machine whose network reaches into a chunk that isn't loaded doesn't
/// tick until it loads.
#[test]
fn a_machine_on_a_frozen_network_doesnt_tick() {
    let mut app = app();
    spawn(&mut app, IVec3::X, Chunk::default());
    app.update();
    let at = v(12, 4, 4);
    let mut voxels = vec![(at, block("test:counter"))];
    voxels.extend((13..20).map(|x| (v(x, 4, 4), block("defaria:wire"))));
    set(&mut app, &voxels);
    ticks(&mut app, 2);
    let saved = unload(&mut app, IVec3::X);
    let frozen = int(&app, at, "ticks");
    ticks(&mut app, 10);
    assert_eq!(int(&app, at, "ticks"), frozen);

    spawn(&mut app, IVec3::X, saved);
    app.update();
    app.update();
    ticks(&mut app, 10);
    assert!(int(&app, at, "ticks") >= frozen + 9);
}

/// A recipe machine's script hears of each recipe it finishes.
#[test]
fn finishing_a_recipe_runs_on_recipe_finished() {
    let mut app = app();
    let at = v(4, 4, 4);
    set(&mut app, &[(at, block("test:oven"))]);
    let ore = content()
        .item("defaria:stone")
        .unwrap()
        .state(&[("ore", "defaria:iron")])
        .unwrap();
    let e = entity(&app, at);
    app.world_mut()
        .get_mut::<Slots>(e)
        .unwrap()
        .group_mut("input")
        .unwrap()
        .insert(content(), ItemStack::new(ore, 2));
    app.world_mut().get_mut::<EnergyBuffer>(e).unwrap().stored = 100_000;
    // Each smelt takes 4 seconds: 80 ticks.
    ticks(&mut app, 79);
    assert_eq!(int(&app, at, "done"), 0);
    ticks(&mut app, 2);
    assert_eq!(int(&app, at, "done"), 1);
    let data = data(&app, at);
    assert_eq!(
        data["last"],
        Value::Text("defaria:smelt_ore[ore=defaria:iron]".into())
    );
    assert_eq!(data["kind"], Value::Text("smelting".into()));
    // Its script can't put the ingot in the input, as a tube or the
    // player's hand couldn't.
    assert_eq!(int(&app, at, "back"), 0);
    let slots = app.world().get::<Slots>(entity(&app, at)).unwrap();
    assert_eq!(slots.group("output").unwrap().total(), 1);
    ticks(&mut app, 80);
    assert_eq!(int(&app, at, "done"), 2);
    assert_eq!(int(&app, at, "back"), 0);
}

/// Machines that finish recipes on the same tick share that tick's pool
/// of steps: once it's spent the rest of their `on_recipe_finished`s wait
/// for the next tick, and none is lost.
#[test]
fn recipe_hooks_share_a_ticks_steps() {
    let mut app = app();
    let ovens: Vec<IVec3> = (0..3).map(|i| v(2 + 2 * i, 4, 4)).collect();
    set(
        &mut app,
        &ovens
            .iter()
            .map(|&pos| (pos, block("test:oven")))
            .collect::<Vec<_>>(),
    );
    let ore = content()
        .item("defaria:stone")
        .unwrap()
        .state(&[("ore", "defaria:iron")])
        .unwrap();
    for &pos in &ovens {
        let e = entity(&app, pos);
        app.world_mut()
            .get_mut::<Slots>(e)
            .unwrap()
            .group_mut("input")
            .unwrap()
            .insert(content(), ItemStack::new(ore, 1));
        app.world_mut().get_mut::<EnergyBuffer>(e).unwrap().stored = 100_000;
    }
    // Any call spends this, so one hook runs a tick.
    app.world_mut()
        .resource_mut::<HookSettings>()
        .steps_per_tick = 1;
    let done = |app: &App| ovens.iter().map(|&pos| int(app, pos, "done")).sum::<i64>();
    // They all finish on tick 80.
    ticks(&mut app, 79);
    assert_eq!(done(&app), 0);
    ticks(&mut app, 1);
    assert_eq!(done(&app), 1);
    ticks(&mut app, 1);
    assert_eq!(done(&app), 2);
    ticks(&mut app, 1);
    assert_eq!(done(&app), 3);
    ticks(&mut app, 5);
    assert_eq!(done(&app), 3);
}

/// A machine broken and replaced by another of its kind before the old
/// one's `on_recipe_finished` ran doesn't get the old one's.
#[test]
fn a_replaced_machine_doesnt_get_the_old_ones_recipe_hook() {
    let mut app = app();
    let ovens: Vec<IVec3> = (0..4).map(|i| v(2 + 2 * i, 4, 4)).collect();
    set(
        &mut app,
        &ovens
            .iter()
            .map(|&pos| (pos, block("test:oven")))
            .collect::<Vec<_>>(),
    );
    let ore = content()
        .item("defaria:stone")
        .unwrap()
        .state(&[("ore", "defaria:iron")])
        .unwrap();
    for &pos in &ovens {
        let e = entity(&app, pos);
        app.world_mut()
            .get_mut::<Slots>(e)
            .unwrap()
            .group_mut("input")
            .unwrap()
            .insert(content(), ItemStack::new(ore, 1));
        app.world_mut().get_mut::<EnergyBuffer>(e).unwrap().stored = 100_000;
    }
    // One hook a tick, so the last oven's waits three ticks.
    app.world_mut()
        .resource_mut::<HookSettings>()
        .steps_per_tick = 1;
    ticks(&mut app, 80);
    let last = *ovens.last().unwrap();
    let old = entity(&app, last);
    set(&mut app, &[(last, BlockState::AIR)]);
    set(&mut app, &[(last, block("test:oven"))]);
    assert_ne!(entity(&app, last), old);
    ticks(&mut app, 5);
    assert_eq!(int(&app, last, "done"), 0);
    for &pos in &ovens[..3] {
        assert_eq!(int(&app, pos, "done"), 1);
    }
}

/// Past its cap the queue of `on_recipe_finished`s drops new ones, so
/// machines finishing faster than their hooks run can't grow it forever.
#[test]
fn the_recipe_hook_queue_is_capped() {
    let mut pending = PendingRecipeHooks::default();
    for i in 0..MAX_WAITING + 10 {
        let done = RecipeFinished {
            pos: IVec3::new(i as i32, 0, 0),
            recipe: "test:r".into(),
        };
        pending.push(done, None);
    }
    assert_eq!(pending.hooks.len(), MAX_WAITING);
    assert!(pending.dropping.dropping());
}

/// The base game's solar panel, in the whole game with no GPU.
mod solar_panel {
    use super::*;
    use crate::headless::{Script, run_more, settle, test_game};

    fn panel_data(app: &App, pos: IVec3) -> (i64, u64) {
        let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
        let made = match app.world().get::<BlockData>(entity).unwrap().0["made"] {
            Value::Int(n) => n,
            Value::Float(n) => n as i64,
            ref other => panic!("made is {other:?}"),
        };
        let stored = app.world().get::<EnergyBuffer>(entity).unwrap().stored;
        (made, stored)
    }

    /// In open sky a panel makes its 60 W, and counts it in its data;
    /// roofed over, it makes nothing. What it counted survives its chunk
    /// being unloaded and loaded again.
    #[test]
    fn a_panel_makes_energy_from_the_sky_and_keeps_its_count() {
        let script = Script::parse("camera 40.5 125 40.5 0 -60\nwait 2").unwrap();
        let mut app = test_game(script);
        settle(&mut app);
        let (open, roofed) = (v(40, 120, 38), v(42, 120, 38));
        run_more(
            &mut app,
            "fill 40 120 38 40 120 38 solar_panel\n\
             fill 42 120 38 42 120 38 solar_panel\n\
             fill 42 121 38 42 121 38 stone\n\
             wait 2",
        );
        let (made, stored) = panel_data(&app, open);
        run_more(&mut app, "wait 120");
        let (made_later, stored_later) = panel_data(&app, open);
        // Two seconds of full sky: 120 J, give or take a tick at either
        // end.
        let gained = made_later - made;
        assert!((114..=126).contains(&gained), "made {gained} J");
        assert_eq!(stored_later - stored, gained as u64);
        assert_eq!(made_later as u64, stored_later, "nothing else filled it");
        assert_eq!(panel_data(&app, roofed), (0, 0));

        // Roofed over, it stops, and its count stays as it is.
        run_more(&mut app, "fill 40 121 38 40 121 38 stone\nwait 2");
        let (kept, _) = panel_data(&app, open);
        run_more(&mut app, "wait 20");
        assert_eq!(panel_data(&app, open).0, kept);
        // Away, so its chunk unloads, and back: it's read from the save.
        let before = app.world().resource::<BlockEntities>().get(open);
        run_more(
            &mut app,
            "camera 5000.5 125 40.5 0 -60\nsettle\ncamera 40.5 125 40.5 0 -60\nsettle\nwait 2",
        );
        assert_ne!(app.world().resource::<BlockEntities>().get(open), before);
        assert_eq!(panel_data(&app, open), (kept, kept as u64));
    }
}
