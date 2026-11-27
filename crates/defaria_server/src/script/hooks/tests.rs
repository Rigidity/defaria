//! Each hook on test blocks and items, in a world of a few chunks with
//! no rendering.

use std::{sync::OnceLock, time::Duration};

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::{
    block_entity::{BlockData, BlockEntities, BlockEntityPlugin},
    script::{
        ScriptPlugin, ScriptSettings,
        random_tick::{RandomTicks, TickingChunks},
        source::ScriptSources,
    },
};
use defaria_core::{
    block_entity::Value,
    content::load::{ModFiles, order},
    inventory::ItemStack,
    testing::{chunk_app, insert_chunk, test_mods},
    world::{Chunk, ChunkCoord, Voxels},
};

const SWITCH: &str = r#"
-- Using it turns it on and off, and counts the uses in its data.
function on_used(block, player)
    block:set_state { lit = not block.state.lit }
    block.data.uses = (block.data.uses or 0) + 1
    block.data.by = if player then player.mode else "nobody"
end
"#;

const MARKER: &str = r#"
-- Placed, it puts glass on top of itself; broken, planks where it was.
function on_placed(block)
    block:neighbour("up"):set("defaria:glass")
end

function on_broken(block, old)
    assert(old.id == "test:marker", old.id)
    block:set("defaria:planks")
end

-- It points at whichever side last changed.
function on_neighbour_changed(block, face)
    block:set_state { face = face }
end
"#;

const GROWER: &str = r#"
-- Grows a step on each random tick, to 3.
function on_random_tick(block)
    local age = tonumber(block.state.age)
    if age < 3 then
        block:set_state { age = age + 1 }
    end
end
"#;

const WAND: &str = r#"
-- Turns what you look at to glass, and is used up doing it.
function on_use(item, player)
    assert(item.id == "test:wand", item.id)
    if player.target then
        return player.target:set("defaria:glass")
    end
    return false
end
"#;

const FAULTY: &str = r#"
function on_used(block)
    error("boom")
end
"#;

const GREEDY: &str = r#"
-- Reaches further than a hook may.
function on_used(block)
    world.block(block.pos + vector.create(3, 0, 0)):set("defaria:glass")
end
"#;

const KEEPER: &str = r#"
local kept

function on_used(block)
    if kept then
        kept:set_state { lit = true }
    end
    kept = block
end
"#;

const READER: &str = r#"
-- Reads the world and the content, and logs it.
function on_used(block)
    local below = world.block(block.pos - vector.create(0, 1, 0))
    local glass = world.block_def("defaria:glass")
    local fluid = below.fluid and (below.fluid.id .. " " .. below.fluid.amount)
    log.info(below.id, fluid, glass.name, world.block_def("nowhere") == nil, block:stat("power"))
end
"#;

const FAR: &str = r#"
-- Asks for blocks at positions no world reaches, and logs what it's told.
function on_used(block)
    local far = pcall(world.block, vector.create(-3e9, 0, 0))
    local three = pcall(world.block, 3e9, 0, 0)
    local nan = pcall(world.block, vector.create(0 / 0, 0, 0))
    local _, error = pcall(world.block, vector.create(0, 2e9, 0))
    log.info(far, three, nan, error)
end
"#;

const SETTER: &str = r#"
-- Tries to make fluid, destroy it (stone where water is leaves it no
-- room, a slab only half), replace a block with a block entity,
-- and make one from air; logs whether each worked, and what it was told.
function on_used(block)
    local up, down, east = block:neighbour("up"), block:neighbour("down"), block:neighbour("east")
    local west = block:neighbour("west")
    local made, said = pcall(up.set, up, "defaria:water")
    local drained = pcall(down.set, down, "defaria:stone")
    local halved = pcall(down.set, down, "defaria:stone_slab")
    -- Air keeps all the water: nothing's lost, so it's allowed.
    local cleared = down:set("air")
    local broke, why = pcall(east.set, east, "air")
    local built, reason = pcall(west.set, west, "test:switch")
    log.info(made, drained, halved, cleared, broke, built, said, why, reason)
