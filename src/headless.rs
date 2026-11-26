//! Headless mode: run the real game with no window, driven by a script.
//!
//! `cargo headless <script>` (an alias for `cargo run -- --headless <script>`)
//! renders every frame into an offscreen image instead of a window, feeds the
//! game the input the script describes, and exits when the script ends. Time
//! advances a fixed 1/60 s per frame, so runs are deterministic.
//!
//! Add `--no-render` to run without a GPU at all: everything works except
//! `screenshot` and `grain`, which are skipped. `--radius <chunks>` loads
//! a smaller world than the default, so the script starts sooner. CI runs
//! every scenario with both (`--no-render --radius 4`).
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
//! | `cursor <x> <y>` | Put the cursor at that pixel of the frame (0 0 is the top left), for pointing at UI. |
//! | `scroll <lines>` | Turn the mouse wheel that many notches this frame: positive away from you (up), negative toward you. |
//! | `screenshot <path.png>` | Save this frame as a PNG. |
//! | `grain` | Print a texture-noise number for this frame (see below). |
//! | `inspect <Type>` | Print that component on every entity that has it, or that resource. |
//! | `fill <x0> <y0> <z0> <x1> <y1> <z1> <block>` | Set every voxel in the box between the two corners (both included) to a block, by id (`defaria:stone`, or `stone` for the base game's; `air` clears), in its default state or with some properties set as in a saved name (`log[axis=x,wood=defaria:birch]`). Like any edit, it's followed by the engine setting connection properties, so `pipe` joins the pipes next to it. |
//! | `pour <x> <y> <z> <fluid> <amount>` | Put `amount` blocks of a fluid (`water`, `lava` or any block with `fluid` set, named as for `fill`) in one voxel, from a sliver to `1`, a full block, replacing what's there. It flows from there. |
//! | `hotbar <slot> <item>` | Put an item in a hotbar slot (1–9) and make it the one in hand: an item id (`defaria:wrench`, or `wrench` for the base game's), in its default state or with some properties set as for `fill` (`wrench[head=defaria:copper]`). It doesn't put any in the inventory. |
//! | `network <x> <y> <z>` | Print the network the conduit at that voxel is part of: its id, kind and solver, how many conduits and chunks it has, its capacity, whether it's running or frozen (a face joined into a chunk that isn't loaded) and the machine faces (ports) it's joined to, each with its role (`produce`, `store`, `input`, ...). On an energy network, also what its storage holds and could, what producers gave and consumers took in the last second, and its throughput (its weakest wire's); on a fluid network, what it holds. Or `none`. |
//! | `put <x> <y> <z> <name> <amount> <what>` | Put something in the machine at that voxel: `amount` of an item (named as blocks are for `fill`, `stone[ore=defaria:iron]`) into its slot group `name`, or `amount` blocks of a fluid (`lava`) into its tank `name`, as a tube or a pipe would. The run fails if there's no machine there, it has no such slot group or tank, or not all of it fits. |
//! | `fluids` | Print how much of each fluid there is in the loaded world, fluid networks' pipes and machines' tanks, in thousandths of a block (with where it is, for a fluid that's in pipes or tanks too), and how many voxels of it are active (see `fluid`). |
//! | `camera <x> <y> <z> <yaw> <pitch>` | Put the camera (the player's eyes) at a position, turned `yaw` degrees left of north (−Z) and tilted `pitch` degrees up. The player stops there, flying, so the view stays put until input moves it. |
//! | `timing` | Print the mean and worst wall-clock frame time since the last `timing` (or the start), and if any `fill` ran since, how many voxels they changed (not ones already that block, nor ones in unloaded chunks) and how long the fills took (setting them, not relighting or remeshing, which come in the frames after). |
//! | `type <text>` | Type the text (the rest of the line), a key press and release per character, into whatever has the keyboard. |
//! | `random-ticks <n>` | Pick `n` voxels in each loaded chunk for random ticks each tick (1 by default), so blocks that grow or spread on random ticks (see `script::random_tick`) do it quickly enough to watch. |
//! | `settle` | Wait until the world around the camera has loaded and been meshed and the fog has opened up to the draw distance, and print how many frames and seconds that took. |
//! | `memory` | Print what the loaded chunks hold, counted from their data rather than the process's memory, so it's the same every run: how many chunks, how many are air or one other block throughout and how many have one light level throughout, the bytes of block states, light and partial fluid, how much of that (in KiB) is in chunks that are one value throughout but still kept a value per voxel, and how many such chunks there are (0 once compaction has caught up), and the chunk meshes and the bytes of vertices and indices they keep in the main world. |
//! | `lookups <x0> <y0> <z0> <x1> <y1> <z1>` | Read every voxel in the box between the two corners twice, timing each: once by world position as most systems do ([`Voxels::get`], a hash and a query per voxel), then a chunk at a time, and print both. A box of more than 16,777,216 voxels (256 a side) fails the run. |
//!
//! The mouse commands (`cursor`, mouse buttons, `scroll`) also drive
//! `bevy_picking`'s mouse pointer, aimed at the image the frame renders
//! into (the primary window without rendering), so UI responds to them as
//! it does to a real mouse: hovering, clicking and dragging nodes.
//!
//! Buttons are Bevy [`KeyCode`] names (`KeyW`, `Space`, `ShiftLeft`,
//! `Escape`, …) or `MouseLeft`, `MouseRight`, `MouseMiddle`. Components are
//! reflected type names, short (`FlyCamera`, `Transform`) or full
//! (`defaria::fly_camera::FlyCamera`).
//!
//! The world loads around the camera in the background, so the script
//! starts only once the world around the starting position has loaded and
//! been meshed, and the fog has opened up (as if it began with `settle`). After a `camera` jump to
//! somewhere that isn't loaded yet, `settle` waits for it the same way. A
//! run fails if the world doesn't settle within [`SETTLE_TIMEOUT_FRAMES`].
//! Headless runs play a world kept in memory, so every run starts from the
//! same world; `--world <folder>` plays (and saves) one on disk instead.
//!
//! `fill` and `camera` set up exact situations (a sealed room, a lamp, a
//! view) without steering there with input. Edits go through the same path
//! as the player's, so they relight and remesh on the frame they're made.
//! `timing` is for comparing performance between runs on the same machine;
//! put a `timing` just before the stretch to measure, to leave startup out.
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
    time::{Duration, Instant},
};

use bevy::{
    app::PluginGroupBuilder,
    camera::{NormalizedRenderTarget, RenderTarget},
    input::{
        ButtonState, InputSystems,
        keyboard::{Key, KeyboardInput, NativeKey},
        mouse::{MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel},
        touch::TouchPhase,
    },
    picking::{
        PickingSystems,
        input::PointerInputSettings,
        pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput},
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
    window::{CursorMoved, ExitCondition, PrimaryWindow, WindowRef, WindowResolution},
    winit::WinitPlugin,
};

use crate::{
    block_entity::BlockEntities,
    content::{BlockState, Content, FULL_FLUID, ItemState, StateName},
    fluid::FluidStats,
    fog::ChunkFog,
    hotbar::{ActiveSlot, Hotbar},
    machine::{Slots, Tanks},
    physics::Body,
    player::{EYE_HEIGHT, LocalPlayer, Look, MAX_PITCH, Player, put_eyes},
    script::random_tick::RandomTicks,
    streaming::StreamingStatus,
    view_bob::ViewBob,
    world::{Chunk, Voxels},
};

mod memory;

