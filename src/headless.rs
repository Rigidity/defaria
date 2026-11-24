//! Headless mode: run the real game with no window, driven by a script.
//!
//! `cargo headless <script>` (an alias for `cargo run -- --headless <script>`)
//! renders every frame into an offscreen image instead of a window, feeds the
//! game the input the script describes, and exits when the script ends. Time
//! advances a fixed 1/60 s per frame, so runs are deterministic.
//!
//! Add `--no-render` to run without a GPU at all: everything works except
//! `screenshot` and `grain`, which are skipped. CI runs every scenario this
//! way.
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
//! | `grain` | Print a texture-noise number for this frame (see below). |
//! | `inspect <Component>` | Print that component on every entity that has it. |
//!
//! Buttons are Bevy [`KeyCode`] names (`KeyW`, `Space`, `ShiftLeft`,
//! `Escape`, …) or `MouseLeft`, `MouseRight`, `MouseMiddle`. Components are
//! reflected type names, short (`FlyCamera`, `Transform`) or full
//! (`defaria::fly_camera::FlyCamera`).
//!
//! Commands between two `wait`s all happen on the same frame. `screenshot`,
//! `grain` and `inspect` see the state at the end of that frame, so put a
//! `wait` between a `screenshot` and input meant to come after it.
//!
//! `grain` is the mean difference between neighbouring pixels (0–255,
//! averaged over RGB) across every pair where neither pixel is the sky
//! ([`ClearColor`]). Lower is smoother. Block edges, face shading and how far
//! apart the tile colours are all count, so only compare it between runs of
//! the same view with the same art, for example before and after a shader or
//! filtering change.
//!
//! A rendered run fails (exits with an error) if a render pipeline failed to
//! compile, such as a shader with an error, since that otherwise only shows
//! up as missing geometry in screenshots. When the script ends, the run waits
//! for pipelines still compiling, but a pipeline that hasn't been requested
//! yet (say, because its textures are still loading on the first frames)
//! can't be checked, so scenarios should run for at least a few frames.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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
        Render, RenderApp, RenderPlugin, RenderSystems,
        render_resource::{CachedPipelineState, PipelineCache, TextureFormat, TextureUsages},
        settings::WgpuSettings,
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
    shader::ShaderCacheError,
    time::TimeUpdateStrategy,
    window::{ExitCondition, PrimaryWindow, WindowResolution},
    winit::WinitPlugin,
};

/// Fixed time step for every headless frame.
const FRAME_TIME: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Size of the offscreen image the camera renders into.
const RESOLUTION: UVec2 = UVec2::new(1280, 720);
/// Give up if captures are still pending or pipelines still compiling this
/// many frames after the script ended, so a broken capture or a shader that
/// never loads can't hang an agent forever.
const END_TIMEOUT_FRAMES: u32 = 600;

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
            share_pipeline_status(app);
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
    Grain,
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
        "grain" => {
            expect(0)?;
            commands.push(Command::Grain);
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
    /// `screenshot` and `grain` commands to handle at the end of this frame.
    captures: Vec<Capture>,
    /// `inspect` commands to handle at the end of this frame.
    inspections: Vec<String>,
    /// Captures requested but not yet delivered by the renderer.
    pending_captures: usize,
    /// Frames since the last command ran, once there are none left.
    frames_after_end: u32,
    rendering: bool,
}

impl ScriptRunner {
    fn finished(&self) -> bool {
        self.commands.is_empty()
            && self.waiting == 0
            && self.captures.is_empty()
            && self.inspections.is_empty()
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
/// The UI draws on the camera that targets the primary window unless one is
/// marked as the default, so mark it, or the HUD wouldn't show.
fn render_to_image(add: On<Add<Camera>>, target: Res<HeadlessTarget>, mut commands: Commands) {
    commands.entity(add.entity).insert((
        RenderTarget::Image(target.0.clone().into()),
        IsDefaultUiCamera,
    ));
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
            Command::Screenshot(path) => runner.captures.push(Capture::Save(path)),
            Command::Grain => runner.captures.push(Capture::Grain),
            Command::Inspect(name) => runner.inspections.push(name),
        }
    }
}

/// A capture of the frame and what to do with it.
#[derive(Debug)]
enum Capture {
    Save(PathBuf),
    Grain,
}