end
"#;

const PEEKER: &str = r#"
-- Reads the data of a block out of its reach, and tries to change it.
function on_used(block)
    local data = world.block(block.pos + vector.create(3, 0, 0)).data
    local wrote = pcall(function()
        data.uses = 999
    end)
    local nested = pcall(function()
        data.inner.n = 999
    end)
    log.info(data.inner.n, wrote, nested)
end
"#;

const SPOILER: &str = r#"
-- Changes its own data, then leaves a function in its neighbour's.
function on_used(block)
    block.data.uses = 1
    block:neighbour("east").data.f = function() end
end
"#;

const TOGGLER: &str = r#"
-- Turns on or off whenever a neighbour changes, which changes them.
function on_neighbour_changed(block)
    block:set_state { lit = not block.state.lit }
end
"#;

fn block_file(script: &str, extra: &str) -> String {
    format!(
        r#"{{ model: "defaria:cube", textures: {{ all: "defaria:lamp" }}, script: "{script}", {extra} }}"#
    )
}

/// The base game and mod `test`, with a block or item for each script.
fn mods() -> Vec<ModFiles> {
    let switch = block_file(
        "switch",
        r#"properties: { lit: { values: ["false", "true"] } }, block_entity: {}"#,
    );
    let marker = block_file(
        "marker",
        r#"properties: { face: { values: ["none", "up", "down", "north", "south", "east", "west"] } }"#,
    );
    let grower = block_file(
        "grower",
        r#"properties: { age: { values: ["0", "1", "2", "3"] } }"#,
    );
    let faulty = block_file("faulty", "");
    let greedy = block_file("greedy", "");
    let keeper = block_file(
        "keeper",
        r#"properties: { lit: { values: ["false", "true"] } }"#,
    );
    let reader = block_file("reader", "stats: { power: 7 }");
    let far = block_file("far", "");
    let setter = block_file("setter", "");
    let peeker = block_file("peeker", "");
    let spoiler = block_file("spoiler", "block_entity: {}");
    let toggler = block_file(
        "toggler",
        r#"properties: { lit: { values: ["false", "true"] } }"#,
    );
    test_mods(&[
        ("blocks/switch.json5", &switch),
        ("blocks/marker.json5", &marker),
        ("blocks/grower.json5", &grower),
        ("blocks/faulty.json5", &faulty),
        ("blocks/greedy.json5", &greedy),
        ("blocks/keeper.json5", &keeper),
        ("blocks/reader.json5", &reader),
        ("blocks/far.json5", &far),
        ("blocks/setter.json5", &setter),
        ("blocks/peeker.json5", &peeker),
        ("blocks/spoiler.json5", &spoiler),
        ("blocks/toggler.json5", &toggler),
        ("items/wand.json5", r#"{ script: "wand" }"#),
        ("scripts/switch.luau", SWITCH),
        ("scripts/marker.luau", MARKER),
        ("scripts/grower.luau", GROWER),
        ("scripts/wand.luau", WAND),
        ("scripts/faulty.luau", FAULTY),
        ("scripts/greedy.luau", GREEDY),
        ("scripts/keeper.luau", KEEPER),
        ("scripts/reader.luau", READER),
        ("scripts/far.luau", FAR),
        ("scripts/setter.luau", SETTER),
        ("scripts/peeker.luau", PEEKER),
        ("scripts/spoiler.luau", SPOILER),
        ("scripts/toggler.luau", TOGGLER),
    ])
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
    let runtime = ScriptRuntime::new(sources, ScriptSettings::untimed());
    assert!(
        runtime.load_errors().is_empty(),
        "{:?}",
        runtime.load_errors()
    );
    runtime
}

fn state(id: &str, values: &[(&str, &str)]) -> BlockState {
    content().block_state(id, values).unwrap()
}

/// The scripts and block entities over two empty chunks, (0, 0, 0) and
/// the one above it, stepping 50 ms (three ticks) a frame.
fn app() -> App {
    let mut app = chunk_app(content().clone(), Duration::from_millis(50));
    app.add_plugins(AssetPlugin::default())
        .insert_resource(ScriptSettings::untimed())
        .insert_resource(HookSettings::untimed())
        .insert_resource(runtime())
        .add_plugins((BlockEntityPlugin, ScriptPlugin));
    for coord in [IVec3::ZERO, IVec3::Y] {
        insert_chunk(&mut app, coord, Chunk::default());
    }
    app.update();
    app
}

fn set(app: &mut App, pos: IVec3, state: BlockState) {
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| voxels.set(pos, state))
        .unwrap();
}