/// Fixed time step for every headless frame.
pub(crate) const FRAME_TIME: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Size of the offscreen image the camera renders into.
const RESOLUTION: UVec2 = UVec2::new(1280, 720);
/// Give up if captures are still pending or pipelines still compiling this
/// many frames after the script ended, so a broken capture or a shader that
/// never loads can't hang an agent forever.
const END_TIMEOUT_FRAMES: u32 = 600;
/// Give up if the world hasn't finished loading this many frames into a
/// `settle` (or the start). Only a guard against a hang: loading is on
/// other threads, so how many frames it takes depends on the machine. The
/// mountains at the full render distance take about 1,900 here, and CI's
/// unoptimized build takes two to four times as many.
pub const SETTLE_TIMEOUT_FRAMES: u32 = 12_000;

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
                settling: Some(0),
                settle_timeout: SETTLE_TIMEOUT_FRAMES,
                rendering: self.rendering,
                ..default()
            })
            // There's no OS window for Bevy's mouse picking input to read:
            // the script drives the mouse pointer itself.
            .insert_resource(PointerInputSettings {
                is_mouse_enabled: false,
                ..default()
            })
            .add_systems(
                PreUpdate,
                (run_script, apply_world_commands)
                    .chain()
                    .before(InputSystems)
                    .before(PickingSystems::ProcessInput),
            )
            .add_systems(
                Last,
                (record_frame_time, observe, exit_when_done)
                    .chain()
                    .after(crate::world::ChunkCompaction),
            );
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
    Cursor(Vec2),
    Scroll(f32),
    Screenshot(PathBuf),
    Grain,
    Inspect(String),
    Fill {
        min: IVec3,
        max: IVec3,
        block: String,
    },
    Pour {
        pos: IVec3,
        fluid: String,
        /// In blocks.
        amount: f32,
    },
    Fluids,
    Hotbar {
        /// From 0.
        slot: usize,
        item: String,
    },
    Network(IVec3),
    /// Put items in a machine's slot group, or fluid in its tank.
    Put {
        pos: IVec3,
        /// The slot group or tank.
        name: String,
        /// Items, or blocks of fluid.
        amount: f32,
        /// The item or fluid, named as for `fill`.
        what: String,
    },
    Camera {
        position: Vec3,
        /// Degrees left of north.
        yaw: f32,
        /// Degrees up.
        pitch: f32,
    },
    Timing,
    Settle,
    Memory,
    Lookups {
        min: IVec3,
        max: IVec3,
    },
    /// Type this text, a key press and release per character.
    Type(String),
    /// Voxels picked for random ticks in each chunk each tick.
    RandomTicks(u32),
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
        "type" => {
            let text = line.trim_start()["type".len()..].trim();
            if text.is_empty() {
                return Err(format!("`type` takes some text: {line}"));
            }
            commands.push(Command::Type(text.to_owned()));
        }
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
        "cursor" => {
            expect(2)?;
            commands.push(Command::Cursor(Vec2::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
            )));
        }
        "scroll" => {
            expect(1)?;
            commands.push(Command::Scroll(parse_number(args[0])?));
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
        "fill" => {
            expect(7)?;
            let corner = |at: usize| -> Result<IVec3, String> {
                Ok(IVec3::new(
                    parse_number(args[at])?,
                    parse_number(args[at + 1])?,
                    parse_number(args[at + 2])?,
                ))
            };
            let (a, b) = (corner(0)?, corner(3)?);
            commands.push(Command::Fill {
                min: a.min(b),
                max: a.max(b),
                block: args[6].to_string(),
            });
        }
        "pour" => {
            expect(5)?;
            commands.push(Command::Pour {
                pos: IVec3::new(
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                ),
                fluid: args[3].to_string(),
                amount: parse_number(args[4])?,
            });
        }
        "put" => {
            expect(6)?;
            commands.push(Command::Put {
                pos: IVec3::new(
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                ),
                name: args[3].to_string(),
                amount: parse_number(args[4])?,
                what: args[5].to_string(),
            });
        }
        "fluids" => {
            expect(0)?;
            commands.push(Command::Fluids);
        }
        "hotbar" => {
            expect(2)?;
            let slot: usize = parse_number(args[0])?;
            if !(1..=crate::hotbar::HOTBAR_SLOTS).contains(&slot) {
                return Err(format!("`hotbar` slots are 1 to 9: {line}"));
            }
            commands.push(Command::Hotbar {
                slot: slot - 1,
                item: args[1].to_string(),
            });
        }
        "random-ticks" => {
            expect(1)?;
            commands.push(Command::RandomTicks(parse_number(args[0])?));
        }
        "network" => {
            expect(3)?;
            commands.push(Command::Network(IVec3::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
                parse_number(args[2])?,
            )));
        }
        "camera" => {
            expect(5)?;
            commands.push(Command::Camera {
                position: Vec3::new(
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                ),
                yaw: parse_number(args[3])?,
                pitch: parse_number(args[4])?,
            });
        }
        "timing" => {
            expect(0)?;
            commands.push(Command::Timing);
        }
        "settle" => {
            expect(0)?;
            commands.push(Command::Settle);
        }
        "memory" => {
            expect(0)?;
            commands.push(Command::Memory);
        }
        "lookups" => {
            expect(6)?;
            let corner = |at: usize| -> Result<IVec3, String> {
                Ok(IVec3::new(
                    parse_number(args[at])?,
                    parse_number(args[at + 1])?,
                    parse_number(args[at + 2])?,
                ))
            };
            let (a, b) = (corner(0)?, corner(3)?);
            commands.push(Command::Lookups {
                min: a.min(b),
                max: a.max(b),
            });
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
    /// Waiting for the world to load before the next command, and for how
    /// many frames so far.
    settling: Option<u32>,
    /// Frames left before the next command runs.
    waiting: u32,
    /// `screenshot` and `grain` commands to handle at the end of this frame.
    captures: Vec<Capture>,
    /// `inspect` commands to handle at the end of this frame.
    inspections: Vec<String>,
    /// `fill` and `camera` commands to apply this frame, after input.
    world_commands: Vec<Command>,
    /// Whether a `timing` report is due at the end of this frame.
    timing: bool,
    /// Whether a `fluids` report is due at the end of this frame.
    fluid_report: bool,
    /// Whether a `memory` report is due at the end of this frame.
    memory_report: bool,
    /// When the current settle began.
    settle_started: Option<Instant>,
    /// Voxels `fill` has changed since the last `timing`, and how long the
    /// fills took.
    filled: (usize, Duration),
    /// `network` reports due at the end of this frame.
    network_reports: Vec<IVec3>,
    /// Wall-clock frame times since the last `timing`.
    frame_times: Vec<Duration>,
    /// Captures requested but not yet delivered by the renderer.
    pending_captures: usize,
    /// Frames since the last command ran, once there are none left.
    frames_after_end: u32,
    /// Give up on a `settle` after this many frames:
    /// [`SETTLE_TIMEOUT_FRAMES`], but tests can wait less.
    settle_timeout: u32,
    rendering: bool,
    /// Where the `cursor` command last put the pointer, in the frame's
    /// pixels.
    cursor: Vec2,
}

