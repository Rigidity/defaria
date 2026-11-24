//! Headless mode: run the real game with no window, driven by a script.
//!
//! `cargo headless <script>` (an alias for `cargo run -- --headless <script>`)
//! renders every frame into an offscreen image instead of a window, feeds the
//! game the input the script describes, and exits when the script ends. Time
//! advances a fixed 1/60 s per frame, so runs are deterministic.
//!
//! Add `--no-render` to run without a GPU at all: everything works except
//! screenshots, which are skipped. CI runs every scenario this way.
//!
//! # Script format
//!
//! One command per line. Blank lines and `#` comments are ignored.
//!
//! | Command | Effect |
//! |---|---|
//! | `wait <frames>` | Let that many frames run before the next command. |
//! | `press <button>` / `release <button>` | Press or release a key or mouse button. |
//! | `hold <button> <frames>` | `press`, `wait <frames>`, `release`. |
//! | `mouse <dx> <dy>` | Move the mouse by that many pixels this frame. |
//! | `screenshot <path.png>` | Save this frame as a PNG. |
//! | `inspect <Component>` | Print that component on every entity that has it. |
//!
//! Buttons are Bevy [`KeyCode`] names (`KeyW`, `Space`, `ShiftLeft`,
//! `Escape`, …) or `MouseLeft`, `MouseRight`, `MouseMiddle`. Components are
//! reflected type names, short (`FlyCamera`, `Transform`) or full
//! (`defaria::fly_camera::FlyCamera`).
//!
//! Commands between two `wait`s all happen on the same frame. `screenshot`
//! and `inspect` see the state at the end of that frame.

use std::{collections::VecDeque, path::PathBuf, time::Duration};

use bevy::{
    app::PluginGroupBuilder,
    camera::RenderTarget,
    input::{
        ButtonState, InputSystems,
        keyboard::{Key, KeyboardInput, NativeKey},
        mouse::{MouseButtonInput, MouseMotion},
    },
    prelude::*,
    reflect::enums::{DynamicEnum, DynamicVariant},
    render::{
        RenderPlugin,
        render_resource::{TextureFormat, TextureUsages},
        settings::WgpuSettings,
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
    time::TimeUpdateStrategy,
    window::{ExitCondition, PrimaryWindow, WindowResolution},
    winit::WinitPlugin,
};

/// Fixed time step for every headless frame.
const FRAME_TIME: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Size of the offscreen image the camera renders into.
const RESOLUTION: UVec2 = UVec2::new(1280, 720);
/// Give up if screenshots are still pending this many frames after the script
/// ended, so a broken capture can't hang an agent forever.
const SCREENSHOT_TIMEOUT_FRAMES: u32 = 600;

/// [`DefaultPlugins`] set up to run without a window. With `rendering` off
/// there's no GPU at all (for CI); screenshots are then skipped.
pub fn default_plugins(rendering: bool) -> PluginGroupBuilder {
    let mut wgpu = WgpuSettings::default();
    if !rendering {
        wgpu.backends = None;
    }
    DefaultPlugins
        .build()
        .disable::<WinitPlugin>()
        .set(WindowPlugin {
            // A primary window entity still exists so cursor grabbing and
            // window-targeted input work, but nothing creates an OS window.
            primary_window: Some(Window {
                resolution: WindowResolution::new(RESOLUTION.x, RESOLUTION.y),
                ..default()
            }),
            exit_condition: ExitCondition::DontExit,
            close_when_requested: false,
            ..default()
        })
        .set(RenderPlugin {
            render_creation: wgpu.into(),
            // Compile shaders up front so early frames aren't blank.
            synchronous_pipeline_compilation: true,
            ..default()
        })
}

pub struct HeadlessPlugin {
    pub script: Script,
    /// Whether a GPU renderer exists. Must match [`default_plugins`].
    pub rendering: bool,
}

impl Plugin for HeadlessPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(FRAME_TIME))
            .insert_resource(ScriptRunner {
                commands: self.script.commands.clone().into(),
                rendering: self.rendering,
                ..default()
            })
            .add_systems(PreUpdate, run_script.before(InputSystems))
            .add_systems(Last, (observe, exit_when_done).chain());
        if self.rendering {
            app.add_systems(PreStartup, create_render_target)
                .add_observer(render_to_image);
        }
    }
}

