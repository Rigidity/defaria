//! End-to-end tests: whole scripts run in the game with no GPU.

use super::SETTLE_TIMEOUT_FRAMES;

use bevy::{prelude::*, window::PrimaryWindow};

use super::{
    Script, memory, run_more,
    runner::ScriptRunner,
    settle,
    test_app::{camera, light_at, play, run_game},
    test_game,
};
use crate::{
    content::BlockState,
    fly_camera::FlyCamera,
    fog::ChunkFog,
    player::{LocalPlayer, Look, Player, eyes},
    world::{Chunk, ChunkCoord, ChunkMap},
};

/// A script with `systems` times the game's systems, and the report
/// names them: the meshing runs every frame, if only to find nothing
/// changed. Systems that only run other schedules aren't listed.
#[test]
fn systems_reports_the_games_systems() {
    let (mut app, exit) = run_game("wait 3\nsystems 3\nwait 2\n");
    assert_eq!(exit, AppExit::Success);
    let lines = super::system_timing::report(app.world_mut(), usize::MAX);
    assert!(lines[0].starts_with("systems over "), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("remesh_changed_chunks")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.ends_with("::run_main")),
        "{lines:?}"
    );
    // Without `systems` in the script, nothing is timed.
    let (mut app, _) = run_game("wait 3\n");
    assert_eq!(
        super::system_timing::report(app.world_mut(), 5),
        ["systems: not timed"]
    );
}

/// `settle` waits for the world around a new camera position to load,
/// so a `fill` right after it lands in loaded chunks.
#[test]
fn settle_waits_for_the_world_after_a_jump() {
    let (mut app, exit) = run_game(
        "camera 5000.5 -1000.5 5000.5 0 0\n\
         settle\n\
         fill 5000 -1000 5000 5000 -1000 5000 red_lamp\n\
         wait 1\n",
    );
    assert_eq!(exit, AppExit::Success);
    let runner = app.world().resource::<ScriptRunner>();
    assert_eq!(runner.settle_started, None, "the settle's timing ended");
    let red = crate::content::base()
        .state(crate::content::block("red_lamp"))
        .light;
    let lamp = light_at(&mut app, IVec3::new(5000, -1000, 5000));
    assert_eq!(lamp.block()[0], red[0], "the lamp was placed and lit");
}

/// `settle`, and the start of a script, wait for the fog to open all the
/// way, so screenshots show the view as it stays.
#[test]
fn settle_waits_for_the_fog() {
    let fog = |app: &App| *app.world().resource::<ChunkFog>();
    let (app, exit) = run_game("wait 1\n");
    assert_eq!(exit, AppExit::Success);
    assert_eq!(fog(&app).scale, 1.0, "open when the script starts");
    assert!(fog(&app).settled());

    let (app, exit) = run_game(
        "camera 5000.5 70.5 5000.5 0 0\n\
         settle\n\
         wait 1\n",
    );
    assert_eq!(exit, AppExit::Success);
    assert_eq!(fog(&app).scale, 1.0, "open after the jump");
    assert!(fog(&app).settled());
}

/// Flying straight up across chunk levels, and sprinting sideways
/// across chunks, the fog stays all the way open: the chunks that start
/// loading as the camera moves on are outside it.
#[test]
fn moving_on_doesnt_bring_the_fog_in() {
    let mut app = test_game(Script::parse("wait 1\n").unwrap());
    // Big enough that the fog has a chunk to spare all round (at a
    // radius of 4 it's held at its one-chunk minimum, which doesn't
    // quite fit), and no more levels than that needs, so loading keeps
    // up on a slow machine whatever the game's defaults are.
    app.insert_resource(crate::streaming::StreamingSettings {
        radius: 6,
        above: 4,
        below: 4,
        ..default()
    });
    run_more(&mut app, "camera 8.5 40 8.5 0 0\nsettle\n");
    for moves in ["hold Space 300\n", "press KeyF\nhold KeyW 300\n"] {
        let script = Script::parse(moves).unwrap();
        app.world_mut()
            .resource_mut::<ScriptRunner>()
            .commands
            .extend(script.commands);
        let mut frames = 0;
        while !app.world().resource::<ScriptRunner>().finished() {
            app.update();
            // Loading and meshing run on other threads. Let them finish
            // before the next frame, so this checks the fog against a
            // world that keeps up, however slow the machine (CI) is.
            crate::streaming::finish_loads(app.world_mut());
            crate::meshing::testing::finish_mesh_tasks(app.world_mut());
            let fog = *app.world().resource::<ChunkFog>();
            assert_eq!(fog.scale, 1.0, "{moves:?}, frame {frames}");
            frames += 1;
            assert!(frames < 400, "the script never finished");
        }
    }
    // It really did climb, then fly on: more than two chunks each way.
    let camera = camera(&mut app).1.translation;
    assert!(camera.y > 40.0 + 32.0, "{camera}");
    assert!(camera.z < 8.5 - 32.0, "{camera}");
}

