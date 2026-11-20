//! Tooling for driving the game without a human watching the window.
//!
//! - Screenshot mode (`--screenshot <path> [--frames N]`): render N frames,
//!   save a PNG, exit. Works in every build.
//! - Bevy Remote Protocol (BRP), `dev` feature only: JSON-RPC over HTTP on
//!   127.0.0.1:15702 (override with `BRP_PORT`) to query/edit the live ECS.

use std::path::PathBuf;

use bevy::{
    diagnostic::FrameCount,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

/// Give up if the screenshot hasn't been captured this many frames after it was
/// requested, so a broken capture can't hang an agent forever.
const SCREENSHOT_TIMEOUT_FRAMES: u32 = 600;

pub struct DevToolsPlugin {
    pub screenshot: Option<ScreenshotMode>,
}

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        if let Some(mode) = self.screenshot.clone() {
            // No BRP here: parallel screenshot runs would fight over the port.
            app.insert_resource(mode)
                .add_systems(Update, capture_screenshot);
        } else {
            #[cfg(feature = "dev")]
            add_remote(app);
        }
    }
}

#[cfg(feature = "dev")]
fn add_remote(app: &mut App) {
    use bevy::remote::{
        RemotePlugin,
        http::{DEFAULT_PORT, RemoteHttpPlugin},
    };

    let port = std::env::var("BRP_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    app.add_plugins((
        RemotePlugin::default(),
        RemoteHttpPlugin::default().with_port(port),
    ));
}

#[derive(Resource, Clone, Debug)]
pub struct ScreenshotMode {
    pub path: PathBuf,
    /// Frame to capture on. Gives assets and the window time to settle.
    pub frames: u32,
}

impl ScreenshotMode {
    /// Parses `--screenshot <path>` and `--frames <n>`. Returns `None` when no
    /// screenshot was requested. Panics on unknown or malformed arguments.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Option<Self> {
        let mut path = None;
        let mut frames = 60;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--screenshot" => {
                    path = Some(PathBuf::from(
                        args.next().expect("--screenshot needs a path"),
                    ));
                }
                "--frames" => {
                    frames = args
                        .next()
                        .and_then(|n| n.parse().ok())
                        .expect("--frames needs a number");
                }
                other => panic!("unknown argument: {other}"),
            }
        }
        path.map(|path| Self { path, frames })
    }
}

fn capture_screenshot(
    mut commands: Commands,
    mode: Res<ScreenshotMode>,
    frame: Res<FrameCount>,
    mut exit: MessageWriter<AppExit>,
    mut requested: Local<bool>,
) {
    if frame.0 < mode.frames {
        return;
    }
    if *requested {
        if frame.0 > mode.frames + SCREENSHOT_TIMEOUT_FRAMES {
            error!("screenshot was never captured; giving up");
            exit.write(AppExit::error());
        }
        return;
    }
    *requested = true;

    if let Some(dir) = mode.path.parent() {
        std::fs::create_dir_all(dir).expect("create screenshot directory");
    }
    // AppExit is handled at the end of the frame, after save_to_disk has run.
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(mode.path.clone()))
        .observe(
            |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_means_no_screenshot() {
        assert!(ScreenshotMode::from_args(args(&[])).is_none());
    }

    #[test]
    fn parses_path_and_frames() {
        let mode = ScreenshotMode::from_args(args(&["--screenshot", "out/a.png", "--frames", "5"]))
            .unwrap();
        assert_eq!(mode.path, PathBuf::from("out/a.png"));
        assert_eq!(mode.frames, 5);
    }
}