fn get(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
        .unwrap()
}

/// Places `state` at `pos` and lets the hooks that follow run.
fn place(app: &mut App, pos: IVec3, state: BlockState) {
    set(app, pos, state);
    frames(app, 2);
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn data(app: &App, pos: IVec3) -> std::collections::BTreeMap<String, Value> {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world().get::<BlockData>(entity).unwrap().0.clone()
}

fn player() -> PlayerView {
    PlayerView {
        pos: Vec3::new(0.5, 5.5, 0.5),
        look: Vec3::NEG_Z,
        mode: GameMode::Creative,
        sneaking: false,
        target: None,
    }
}

fn use_block(app: &mut App, pos: IVec3) {
    app.world_mut().write_message(BlockUsed {
        pos,
        player: Some(player()),
    });
    frames(app, 2);
}

fn disabled(app: &App, pos: IVec3) -> bool {
    app.world()
        .resource::<ScriptRuntime>()
        .is_disabled(Caller::Block(pos))
}

const AT: IVec3 = IVec3::new(4, 4, 4);

#[test]
fn the_index_knows_each_blocks_hooks() {
    let index = ScriptIndex::new(content(), &runtime());
    let switch = index.block(content(), state("test:switch", &[])).unwrap();
    assert_eq!(switch.script, "test:switch");
    assert!(switch.used && !switch.placed && !switch.random_tick);
    let marker = index.block(content(), state("test:marker", &[])).unwrap();
    assert!(marker.placed && marker.broken && marker.neighbour_changed && !marker.used);
    assert!(index.ticks_randomly(content(), state("test:grower", &[])));
    assert!(
        index
            .block(content(), state("defaria:glass", &[]))
            .is_none()
    );
    let wand = content().default_item("test:wand").unwrap();
    assert_eq!(index.item_use(content(), wand), Some("test:wand"));
    assert!(index.any_random_tick());
    // With no scripts, nothing ticks randomly, and random ticks skip
    // looking for blocks that do.
    let none = ScriptIndex::new(
        content(),
        &ScriptRuntime::new(ScriptSources::default(), default()),
    );
    assert!(!none.any_random_tick());
}

#[test]
fn using_a_block_runs_on_used_which_changes_its_state_and_data() {
    let mut app = app();
    place(&mut app, AT, state("test:switch", &[]));
    use_block(&mut app, AT);
    assert_eq!(get(&mut app, AT), state("test:switch", &[("lit", "true")]));
    let data = data(&app, AT);
    assert_eq!(data["uses"], Value::Int(1));
    assert_eq!(data["by"], Value::Text("creative".into()));
    use_block(&mut app, AT);
    assert_eq!(get(&mut app, AT), state("test:switch", &[("lit", "false")]));
    assert_eq!(self::data(&app, AT)["uses"], Value::Int(2));
}

#[test]
fn placing_and_breaking_run_on_placed_and_on_broken() {
    let mut app = app();
    place(&mut app, AT, state("test:marker", &[]));
    assert_eq!(get(&mut app, AT + IVec3::Y), state("defaria:glass", &[]));
    place(&mut app, AT, BlockState::AIR);
    assert_eq!(get(&mut app, AT), state("defaria:planks", &[]));
}

#[test]
fn a_change_next_to_a_block_runs_on_neighbour_changed_with_the_face() {
    let mut app = app();
    place(&mut app, AT, state("test:marker", &[]));
    for (offset, face) in [(IVec3::X, "east"), (IVec3::NEG_Z, "north")] {
        place(&mut app, AT + offset, state("defaria:planks", &[]));
        assert_eq!(get(&mut app, AT), state("test:marker", &[("face", face)]));
    }
}

#[test]
fn random_ticks_grow_blocks_that_have_the_hook_and_are_capped() {
    let mut app = app();
    app.insert_resource(RandomTicks {
        per_chunk: 4096,
        max_per_tick: 5,
        ..default()
    });
    let young = state("test:grower", &[]);
    let positions: Vec<IVec3> = (0..16).map(|x| IVec3::new(x, 2, 2)).collect();
    for &pos in &positions {
        set(&mut app, pos, young);
    }
    frames(&mut app, 1);
    assert!(
        app.world()
            .resource::<TickingChunks>()
            .contains(ChunkCoord(IVec3::ZERO))
    );
    // The chunk above has no growers, so nothing in it is ticked.
    assert!(
        !app.world()
            .resource::<TickingChunks>()
            .contains(ChunkCoord(IVec3::Y))
    );
    let age = |app: &mut App| -> usize {
        positions
            .iter()
            .map(|&pos| {
                let state = get(app, pos);
                let (_, age) = &content().state(state).values[0];
                age.parse::<usize>().unwrap()
            })
            .sum()
    };
    let before = age(&mut app);
    // One frame is three ticks, at most five steps each.
    frames(&mut app, 1);
    let grown = age(&mut app) - before;
    assert!(grown > 0 && grown <= 15, "{grown}");
    frames(&mut app, 20);
    assert_eq!(age(&mut app), 3 * positions.len());
}

#[test]
fn chunks_loaded_with_random_ticking_blocks_are_found() {
    let mut app = app();
    let mut chunk = Chunk::default();
    chunk.set_at(IVec3::new(1, 1, 1), state("test:grower", &[]));
    let coord = ChunkCoord(IVec3::X);
    insert_chunk(&mut app, coord.0, chunk);
    frames(&mut app, 1);
    assert!(app.world().resource::<TickingChunks>().contains(coord));
}

#[test]
fn using_an_item_runs_on_use_and_uses_one_up_in_survival() {
    let mut app = app();
    let wand = content().default_item("test:wand").unwrap();
    let mut inventory = Inventory::new();
    assert!(inventory.add(content(), ItemStack::new(wand, 2)).is_none());
    let player_entity = app.world_mut().spawn((inventory, GameMode::Survival)).id();
    place(&mut app, AT, state("defaria:planks", &[]));
    let view = PlayerView {
        mode: GameMode::Survival,
        target: Some((AT, Some(Direction::Up))),
        ..player()
    };
    app.world_mut().write_message(ItemUsed {
        player: player_entity,
        item: wand,
        view,
    });
    frames(&mut app, 2);
    assert_eq!(get(&mut app, AT), state("defaria:glass", &[]));
    let inventory = app.world().get::<Inventory>(player_entity).unwrap();
    assert_eq!(inventory.count(wand), 1);
    // With nothing targeted it does nothing, and isn't used up.
    app.world_mut().write_message(ItemUsed {
        player: player_entity,
        item: wand,
        view: player(),
    });
    frames(&mut app, 2);
    let inventory = app.world().get::<Inventory>(player_entity).unwrap();
    assert_eq!(inventory.count(wand), 1);
}

#[test]
fn an_error_disables_only_that_voxel_until_another_block_is_placed() {
    let mut app = app();
    let other = AT + IVec3::X * 2;
    place(&mut app, AT, state("test:faulty", &[]));
    place(&mut app, other, state("test:switch", &[]));
    use_block(&mut app, AT);
    assert!(disabled(&app, AT));
    let line = app.world().resource::<ScriptRuntime>().log_lines();
    assert!(line.is_empty());
    use_block(&mut app, other);
    assert!(!disabled(&app, other));
    assert_eq!(
        get(&mut app, other),
        state("test:switch", &[("lit", "true")])
    );
    place(&mut app, AT, state("defaria:planks", &[]));
    assert!(!disabled(&app, AT));
}

#[test]
fn hooks_cant_change_blocks_out_of_their_reach() {
    let mut app = app();
    place(&mut app, AT, state("test:greedy", &[]));
    use_block(&mut app, AT);
    assert!(disabled(&app, AT));
    assert!(get(&mut app, AT + IVec3::X * 3).is_air());
}

#[test]
fn positions_no_world_reaches_are_errors() {
    let mut app = app();
    place(&mut app, AT, state("test:far", &[]));
    use_block(&mut app, AT);
    assert!(!disabled(&app, AT));
    let lines = app.world().resource::<ScriptRuntime>().log_lines();
    let message = &lines.last().unwrap().message;
    assert!(message.starts_with("false false false "), "{message}");
    assert!(message.contains("at most 1073741824"), "{message}");
}

/// `set` neither makes nor destroys fluid, nor replaces a block with a
/// block entity (with what that holds), nor makes one.
#[test]
fn set_doesnt_make_or_lose_fluid_or_block_entities() {
    let mut app = app();
    let water = state("defaria:water", &[]);
    let switch = state("test:switch", &[]);
    place(&mut app, AT - IVec3::Y, water);
    place(&mut app, AT + IVec3::X, switch);
    place(&mut app, AT, state("test:setter", &[]));
    use_block(&mut app, AT);
    assert!(!disabled(&app, AT));
    let lines = app.world().resource::<ScriptRuntime>().log_lines();
    let message = &lines.last().unwrap().message;
    assert!(
        message.starts_with("false false false false false false "),
        "{message}"
    );
    assert!(message.contains("tanks pour and draw"), "{message}");
    assert!(message.contains("can't replace `test:switch`"), "{message}");
    assert!(message.contains("can't make `test:switch`"), "{message}");
    assert!(get(&mut app, AT + IVec3::Y).is_air());
    assert!(get(&mut app, AT - IVec3::X).is_air());
    let below = app
        .world_mut()
        .run_system_once(|voxels: Voxels| voxels.get_with_fluid(AT - IVec3::Y).unwrap())
        .unwrap();
    let full = defaria_core::world::VoxelFluid::full(water);
    assert_eq!(
        below,
        (BlockState::AIR, Some(full)),
        "the water is still there"
    );
    assert_eq!(get(&mut app, AT + IVec3::X), switch);
}

/// Out of its reach, a hook reads a block's data but can't change it.
#[test]
fn hooks_cant_change_data_out_of_their_reach() {
    let mut app = app();
    let far = AT + IVec3::X * 3;
    place(&mut app, far, state("test:switch", &[]));
    let inner = Value::Table([("n".to_string(), Value::Int(1))].into());
    let entity = app.world().resource::<BlockEntities>().get(far).unwrap();
    app.world_mut().get_mut::<BlockData>(entity).unwrap().0 = [("inner".to_string(), inner)].into();
    let before = data(&app, far);
    place(&mut app, AT, state("test:peeker", &[]));
    use_block(&mut app, AT);
    assert!(!disabled(&app, AT));
    let lines = app.world().resource::<ScriptRuntime>().log_lines();
    assert_eq!(lines.last().unwrap().message, "1 false false");
    assert_eq!(data(&app, far), before);
}

/// A hook that leaves bad data in one block saves none of its changes.
#[test]
fn bad_data_anywhere_fails_every_change_to_data() {
    let mut app = app();
    place(&mut app, AT + IVec3::X, state("test:switch", &[]));
    place(&mut app, AT, state("test:spoiler", &[]));
    use_block(&mut app, AT);
    assert!(disabled(&app, AT));
    assert!(data(&app, AT).is_empty());
    assert!(data(&app, AT + IVec3::X).is_empty());
}

/// Once a tick's hooks have used its steps, the rest wait, in order.
#[test]
fn hooks_wait_once_the_ticks_steps_are_spent() {
    let mut app = app();
    // Any call spends this, so one hook runs a tick.
    app.world_mut()
        .resource_mut::<HookSettings>()
        .steps_per_tick = 1;
    let switches: Vec<IVec3> = (0..8).map(|x| IVec3::new(2 * x, 4, 4)).collect();
    for &pos in &switches {
        place(&mut app, pos, state("test:switch", &[]));
    }
    for &pos in &switches {
        app.world_mut().write_message(BlockUsed {
            pos,
            player: Some(player()),
        });
    }
    let lit = state("test:switch", &[("lit", "true")]);
    let mut counts = Vec::new();
    for _ in 0..5 {
        frames(&mut app, 1);
        let lit: Vec<bool> = switches
            .iter()
            .map(|&pos| get(&mut app, pos) == lit)
            .collect();
        // Oldest first: every switch lit comes before every one that isn't.
        assert!(lit.is_sorted_by(|a, b| a >= b), "{lit:?}");
        counts.push(lit.iter().filter(|&&lit| lit).count());
    }
    // A frame is three ticks, so three a frame.
    assert!(counts[0] < switches.len(), "{counts:?}");
    assert_eq!(*counts.last().unwrap(), switches.len(), "{counts:?}");
}

/// Once a tick's hooks have taken its time, the rest wait, in order; the
/// first of each tick still runs.
#[test]
fn hooks_wait_once_the_ticks_time_is_spent() {
    let mut app = app();
    app.world_mut().resource_mut::<HookSettings>().time_per_tick = Duration::ZERO;
    let switches: Vec<IVec3> = (0..8).map(|x| IVec3::new(2 * x, 4, 4)).collect();
    for &pos in &switches {
        place(&mut app, pos, state("test:switch", &[]));
    }
    for &pos in &switches {
        app.world_mut().write_message(BlockUsed {
            pos,
            player: Some(player()),
        });
    }
    let lit = state("test:switch", &[("lit", "true")]);
    let mut counts = Vec::new();
    for _ in 0..5 {
        frames(&mut app, 1);
        let lit: Vec<bool> = switches
            .iter()
            .map(|&pos| get(&mut app, pos) == lit)
            .collect();
        assert!(lit.is_sorted_by(|a, b| a >= b), "{lit:?}");
        counts.push(lit.iter().filter(|&&lit| lit).count());
    }
    // One a tick, three ticks a frame.
    assert!(counts[0] > 0 && counts[0] < switches.len(), "{counts:?}");
    assert_eq!(*counts.last().unwrap(), switches.len(), "{counts:?}");
}

#[test]
fn a_full_queue_drops_new_hooks_until_it_drains() {
    let mut pending = PendingHooks::default();
    for i in 0..MAX_WAITING + 10 {
        pending.push(Hook::Placed(IVec3::new(i as i32, 0, 0)));
    }
    assert_eq!(pending.len(), MAX_WAITING);
    assert!(pending.dropping.dropping());
    pending.hooks.drain(..5);
    pending.push(Hook::Placed(IVec3::NEG_ONE));
    assert_eq!(pending.len(), MAX_WAITING - 4);
    assert_eq!(pending.hooks.last(), Some(&Hook::Placed(IVec3::NEG_ONE)));
    // Still nearly full: a place freeing up doesn't make it warn again.
    assert!(pending.dropping.dropping());
    pending.hooks.drain(..MAX_WAITING / 2);
    pending.push(Hook::Placed(IVec3::ONE));
    assert!(!pending.dropping.dropping());
}

/// A queue held full warns once, not each time it frees a place, until
/// it has drained below half its cap.
#[test]
fn a_full_queue_warns_once_until_it_drains_by_half() {
    let mut warning = DropWarning::default();
    assert!(warning.admits(9, 10, "things"));
    assert!(!warning.admits(10, 10, "things"));
    assert!(warning.dropping());
    assert!(warning.admits(9, 10, "things"));
    assert!(warning.dropping());
    assert!(warning.admits(4, 10, "things"));
    assert!(!warning.dropping());
}

/// While more than a tick's worth of hooks waits, no random ticks are
/// picked.
#[test]
fn a_backlog_stops_random_ticks_being_picked() {
    let mut app = app();
    app.insert_resource(RandomTicks {
        per_chunk: 4096,
        ..default()
    });
    // Nothing runs, so what's queued stays.
    *app.world_mut().resource_mut::<HookSettings>() = HookSettings {
        max_per_tick: 2,
        steps_per_tick: 0,
        ..default()
    };
    let young = state("test:grower", &[]);
    set(&mut app, AT, young);
    {
        let mut pending = app.world_mut().resource_mut::<PendingHooks>();
        for x in 0..3 {
            pending.push(Hook::Placed(IVec3::new(x, 10, 10)));
        }
    }
    frames(&mut app, 5);
    let pending = app.world().resource::<PendingHooks>();
    assert_eq!(pending.len(), 3, "{:?}", pending.hooks);
    assert!(
        app.world()
            .resource::<TickingChunks>()
            .contains(ChunkCoord(IVec3::ZERO))
    );
    // With the backlog gone, they're picked again.
    app.world_mut().resource_mut::<PendingHooks>().hooks.clear();
    frames(&mut app, 1);
    let pending = app.world().resource::<PendingHooks>();
    assert!(
        pending
            .hooks
            .iter()
            .any(|hook| matches!(hook, Hook::RandomTick(_))),
        "{:?}",
        pending.hooks
    );
}

/// Blocks that keep setting each other off, faster than their hooks run,
/// don't queue more and more: a neighbour hook already waiting isn't
/// queued again.
#[test]
fn blocks_setting_each_other_off_keep_the_queue_bounded() {
    let mut app = app();
    // Any call spends this, so one hook runs a tick.
    app.world_mut()
        .resource_mut::<HookSettings>()
        .steps_per_tick = 1;
    let togglers: Vec<IVec3> = (0..3).map(|x| IVec3::new(4 + x, 4, 4)).collect();
    for &pos in &togglers {
        set(&mut app, pos, state("test:toggler", &[]));
    }
    // Sets them off.
    place(&mut app, IVec3::new(4, 5, 4), state("defaria:planks", &[]));
    let mut most = 0;
    for _ in 0..100 {
        frames(&mut app, 1);
        most = most.max(app.world().resource::<PendingHooks>().len());
    }
    // Each face of each toggler next to another: four, plus the plank's.
    assert!(most <= 5, "{most} waiting");
    // They're still going.
    assert!(app.world().resource::<PendingHooks>().len() > 0);
}

#[test]
fn handles_kept_from_an_earlier_call_are_refused() {
    let mut app = app();
    let keeper = state("test:keeper", &[]);
    place(&mut app, AT, keeper);
    use_block(&mut app, AT);
    assert!(!disabled(&app, AT));
    use_block(&mut app, AT);
    assert!(disabled(&app, AT));
    assert_eq!(get(&mut app, AT), keeper);
}

#[test]
fn hooks_read_the_world_and_the_content() {
    let mut app = app();
    place(&mut app, AT - IVec3::Y, state("defaria:planks", &[]));
    place(&mut app, AT, state("test:reader", &[]));
    use_block(&mut app, AT);
    assert!(!disabled(&app, AT));
    let lines = app.world().resource::<ScriptRuntime>().log_lines();
    assert_eq!(
        lines.last().unwrap().message,
        "defaria:planks nil Glass true 7"
    );
}

/// A hook sees the fluid in a voxel, beside its block: water is air with
/// water in it, and a slab in water holds half a block of it.
#[test]
fn hooks_read_a_voxels_fluid() {
    let mut app = app();
    place(&mut app, AT - IVec3::Y, state("defaria:water", &[]));
    place(&mut app, AT, state("test:reader", &[]));
    use_block(&mut app, AT);
    let message = |app: &App| {
        let lines = app.world().resource::<ScriptRuntime>().log_lines();
        lines.last().unwrap().message.clone()
    };
    assert_eq!(message(&app), "air defaria:water 1000 Glass true 7");
    place(&mut app, AT - IVec3::Y, state("defaria:stone_slab", &[]));
    use_block(&mut app, AT);
    assert_eq!(
        message(&app),
        "defaria:stone_slab defaria:water 500 Glass true 7"
    );
}

/// The base game's switched lamp turns on and off, and counts it.
#[test]
fn the_switched_lamp_turns_on_and_off() {
    let mut app = app();
    let off = state("defaria:switch_lamp", &[]);
    place(&mut app, AT, off);
    assert_eq!(data(&app, AT)["switched"], Value::Int(0));
    use_block(&mut app, AT);
    let on = get(&mut app, AT);
    assert_eq!(on, state("defaria:switch_lamp", &[("lit", "true")]));
    assert!(content().state(on).emits_light());
    assert!(!content().state(off).emits_light());
    use_block(&mut app, AT);
    assert_eq!(get(&mut app, AT), off);
    assert_eq!(data(&app, AT)["switched"], Value::Int(2));
}

/// The base game's mossy stone spreads onto bare stone open to the air,
/// and nothing else: not stone under water, whose voxels are air with
/// water in them.
#[test]
fn moss_creeps_onto_bare_stone_open_to_the_air() {
    let mut app = app();
    app.insert_resource(RandomTicks {
        per_chunk: 4096,
        ..default()
    });
    let stone = state("defaria:stone", &[]);
    let ore = state("defaria:stone", &[("ore", "defaria:iron")]);
    let planks = state("defaria:planks", &[]);
    let mossy = state("defaria:mossy_stone", &[]);
    let (open, buried, with_ore) = (AT + IVec3::X, AT + IVec3::Z, AT - IVec3::X);
    set(&mut app, open, stone);
    set(&mut app, with_ore, ore);
    set(&mut app, buried, stone);
    for side in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z] {
        set(&mut app, buried + side, planks);
    }
    let drowned = AT - IVec3::Z;
    set(&mut app, drowned, stone);
    for side in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::NEG_Z] {
        set(&mut app, drowned + side, state("defaria:water", &[]));
    }
    place(&mut app, AT, mossy);
    frames(&mut app, 60);
    assert_eq!(get(&mut app, open), mossy);
    assert_eq!(get(&mut app, buried), stone);
    assert_eq!(get(&mut app, drowned), stone);
    assert_eq!(get(&mut app, with_ore), ore);
}

