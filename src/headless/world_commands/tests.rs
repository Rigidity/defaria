use super::*;
use crate::{
    headless::{
        Script,
        test_app::{camera, light_at, play, run_game},
    },
    world::{ChunkCoord, ChunkMap},
};

/// End to end: `fill` builds a sealed room through the game's own edit
/// path, so it's lit like the player's edits: dark inside, with a lamp's
/// light falling off a level per block. `camera` moves the camera.
#[test]
fn fill_and_camera_set_up_a_scene() {
    let (mut app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 60 30 60 stone\n\
         fill 51 21 51 59 29 59 air\n\
         fill 55 25 55 55 25 55 red_lamp\n\
         camera 55.5 26.5 52 180 -10\n\
         wait 1\n\
         timing\n",
    );
    assert_eq!(exit, AppExit::Success);
    let red = crate::content::base()
        .state(crate::content::block("red_lamp"))
        .light;
    for d in 1..4 {
        let light = light_at(&mut app, IVec3::new(55 - d, 25, 55));
        assert_eq!(light.sky(), 0, "the room is sealed");
        assert_eq!(light.block()[0], red[0] - d as u8);
    }
    assert_eq!(light_at(&mut app, IVec3::new(52, 22, 52)).sky(), 0);
    let (camera, transform) = camera(&mut app);
    assert!(
        transform.translation.distance(Vec3::new(55.5, 26.5, 52.0)) < 1e-4,
        "{transform:?}"
    );
    assert!((camera.yaw - std::f32::consts::PI).abs() < 1e-5);
    assert!((camera.pitch + 10f32.to_radians()).abs() < 1e-5);
    // Yaw 180 faces south (+Z), at the lamp.
    assert!(transform.forward().z > 0.9);
}

/// End to end: `random-ticks` speeds random ticks up, so mossy stone
/// spreads over a stone wall within a second; right click on the
/// switched lamp turns it on through its script, without placing.
#[test]
fn scripted_blocks_run_in_the_game() {
    let (mut app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 54 22 50 stone\n\
         fill 50 20 50 50 20 50 mossy_stone\n\
         fill 52 24 52 52 24 52 switch_lamp\n\
         camera 52.5 26.5 52.5 0 -89\n\
         random-ticks 512\n\
         hold MouseLeft 1\n\
         wait 2\n\
         hold MouseRight 1\n\
         wait 60\n",
    );
    assert_eq!(exit, AppExit::Success);
    let base = crate::content::base();
    let mut at = |pos: IVec3| -> BlockState {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(move |voxels: Voxels| voxels.get(pos))
            .unwrap()
            .unwrap()
    };
    let mossy = crate::content::block("mossy_stone");
    let wall = (50..=54).flat_map(|x| (20..=22).map(move |y| IVec3::new(x, y, 50)));
    let mossy_count = wall.filter(|&pos| at(pos) == mossy).count();
    assert!(mossy_count > 3, "{mossy_count}");
    let lit = base.block_state("defaria:switch_lamp", &[("lit", "true")]);
    assert_eq!(Some(at(IVec3::new(52, 24, 52))), lit);
    assert!(at(IVec3::new(52, 25, 52)).is_air());
}

/// End to end: pipes `fill` sets in the game join the pipes next to
/// them, whatever sides the fill named, and a wire beside them doesn't.
#[test]
fn filled_pipes_join_each_other() {
    let (mut app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 52 20 50 pipe[up=true]\n\
         fill 51 21 50 51 21 50 pipe\n\
         fill 53 20 50 53 20 50 wire\n\
         wait 1\n",
    );
    assert_eq!(exit, AppExit::Success);
    let content = crate::content::base();
    use crate::content::Direction::{self, *};
    let world = app.world_mut();
    let joined = |world: &mut World, pos: IVec3| {
        let (coord, local) = ChunkCoord::split(pos);
        let entity = world.resource::<ChunkMap>().get(coord).unwrap();
        let state = world.get::<crate::world::Chunk>(entity).unwrap().at(local);
        Direction::ALL
            .into_iter()
            .filter(|&side| content.joined(state, side))
            .collect::<Vec<_>>()
    };
    assert_eq!(joined(world, IVec3::new(50, 20, 50)), [East]);
    assert_eq!(joined(world, IVec3::new(51, 20, 50)), [Up, East, West]);
    assert_eq!(joined(world, IVec3::new(52, 20, 50)), [West]);
    assert_eq!(joined(world, IVec3::new(51, 21, 50)), [Down]);
    assert_eq!(joined(world, IVec3::new(53, 20, 50)), []);
}