/// A world that never finishes loading fails the run rather than
/// hanging.
#[test]
fn a_world_that_never_loads_fails_the_run() {
    let mut app = test_game(Script::parse("wait 1\n").unwrap());
    app.insert_resource(crate::streaming::StreamingSettings {
        max_loading: 0,
        ..default()
    });
    // Not the real limit, which would take most of a minute in CI.
    app.world_mut()
        .resource_mut::<ScriptRunner>()
        .settle_timeout = 100;
    for _ in 0..110 {
        app.update();
        if let Some(exit) = app.should_exit() {
            assert!(exit.is_error());
            return;
        }
    }
    panic!("the run never gave up");
}

/// Each `timing` covers the frames since the one before.
#[test]
fn timing_starts_over_after_each_report() {
    let (app, exit) = run_game("wait 3\ntiming\nwait 2\n");
    assert_eq!(exit, AppExit::Success);
    let runner = app.world().resource::<ScriptRunner>();
    assert!(!runner.timing);
    assert!(
        runner.frame_times.len() <= 3,
        "{} frames kept after the report",
        runner.frame_times.len()
    );
}

/// `memory` measures the loaded world: its chunks, their light and
/// their meshes, which stay in the main world. A chunk filled with
/// stone and dug back out to air through the game's edits is kept as
/// one value again.
#[test]
fn memory_measures_the_loaded_world() {
    let (mut app, exit) = run_game(
        "memory\n\
         lookups -32 0 -32 31 31 31\n\
         wait 1\n\
         fill 64 64 80 79 79 95 stone\n\
         wait 1\n\
         fill 64 64 80 79 79 95 air\n\
         wait 2\n",
    );
    assert_eq!(exit, AppExit::Success);
    assert!(!app.world().resource::<ScriptRunner>().memory_report);
    let memory = memory::ChunkMemory::measure(app.world_mut());
    assert!(memory.chunks > 100, "{memory}");
    // Chunks of one block or light throughout hold none.
    assert!(memory.air + memory.solid > 0, "{memory}");
    let dense_blocks = memory.chunks - memory.air - memory.solid;
    assert_eq!(memory.blocks, dense_blocks * 8192, "{memory}");
    assert_eq!(
        memory.light,
        (memory.chunks - memory.even_light) * 8192,
        "{memory}"
    );
    assert_eq!(memory.uniform_dense, 0, "{memory}");
    assert_eq!(memory.uniform_dense_chunks, 0, "{memory}");
    assert!(memory.meshes > 0 && memory.mesh_bytes > 0, "{memory}");
    let dug = app
        .world()
        .resource::<ChunkMap>()
        .get(ChunkCoord(IVec3::new(4, 4, 5)))
        .unwrap();
    assert!(matches!(
        app.world().get::<Chunk>(dug).unwrap().voxels(),
        crate::world::ChunkArray::Uniform(BlockState::AIR)
    ));
}

/// `cursor` puts the window's cursor where it says, and `scroll` turns
/// the wheel, which while playing steps through the hotbar.
#[test]
fn script_points_the_cursor_and_turns_the_wheel() {
    let mut app = test_game(Script::parse("wait 1").unwrap());
    settle(&mut app);
    run_more(&mut app, "cursor 100 200.5\nscroll -3\nwait 2");
    let mut windows = app
        .world_mut()
        .query_filtered::<&Window, With<PrimaryWindow>>();
    let window = windows.single(app.world()).unwrap();
    assert_eq!(window.cursor_position(), Some(Vec2::new(100.0, 200.5)));
    let mut players = app
        .world_mut()
        .query_filtered::<&crate::hotbar::ActiveSlot, With<crate::player::LocalPlayer>>();
    assert_eq!(players.single(app.world()).unwrap().0, 3);
}