/// A parsed script. See the module docs for the format.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Script {
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Wait(u32),
    Press(Button),
    Release(Button),
    MoveMouse(Vec2),
    Screenshot(PathBuf),
    Inspect(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Button {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl Script {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("can't read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut commands = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            parse_line(line, &mut commands)
                .map_err(|error| format!("line {}: {error}", index + 1))?;
        }
        Ok(Self { commands })
    }
}

fn parse_line(line: &str, commands: &mut Vec<Command>) -> Result<(), String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let args = &words[1..];
    let expect = |count: usize| {
        if args.len() == count {
            Ok(())
        } else {
            Err(format!("`{}` takes {count} argument(s): {line}", words[0]))
        }
    };
    match words[0] {
        "wait" => {
            expect(1)?;
            commands.push(Command::Wait(parse_number(args[0])?));
        }
        "press" => {
            expect(1)?;
            commands.push(Command::Press(parse_button(args[0])?));
        }
        "release" => {
            expect(1)?;
            commands.push(Command::Release(parse_button(args[0])?));
        }
        "hold" => {
            expect(2)?;
            let button = parse_button(args[0])?;
            commands.extend([
                Command::Press(button),
                Command::Wait(parse_number(args[1])?),
                Command::Release(button),
            ]);
        }
        "mouse" => {
            expect(2)?;
            commands.push(Command::MoveMouse(Vec2::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
            )));
        }
        "screenshot" => {
            expect(1)?;
            commands.push(Command::Screenshot(PathBuf::from(args[0])));
        }
        "inspect" => {
            expect(1)?;
            commands.push(Command::Inspect(args[0].to_string()));
        }
        other => return Err(format!("unknown command `{other}`")),
    }
    Ok(())
}

fn parse_number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("`{text}` isn't a valid number"))
}

fn parse_button(name: &str) -> Result<Button, String> {
    match name {
        "MouseLeft" => return Ok(Button::Mouse(MouseButton::Left)),
        "MouseRight" => return Ok(Button::Mouse(MouseButton::Right)),
        "MouseMiddle" => return Ok(Button::Mouse(MouseButton::Middle)),
        _ => {}
    }
    // Any unit variant of KeyCode, by name, via reflection.
    KeyCode::from_reflect(&DynamicEnum::new(name, DynamicVariant::Unit))
        .map(Button::Key)
        .ok_or_else(|| {
            format!("unknown button `{name}` (use KeyCode names like `KeyW`, or `MouseLeft`)")
        })
}

#[derive(Resource, Default)]
struct ScriptRunner {
    commands: VecDeque<Command>,
    /// Frames left before the next command runs.
    waiting: u32,
    /// `screenshot` and `inspect` commands to handle at the end of this frame.
    observations: Vec<Command>,
    pending_screenshots: usize,
    /// Frames since the last command ran, once there are none left.
    frames_after_end: u32,
    rendering: bool,
}

impl ScriptRunner {
    fn finished(&self) -> bool {
        self.commands.is_empty() && self.waiting == 0 && self.observations.is_empty()
    }
}

/// The offscreen image the camera renders into.
#[derive(Resource)]
struct HeadlessTarget(Handle<Image>);

