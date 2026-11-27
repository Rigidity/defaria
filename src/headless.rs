//! Headless mode: run the real game with no window, driven by a script.
//!
//! `cargo headless <script>` (an alias for `cargo run -- --headless <script>`)
//! renders every frame into an offscreen image instead of a window, feeds the
//! game the input the script describes, and exits when the script ends. Time
//! advances a fixed 1/60 s per frame, so runs are deterministic.
//!
//! Frames are 1280×720 unless `--size <width>x<height>` says otherwise
//! (blending costs per pixel, so a full-screen window can cost more).
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
//! | `give <item> <count> [<fluid> <amount>]` | Put `count` of an item (named as for `hotbar`) in the player's inventory, as many as its weight limit lets in. A fluid container can be given filled, each holding `amount` blocks of a fluid (named as for `pour`): `give bucket 2 water 1`. |
//! | `network <x> <y> <z>` | Print the network the conduit at that voxel is part of: its id, kind and solver, how many conduits and chunks it has, its capacity, whether it's running or frozen (a face joined into a chunk that isn't loaded) and the machine faces (ports) it's joined to, each with its role (`produce`, `store`, `input`, ...). On an energy network, also what its storage holds and could, what producers gave and consumers took in the last second, and its throughput (its weakest wire's); on a fluid network, what it holds. Or `none`. |
//! | `put <x> <y> <z> <name> <amount> <what>` | Put something in the machine at that voxel: `amount` of an item (named as blocks are for `fill`, `stone[ore=defaria:iron]`) into its slot group `name`, or `amount` blocks of a fluid (`lava`) into its tank `name`, as a tube or a pipe would. The run fails if there's no machine there, it has no such slot group or tank, or not all of it fits. |
//! | `fluids [<x0> <y0> <z0> <x1> <y1> <z1>]` | Print how much of each fluid there is in a fixed part of the world, in thousandths of a block: the voxels and machines' tanks in the box between the two corners (both included), or without corners in the chunks the camera's position loads (its window, without the margin chunks stay loaded for after it moves on). A fluid network with a pipe in the region counts whole, as far as it's loaded, since its fluid is one shared amount, not kept per pipe. It says how many chunks that is and how many of them aren't loaded (and so weren't counted), and, for a fluid that's in pipes or tanks too, where it is, so two counts of the same box compare. Also how many voxels of fluid are active in the whole world (see `fluid`). |
//! | `camera <x> <y> <z> <yaw> <pitch>` | Put the camera (the player's eyes) at a position, turned `yaw` degrees left of north (−Z) and tilted `pitch` degrees up. The player stops there, flying, so the view stays put until input moves it. |
//! | `timing` | Print the mean and worst wall-clock frame time since the last `timing` (or the start), and if any `fill` ran since, how many voxels they changed (not ones already that block, nor ones in unloaded chunks) and how long the fills took (setting them, not relighting or remeshing, which come in the frames after). |
//! | `systems [<count>]` | Print, for the frames since the last `systems` (or the start), the busiest frame (the one whose systems used the most CPU time) and the `count` systems (12 by default) that used the most CPU in it, each with its CPU time there, its mean and worst per frame over all those frames, and its wall-clock time there; render-world systems are marked `render:`. CPU time is barely changed by other work on the machine, where wall-clock time can be. Systems run in parallel, and work they hand to other threads and their commands aren't counted, so the times don't add up to the frame's. The runner's own systems count too (an `inspect` of thousands of entities can make its frame the busiest). Only a script that uses `systems` times them (see `system_timing`). |
//! | `type <text>` | Type the text (the rest of the line), a key press and release per character, into whatever has the keyboard. |
//! | `random-ticks <n>` | Pick `n` voxels in each loaded chunk for random ticks each tick (1 by default), so blocks that grow or spread on random ticks (see `script::random_tick`) do it quickly enough to watch. |
//! | `settle` | Wait until the world around the camera has loaded and been meshed, what has gone out of reach has unloaded, and the fog has opened up to the draw distance, and print how many frames and seconds that took. |
//! | `memory` | Print what the loaded chunks hold, counted from their data rather than the process's memory, so it's the same every run: how many chunks, how many are air or one other block throughout and how many have one light level throughout, the bytes of block states, light and partial fluid, how much of that (in KiB) is in chunks that are one value throughout but still kept a value per voxel, and how many such chunks there are (0 once compaction has caught up), and the chunk meshes and the bytes of vertices and indices they keep in the main world. |
//! | `lookups <x0> <y0> <z0> <x1> <y1> <z1>` | Read every voxel in the box between the two corners twice, timing each: once by world position as most systems do ([`Voxels::get`](crate::world::Voxels::get), a hash and a query per voxel), then a chunk at a time, and print both. A box of more than 16,777,216 voxels (256 a side) fails the run. |
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

use std::time::Duration;

use bevy::{
    app::PluginGroupBuilder,
    prelude::*,
    render::{RenderPlugin, settings::WgpuSettings},
    window::{ExitCondition, WindowResolution},
    winit::WinitPlugin,
};

mod capture;
mod input;
mod memory;
mod pipelines;
mod runner;
mod script;
mod system_timing;
#[cfg(test)]
mod test_app;
#[cfg(test)]
mod tests;
mod world_commands;

pub use runner::HeadlessPlugin;
pub use script::Script;
#[cfg(test)]
pub use test_app::{run_more, settle, test_game};

/// Fixed time step for every headless frame.
pub(crate) const FRAME_TIME: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Size of the offscreen image the camera renders into, unless
/// `--size` gives another.
pub const RESOLUTION: UVec2 = UVec2::new(1280, 720);
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
    sized_plugins(rendering, RESOLUTION)
}

/// [`default_plugins`], rendering frames `size` pixels big.
pub fn sized_plugins(rendering: bool, size: UVec2) -> PluginGroupBuilder {
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
                resolution: WindowResolution::new(size.x, size.y),
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