/// `timing` counts the voxels `fill` set since the last one, and
/// starts over.
#[test]
fn timing_counts_filled_voxels() {
    // Eight voxels of sky above the start set to stone, then the same again (changing
    // nothing), then a box in unloaded chunks far below.
    let (app, exit) = run_game(
        "fill 64 70 80 65 71 81 stone\n\
         wait 1\n\
         fill 64 70 80 65 71 81 stone\n\
         fill 0 -100000 0 1 -99999 1 stone\n\
         wait 1\n",
    );
    assert_eq!(exit, AppExit::Success);
    assert_eq!(app.world().resource::<ScriptRunner>().filled.0, 8);
    let (app, exit) = run_game("fill 0 100 0 1 101 1 air\nwait 1\ntiming\nwait 1\n");
    assert_eq!(exit, AppExit::Success);
    assert_eq!(app.world().resource::<ScriptRunner>().filled.0, 0);
}

#[test]
fn fill_names_a_block_and_some_of_its_properties() {
    let content = crate::content::base();
    let log = content.block("defaria:log").unwrap();
    assert_eq!(fill_state(content, "log"), Ok(log.default_state()));
    assert_eq!(
        fill_state(content, "defaria:log[wood=defaria:birch,axis=x]"),
        Ok(log
            .state(&[("axis", "x"), ("wood", "defaria:birch")])
            .unwrap())
    );
    for bad in ["log[axis=w]", "log[axis]", "log[axis=x", "nonsense"] {
        assert!(fill_state(content, bad).is_err(), "{bad}");
    }
}

/// Every scenario in `scenarios/` parses, and every block it fills
/// with (and item it holds) exists, so a renamed block fails `cargo test` and not just the
/// scenario run in CI.
#[test]
fn every_scenario_parses_and_fills_with_real_blocks() {
    let content = crate::content::base();
    let mut scenarios = 0;
    for entry in std::fs::read_dir("scenarios").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        scenarios += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        let script = Script::parse(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        for command in script.commands {
            if let Command::Fill { block, .. } = &command
                && block != crate::content::AIR
            {
                fill_state(content, block).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            }
            if let Command::Hotbar { item, .. } = &command {
                item_state(content, item).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            }
        }
    }
    assert!(scenarios > 0, "no scenarios found");
}

/// `hotbar` puts an item in a slot and makes it the one in hand,
/// with the properties it's given.
#[test]
fn hotbar_puts_an_item_in_hand() {
    assert_eq!(
        Script::parse("hotbar 3 wrench[head=defaria:copper]")
            .unwrap()
            .commands,
        [Command::Hotbar {
            slot: 2,
            item: "wrench[head=defaria:copper]".into()
        }]
    );
    let (mut app, exit) = run_game("wait 2\nhotbar 3 wrench[head=defaria:copper]\nwait 1\n");
    assert!(!exit.is_error());
    let content = app.world().resource::<Content>();
    let copper = content
        .item("defaria:wrench")
        .unwrap()
        .state(&[("head", "defaria:copper")])
        .unwrap();
    let (hotbar, active) = app
        .world_mut()
        .query_filtered::<(&Hotbar, &ActiveSlot), With<LocalPlayer>>()
        .single(app.world())
        .unwrap();
    assert_eq!(*active, ActiveSlot(2));
    assert_eq!(hotbar.item(*active), Some(copper));

    let (_, exit) = run_game("wait 2\nhotbar 1 nonsense\nwait 1\n");
    assert!(exit.is_error());
}

#[test]
fn filling_with_an_unknown_block_fails_the_run() {
    let (_, exit) = run_game("wait 2\nfill 1 1 1 2 2 2 nonsense\nwait 1\n");
    assert!(exit.is_error());
}

/// The amount of fluid in the voxel at `pos`.
fn fluid_at(app: &App, pos: IVec3) -> u16 {
    let (coord, local) = ChunkCoord::split(pos);
    let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
    let chunk = app.world().get::<Chunk>(entity).unwrap();
    chunk.fluid_amount_at(local, app.world().resource::<Content>())
}

/// `pour` puts fluid in a voxel through the game's own edit path, and
/// it flows from there: a block of water poured in a sealed trench
/// spreads along it, all of it still there.
#[test]
fn poured_water_spreads_and_keeps_all_of_itself() {
    let (app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 60 22 52 stone\n\
         fill 51 21 51 59 22 51 air\n\
         pour 55 21 51 water 1\n\
         wait 120\n\
         fluids\n",
    );
    assert_eq!(exit, AppExit::Success);
    let row: Vec<u16> = (51..=59)
        .map(|x| fluid_at(&app, IVec3::new(x, 21, 51)))
        .collect();
    assert_eq!(row.iter().map(|&a| a as u32).sum::<u32>(), 1000, "{row:?}");
    assert!(row.iter().filter(|&&a| a > 0).count() >= 7, "{row:?}");
}