/// End to end: the whole game, no GPU, driven by a script.
#[test]
fn script_drives_the_fly_camera() {
    let script = Script::parse(
        "camera 64.5 62 92.5 0 0  # flying, facing north\n\
         wait 1\n\
         hold MouseLeft 1  # lock the cursor\n\
         hold KeyW 30\n\
         mouse 100 0\n\
         wait 1\n\
         inspect FlyCamera\n\
         screenshot target/never-written.png\n",
    )
    .unwrap();
    let mut app = test_game(script);
    let mut frames = 0;
    let exit = loop {
        app.update();
        frames += 1;
        if let Some(exit) = app.should_exit() {
            break exit;
        }
        assert!(frames < SETTLE_TIMEOUT_FRAMES, "script never finished");
    };
    assert_eq!(exit, AppExit::Success);

    let (end_camera, end_transform) = camera(&mut app);
    // Half a second of W, flying, from a standstill: about 5 blocks
    // north (−Z), level, along where it faced before turning.
    let moved = end_transform.translation - Vec3::new(64.5, 62.0, 92.5);
    assert!((-5.5..-4.6).contains(&moved.z), "{moved}");
    assert!(moved.x.abs() < 1e-3 && moved.y.abs() < 1e-3, "{moved}");
    // Moving the mouse 100px right turns the player right.
    let turned = end_camera.yaw;
    assert!(
        (turned + 100.0 * FlyCamera::default().sensitivity).abs() < 1e-5,
        "{turned}"
    );
}

/// The game draws through one camera, the eye view: a child of the
/// eyes, with a 70° field of view and the colours left alone.
#[test]
fn the_game_draws_through_one_eye_view() {
    use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};

    use crate::fly_camera::{EyeView, FOV, NEAR};

    let mut app = play("wait 1\n");
    let world = app.world_mut();
    let cameras: Vec<_> = world
        .query_filtered::<Entity, With<Camera>>()
        .iter(world)
        .collect();
    assert_eq!(cameras.len(), 1, "{cameras:?}");
    let view = world.entity(cameras[0]);
    assert!(view.contains::<EyeView>());
    let parent = view.get::<ChildOf>().expect("a child").parent();
    let camera = world.entity(parent);
    assert!(camera.contains::<FlyCamera>());
    // The camera is a child of the local player, drawn at its eyes.
    let player = camera.get::<ChildOf>().expect("a child").parent();
    let player = world.entity(player);
    assert!(player.contains::<LocalPlayer>());
    let eyes = eyes(
        player.get::<Transform>().unwrap(),
        player.get::<Player>().unwrap(),
        player.get::<Look>().unwrap(),
    );
    let drawn = camera.get::<GlobalTransform>().unwrap().compute_transform();
    assert!(
        drawn.translation.distance(eyes.translation) < 1e-4,
        "{drawn:?} {eyes:?}"
    );
    assert!(
        drawn.rotation.angle_between(eyes.rotation) < 1e-4,
        "{drawn:?} {eyes:?}"
    );
    let Some(Projection::Perspective(projection)) = view.get::<Projection>() else {
        panic!("not a perspective projection");
    };
    assert!((projection.fov - FOV.to_radians()).abs() < 1e-6);
    assert_eq!(FOV, 70.0);
    // Near enough to see a water surface the eyes are just off.
    assert_eq!(projection.near, NEAR);
    const { assert!(NEAR <= 0.01) };
    assert_eq!(view.get::<Tonemapping>(), Some(&Tonemapping::None));
    assert_eq!(view.get::<DebandDither>(), Some(&DebandDither::Disabled));
}

/// End to end, the eye view the game draws through sways as the player
/// walks and widens as it sprints.
#[test]
fn walking_sways_and_sprinting_widens_the_eye_view() {
    use crate::fly_camera::{EyeView, FOV};

    let mut app = play(
        "fill 56 50 84 72 50 100 stone\n\
         camera 64.5 52.7 92.5 0 0\n\
         hold ShiftLeft 40  # down onto the stone\n\
         wait 10\n\
         press KeyW\n\
         press KeyF\n\
         wait 40\n",
    );
    let world = app.world_mut();
    let player = world
        .query_filtered::<&Player, With<LocalPlayer>>()
        .single(world)
        .unwrap();
    assert!(!player.flying && player.sprinting, "{player:?}");
    let (sway, projection) = world
        .query_filtered::<(&Transform, &Projection), With<EyeView>>()
        .single(world)
        .unwrap();
    assert_ne!(*sway, Transform::IDENTITY);
    let Projection::Perspective(projection) = projection else {
        panic!("not a perspective projection");
    };
    let fov = projection.fov.to_degrees();
    assert!(fov > FOV * 1.1, "{fov}");
}