impl ScriptRunner {
    fn finished(&self) -> bool {
        self.commands.is_empty()
            && self.settling.is_none()
            && self.waiting == 0
            && self.captures.is_empty()
            && self.inspections.is_empty()
            && self.world_commands.is_empty()
            && !self.timing
            && !self.fluid_report
            && !self.memory_report
            && self.network_reports.is_empty()
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
///
/// Mouse input goes to picking too, as [`PointerInput`] for the mouse
/// pointer, aimed at what the cameras draw into: the offscreen image, or
/// without rendering the primary window. That's what Bevy's own mouse
/// input does with a real window's events.
fn run_script(
    mut runner: ResMut<ScriptRunner>,
    image: Option<Res<HeadlessTarget>>,
    mut pointer: MessageWriter<PointerInput>,
    status: Option<Res<StreamingStatus>>,
    fog: Option<Res<ChunkFog>>,
    mut window: Option<Single<(Entity, &mut Window), With<PrimaryWindow>>>,
    mut keys: MessageWriter<KeyboardInput>,
    mut mouse_buttons: MessageWriter<MouseButtonInput>,
    mut mouse_motion: MessageWriter<MouseMotion>,
    mut mouse_wheel: MessageWriter<MouseWheel>,
    mut cursor_moved: MessageWriter<CursorMoved>,
    mut exit: MessageWriter<AppExit>,
    frame: Option<Res<bevy::diagnostic::FrameCount>>,
) {
    if let Some(frames) = runner.settling {
        let started = *runner.settle_started.get_or_insert_with(Instant::now);
        // Without streaming there's nothing to wait for. The status is last
        // frame's, and a camera moved on the frame of the `settle` only
        // reaches the loading that frame's end (with its global
        // transform), so the first two frames' statuses don't count.
        // Screenshots taken then should show the world as it stays, so the
        // fog has to have opened up too.
        let settled =
            status.is_none_or(|status| status.settled) && fog.is_none_or(|fog| fog.settled());
        if frames >= 2 && settled {
            runner.settling = None;
            runner.settle_started = None;
            let frame = frame.map_or(0, |frame| frame.0);
            println!(
                "[frame {frame}] settled in {frames} frames, {:.2} s",
                started.elapsed().as_secs_f64()
            );
        } else if frames >= runner.settle_timeout {
            error!("the world didn't finish loading in {frames} frames; giving up");
            exit.write(AppExit::error());
            runner.settling = None;
            runner.settle_started = None;
            runner.commands.clear();
            return;
        } else {
            runner.settling = Some(frames + 1);
            return;
        }
    }
    if runner.waiting > 0 {
        runner.waiting -= 1;
        if runner.waiting > 0 {
            return;
        }
    }
    let window_entity = window
        .as_ref()
        .map_or(Entity::PLACEHOLDER, |window| window.0);
    let target = pointer_target(image.as_deref(), window_entity);
    while let Some(command) = runner.commands.pop_front() {
        let point = |action, position| {
            target.clone().map(|target| {
                let location = Location { target, position };
                PointerInput::new(PointerId::Mouse, location, action)
            })
        };
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
                        let (logical_key, text) = logical_key(key_code);
                        keys.write(KeyboardInput {
                            key_code,
                            logical_key,
                            state,
                            text: text
                                .filter(|_| state == ButtonState::Pressed)
                                .map(Into::into),
                            repeat: false,
                            window: window_entity,
                        });
                    }
                    Button::Mouse(button) => {
                        mouse_buttons.write(MouseButtonInput {
                            button,
                            state,
                            window: window_entity,
                        });
                        let button = match button {
                            MouseButton::Right => PointerButton::Secondary,
                            MouseButton::Middle => PointerButton::Middle,
                            _ => PointerButton::Primary,
                        };
                        let action = if state == ButtonState::Pressed {
                            PointerAction::Press(button)
                        } else {
                            PointerAction::Release(button)
                        };
                        if let Some(input) = point(action, runner.cursor) {
                            pointer.write(input);
                        }
                    }
                }
            }
            Command::MoveMouse(delta) => {
                mouse_motion.write(MouseMotion { delta });
            }
            Command::Cursor(position) => {
                // No OS window keeps the cursor, so set it on the window
                // the way winit would. The frame is the window's size at
                // a scale factor of 1, so its pixels are logical pixels.
                if let Some((_, window)) = window.as_deref_mut() {
                    window.set_cursor_position(Some(position));
                }
                cursor_moved.write(CursorMoved {
                    window: window_entity,
                    position,
                    delta: None,
                });
                let delta = position - runner.cursor;
                runner.cursor = position;
                if let Some(input) = point(PointerAction::Move { delta }, position) {
                    pointer.write(input);
                }
            }
            Command::Scroll(lines) => {
                mouse_wheel.write(MouseWheel {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y: lines,
                    window: window_entity,
                    phase: TouchPhase::Moved,
                });
                let action = PointerAction::Scroll {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y: lines,
                    phase: TouchPhase::Moved,
                };
                if let Some(input) = point(action, runner.cursor) {
                    pointer.write(input);
                }
            }
            Command::Screenshot(path) => runner.captures.push(Capture::Save(path)),
            Command::Grain => runner.captures.push(Capture::Grain),
            Command::Inspect(name) => runner.inspections.push(name),
            Command::Fill { .. }
            | Command::Pour { .. }
            | Command::Put { .. }
            | Command::Camera { .. }
            | Command::Hotbar { .. }
            | Command::RandomTicks(_)
            | Command::Lookups { .. } => runner.world_commands.push(command),
            Command::Fluids => runner.fluid_report = true,
            Command::Memory => runner.memory_report = true,
            Command::Network(pos) => runner.network_reports.push(pos),
            Command::Type(text) => {
                for character in text.chars() {
                    let key_code = key_for(character);
                    let logical_key = if character == ' ' {
                        Key::Space
                    } else {
                        Key::Character(character.to_string().into())
                    };
                    for state in [ButtonState::Pressed, ButtonState::Released] {
                        keys.write(KeyboardInput {
                            key_code,
                            logical_key: logical_key.clone(),
                            state,
                            text: (state == ButtonState::Pressed)
                                .then(|| character.to_string().into()),
                            repeat: false,
                            window: window_entity,
                        });
                    }
                }
            }
            Command::Timing => runner.timing = true,
            Command::Settle => {
                runner.settling = Some(0);
                break;
            }
        }
    }
}

/// What a key means on a US layout, and the text it types, as winit
/// would report it, so text fields and shortcuts see real keys.
fn logical_key(key_code: KeyCode) -> (Key, Option<String>) {
    let named = match key_code {
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
        KeyCode::Space => return (Key::Space, Some(" ".into())),
        _ => {
            return match character_for(key_code) {
                Some(character) => {
                    let text = character.to_string();
                    (Key::Character(text.clone().into()), Some(text))
                }
                None => (Key::Unidentified(NativeKey::Unidentified), None),
            };
        }
    };
    (named, None)
}

/// The letter or digit a key types, unshifted.
fn character_for(key_code: KeyCode) -> Option<char> {
    let name = format!("{key_code:?}");
    let character = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .filter(|rest| rest.len() == 1)?
        .chars()
        .next()?;
    Some(character.to_ascii_lowercase())
}

/// The key that types `character`, for the letters and digits; others
/// are unidentified.
fn key_for(character: char) -> KeyCode {
    use bevy::input::keyboard::NativeKeyCode;
    let name = if character.is_ascii_alphabetic() {
        format!("Key{}", character.to_ascii_uppercase())
    } else if character.is_ascii_digit() {
        format!("Digit{character}")
    } else if character == ' ' {
        return KeyCode::Space;
    } else {
        return KeyCode::Unidentified(NativeKeyCode::Unidentified);
    };
    parse_button(&name)
        .ok()
        .and_then(|button| match button {
            Button::Key(key) => Some(key),
            Button::Mouse(_) => None,
        })
        .unwrap_or(KeyCode::Unidentified(NativeKeyCode::Unidentified))
}

/// What the mouse pointer points at: the image the cameras render into,
/// or with no renderer the primary window.
fn pointer_target(
    image: Option<&HeadlessTarget>,
    window: Entity,
) -> Option<NormalizedRenderTarget> {
    match image {
        Some(image) => RenderTarget::Image(image.0.clone().into()).normalize(None),
        None => RenderTarget::Window(WindowRef::Primary).normalize(Some(window)),
    }
}