/// The `network` report: pipes `fill` puts down form one network, a
/// wire beside them another, and a voxel with no conduit has none.
#[test]
fn the_network_report_describes_the_conduits_network() {
    let (mut app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 54 20 50 pipe\n\
         fill 52 21 50 52 22 50 pipe\n\
         fill 50 20 51 53 20 51 wire\n\
         wait 1\n\
         network 52 22 50\n",
    );
    assert_eq!(exit, AppExit::Success);
    let world = app.world_mut();
    let pipes = network_report(world, IVec3::new(50, 20, 50));
    assert!(pipes.starts_with("network at 50 20 50: #"), "{pipes}");
    assert!(
        pipes.contains(" defaria:fluid (Fluid), 7 conduits, 1 chunk, capacity 3.5, running"),
        "{pipes}"
    );
    let wires = network_report(world, IVec3::new(53, 20, 51));
    assert!(
        wires.contains(" defaria:electric (Energy), 4 conduits"),
        "{wires}"
    );
    assert_eq!(
        network_report(world, IVec3::new(50, 21, 50)),
        "network at 50 21 50: none"
    );
}

/// On an energy network, `network` adds what it stored, produced and
/// consumed: a generator lighting a lamp over a wire.
#[test]
fn the_network_report_says_what_energy_moved() {
    let (mut app, exit) = run_game(
        "wait 2\n\
         fill 49 20 50 49 20 50 creative_generator\n\
         fill 50 20 50 53 20 50 wire\n\
         fill 52 21 50 52 21 50 powered_lamp\n\
         wait 90\n",
    );
    assert_eq!(exit, AppExit::Success);
    let line = network_report(app.world_mut(), IVec3::new(51, 20, 50));
    assert!(
        line.ends_with(
            "; energy: stored 0 of 0 J, produced 20 J and consumed 20 J in the last second, throughput 1000 W"
        ),
        "{line}"
    );
}

/// The slots of the machine at `pos`.
fn machine_slots(app: &App, pos: IVec3) -> Slots {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world().get::<Slots>(entity).unwrap().clone()
}

/// `put` fills a machine's slots and tanks as a tube or pipe would: a
/// powered furnace given an ore smelts it, and a generator's tank takes
/// lava.
#[test]
fn put_fills_machines_and_a_powered_furnace_smelts() {
    let (app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 50 20 50 creative_generator\n\
         fill 50 20 51 50 20 51 wire\n\
         fill 50 20 52 50 20 52 electric_furnace[facing=south]\n\
         fill 52 20 52 52 20 52 combustion_generator\n\
         wait 1\n\
         put 50 20 52 input 1 stone[rock=defaria:granite,ore=defaria:gold]\n\
         put 52 20 52 fuel 0.5 lava\n\
         wait 360\n",
    );
    assert_eq!(exit, AppExit::Success);
    let content = app.world().resource::<Content>();
    let gold = content
        .item("defaria:ingot")
        .unwrap()
        .state(&[("metal", "defaria:gold")])
        .unwrap();
    let output = machine_slots(&app, IVec3::new(50, 20, 52));
    assert_eq!(
        output.group("output").unwrap().slots[0],
        Some(crate::inventory::ItemStack::new(gold, 1))
    );
    let entity = app
        .world()
        .resource::<BlockEntities>()
        .get(IVec3::new(52, 20, 52))
        .unwrap();
    // Its first tenth is burning.
    assert_eq!(app.world().get::<Tanks>(entity).unwrap().0[0].amount, 400);
}

/// `put` fails the run where there's no machine, no such slot group or
/// tank, or not room for it all.
#[test]
fn putting_what_doesnt_fit_fails_the_run() {
    for line in [
        "put 50 20 50 input 1 stone",
        "fill 50 20 50 50 20 50 electric_furnace\nput 50 20 50 fuel 1 stone",
        "fill 50 20 50 50 20 50 electric_furnace\nput 50 20 50 input 65 stone",
        "fill 50 20 50 50 20 50 electric_furnace\nput 50 20 50 input 1 nothing",
        "fill 50 20 50 50 20 50 combustion_generator\nput 50 20 50 fuel 5 lava",
        "fill 50 20 50 50 20 50 combustion_generator\nput 50 20 50 fuel 1 water",
    ] {
        let (_, exit) = run_game(&format!(
            "wait 2\nfill 50 20 50 50 20 50 air\n{line}\nwait 1\n"
        ));
        assert!(exit.is_error(), "{line}");
    }
}

