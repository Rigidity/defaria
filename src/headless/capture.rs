//! Captures: the offscreen image the game renders into, and what a script
//! reads off each frame: screenshots, `grain`, `timing`, `inspect` and the
//! reports.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
};

use super::{RESOLUTION, memory, runner::ScriptRunner, system_timing};

/// The offscreen image the camera renders into.
#[derive(Resource)]
pub(super) struct HeadlessTarget(pub(super) Handle<Image>);

/// As big as the primary window says, which is what
/// [`sized_plugins`](super::sized_plugins) set.
pub(super) fn create_render_target(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let size = window
        .single()
        .map_or(RESOLUTION, |window| window.physical_size());
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    // Screenshots copy out of the texture.
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    commands.insert_resource(HeadlessTarget(images.add(image)));
}

/// Points every camera at the offscreen image, since there's no window.
/// The UI draws on the camera that targets the primary window unless one is
/// marked as the default, so mark it, or the HUD wouldn't show.
pub(super) fn render_to_image(
    add: On<Add<Camera>>,
    target: Res<HeadlessTarget>,
    mut commands: Commands,
) {
    commands.entity(add.entity).insert((
        RenderTarget::Image(target.0.clone().into()),
        IsDefaultUiCamera,
    ));
}

/// Keeps each frame's wall-clock time for `timing`: from the end of the
/// last frame to the end of this one. Not from [`Time<Real>`], which steps
/// by the fixed frame time here like the other clocks.
pub(super) fn record_frame_time(
    mut last: Local<Option<Instant>>,
    mut runner: ResMut<ScriptRunner>,
) {
    let now = Instant::now();
    if let Some(last) = last.replace(now) {
        runner.frame_times.push(now - last);
    }
}

/// Reads the systems' clocks at the end of the frame, for `systems`.
pub(super) fn record_system_times(world: &mut World) {
    let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
    let took = (world.resource::<ScriptRunner>().frame_times.last().copied()).unwrap_or_default();
    system_timing::record(world, frame, took);
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
pub(super) enum Capture {
    Save(PathBuf),
    Grain,
}

/// Handles this frame's `screenshot`, `grain` and `inspect` commands, after
/// the game's systems have run.
pub(super) fn observe(world: &mut World) {
    let mut runner = world.resource_mut::<ScriptRunner>();
    let inspections = std::mem::take(&mut runner.inspections);
    let captures = std::mem::take(&mut runner.captures);
    let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
    {
        let mut runner = world.resource_mut::<ScriptRunner>();
        // Once the fills sent have been answered, so they're counted.
        if runner.timing && runner.awaiting.is_empty() {
            report_timing(&mut runner, frame);
        }
    }
    if std::mem::take(&mut world.resource_mut::<ScriptRunner>().memory_report) {
        println!("[frame {frame}] {}", memory::ChunkMemory::measure(world));
    }
    for count in std::mem::take(&mut world.resource_mut::<ScriptRunner>().system_reports) {
        for line in system_timing::report(world, count) {
            println!("[frame {frame}] {line}");
        }
    }
    for name in inspections {
        // The client's first; what only the server has (block entities,
        // networks, the simulation's state), the server's.
        let found = match inspect(world, &name) {
            Ok(lines) if lines.is_empty() => None,
            Ok(lines) => Some(Ok(lines)),
            Err(error) => Some(Err(error)),
        };
        let result = match found {
            Some(Ok(lines)) => Ok(lines),
            // A type only the server registered.
            Some(Err(error)) => {
                match crate::integrated::with_server(world, |server| inspect(server, &name)) {
                    Some(Ok(lines)) => Ok(lines),
                    _ => Err(error),
                }
            }
            None => crate::integrated::with_server(world, |server| inspect(server, &name))
                .unwrap_or(Ok(Vec::new())),
        };
        match result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_summarizes_the_frames() {
        let ms = Duration::from_millis;
        assert_eq!(
            timing_summary(&[ms(2), ms(4), ms(9)]),
            "3 frames, mean 5.00 ms, worst 9.00 ms"
        );
        assert_eq!(timing_summary(&[]), "no frames yet");
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
}
