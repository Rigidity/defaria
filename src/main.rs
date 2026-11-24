use std::time::Duration;

use bevy::{app::ScheduleRunnerPlugin, prelude::*, window::PresentMode};
use defaria::{
    Cli, DevToolsPlugin, GamePlugin,
    headless::{self, HeadlessPlugin, Script},
};

fn main() -> AppExit {
    let cli = Cli::from_args(std::env::args().skip(1));
    let mut app = App::new();

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
            headless::default_plugins(rendering),
            // No window means no winit event loop; run frames back to back.
            ScheduleRunnerPlugin::run_loop(Duration::ZERO),
            GamePlugin,
            HeadlessPlugin { script, rendering },
        ));
    } else {
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Defaria".into(),
                // Uncapped frame rate: Immediate, then Mailbox, falling back
                // to Fifo (vsync) only where neither is supported.
                present_mode: PresentMode::AutoNoVsync,
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

    app.run()
}