/// Handles this frame's `screenshot`, `grain` and `inspect` commands, after
/// the game's systems have run.
fn observe(world: &mut World) {
    let mut runner = world.resource_mut::<ScriptRunner>();
    let inspections = std::mem::take(&mut runner.inspections);
    let captures = std::mem::take(&mut runner.captures);
    let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
    for name in inspections {
        match inspect(world, &name) {
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
        }
    }
    capture(world, captures, frame);
}

/// Captures this frame once for all of its `screenshot` and `grain`
/// commands: Bevy takes only one screenshot per render target per frame.
fn capture(world: &mut World, captures: Vec<Capture>, frame: u32) {
    if captures.is_empty() {
        return;
    }
    let Some(target) = world.get_resource::<HeadlessTarget>() else {
        for capture in captures {
            match capture {
                Capture::Save(path) => println!(
                    "[frame {frame}] skipped screenshot {} (rendering is off)",
                    path.display()
                ),
                Capture::Grain => println!("[frame {frame}] skipped grain (rendering is off)"),
            }
        }
        return;
    };
    let image = target.0.clone();
    world.resource_mut::<ScriptRunner>().pending_captures += 1;
    let mut entity = world.spawn(Screenshot::image(image));
    entity.observe(
        |_: On<ScreenshotCaptured>, mut runner: ResMut<ScriptRunner>| {
            runner.pending_captures -= 1;
        },
    );
    for capture in captures {
        match capture {
            Capture::Save(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).expect("create screenshot directory");
                }
                println!("[frame {frame}] screenshot {}", path.display());
                entity.observe(save_to_disk(path));
            }
            Capture::Grain => {
                entity.observe(print_grain(frame));
            }
        }
    }
}

/// An observer that prints the captured frame's [`grain`].
fn print_grain(frame: u32) -> impl FnMut(On<ScreenshotCaptured>, Res<ClearColor>) {
    move |captured, sky| match grain(&captured.image, sky.0) {
        Some((value, pairs)) => {
            println!("[frame {frame}] grain {value:.2} over {pairs} pixel pairs")
        }
        None => println!("[frame {frame}] grain: no non-sky pixels"),
    }
}

/// How grainy an image is: the mean absolute difference between each pixel
/// and its right and lower neighbours, in 0–255 units averaged over RGB,
/// counting only pairs where neither pixel is within a few units of `sky`.
/// Returns the value and the number of pairs, or `None` if there are none.
fn grain(image: &Image, sky: Color) -> Option<(f32, usize)> {
    let (width, height) = (image.width(), image.height());
    let to_rgb = |color: Color| {
        let [r, g, b, _] = color.to_srgba().to_u8_array();
        [r, g, b]
    };
    let sky = to_rgb(sky);
    let pixels: Vec<Option<[u8; 3]>> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let rgb = to_rgb(image.get_color_at(x, y).ok()?);
            let is_sky = rgb.iter().zip(sky).all(|(&c, s)| c.abs_diff(s) <= 6);
            (!is_sky).then_some(rgb)
        })
        .collect();
    let at = |x: u32, y: u32| pixels[(y * width + x) as usize];
    let (mut total, mut pairs) = (0u64, 0usize);
    for y in 0..height {
        for x in 0..width {
            let Some(here) = at(x, y) else { continue };
            let neighbours = [
                (x + 1 < width).then(|| at(x + 1, y)).flatten(),
                (y + 1 < height).then(|| at(x, y + 1)).flatten(),
            ];
            for there in neighbours.into_iter().flatten() {
                total += here
                    .iter()
                    .zip(there)
                    .map(|(&a, b)| a.abs_diff(b) as u64)
                    .sum::<u64>();
                pairs += 1;
            }
        }
    }
    (pairs > 0).then(|| (total as f32 / pairs as f32 / 3.0, pairs))
}

/// How many render pipelines have failed and how many are still pending,
/// written in the render world and read in the main world when the script
/// ends. Both counts live in one atomic so a reader never mixes two frames.
#[derive(Resource, Clone, Default)]
struct PipelineStatus(Arc<AtomicU64>);

impl PipelineStatus {
    fn store(&self, failed: u32, pending: u32) {
        self.0.store(
            (u64::from(failed) << 32) | u64::from(pending),
            Ordering::Relaxed,
        );
    }