/// The `fluids` report counts each fluid in the loaded world: deep
/// underground, where there's no other water, just what was poured.
#[test]
fn the_fluids_report_counts_what_was_poured() {
    let (mut app, exit) = run_game(
        "camera 5000.5 -1000.5 5000.5 0 0\n\
         settle\n\
         fill 4990 -1001 4990 5010 -1000 5010 air\n\
         pour 5000 -1000 5000 water 0.75\n\
         wait 1\n",
    );
    assert_eq!(exit, AppExit::Success);
    let report = fluid_report(app.world_mut());
    assert!(report.starts_with("fluids: "), "{report}");
    assert!(report.ends_with(" active, defaria:water 750"), "{report}");
}

/// The `fluids` report counts what's in machines' tanks too, and says
/// where it is: a pump has drawn the poured water into a tank. The
/// `network` report gives the pump and tank's roles.
#[test]
fn the_fluids_report_counts_tanks_and_pipes() {
    let (mut app, exit) = run_game(
        "camera 5000.5 -1000.5 5000.5 0 0\n\
         settle\n\
         fill 4996 -1002 4998 5004 -999 5002 stone\n\
         fill 4999 -1001 5000 5003 -1001 5000 air\n\
         pour 5002 -1001 5000 water 1\n\
         fill 4999 -1001 5000 4999 -1001 5000 pump[facing=east]\n\
         fill 4998 -1001 5000 4998 -1001 5000 pipe\n\
         fill 4998 -1000 5000 4998 -1000 5000 tank\n\
         wait 180\n",
    );
    assert_eq!(exit, AppExit::Success);
    let report = fluid_report(app.world_mut());
    assert!(
        report.ends_with(" active, defaria:water 1000 (world 0, pipes 0, machines 1000)"),
        "{report}"
    );
    let network = network_report(app.world_mut(), IVec3::new(4998, -1001, 5000));
    assert!(
        network.contains("4999 -1001 5000 west (output)")
            && network.contains("4998 -1000 5000 down (store)"),
        "{network}"
    );
}

/// Pouring something that isn't a fluid, or no amount, fails the run.
#[test]
fn pouring_a_solid_or_nothing_fails_the_run() {
    let (_, exit) = run_game("wait 2\npour 1 1 1 stone 1\nwait 1\n");
    assert!(exit.is_error());
    let (_, exit) = run_game("wait 2\npour 1 1 1 water 0\nwait 1\n");
    assert!(exit.is_error());
}

/// Placing a block into water deletes the water in that voxel, and
/// wakes the water around it: water held back by a wall flows on once
/// the wall is gone.
#[test]
fn blocks_delete_fluid_and_edits_wake_it() {
    let (app, exit) = run_game(
        "wait 2\n\
         fill 50 20 50 60 22 52 stone\n\
         fill 51 21 51 59 22 51 air\n\
         fill 55 21 51 55 21 51 stone\n\
         fill 51 21 51 52 21 51 water\n\
         wait 120\n\
         fill 51 21 51 51 21 51 stone\n\
         fill 55 21 51 55 21 51 air\n\
         wait 240\n",
    );
    assert_eq!(exit, AppExit::Success);
    let row: Vec<u16> = (51..=59)
        .map(|x| fluid_at(&app, IVec3::new(x, 21, 51)))
        .collect();
    assert_eq!(row[0], 0, "the stone took the water's place");
    // What the stone took is counted, and the rest is all there.
    let displaced = app.world().resource::<FluidStats>().displaced;
    assert!(displaced > 0);
    let total = row.iter().map(|&a| a as u64).sum::<u64>();
    assert_eq!(total + displaced, 2000, "{row:?}");
    assert!(row[8] > 0, "flowed past where the wall was: {row:?}");
}

/// `camera` puts the eyes where it says (to within f32 rounding), level, even straight
/// after sneaking along the ground (lower eyes, a swaying view).
#[test]
fn camera_resets_sneaking_eyes_and_the_sway() {
    use crate::view_bob::ViewBob;

    let mut app = play(
        "fill 56 50 84 72 50 100 stone\n\
         camera 64.5 52.7 92.5 0 0\n\
         press ShiftLeft  # down onto the stone, then sneak north\n\
         wait 40\n\
         press KeyW\n\
         wait 40\n\
         camera 64.5 70 92.5 0 0\n\
         release ShiftLeft\n\
         release KeyW\n\
         wait 1\n",
    );
    let (_, eyes) = camera(&mut app);
    assert!((eyes.translation.y - 70.0).abs() < 1e-4, "{eyes:?}");
    let world = app.world_mut();
    let bob = *world.query::<&ViewBob>().single(world).unwrap();
    assert_eq!(bob.amount, 0.0, "{bob:?}");
}