/// Applies this frame's `fill` and `camera` commands. A mistake in one (an
/// unknown block, no camera) fails the run.
fn apply_world_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<ScriptRunner>().world_commands);
    for command in commands {
        let result = match command {
            Command::Fill { min, max, block } => {
                let start = Instant::now();
                let result = world
                    .run_system_cached_with(fill, (min, max, block))
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                let elapsed = start.elapsed();
                result.map(|set| {
                    let filled = &mut world.resource_mut::<ScriptRunner>().filled;
                    filled.0 += set;
                    filled.1 += elapsed;
                })
            }
            Command::Lookups { min, max } => world
                .run_system_cached_with(memory::measure_lookups, (min, max))
                .map_err(|error| error.to_string())
                .and_then(|result| result)
                .map(|cost| {
                    let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
                    println!("[frame {frame}] {cost}");
                }),
            Command::Pour { pos, fluid, amount } => world
                .run_system_cached_with(pour, (pos, fluid, amount))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Put {
                pos,
                name,
                amount,
                what,
            } => world
                .run_system_cached_with(put, (pos, name, amount, what))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Camera {
                position,
                yaw,
                pitch,
            } => world
                .run_system_cached_with(place_camera, (position, yaw, pitch))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Hotbar { slot, item } => world
                .run_system_cached_with(hold_item, (slot, item))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::RandomTicks(per_chunk) => {
                world.resource_mut::<RandomTicks>().per_chunk = per_chunk;
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            error!("{error}");
            world.write_message(AppExit::error());
        }
    }
}

/// The state name `name` (see [`StateName`]), its id qualified with
/// `defaria:` if it has no mod.
fn split_name(name: &str) -> Result<StateName, String> {
    let mut parsed = StateName::parse(name)?;
    if !parsed.id.contains(':') {
        parsed.id = format!("defaria:{}", parsed.id);
    }
    Ok(parsed)
}

/// The item state `name` means, named as for [`fill_state`].
fn item_state(content: &Content, name: &str) -> Result<ItemState, String> {
    let parsed = split_name(name)?;
    let id = &parsed.id;
    let item = content.item(id).ok_or_else(|| format!("no item `{id}`"))?;
    item.state(&parsed.pairs())
        .ok_or_else(|| format!("item `{id}` has no state `{name}`"))
}

/// Puts the item `name` in hotbar slot `slot` and makes it the active one.
fn hold_item(
    In((slot, name)): In<(usize, String)>,
    content: Res<Content>,
    mut players: Query<(&mut Hotbar, &mut ActiveSlot), With<LocalPlayer>>,
) -> Result<(), String> {
    let item = item_state(&content, &name).map_err(|error| format!("hotbar: {error}"))?;
    let (mut hotbar, mut active) = players
        .single_mut()
        .map_err(|_| "hotbar: there's no player".to_string())?;
    hotbar.assign(slot, Some(item));
    *active = ActiveSlot(slot);
    Ok(())
}

/// The state `name` means: a block id (the base game's without
/// `defaria:`), with `[property=value,…]` setting some of its properties.
fn fill_state(content: &Content, name: &str) -> Result<BlockState, String> {
    let parsed = split_name(name)?;
    let id = &parsed.id;
    let block = content
        .block(id)
        .ok_or_else(|| format!("no block `{id}`"))?;
    block
        .state(&parsed.pairs())
        .ok_or_else(|| format!("block `{id}` has no state `{name}`"))
}

fn fill(
    In((min, max, id)): In<(IVec3, IVec3, String)>,
    content: Res<Content>,
    mut voxels: Voxels,
) -> Result<usize, String> {
    let state = if id == crate::content::AIR {
        BlockState::AIR
    } else {
        fill_state(&content, &id).map_err(|error| format!("fill: {error}"))?
    };
    // The voxels it changed: not ones already full of `state`, nor ones
    // in chunks that aren't loaded.
    let mut set = 0;
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let pos = IVec3::new(x, y, z);
                let changes = voxels
                    .get_with_fluid(pos)
                    .is_some_and(|before| before != (state, None));
                if voxels.set(pos, state) && changes {
                    set += 1;
                }
            }
        }
    }
    Ok(set)
}

fn pour(
    In((pos, id, amount)): In<(IVec3, String, f32)>,
    content: Res<Content>,
    mut voxels: Voxels,
) -> Result<(), String> {
    let state = fill_state(&content, &id).map_err(|error| format!("pour: {error}"))?;
    if !content.state(state).is_fluid() {
        return Err(format!("pour: `{id}` isn't a fluid"));
    }
    let units = (amount * FULL_FLUID as f32).round();
    if !(1.0..=FULL_FLUID as f32).contains(&units) {
        return Err(format!(
            "pour: {amount} blocks is out of range; it must be over 0 and at most 1"
        ));
    }
    voxels.set(pos, state);
    voxels.set_fluid(pos, units as u16);
    Ok(())
}

fn put(
    In((pos, name, amount, what)): In<(IVec3, String, f32, String)>,
    content: Res<Content>,
    index: Res<BlockEntities>,
    mut machines: Query<(Option<&mut Slots>, Option<&mut Tanks>)>,
) -> Result<(), String> {
    let at = |error: String| format!("put at {} {} {}: {error}", pos.x, pos.y, pos.z);
    let (slots, tanks) = index
        .get(pos)
        .and_then(|entity| machines.get_mut(entity).ok())
        .ok_or_else(|| at("there's no machine here".into()))?;
    // A fluid goes in a tank, anything else in a slot group: a machine
    // may have both by one name (a generator's `fuel`).
    let fluid = fill_state(&content, &what)
        .ok()
        .filter(|&state| content.state(state).is_fluid());
    if let (Some(fluid), Some(tank)) = (
        fluid,
        tanks
            .map(|tanks| tanks.into_inner())
            .and_then(|tanks| tanks.get_mut(&name)),
    ) {
        let id = content.block_of(fluid).id.clone();
        let units = (amount * FULL_FLUID as f32).round();
        if units < 1.0 {
            return Err(at(format!("{amount} blocks is too little")));
        }
        let units = units as u64;
        if !tank.takes(&id) || tank.room() < units {
            return Err(at(format!(
                "{amount} blocks of `{id}` don't fit in `{name}`"
            )));
        }
        tank.fill(&id, units);
        return Ok(());
    }
    if let Some(group) = slots
        .map(|slots| slots.into_inner())
        .and_then(|slots| slots.group_mut(&name))
    {
        let item = item_state(&content, &what).map_err(at)?;
        if amount < 1.0 || amount.fract() != 0.0 {
            return Err(at(format!("{amount} isn't a whole number of items")));
        }
        let stack = crate::inventory::ItemStack::new(item, amount as u32);
        let room = group.room_for(&content, &stack);
        if room < stack.count {
            return Err(at(format!("only {room} of `{what}` fit in `{name}`")));
        }
        group.insert(&content, stack);
        return Ok(());
    }
    Err(at(format!(
        "the machine has no slot group, or tank for `{what}`, called `{name}`"
    )))
}

/// The `fluids` report: each fluid's total in the loaded world, the pipes
/// and machines' tanks, in thousandths of a block, and the active voxels.
/// A fluid that's anywhere but the world also shows where it is.
fn fluid_report(world: &mut World) -> String {
    use std::collections::BTreeMap;
    let mut in_world: BTreeMap<String, u64> = default();
    let mut chunks = world.query::<&Chunk>();
    let content = world.resource::<Content>();
    for chunk in chunks.iter(world) {
        for index in 0..chunk.voxels().len() {
            let pos = Chunk::position_of(index);
            let amount = chunk.fluid_amount_at(pos, content);
            if amount > 0 {
                let name = &content.blocks[content.state(chunk.at(pos)).block].id;
                *in_world.entry(name.clone()).or_default() += amount as u64;
            }
        }
    }
    let in_pipes = crate::fluid_network::held_in_pipes(world);
    let mut in_machines: BTreeMap<String, u64> = default();
    let mut tanks = world.query::<&crate::machine::Tanks>();
    for tanks in tanks.iter(world) {
        for tank in &tanks.0 {
            if let Some(fluid) = &tank.fluid {
                *in_machines.entry(fluid.clone()).or_default() += tank.amount;
            }
        }
    }
    let names: std::collections::BTreeSet<&String> = in_world
        .keys()
        .chain(in_pipes.keys())
        .chain(in_machines.keys())
        .collect();
    let stats = world.resource::<FluidStats>();
    let mut line = format!("fluids: {} active", stats.active);
    for name in names {
        let get = |map: &BTreeMap<String, u64>| map.get(name).copied().unwrap_or_default();
        let (here, pipes, machines) = (get(&in_world), get(&in_pipes), get(&in_machines));
        line += &format!(", {name} {}", here + pipes + machines);
        if pipes + machines > 0 {
            line += &format!(" (world {here}, pipes {pipes}, machines {machines})");
        }
    }
    line
}

