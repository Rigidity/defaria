use std::time::Duration;

use bevy::{app::ScheduleRunnerPlugin, prelude::*, window::PresentMode};
use defaria::{
    Cli, DevToolsPlugin, GamePlugin, dedicated,
    headless::{self, HeadlessPlugin, Script},
    identity::DataFolder,
    integrated::RemoteServer,
};
use defaria_client::session::{StartAtMenu, WorldsFolder};
use defaria_core::{
    save::{self, WorldFolder},
    streaming::StreamingSettings,
};

fn main() -> AppExit {
    let cli = Cli::from_args(std::env::args().skip(1));
    if cli.server {
        return dedicated::run(dedicated::DedicatedSettings {
            port: cli.port,
            world: cli.world,
            radius: cli.radius,
            data: cli.data.or_else(save::data_dir),
        });
    }
    let mut app = App::new();
    if let Some(address) = cli.connect.clone() {
        app.insert_resource(RemoteServer(address));
    }
    // `--world` plays that world at once. Otherwise the game opens at the
    // main menu, and headless runs play a world in memory, so every run of
    // a scenario starts from the same world; their world list (reached
    // through the pause menu) is a folder of their own, never the player's.
    match cli.world.clone() {
        Some(folder) => {
            app.insert_resource(WorldFolder(folder));
        }
        // A windowed screenshot (`cargo shot`) is of a world, in memory.
        None if cli.headless.is_none() && cli.screenshot.is_none() && cli.connect.is_none() => {
            app.insert_resource(StartAtMenu);
        }
        None => {}
    }
    // The game's data folder: the install's identity and the worlds. A
    // headless run's is its own, never the player's, so it plays as
    // someone new and its world list is empty.
    let run_data = std::env::temp_dir().join(format!("defaria-headless-{}", std::process::id()));
    let data = match (&cli.data, &cli.headless) {
        (Some(data), _) => Some(data.clone()),
        (None, Some(_)) => Some(run_data.clone()),
        (None, None) => save::data_dir(),
    };
    if let Some(data) = data {
        app.insert_resource(WorldsFolder(data.join("worlds")))
            .insert_resource(DataFolder(data));
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
    // A headless run's data is its own, and goes with it.
    let _ = std::fs::remove_dir_all(run_data);
    exit
}
