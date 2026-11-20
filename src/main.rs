use bevy::prelude::*;
use defaria_2026::{DevToolsPlugin, GamePlugin, ScreenshotMode};

fn main() -> AppExit {
    let screenshot = ScreenshotMode::from_args(std::env::args().skip(1));

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "defaria-2026".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((GamePlugin, DevToolsPlugin { screenshot }))
        .run()
}
