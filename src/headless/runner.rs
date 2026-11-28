//! The runner: [`HeadlessPlugin`] steps through the script a frame at a
//! time, feeding the game its input, and exits when it's done.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use bevy::{
    input::{
        ButtonState, InputSystems,
        keyboard::{Key, KeyboardInput},
        mouse::{MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel},
        touch::TouchPhase,
    },
    picking::{
        PickingSystems,
        input::PointerInputSettings,
        pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput},
    },
    prelude::*,
    time::TimeUpdateStrategy,
    window::{CursorMoved, PrimaryWindow},
};

use super::{
    END_TIMEOUT_FRAMES, FRAME_TIME, SETTLE_TIMEOUT_FRAMES,
    capture::{
        Capture, HeadlessTarget, create_render_target, observe, record_frame_time,
        record_system_times, render_to_image,
    },
    input::{key_for, logical_key, pointer_target},
    pipelines::{PipelineStatus, share_pipeline_status},
    script::{Button, Command, Script, ServerCommand},
    world_commands::apply_world_commands,
};
use defaria_client::{
    chat::CommandIds,
    connecting::ConnectStatus,
    fog::ChunkFog,
    net::{FromServer, ServerConnection, ToServer},
};
use defaria_core::{
    net::{ClientMessage, ServerMessage},
    streaming::StreamingStatus,
};

pub struct HeadlessPlugin {
    pub script: Script,
    /// Whether a GPU renderer exists. Must match
    /// [`default_plugins`](super::default_plugins).
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
            // What server commands are sent as and answered with (the
            // client's net adds them too, in the game).
            .add_message::<ToServer>()
            .add_message::<FromServer>()
            .init_resource::<CommandIds>()
            .add_systems(
                PreUpdate,
                (
                    take_replies,
                    run_script,
                    (send_server_commands, apply_world_commands),
                )
                    .chain()
                    .after(defaria_client::prediction::ReconcileSystems)
                    .before(InputSystems)
                    .before(PickingSystems::ProcessInput),
            )
            .add_systems(
                Last,
                (
                    record_frame_time,
                    record_system_times,
                    observe,
                    exit_when_done,
                    keep_pace_with_the_server,
                )
                    .chain()
                    .after(defaria_core::world::ChunkCompaction),
            );
        if self.rendering {
            app.add_observer(render_to_image);
            share_pipeline_status(app);
        }
    }

    fn finish(&self, app: &mut App) {
        // Once every plugin has added its systems, before any has run, and
        // while the render world is still a sub-app (pipelined rendering
        // takes it away in `cleanup`).
        let timed = (self.script.commands.iter()).any(|c| matches!(c, Command::Systems(_)));
        if timed {
            super::system_timing::time_systems(app);
        }
        // Before the first frame, when play starts and the player's camera
        // spawns (the initial state is entered before `PreStartup`).
        if self.rendering {
            app.world_mut()
                .run_system_cached(create_render_target)
                .expect("the offscreen image");
        }
    }
}

/// Give up on a `joined` after this long: connecting has ten seconds to
/// get an answer and five minutes for the mods, but a scenario's server
/// is on this machine.
const JOIN_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Resource, Default)]
pub(super) struct ScriptRunner {
    pub(super) commands: VecDeque<Command>,
    /// Waiting for the world to load before the next command, and for how
    /// many frames so far.
    pub(super) settling: Option<u32>,
    /// Frames left before the next command runs.
    pub(super) waiting: u32,
    /// `screenshot` and `grain` commands to handle at the end of this frame.
    pub(super) captures: Vec<Capture>,
    /// `inspect` commands to handle at the end of this frame.
    pub(super) inspections: Vec<String>,
    /// `camera` and `lookups` commands to apply this frame, after input.
    pub(super) world_commands: Vec<Command>,
    /// Server commands to send this frame, by their lines.
    pub(super) to_send: Vec<(String, ServerCommand)>,
    /// Server commands sent and not answered yet, oldest first.
    pub(super) awaiting: VecDeque<(u32, ServerCommand)>,
    /// Whether a `timing` report is due at the end of this frame.
    pub(super) timing: bool,
    /// Whether a `memory` report is due at the end of this frame.
    pub(super) memory_report: bool,
    /// Waiting for joining a server to finish, since when.
    pub(super) joining: Option<Instant>,
    /// When the current settle began.
    pub(super) settle_started: Option<Instant>,
    /// Voxels `fill` has changed since the last `timing`, and how long the
    /// fills took.
    pub(super) filled: (usize, Duration),
    /// `systems` reports due at the end of this frame, by how many systems.
    pub(super) system_reports: Vec<usize>,
    /// Wall-clock frame times since the last `timing`.
    pub(super) frame_times: Vec<Duration>,
    /// Captures requested but not yet delivered by the renderer.
    pub(super) pending_captures: usize,
    /// Frames since the last command ran, once there are none left.
    pub(super) frames_after_end: u32,
    /// Give up on a `settle` after this many frames:
    /// [`SETTLE_TIMEOUT_FRAMES`], but tests can wait less.
    pub(super) settle_timeout: u32,
    pub(super) rendering: bool,
    /// Where the `cursor` command last put the pointer, in the frame's
    /// pixels.
    pub(super) cursor: Vec2,
}

