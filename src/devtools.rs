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

/// Command-line flags.
#[derive(Debug, Default)]
pub struct Cli {
    /// `--screenshot <path> [--frames N]`: windowed screenshot, then exit.
    pub screenshot: Option<ScreenshotMode>,
    /// `--headless <script>`: no window; run the script, then exit.
    pub headless: Option<PathBuf>,
    /// `--no-render` (with `--headless`): no GPU at all; screenshots are
    /// skipped. For CI, which has no GPU.
    pub no_render: bool,
    /// `--world <folder>`: the world to load and save. Without it, the game
    /// plays the default world in the data folder, and headless runs play
    /// a world kept in memory.
    pub world: Option<PathBuf>,
}

impl Cli {
    /// Parses the flags above. Panics on unknown or malformed arguments.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut path = None;
        let mut frames = 60;
        let mut headless = None;
        let mut no_render = false;
        let mut world = None;
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
                "--headless" => {
                    headless = Some(PathBuf::from(
                        args.next().expect("--headless needs a script path"),
                    ));
                }
                "--no-render" => no_render = true,
                "--world" => {
                    world = Some(PathBuf::from(args.next().expect("--world needs a folder")));
                }
                other => panic!("unknown argument: {other}"),
            }
        }
        assert!(
            path.is_none() || headless.is_none(),
            "--headless takes screenshots from its script; don't combine it with --screenshot"
        );
        assert!(
            !no_render || headless.is_some(),
            "--no-render only works with --headless"
        );
        Self {
            screenshot: path.map(|path| ScreenshotMode { path, frames }),
            headless,
            no_render,
            world,
        }
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
    fn no_args_means_windowed_play() {
        let cli = Cli::from_args(args(&[]));
        assert!(cli.screenshot.is_none());
        assert!(cli.headless.is_none());
    }

    #[test]
    fn parses_path_and_frames() {
        let mode = Cli::from_args(args(&["--screenshot", "out/a.png", "--frames", "5"]))
            .screenshot
            .unwrap();
        assert_eq!(mode.path, PathBuf::from("out/a.png"));
        assert_eq!(mode.frames, 5);
    }

    #[test]
    fn parses_headless_script() {
        let cli = Cli::from_args(args(&["--headless", "scenarios/fly.txt"]));
        assert_eq!(cli.headless, Some(PathBuf::from("scenarios/fly.txt")));
        assert!(!cli.no_render);

        let cli = Cli::from_args(args(&["--headless", "scenarios/fly.txt", "--no-render"]));
        assert!(cli.no_render);
    }

    #[test]
    fn parses_world_folder() {
        let cli = Cli::from_args(args(&["--headless", "a.txt", "--world", "saves/test"]));
        assert_eq!(cli.world, Some(PathBuf::from("saves/test")));
        assert_eq!(Cli::from_args(args(&[])).world, None);
    }

    #[test]
    #[should_panic(expected = "--no-render only works with --headless")]
    fn no_render_needs_headless() {
        Cli::from_args(args(&["--no-render"]));
    }
}