    /// `(failed, pending)`.
    fn load(&self) -> (u32, u32) {
        let packed = self.0.load(Ordering::Relaxed);
        ((packed >> 32) as u32, packed as u32)
    }
}

/// Shares one [`PipelineStatus`] between the main world and the render
/// world, and keeps it up to date from the render world.
fn share_pipeline_status(app: &mut App) {
    let status = PipelineStatus::default();
    app.insert_resource(status.clone());
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(status).add_systems(
            Render,
            // After `RenderSystems::Render`, where the pipeline queue is
            // processed, so this frame's results are in.
            record_pipeline_status.in_set(RenderSystems::Cleanup),
        );
    }
}

fn record_pipeline_status(cache: Res<PipelineCache>, status: Res<PipelineStatus>) {
    let (mut failed, mut pending) = (0, 0);
    for pipeline in cache.pipelines() {
        match pipeline_outcome(&pipeline.state) {
            PipelineOutcome::Ready => {}
            PipelineOutcome::Failed => failed += 1,
            PipelineOutcome::Pending => pending += 1,
        }
    }
    status.store(failed, pending);
}

#[derive(Debug, PartialEq)]
enum PipelineOutcome {
    Ready,
    /// Failed in a way Bevy won't retry, such as a shader that doesn't
    /// compile.
    Failed,
    /// Queued, compiling, or waiting to retry a shader or import that hasn't
    /// loaded yet.
    Pending,
}