impl ScriptRunner {
    pub(super) fn finished(&self) -> bool {
        self.commands.is_empty()
            && self.settling.is_none()
            && self.joining.is_none()
            && self.waiting == 0
            && self.captures.is_empty()
            && self.inspections.is_empty()
            && self.world_commands.is_empty()
            && self.to_send.is_empty()
            && self.awaiting.is_empty()
            && !self.timing
            && !self.memory_report
            && self.system_reports.is_empty()
    }
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
    joining: Option<Res<ConnectStatus>>,
    mut connect_settings: Option<ResMut<crate::connecting::ConnectSettings>>,
) {
    let connecting = joining
        .as_ref()
        .is_some_and(|status| status.is_connecting());
    // Joining a server (`--connect`) happens in the background, as fast
    // as the network goes, while the menu's frames fly by: they don't
    // count towards giving up on loading.
    if runner.settling.is_some() && connecting {
        return;
    }
    if let Some(started) = runner.joining {
        if connecting {
            if started.elapsed() > JOIN_TIMEOUT {
                error!("joining a server took more than {JOIN_TIMEOUT:?}; giving up");
                exit.write(AppExit::error());
                runner.joining = None;
                runner.commands.clear();
            }
            return;
        }
        runner.joining = None;
        let frame = frame.as_ref().map_or(0, |frame| frame.0);
        let how = match joining.as_deref() {
            Some(ConnectStatus::Playing { server }) => format!("joined {}", server.address),
            Some(ConnectStatus::Failed { server, error, .. }) => {
                format!("couldn't join {}: {error}", server.address)
            }
            _ => "not joining a server".to_string(),
        };
        println!("[frame {frame}] {how}");
    }
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
            Command::Camera { .. } | Command::Lookups { .. } => runner.world_commands.push(command),
            Command::Server { line, command } => runner.to_send.push((line, command)),
            Command::Memory => runner.memory_report = true,
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
            Command::Systems(count) => runner.system_reports.push(count),
            Command::Settle => {
                runner.settling = Some(0);
                break;
            }
            Command::Joined => {
                runner.joining = Some(Instant::now());
                break;
            }
            Command::ConnectTimeout(seconds) => {
                if let Some(settings) = connect_settings.as_deref_mut() {
                    settings.timeout = Duration::from_secs_f32(seconds);
                }
            }
        }
    }
}

/// Sends this frame's server commands, as a player typing them would.
/// Without a server to send them to, the run fails.
fn send_server_commands(
    mut runner: ResMut<ScriptRunner>,
    connection: Option<Res<ServerConnection>>,
    mut to_server: MessageWriter<ToServer>,
    mut exit: MessageWriter<AppExit>,
    mut ids: ResMut<CommandIds>,
) {
    let runner = &mut *runner;
    for (line, command) in runner.to_send.drain(..) {
        if connection.is_none() {
            error!("`{line}`: there's no server to send it to");
            exit.write(AppExit::error());
            continue;
        }
        // Not the chat line's: its answer stays out of the chat.
        let id = ids.quiet();
        to_server.write(ToServer(ClientMessage::Command { id, line }));
        runner.awaiting.push_back((id, command));
    }
}

/// Takes what the server answered the commands sent: one that failed
/// fails the run; the reports (`fluids`, `network`, `list`, `help`) are
/// printed; `fill`s count towards `timing`.
fn take_replies(
    mut runner: ResMut<ScriptRunner>,
    mut messages: MessageReader<FromServer>,
    mut exit: MessageWriter<AppExit>,
    frame: Option<Res<bevy::diagnostic::FrameCount>>,
) {
    let frame = frame.map_or(0, |frame| frame.0);
    for FromServer(message) in messages.read() {
        let ServerMessage::CommandReply(reply) = message else {
            continue;
        };
        // Only the scenario's own: the chat line's go to the chat.
        let Some(at) = (runner.awaiting.iter()).position(|(id, _)| *id == reply.id) else {
            continue;
        };
        let (_, command) = runner.awaiting.remove(at).unwrap();
        if !reply.ok {
            error!("{}", reply.text);
            exit.write(AppExit::error());
            continue;
        }
        if let Some((voxels, nanos)) = reply.filled {
            runner.filled.0 += voxels as usize;
            runner.filled.1 += Duration::from_nanos(nanos);
        }
        let reports = matches!(
            command,
            ServerCommand::Fluids(_)
                | ServerCommand::Network(_)
                | ServerCommand::List
                | ServerCommand::Help
        );
        if reports {
            println!("[frame {frame}] {}", reply.text);
        }
    }
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

/// Playing on a server over the network (`--connect`), frames take at
/// least [`FRAME_TIME`] of wall-clock time. That server steps players in
/// real time, at most as fast as time passes, so a client running its
/// frames back to back makes inputs faster than they're stepped; they
/// pile up there, and those past what it keeps waiting are dropped,
/// among them a `camera` teleport. A server in the game's own process
/// steps with the client, so runs that play one go as fast as they can.
pub(super) fn keep_pace_with_the_server(
    remote: Option<Res<crate::connecting::RemoteServer>>,
    mut last: Local<Option<std::time::Instant>>,
) {
    if remote.is_none() {
        *last = None;
        return;
    }
    if let Some(last) = *last {
        let elapsed = last.elapsed();
        if elapsed < FRAME_TIME {
            std::thread::sleep(FRAME_TIME - elapsed);
        }
    }
    *last = Some(std::time::Instant::now());
}