/// The `network` report for the conduit at `pos`.
fn network_report(world: &World, pos: IVec3) -> String {
    let ports = world.resource::<crate::machine::MachinePorts>();
    let networks = world.resource::<crate::network::Networks>();
    let mut line =
        crate::network::describe(networks, world.resource::<Content>(), pos, |id, port| {
            ports.role(id, port)
        });
    let energy = networks.at(pos).and_then(|id| {
        let kind = networks.get(id)?.kind();
        let report = world.get_resource::<crate::network::energy::EnergyReport>()?;
        (world.resource::<Content>().networks[kind].solver == crate::content::Solver::Energy)
            .then(|| crate::network::energy::describe(report, id))
    });
    if let Some(energy) = energy {
        line += &format!("; {energy}");
    }
    let fluid = networks.at(pos).and_then(|id| {
        world
            .get_resource::<crate::network::NetworkMap<crate::fluid_network::FluidNet>>()?
            .get(id)
    });
    if let Some(net) = fluid.filter(|net| net.total() > 0) {
        let held: Vec<String> = net
            .held
            .iter()
            .map(|held| format!("{} {}", held.fluid, held.amount))
            .collect();
        line += &format!(", holding {}", held.join(", "));
    }
    line
}

/// Puts the local player's eyes at `position`, looking along `yaw` and
/// `pitch`, flying, so the view stays put.
fn place_camera(
    In((position, yaw, pitch)): In<(Vec3, f32, f32)>,
    mut players: Query<
        (
            &mut Look,
            &mut Transform,
            &mut Player,
            &mut Body,
            Option<&mut ViewBob>,
        ),
        With<LocalPlayer>,
    >,
) -> Result<(), String> {
    let (mut look, mut transform, mut player, mut body, bob) = players
        .single_mut()
        .map_err(|error| format!("camera: {error}"))?;
    // The body turns to face the look as the frame is drawn.
    *look = Look {
        yaw: yaw.to_radians(),
        pitch: pitch.to_radians().clamp(-MAX_PITCH, MAX_PITCH),
    };
    player.flying = true;
    player.eye_height = EYE_HEIGHT;
    player.previous_eye_height = EYE_HEIGHT;
    player.drawn_eye_height = EYE_HEIGHT;
    if let Some(mut bob) = bob {
        bob.settle();
    }
    put_eyes(position.as_dvec3(), EYE_HEIGHT, &mut body, &mut transform);
    Ok(())
}

/// Keeps each frame's wall-clock time for `timing`: from the end of the
/// last frame to the end of this one. Not from [`Time<Real>`], which steps
/// by the fixed frame time here like the other clocks.
fn record_frame_time(mut last: Local<Option<Instant>>, mut runner: ResMut<ScriptRunner>) {
    let now = Instant::now();
    if let Some(last) = last.replace(now) {
        runner.frame_times.push(now - last);
    }
}

/// Prints the frame times since the last report, and starts over.
fn report_timing(runner: &mut ScriptRunner, frame: u32) {
    let times = std::mem::take(&mut runner.frame_times);
    let (voxels, time) = std::mem::take(&mut runner.filled);
    runner.timing = false;
    let mut line = timing_summary(&times);
    if voxels > 0 {
        line += &format!(
            "; fill set {voxels} voxels in {:.2} ms ({:.0} ns each)",
            time.as_secs_f64() * 1000.0,
            time.as_nanos() as f64 / voxels as f64
        );
    }
    println!("[frame {frame}] timing: {line}");
}