fn pipeline_outcome(state: &CachedPipelineState) -> PipelineOutcome {
    match state {
        CachedPipelineState::Ok(_) => PipelineOutcome::Ready,
        CachedPipelineState::Queued | CachedPipelineState::Creating(_) => PipelineOutcome::Pending,
        CachedPipelineState::Err(error) => match error {
            ShaderCacheError::ShaderNotLoaded(_)
            | ShaderCacheError::ShaderImportNotYetAvailable => PipelineOutcome::Pending,
            ShaderCacheError::ProcessShaderError(_) | ShaderCacheError::CreateShaderModule(_) => {
                PipelineOutcome::Failed
            }
        },
    }
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

fn exit_when_done(
    mut runner: ResMut<ScriptRunner>,
    pipelines: Option<Res<PipelineStatus>>,
    mut exit: MessageWriter<AppExit>,
) {
    if !runner.finished() {
        return;
    }
    let (failed, pending) = pipelines.map_or((0, 0), |status| status.load());
    let screenshots_pending = runner.rendering && runner.pending_captures > 0;
    // Wait for screenshots to be saved, so they're there to look at, and for
    // pipelines to finish compiling, so a broken shader can't slip through.
    if screenshots_pending || pending > 0 {
        runner.frames_after_end += 1;
        if runner.frames_after_end > END_TIMEOUT_FRAMES {
            if screenshots_pending {
                error!("screenshots were never captured; giving up");
            } else {
                error!("{pending} render pipeline(s) never finished compiling; giving up");
            }
            exit.write(AppExit::error());
        }
        return;
    }
    if failed > 0 {
        error!("{failed} render pipeline(s) failed to compile; see the errors above");
        exit.write(AppExit::error());
    } else {
        exit.write(AppExit::Success);
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
             grain\n\
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
                Command::Grain,
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
        assert!(Script::parse("grain 3").is_err());
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

    /// An image whose pixel (x, y) is `paint(x, y)`.
    fn image(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let data = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [r, g, b] = paint(x, y);
                [r, g, b, 255]
            })
            .collect();
        Image::new(
            bevy::render::render_resource::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            bevy::render::render_resource::TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::all(),
        )
    }

    #[test]
    fn grain_measures_neighbour_differences_away_from_the_sky() {
        let sky = Color::srgb_u8(100, 150, 200);
        let flat = image(8, 8, |_, _| [40, 90, 40]);
        assert_eq!(grain(&flat, sky), Some((0.0, 2 * 7 * 8)));

        let checker = image(
            8,
            8,
            |x, y| if (x + y) % 2 == 0 { [0; 3] } else { [255; 3] },
        );
        assert_eq!(grain(&checker, sky).unwrap().0, 255.0);

        // Edges against the sky don't count: black ground under a sky band.
        let horizon = image(8, 8, |_, y| if y < 4 { [100, 150, 200] } else { [0; 3] });
        assert_eq!(grain(&horizon, sky), Some((0.0, 2 * 7 * 4 - 4)));

        let all_sky = image(4, 4, |_, _| [101, 149, 200]);
        assert_eq!(grain(&all_sky, sky), None);
    }

    /// A headless app with no GPU running `script`, with `status` standing
    /// in for what the render world would report.
    fn app_with_pipelines(script: &str, status: PipelineStatus) -> App {
        let mut app = App::new();
        app.add_plugins((
            default_plugins(false).disable::<LogPlugin>(),
            HeadlessPlugin {
                script: Script::parse(script).unwrap(),
                rendering: false,
            },
        ))
        .insert_resource(status);
        app.finish();
        app.cleanup();
        app
    }

    fn status(failed: u32, pending: u32) -> PipelineStatus {
        let status = PipelineStatus::default();
        status.store(failed, pending);
        status
    }

    fn run_until_exit(app: &mut App, frames: usize) -> Option<AppExit> {
        (0..frames).find_map(|_| {
            app.update();
            app.should_exit()
        })
    }

    /// A pipeline that failed to compile (say, a broken shader) fails the
    /// run. Simulated, since tests have no GPU to compile shaders on.
    #[test]
    fn failed_pipelines_fail_the_run() {
        let mut app = app_with_pipelines("wait 1", status(1, 0));
        assert_eq!(run_until_exit(&mut app, 10), Some(AppExit::error()));
    }

    /// The run doesn't end while pipelines are still compiling, so a short
    /// script can't finish before a broken shader is found.
    #[test]
    fn exit_waits_for_pipelines_to_finish_compiling() {
        let status = status(0, 1);
        let mut app = app_with_pipelines("wait 1", status.clone());
        assert_eq!(run_until_exit(&mut app, 20), None);
        status.store(1, 0);
        assert_eq!(run_until_exit(&mut app, 5), Some(AppExit::error()));
    }

    /// A pipeline that never finishes compiling (a shader that never loads)
    /// ends the run with an error instead of hanging it.
    #[test]
    fn gives_up_on_pipelines_that_never_finish() {
        let mut app = app_with_pipelines("wait 1", status(0, 1));
        let frames = END_TIMEOUT_FRAMES as usize + 10;
        assert_eq!(run_until_exit(&mut app, frames), Some(AppExit::error()));
    }

    #[test]
    fn pipeline_status_round_trips_both_counts() {
        let status = status(3, 70_000);
        assert_eq!(status.load(), (3, 70_000));
    }

    /// Shader errors Bevy retries are pending, not failed, and ones it won't
    /// retry are failures.
    #[test]
    fn sorts_pipeline_states_into_outcomes() {
        use CachedPipelineState::{Err, Queued};
        let cases = [
            (Queued, PipelineOutcome::Pending),
            (
                Err(ShaderCacheError::ShaderNotLoaded(default())),
                PipelineOutcome::Pending,
            ),
            (
                Err(ShaderCacheError::ShaderImportNotYetAvailable),
                PipelineOutcome::Pending,
            ),
            (
                Err(ShaderCacheError::ProcessShaderError(String::new())),
                PipelineOutcome::Failed,
            ),
            (
                Err(ShaderCacheError::CreateShaderModule(String::new())),
                PipelineOutcome::Failed,
            ),
        ];
        for (state, outcome) in cases {
            assert_eq!(pipeline_outcome(&state), outcome, "{state:?}");
        }
    }

    /// The main world reads the very counters the render world writes, and
    /// the render world updates them every frame.
    #[test]
    fn pipeline_status_is_shared_with_the_render_world() {
        let mut app = App::new();
        app.insert_sub_app(RenderApp, SubApp::new());
        share_pipeline_status(&mut app);
        let main = app.world().resource::<PipelineStatus>().clone();
        let render_app = app.sub_app(RenderApp);
        let render = render_app.world().resource::<PipelineStatus>();
        assert!(Arc::ptr_eq(&main.0, &render.0));
        let schedule = render_app
            .world()
            .resource::<Schedules>()
            .get(Render)
            .expect("no Render schedule");
        assert!(
            schedule
                .graph()
                .systems
                .iter()
                .any(|(_, system, _)| system.name().to_string().contains("record_pipeline_status")),
            "record_pipeline_status isn't in the Render schedule"
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
