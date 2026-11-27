use std::time::Duration;

use bevy::{app::ScheduleRunnerPlugin, prelude::*, window::PresentMode};
use defaria::{
    Cli, DevToolsPlugin, GamePlugin,
    headless::{self, HeadlessPlugin, Script},
    save::{self, WorldFolder},
    streaming::StreamingSettings,
};

fn main() -> AppExit {
    let cli = Cli::from_args(std::env::args().skip(1));
    let mut app = App::new();
    // Headless runs play a world in memory unless told otherwise, so every
    // run of a scenario starts from the same world.
    let world = match cli.world.clone() {
        Some(folder) => Some(folder),
        None if cli.headless.is_none() => save::default_world(),
        None => None,
    };
    if let Some(folder) = world {
        app.insert_resource(WorldFolder(folder));
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

    app.run()
}
