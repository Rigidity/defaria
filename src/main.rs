use std::time::Duration;

use bevy::{app::ScheduleRunnerPlugin, prelude::*, window::PresentMode};
use defaria::{
    Cli, DevToolsPlugin, GamePlugin,
    headless::{self, HeadlessPlugin, Script},
};
use defaria_client::session::{StartAtMenu, WorldsFolder};
use defaria_core::{
    save::{self, WorldFolder},
    streaming::StreamingSettings,
};

fn main() -> AppExit {
    let cli = Cli::from_args(std::env::args().skip(1));
    let mut app = App::new();
    // `--world` plays that world at once. Otherwise the game opens at the
    // main menu, and headless runs play a world in memory, so every run of
    // a scenario starts from the same world; their world list (reached
    // through the pause menu) is a folder of their own, never the player's.
    match cli.world.clone() {
        Some(folder) => {
            app.insert_resource(WorldFolder(folder));
        }
        // A windowed screenshot (`cargo shot`) is of a world, in memory.
        None if cli.headless.is_none() && cli.screenshot.is_none() => {
            app.insert_resource(StartAtMenu);
        }
        None => {}
    }
    let run_worlds =
        std::env::temp_dir().join(format!("defaria-headless-worlds-{}", std::process::id()));
    let worlds = match cli.headless {
        Some(_) => Some(run_worlds.clone()),
        None => save::worlds_dir(),
    };
    if let Some(worlds) = worlds {
        app.insert_resource(WorldsFolder(worlds));
    }

    if let Some(path) = cli.headless {
        let script = match Script::load(&path) {
            Ok(script) => script,
            Err(error) => {
                eprintln!("error: {error}");
                return AppExit::error();
            }
        };
        let rendering = !cli.no_render;
        app.add_plugins((
            headless::sized_plugins(rendering, cli.size.unwrap_or(headless::RESOLUTION)),
            // No window means no winit event loop; run frames back to back.
            ScheduleRunnerPlugin::run_loop(Duration::ZERO),
            GamePlugin,
            HeadlessPlugin { script, rendering },
        ));
    } else {
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Defaria".into(),
                // One frame per display refresh, so motion advances evenly
                // and nothing tears (an uncapped frame rate did both, and
                // felt unsmooth). `--no-vsync` uncaps it to measure speed.
                present_mode: if cli.no_vsync {
                    PresentMode::AutoNoVsync
                } else {
                    PresentMode::AutoVsync
                },
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            GamePlugin,
            DevToolsPlugin {
                screenshot: cli.screenshot,
            },
        ));
    }

    if let Some(radius) = cli.radius {
        app.insert_resource(StreamingSettings {
            radius,
            ..default()
        });
    }

    let exit = app.run();
    // A headless run's worlds are its own, and go with it.
    let _ = std::fs::remove_dir_all(run_worlds);
    exit
}