/// How many frames, and their mean and worst times.
fn timing_summary(times: &[Duration]) -> String {
    if times.is_empty() {
        return "no frames yet".into();
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = times.iter().map(|&d| ms(d)).sum::<f64>() / times.len() as f64;
    let worst = times.iter().map(|&d| ms(d)).fold(0.0, f64::max);
    format!(
        "{} frames, mean {mean:.2} ms, worst {worst:.2} ms",
        times.len()
    )
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
    {
        let mut runner = world.resource_mut::<ScriptRunner>();
        if runner.timing {
            report_timing(&mut runner, frame);
        }
    }
    if std::mem::take(&mut world.resource_mut::<ScriptRunner>().fluid_report) {
        println!("[frame {frame}] {}", fluid_report(world));
    }
    if std::mem::take(&mut world.resource_mut::<ScriptRunner>().memory_report) {
        println!("[frame {frame}] {}", memory::ChunkMemory::measure(world));
    }
    for pos in std::mem::take(&mut world.resource_mut::<ScriptRunner>().network_reports) {
        println!("[frame {frame}] {}", network_report(world, pos));
    }
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
    // Resources are components too, on an entity of their own.
    let component = registration.data::<ReflectComponent>().ok_or_else(|| {
        format!("`{name}` isn't a component or resource with #[reflect(Component)] or #[reflect(Resource)]")
    })?;

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

/// The whole game with no GPU, running `script`, loading a smaller world
/// than the game does so tests are quick.
#[cfg(test)]
pub fn test_game(script: Script) -> App {
    let mut app = App::new();
    app.add_plugins((
        default_plugins(false).disable::<bevy::log::LogPlugin>(),
        crate::GamePlugin,
        HeadlessPlugin {
            script,
            rendering: false,
        },
    ))
    .insert_resource(crate::streaming::StreamingSettings {
        radius: 4,
        ..default()
    });
    app.finish();
    app.cleanup();
    app
}

/// Runs `script` on a [`test_game`] app that has already finished its own,
/// updating it until this one has finished too, so a test can look at the
/// game between steps.
#[cfg(test)]
pub fn run_more(app: &mut App, script: &str) {
    let script = Script::parse(script).unwrap();
    app.world_mut()
        .resource_mut::<ScriptRunner>()
        .commands
        .extend(script.commands);
    for _ in 0..SETTLE_TIMEOUT_FRAMES {
        app.update();
        if app.world().resource::<ScriptRunner>().finished() {
            return;
        }
    }
    panic!("the script never finished");
}

/// Updates `app` until the world around its camera has loaded. Unlike the
/// script's `settle`, it doesn't wait for the fog to open, which tests that
/// look at the world rather than the view don't need.
#[cfg(test)]
pub fn settle(app: &mut App) {
    // A camera moved this frame reaches the loading a frame late.
    app.update();
    app.update();
    for _ in 0..SETTLE_TIMEOUT_FRAMES {
        app.update();
        if app.world().resource::<StreamingStatus>().settled {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the world never settled");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fly_camera::FlyCamera,
        light::{ChunkLight, Light},
        player::eyes,
        world::{ChunkCoord, ChunkMap},
    };

    #[test]
    fn parses_every_command() {
        let script = Script::parse(
            "# comment\n\
             wait 2\n\
             press KeyW # trailing comment\n\
             release Space\n\
             hold MouseLeft 3\n\
             mouse 10 -5.5\n\
             cursor 640 360.5\n\
             scroll -2\n\
             \n\
             screenshot target/a.png\n\
             grain\n\
             inspect Transform\n\
             fill 3 9 -2 1 4 5 red_lamp\n\
             camera 1 2.5 3 90 -45\n\
             timing\n\
             settle\n\
             pour 4 -5 6 water 0.25\n\
             fluids\n\
             network 1 -2 3\n\
             put 1 2 3 input 4 stone[ore=defaria:iron]
             type  Iron ore 2 \n\
             random-ticks 64\n\
             memory\n\
             lookups 5 0 -1 1 2 3\n",
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
                Command::Cursor(Vec2::new(640.0, 360.5)),
                Command::Scroll(-2.0),
                Command::Screenshot(PathBuf::from("target/a.png")),
                Command::Grain,
                Command::Inspect("Transform".into()),
                Command::Fill {
                    min: IVec3::new(1, 4, -2),
                    max: IVec3::new(3, 9, 5),
                    block: "red_lamp".into(),
                },
                Command::Camera {
                    position: Vec3::new(1.0, 2.5, 3.0),
                    yaw: 90.0,
                    pitch: -45.0,
                },
                Command::Timing,
                Command::Settle,
                Command::Pour {
                    pos: IVec3::new(4, -5, 6),
                    fluid: "water".into(),
                    amount: 0.25,
                },
                Command::Fluids,
                Command::Network(IVec3::new(1, -2, 3)),
                Command::Put {
                    pos: IVec3::new(1, 2, 3),
                    name: "input".into(),
                    amount: 4.0,
                    what: "stone[ore=defaria:iron]".into(),
                },
                Command::Type("Iron ore 2".into()),
                Command::RandomTicks(64),
                Command::Memory,
                Command::Lookups {
                    min: IVec3::new(1, 0, -1),
                    max: IVec3::new(5, 2, 3),
                },
            ]
        );
    }

    #[test]
    fn parse_errors_name_the_line() {
        let error = Script::parse("wait 1\npress KeyQQ").unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
        assert!(Script::parse("jump").is_err());
        assert!(Script::parse("type").is_err());
        assert!(Script::parse("wait").is_err());
        assert!(Script::parse("wait soon").is_err());
        assert!(Script::parse("grain 3").is_err());
        assert!(Script::parse("network 1 2").is_err());
        assert!(Script::parse("hotbar 0 wrench").is_err());
        assert!(Script::parse("hotbar 10 wrench").is_err());
        assert!(Script::parse("hotbar 1").is_err());
        assert!(Script::parse("fill 1 2 3 4 5 6").is_err());
        assert!(Script::parse("fill 1 2 3 4 5 x stone").is_err());
        assert!(Script::parse("camera 1 2 3 4").is_err());
        assert!(Script::parse("timing now").is_err());
        assert!(Script::parse("settle 5").is_err());
        assert!(Script::parse("cursor 4").is_err());
        assert!(Script::parse("scroll").is_err());
        assert!(Script::parse("pour 1 2 3 water").is_err());
        assert!(Script::parse("pour 1 2 3 water lots").is_err());
        assert!(Script::parse("fluids all").is_err());
        assert!(Script::parse("random-ticks").is_err());
        assert!(Script::parse("random-ticks many").is_err());
        assert!(Script::parse("memory all").is_err());
        assert!(Script::parse("lookups 1 2 3 4 5").is_err());
    }

    /// Runs the whole game with no GPU until the script ends, and returns
    /// how it exited.
    fn run_game(script: &str) -> (App, AppExit) {
        let mut app = test_game(Script::parse(script).unwrap());
        for _ in 0..SETTLE_TIMEOUT_FRAMES + 200 {
            app.update();
            if let Some(exit) = app.should_exit() {
                return (app, exit);
            }
        }
        panic!("script never finished");
    }

    fn light_at(app: &mut App, pos: IVec3) -> Light {
        let (coord, local) = ChunkCoord::split(pos);
        let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
        app.world().get::<ChunkLight>(entity).unwrap().at(local)
    }

    /// End to end: `fill` builds a sealed room through the game's own edit
    /// path, so it's lit like the player's edits: dark inside, with a lamp's
    /// light falling off a level per block. `camera` moves the camera.
    #[test]
    fn fill_and_camera_set_up_a_scene() {
        let (mut app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 60 30 60 stone\n\
             fill 51 21 51 59 29 59 air\n\
             fill 55 25 55 55 25 55 red_lamp\n\
             camera 55.5 26.5 52 180 -10\n\
             wait 1\n\
             timing\n",
        );
        assert_eq!(exit, AppExit::Success);
        let red = crate::content::base()
            .state(crate::content::block("red_lamp"))
            .light;
        for d in 1..4 {
            let light = light_at(&mut app, IVec3::new(55 - d, 25, 55));
            assert_eq!(light.sky(), 0, "the room is sealed");
            assert_eq!(light.block()[0], red[0] - d as u8);
        }
        assert_eq!(light_at(&mut app, IVec3::new(52, 22, 52)).sky(), 0);
        let (camera, transform) = camera(&mut app);
        assert!(
            transform.translation.distance(Vec3::new(55.5, 26.5, 52.0)) < 1e-4,
            "{transform:?}"
        );
        assert!((camera.yaw - std::f32::consts::PI).abs() < 1e-5);
        assert!((camera.pitch + 10f32.to_radians()).abs() < 1e-5);
        // Yaw 180 faces south (+Z), at the lamp.
        assert!(transform.forward().z > 0.9);
    }

    /// End to end: `random-ticks` speeds random ticks up, so mossy stone
    /// spreads over a stone wall within a second; right click on the
    /// switched lamp turns it on through its script, without placing.
    #[test]
    fn scripted_blocks_run_in_the_game() {
        let (mut app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 54 22 50 stone\n\
             fill 50 20 50 50 20 50 mossy_stone\n\
             fill 52 24 52 52 24 52 switch_lamp\n\
             camera 52.5 26.5 52.5 0 -89\n\
             random-ticks 512\n\
             hold MouseLeft 1\n\
             wait 2\n\
             hold MouseRight 1\n\
             wait 60\n",
        );
        assert_eq!(exit, AppExit::Success);
        let base = crate::content::base();
        let mut at = |pos: IVec3| -> BlockState {
            use bevy::ecs::system::RunSystemOnce;
            app.world_mut()
                .run_system_once(move |voxels: Voxels| voxels.get(pos))
                .unwrap()
                .unwrap()
        };
        let mossy = crate::content::block("mossy_stone");
        let wall = (50..=54).flat_map(|x| (20..=22).map(move |y| IVec3::new(x, y, 50)));
        let mossy_count = wall.filter(|&pos| at(pos) == mossy).count();
        assert!(mossy_count > 3, "{mossy_count}");
        let lit = base.block_state("defaria:switch_lamp", &[("lit", "true")]);
        assert_eq!(Some(at(IVec3::new(52, 24, 52))), lit);
        assert!(at(IVec3::new(52, 25, 52)).is_air());
    }

    /// End to end: pipes `fill` sets in the game join the pipes next to
    /// them, whatever sides the fill named, and a wire beside them doesn't.
    #[test]
    fn filled_pipes_join_each_other() {
        let (mut app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 52 20 50 pipe[up=true]\n\
             fill 51 21 50 51 21 50 pipe\n\
             fill 53 20 50 53 20 50 wire\n\
             wait 1\n",
        );
        assert_eq!(exit, AppExit::Success);
        let content = crate::content::base();
        use crate::content::Direction::{self, *};
        let world = app.world_mut();
        let joined = |world: &mut World, pos: IVec3| {
            let (coord, local) = ChunkCoord::split(pos);
            let entity = world.resource::<ChunkMap>().get(coord).unwrap();
            let state = world.get::<crate::world::Chunk>(entity).unwrap().at(local);
            Direction::ALL
                .into_iter()
                .filter(|&side| content.joined(state, side))
                .collect::<Vec<_>>()
        };
        assert_eq!(joined(world, IVec3::new(50, 20, 50)), [East]);
        assert_eq!(joined(world, IVec3::new(51, 20, 50)), [Up, East, West]);
        assert_eq!(joined(world, IVec3::new(52, 20, 50)), [West]);
        assert_eq!(joined(world, IVec3::new(51, 21, 50)), [Down]);
        assert_eq!(joined(world, IVec3::new(53, 20, 50)), []);
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
                crate::streaming::finish_background_work(app.world_mut());
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

    #[test]
    fn timing_summarizes_the_frames() {
        let ms = Duration::from_millis;
        assert_eq!(
            timing_summary(&[ms(2), ms(4), ms(9)]),
            "3 frames, mean 5.00 ms, worst 9.00 ms"
        );
        assert_eq!(timing_summary(&[]), "no frames yet");
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

    /// `timing` counts the voxels `fill` set since the last one, and
    /// starts over.
    #[test]
    fn timing_counts_filled_voxels() {
        // Eight voxels of sky above the start set to stone, then the same again (changing
        // nothing), then a box in unloaded chunks far below.
        let (app, exit) = run_game(
            "fill 64 70 80 65 71 81 stone\n\
             wait 1\n\
             fill 64 70 80 65 71 81 stone\n\
             fill 0 -100000 0 1 -99999 1 stone\n\
             wait 1\n",
        );
        assert_eq!(exit, AppExit::Success);
        assert_eq!(app.world().resource::<ScriptRunner>().filled.0, 8);
        let (app, exit) = run_game("fill 0 100 0 1 101 1 air\nwait 1\ntiming\nwait 1\n");
        assert_eq!(exit, AppExit::Success);
        assert_eq!(app.world().resource::<ScriptRunner>().filled.0, 0);
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

    #[test]
    fn fill_names_a_block_and_some_of_its_properties() {
        let content = crate::content::base();
        let log = content.block("defaria:log").unwrap();
        assert_eq!(fill_state(content, "log"), Ok(log.default_state()));
        assert_eq!(
            fill_state(content, "defaria:log[wood=defaria:birch,axis=x]"),
            Ok(log
                .state(&[("axis", "x"), ("wood", "defaria:birch")])
                .unwrap())
        );
        for bad in ["log[axis=w]", "log[axis]", "log[axis=x", "nonsense"] {
            assert!(fill_state(content, bad).is_err(), "{bad}");
        }
    }

    /// Every scenario in `scenarios/` parses, and every block it fills
    /// with (and item it holds) exists, so a renamed block fails `cargo test` and not just the
    /// scenario run in CI.
    #[test]
    fn every_scenario_parses_and_fills_with_real_blocks() {
        let content = crate::content::base();
        let mut scenarios = 0;
        for entry in std::fs::read_dir("scenarios").unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "txt") {
                continue;
            }
            scenarios += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            let script = Script::parse(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            for command in script.commands {
                if let Command::Fill { block, .. } = &command
                    && block != crate::content::AIR
                {
                    fill_state(content, block).unwrap_or_else(|e| panic!("{path:?}: {e}"));
                }
                if let Command::Hotbar { item, .. } = &command {
                    item_state(content, item).unwrap_or_else(|e| panic!("{path:?}: {e}"));
                }
            }
        }
        assert!(scenarios > 0, "no scenarios found");
    }

    /// `hotbar` puts an item in a slot and makes it the one in hand,
    /// with the properties it's given.
    #[test]
    fn hotbar_puts_an_item_in_hand() {
        assert_eq!(
            Script::parse("hotbar 3 wrench[head=defaria:copper]")
                .unwrap()
                .commands,
            [Command::Hotbar {
                slot: 2,
                item: "wrench[head=defaria:copper]".into()
            }]
        );
        let (mut app, exit) = run_game("wait 2\nhotbar 3 wrench[head=defaria:copper]\nwait 1\n");
        assert!(!exit.is_error());
        let content = app.world().resource::<Content>();
        let copper = content
            .item("defaria:wrench")
            .unwrap()
            .state(&[("head", "defaria:copper")])
            .unwrap();
        let (hotbar, active) = app
            .world_mut()
            .query_filtered::<(&Hotbar, &ActiveSlot), With<LocalPlayer>>()
            .single(app.world())
            .unwrap();
        assert_eq!(*active, ActiveSlot(2));
        assert_eq!(hotbar.item(*active), Some(copper));

        let (_, exit) = run_game("wait 2\nhotbar 1 nonsense\nwait 1\n");
        assert!(exit.is_error());
    }

    #[test]
    fn filling_with_an_unknown_block_fails_the_run() {
        let (_, exit) = run_game("wait 2\nfill 1 1 1 2 2 2 nonsense\nwait 1\n");
        assert!(exit.is_error());
    }

    /// The amount of fluid in the voxel at `pos`.
    fn fluid_at(app: &App, pos: IVec3) -> u16 {
        let (coord, local) = ChunkCoord::split(pos);
        let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
        let chunk = app.world().get::<Chunk>(entity).unwrap();
        chunk.fluid_amount_at(local, app.world().resource::<Content>())
    }

    /// `pour` puts fluid in a voxel through the game's own edit path, and
    /// it flows from there: a block of water poured in a sealed trench
    /// spreads along it, all of it still there.
    #[test]
    fn poured_water_spreads_and_keeps_all_of_itself() {
        let (app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 60 22 52 stone\n\
             fill 51 21 51 59 22 51 air\n\
             pour 55 21 51 water 1\n\
             wait 120\n\
             fluids\n",
        );
        assert_eq!(exit, AppExit::Success);
        let row: Vec<u16> = (51..=59)
            .map(|x| fluid_at(&app, IVec3::new(x, 21, 51)))
            .collect();
        assert_eq!(row.iter().map(|&a| a as u32).sum::<u32>(), 1000, "{row:?}");
        assert!(row.iter().filter(|&&a| a > 0).count() >= 7, "{row:?}");
    }

    /// The `network` report: pipes `fill` puts down form one network, a
    /// wire beside them another, and a voxel with no conduit has none.
    #[test]
    fn the_network_report_describes_the_conduits_network() {
        let (mut app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 54 20 50 pipe\n\
             fill 52 21 50 52 22 50 pipe\n\
             fill 50 20 51 53 20 51 wire\n\
             wait 1\n\
             network 52 22 50\n",
        );
        assert_eq!(exit, AppExit::Success);
        let world = app.world_mut();
        let pipes = network_report(world, IVec3::new(50, 20, 50));
        assert!(pipes.starts_with("network at 50 20 50: #"), "{pipes}");
        assert!(
            pipes.contains(" defaria:fluid (Fluid), 7 conduits, 1 chunk, capacity 3.5, running"),
            "{pipes}"
        );
        let wires = network_report(world, IVec3::new(53, 20, 51));
        assert!(
            wires.contains(" defaria:electric (Energy), 4 conduits"),
            "{wires}"
        );
        assert_eq!(
            network_report(world, IVec3::new(50, 21, 50)),
            "network at 50 21 50: none"
        );
    }

    /// On an energy network, `network` adds what it stored, produced and
    /// consumed: a generator lighting a lamp over a wire.
    #[test]
    fn the_network_report_says_what_energy_moved() {
        let (mut app, exit) = run_game(
            "wait 2\n\
             fill 49 20 50 49 20 50 creative_generator\n\
             fill 50 20 50 53 20 50 wire\n\
             fill 52 21 50 52 21 50 powered_lamp\n\
             wait 90\n",
        );
        assert_eq!(exit, AppExit::Success);
        let line = network_report(app.world_mut(), IVec3::new(51, 20, 50));
        assert!(
            line.ends_with(
                "; energy: stored 0 of 0 J, produced 20 J and consumed 20 J in the last second, throughput 1000 W"
            ),
            "{line}"
        );
    }

    /// The slots of the machine at `pos`.
    fn machine_slots(app: &App, pos: IVec3) -> Slots {
        let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
        app.world().get::<Slots>(entity).unwrap().clone()
    }

    /// `put` fills a machine's slots and tanks as a tube or pipe would: a
    /// powered furnace given an ore smelts it, and a generator's tank takes
    /// lava.
    #[test]
    fn put_fills_machines_and_a_powered_furnace_smelts() {
        let (app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 50 20 50 creative_generator\n\
             fill 50 20 51 50 20 51 wire\n\
             fill 50 20 52 50 20 52 electric_furnace[facing=south]\n\
             fill 52 20 52 52 20 52 combustion_generator\n\
             wait 1\n\
             put 50 20 52 input 1 stone[rock=defaria:granite,ore=defaria:gold]\n\
             put 52 20 52 fuel 0.5 lava\n\
             wait 360\n",
        );
        assert_eq!(exit, AppExit::Success);
        let content = app.world().resource::<Content>();
        let gold = content
            .item("defaria:ingot")
            .unwrap()
            .state(&[("metal", "defaria:gold")])
            .unwrap();
        let output = machine_slots(&app, IVec3::new(50, 20, 52));
        assert_eq!(
            output.group("output").unwrap().slots[0],
            Some(crate::inventory::ItemStack::new(gold, 1))
        );
        let entity = app
            .world()
            .resource::<BlockEntities>()
            .get(IVec3::new(52, 20, 52))
            .unwrap();
        // Its first tenth is burning.
        assert_eq!(app.world().get::<Tanks>(entity).unwrap().0[0].amount, 400);
    }

    /// `put` fails the run where there's no machine, no such slot group or
    /// tank, or not room for it all.
    #[test]
    fn putting_what_doesnt_fit_fails_the_run() {
        for line in [
            "put 50 20 50 input 1 stone",
            "fill 50 20 50 50 20 50 electric_furnace\nput 50 20 50 fuel 1 stone",
            "fill 50 20 50 50 20 50 electric_furnace\nput 50 20 50 input 65 stone",
            "fill 50 20 50 50 20 50 electric_furnace\nput 50 20 50 input 1 nothing",
            "fill 50 20 50 50 20 50 combustion_generator\nput 50 20 50 fuel 5 lava",
            "fill 50 20 50 50 20 50 combustion_generator\nput 50 20 50 fuel 1 water",
        ] {
            let (_, exit) = run_game(&format!(
                "wait 2\nfill 50 20 50 50 20 50 air\n{line}\nwait 1\n"
            ));
            assert!(exit.is_error(), "{line}");
        }
    }

    /// The `fluids` report counts each fluid in the loaded world: deep
    /// underground, where there's no other water, just what was poured.
    #[test]
    fn the_fluids_report_counts_what_was_poured() {
        let (mut app, exit) = run_game(
            "camera 5000.5 -1000.5 5000.5 0 0\n\
             settle\n\
             fill 4990 -1001 4990 5010 -1000 5010 air\n\
             pour 5000 -1000 5000 water 0.75\n\
             wait 1\n",
        );
        assert_eq!(exit, AppExit::Success);
        let report = fluid_report(app.world_mut());
        assert!(report.starts_with("fluids: "), "{report}");
        assert!(report.ends_with(" active, defaria:water 750"), "{report}");
    }

    /// The `fluids` report counts what's in machines' tanks too, and says
    /// where it is: a pump has drawn the poured water into a tank. The
    /// `network` report gives the pump and tank's roles.
    #[test]
    fn the_fluids_report_counts_tanks_and_pipes() {
        let (mut app, exit) = run_game(
            "camera 5000.5 -1000.5 5000.5 0 0\n\
             settle\n\
             fill 4996 -1002 4998 5004 -999 5002 stone\n\
             fill 4999 -1001 5000 5003 -1001 5000 air\n\
             pour 5002 -1001 5000 water 1\n\
             fill 4999 -1001 5000 4999 -1001 5000 pump[facing=east]\n\
             fill 4998 -1001 5000 4998 -1001 5000 pipe\n\
             fill 4998 -1000 5000 4998 -1000 5000 tank\n\
             wait 180\n",
        );
        assert_eq!(exit, AppExit::Success);
        let report = fluid_report(app.world_mut());
        assert!(
            report.ends_with(" active, defaria:water 1000 (world 0, pipes 0, machines 1000)"),
            "{report}"
        );
        let network = network_report(app.world_mut(), IVec3::new(4998, -1001, 5000));
        assert!(
            network.contains("4999 -1001 5000 west (output)")
                && network.contains("4998 -1000 5000 down (store)"),
            "{network}"
        );
    }

    /// Pouring something that isn't a fluid, or no amount, fails the run.
    #[test]
    fn pouring_a_solid_or_nothing_fails_the_run() {
        let (_, exit) = run_game("wait 2\npour 1 1 1 stone 1\nwait 1\n");
        assert!(exit.is_error());
        let (_, exit) = run_game("wait 2\npour 1 1 1 water 0\nwait 1\n");
        assert!(exit.is_error());
    }

    /// Placing a block into water deletes the water in that voxel, and
    /// wakes the water around it: water held back by a wall flows on once
    /// the wall is gone.
    #[test]
    fn blocks_delete_fluid_and_edits_wake_it() {
        let (app, exit) = run_game(
            "wait 2\n\
             fill 50 20 50 60 22 52 stone\n\
             fill 51 21 51 59 22 51 air\n\
             fill 55 21 51 55 21 51 stone\n\
             fill 51 21 51 52 21 51 water\n\
             wait 120\n\
             fill 51 21 51 51 21 51 stone\n\
             fill 55 21 51 55 21 51 air\n\
             wait 240\n",
        );
        assert_eq!(exit, AppExit::Success);
        let row: Vec<u16> = (51..=59)
            .map(|x| fluid_at(&app, IVec3::new(x, 21, 51)))
            .collect();
        assert_eq!(row[0], 0, "the stone took the water's place");
        // What the stone took is counted, and the rest is all there.
        let displaced = app.world().resource::<FluidStats>().displaced;
        assert!(displaced > 0);
        let total = row.iter().map(|&a| a as u64).sum::<u64>();
        assert_eq!(total + displaced, 2000, "{row:?}");
        assert!(row[8] > 0, "flowed past where the wall was: {row:?}");
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

    /// Runs `script` in the test game until it ends.
    fn play(script: &str) -> App {
        let mut app = test_game(Script::parse(script).unwrap());
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
        app
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

    /// `camera` puts the eyes where it says (to within f32 rounding), level, even straight
    /// after sneaking along the ground (lower eyes, a swaying view).
    #[test]
    fn camera_resets_sneaking_eyes_and_the_sway() {
        use crate::view_bob::ViewBob;

        let mut app = play(
            "fill 56 50 84 72 50 100 stone\n\
             camera 64.5 52.7 92.5 0 0\n\
             press ShiftLeft  # down onto the stone, then sneak north\n\
             wait 40\n\
             press KeyW\n\
             wait 40\n\
             camera 64.5 70 92.5 0 0\n\
             release ShiftLeft\n\
             release KeyW\n\
             wait 1\n",
        );
        let (_, eyes) = camera(&mut app);
        assert!((eyes.translation.y - 70.0).abs() < 1e-4, "{eyes:?}");
        let world = app.world_mut();
        let bob = *world.query::<&ViewBob>().single(world).unwrap();
        assert_eq!(bob.amount, 0.0, "{bob:?}");
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
            default_plugins(false).disable::<bevy::log::LogPlugin>(),
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

    /// The local player's look, and its eyes.
    fn camera(app: &mut App) -> (Look, Transform) {
        let (transform, player, look) = app
            .world_mut()
            .query_filtered::<(&Transform, &Player, &Look), With<LocalPlayer>>()
            .single(app.world())
            .unwrap();
        (*look, eyes(transform, player, look))
    }
}