fn create_render_target(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_target_texture(
        RESOLUTION.x,
        RESOLUTION.y,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    // Screenshots copy out of the texture.
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    commands.insert_resource(HeadlessTarget(images.add(image)));
}

/// Points every camera at the offscreen image, since there's no window.
fn render_to_image(add: On<Add<Camera>>, target: Res<HeadlessTarget>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert(RenderTarget::Image(target.0.clone().into()));
}

/// Runs this frame's commands, up to the next `wait`. Runs before Bevy's input
/// systems so injected input is seen this frame.
fn run_script(
    mut runner: ResMut<ScriptRunner>,
    window: Option<Single<Entity, With<PrimaryWindow>>>,
    mut keys: MessageWriter<KeyboardInput>,
    mut mouse_buttons: MessageWriter<MouseButtonInput>,
    mut mouse_motion: MessageWriter<MouseMotion>,
) {
    if runner.waiting > 0 {
        runner.waiting -= 1;
        if runner.waiting > 0 {
            return;
        }
    }
    let window = window.map_or(Entity::PLACEHOLDER, |window| *window);
    while let Some(command) = runner.commands.pop_front() {
        match command {
            Command::Wait(frames) => {
                runner.waiting = frames;
                if frames > 0 {
                    break;
                }
            }
            Command::Press(button) | Command::Release(button) => {
                let state = if matches!(command, Command::Press(_)) {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                match button {
                    Button::Key(key_code) => {
                        keys.write(KeyboardInput {
                            key_code,
                            logical_key: Key::Unidentified(NativeKey::Unidentified),
                            state,
                            text: None,
                            repeat: false,
                            window,
                        });
                    }
                    Button::Mouse(button) => {
                        mouse_buttons.write(MouseButtonInput {
                            button,
                            state,
                            window,
                        });
                    }
                }
            }
            Command::MoveMouse(delta) => {
                mouse_motion.write(MouseMotion { delta });
            }
            Command::Screenshot(_) | Command::Inspect(_) => runner.observations.push(command),
        }
    }
}

/// Handles this frame's `screenshot` and `inspect` commands, after the game's
/// systems have run.
fn observe(world: &mut World) {
    let observations = std::mem::take(&mut world.resource_mut::<ScriptRunner>().observations);
    let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
    for observation in observations {
        match observation {
            Command::Inspect(name) => match inspect(world, &name) {
                Ok(lines) if lines.is_empty() => println!("[frame {frame}] no entity has {name}"),
                Ok(lines) => {
                    for line in lines {
                        println!("[frame {frame}] {line}");
                    }
                }
                Err(error) => {
                    error!("inspect: {error}");
                    world.write_message(AppExit::error());
                }
            },
            Command::Screenshot(path) => screenshot(world, path, frame),
            _ => unreachable!("only screenshot and inspect are deferred"),
        }
    }
}

fn screenshot(world: &mut World, path: PathBuf, frame: u32) {
    let Some(target) = world.get_resource::<HeadlessTarget>() else {
        println!(
            "[frame {frame}] skipped screenshot {} (rendering is off)",
            path.display()
        );
        return;
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create screenshot directory");
    }
    let image = target.0.clone();
    world.resource_mut::<ScriptRunner>().pending_screenshots += 1;
    println!("[frame {frame}] screenshot {}", path.display());
    world
        .spawn(Screenshot::image(image))
        .observe(save_to_disk(path))
        .observe(
            |_: On<ScreenshotCaptured>, mut runner: ResMut<ScriptRunner>| {
                runner.pending_screenshots -= 1;
            },
        );
}

/// Formats `name` (a reflected component type) on every entity that has it.
fn inspect(world: &mut World, name: &str) -> Result<Vec<String>, String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry
        .get_with_type_path(name)
        .or_else(|| registry.get_with_short_type_path(name))
        .ok_or_else(|| {
            format!("no reflected type named `{name}` (it may be ambiguous; try the full path)")
        })?;
    let component = registration
        .data::<ReflectComponent>()
        .ok_or_else(|| format!("`{name}` isn't a component with #[reflect(Component)]"))?;

    let mut entities = world.query::<EntityRef>();
    Ok(entities
        .iter(world)
        .filter_map(|entity| {
            let value = component.reflect(entity)?;
            let label = entity
                .get::<Name>()
                .map_or_else(String::new, |name| format!(" \"{name}\""));
            Some(format!(
                "{}{label}: {:?}",
                entity.id(),
                value.as_partial_reflect()
            ))
        })
        .collect())
}

fn exit_when_done(mut runner: ResMut<ScriptRunner>, mut exit: MessageWriter<AppExit>) {
    if !runner.finished() {
        return;
    }
    if runner.pending_screenshots == 0 || !runner.rendering {
        exit.write(AppExit::Success);
        return;
    }
    runner.frames_after_end += 1;
    if runner.frames_after_end > SCREENSHOT_TIMEOUT_FRAMES {
        error!("screenshots were never captured; giving up");
        exit.write(AppExit::error());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GamePlugin, fly_camera::FlyCamera};
    use bevy::log::LogPlugin;

    #[test]
    fn parses_every_command() {
        let script = Script::parse(
            "# comment\n\
             wait 2\n\
             press KeyW # trailing comment\n\
             release Space\n\
             hold MouseLeft 3\n\
             mouse 10 -5.5\n\
             \n\
             screenshot target/a.png\n\
             inspect Transform\n",
        )
        .unwrap();
        assert_eq!(
            script.commands,
            vec![
                Command::Wait(2),
                Command::Press(Button::Key(KeyCode::KeyW)),
                Command::Release(Button::Key(KeyCode::Space)),
                Command::Press(Button::Mouse(MouseButton::Left)),
                Command::Wait(3),
                Command::Release(Button::Mouse(MouseButton::Left)),
                Command::MoveMouse(Vec2::new(10.0, -5.5)),
                Command::Screenshot(PathBuf::from("target/a.png")),
                Command::Inspect("Transform".into()),
            ]
        );
    }

    #[test]
    fn parse_errors_name_the_line() {
        let error = Script::parse("wait 1\npress KeyQQ").unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
        assert!(Script::parse("jump").is_err());
        assert!(Script::parse("wait").is_err());
        assert!(Script::parse("wait soon").is_err());
    }

    /// End to end: the whole game, no GPU, driven by a script.
    #[test]
    fn script_drives_the_fly_camera() {
        let script = Script::parse(
            "wait 1\n\
             hold MouseLeft 1  # lock the cursor\n\
             hold KeyW 30\n\
             mouse 100 0\n\
             wait 1\n\
             inspect FlyCamera\n\
             screenshot target/never-written.png\n",
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugins((
            default_plugins(false).disable::<LogPlugin>(),
            GamePlugin,
            HeadlessPlugin {
                script,
                rendering: false,
            },
        ));
        app.finish();
        app.cleanup();

        // First frame: Startup spawns the camera; the script is still waiting.
        app.update();
        let start = camera(&mut app);

        let mut frames = 1;
        let exit = loop {
            app.update();
            frames += 1;
            if let Some(exit) = app.should_exit() {
                break exit;
            }
            assert!(frames < 100, "script never finished");
        };
        assert_eq!(exit, AppExit::Success);

        let (end_camera, end_transform) = camera(&mut app);
        // 30 frames of W at 1/60 s each, at the camera's speed, along where
        // it faced before turning.
        let forward = Quat::from_rotation_y(start.0.yaw) * Vec3::NEG_Z;
        let expected = start.1.translation + forward * start.0.speed * 0.5;
        assert!(
            end_transform.translation.abs_diff_eq(expected, 1e-3),
            "{} vs {expected}",
            end_transform.translation
        );
        // Moving the mouse 100px right turns the camera right.
        let turned = end_camera.yaw - start.0.yaw;
        assert!(
            (turned + 100.0 * start.0.sensitivity).abs() < 1e-5,
            "{turned}"
        );
    }

    fn camera(app: &mut App) -> (FlyCamera, Transform) {
        let (camera, transform) = app
            .world_mut()
            .query::<(&FlyCamera, &Transform)>()
            .single(app.world())
            .unwrap();
        (camera.clone(), *transform)
    }
}