/// The base game's moss item makes bare stone mossy, and is used up.
#[test]
fn moss_used_on_stone_makes_it_mossy() {
    let mut app = app();
    let moss = content().default_item("defaria:moss").unwrap();
    let mut inventory = Inventory::new();
    assert!(inventory.add(content(), ItemStack::new(moss, 1)).is_none());
    let player_entity = app.world_mut().spawn((inventory, GameMode::Survival)).id();
    place(&mut app, AT, state("defaria:stone", &[]));
    app.world_mut().write_message(ItemUsed {
        player: player_entity,
        item: moss,
        view: PlayerView {
            mode: GameMode::Survival,
            target: Some((AT, Some(Direction::Up))),
            ..player()
        },
    });
    frames(&mut app, 2);
    assert_eq!(get(&mut app, AT), state("defaria:mossy_stone", &[]));
    let inventory = app.world().get::<Inventory>(player_entity).unwrap();
    assert_eq!(inventory.count(moss), 0);
}

/// Every script the base game's blocks and items name is there and loads.
#[test]
fn the_base_games_scripts_load() {
    let content = defaria_core::content::base();
    let ids: Vec<String> = content.mods.iter().map(|m| m.id.clone()).collect();
    let runtime = ScriptRuntime::new(
        ScriptSources::read(&defaria_core::content::mods_dir(), &ids),
        ScriptSettings::untimed(),
    );
    assert!(
        runtime.load_errors().is_empty(),
        "{:?}",
        runtime.load_errors()
    );
    let scripts = content
        .blocks
        .iter()
        .filter_map(|b| b.script.as_ref())
        .chain(content.items.iter().filter_map(|i| i.script.as_ref()));
    let mut count = 0;
    for script in scripts {
        assert!(runtime.is_loaded(script), "{script}");
        count += 1;
    }
    assert!(count >= 3);
}
