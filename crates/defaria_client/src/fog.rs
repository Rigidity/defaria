//! Distance fog that hides the edge of the drawn world.
//!
//! The fog is a cylinder around the camera, like the loaded world (see
//! `streaming`): it thickens toward the cylinder's side, top and bottom,
//! and hides everything beyond them. Its full size, [`FogShape`], follows
//! the camera smoothly and always fits inside what's drawn once the world
//! around the camera has loaded, with a chunk to spare. So moving into
//! another chunk, sideways or up and down, doesn't bring chunks that
//! haven't loaded yet inside it: the chunks that start loading then are
//! already in the fog.
//!
//! When chunks inside the cylinder aren't drawn (after a jump, at the start,
//! or moving faster than the world loads), the fog shrinks the cylinder
//! until the nearest one is just outside it, and opens back up as they
//! arrive. Chunks load nearest first, so it opens up steadily. It closes in
//! quickly and opens up slowly ([`CLOSE_SECONDS`], [`OPEN_SECONDS`]).
//!
//! The fog is Bevy's [`DistanceFog`] on the camera, so it reaches the chunk
//! shader in the view's bindings rather than in the material, which would
//! make every chunk be prepared again each frame it changes. Bevy's falloffs
//! don't have a cylinder, so the shape travels in the
//! [`FogFalloff::Atmospheric`] fields, which are passed to the shader as
//! they are (see [`apply_fog`] and `assets/shaders/voxel.wesl`). The colour
//! is the sky's ([`ClearColor`]), so the world fades into it.

use std::f32::consts::SQRT_2;

use bevy::{pbr::FogFalloff, prelude::*};

use crate::{fly_camera::EyeView, meshing::Meshed};
use defaria_core::{
    fluid::FluidView,
    streaming::StreamingSettings,
    world::{CHUNK_SIZE, ChunkCoord, ChunkMap},
};

/// The fog starts this far out as a fraction of how far it reaches.
pub const FOG_START: f32 = 0.7;
/// The fog never shrinks below this fraction of its full size, so what's
/// right around the camera stays visible while the world loads.
pub const MIN_SCALE: f32 = 0.15;
/// How quickly the fog closes in on chunks that aren't drawn: the time
/// constant of its approach, in seconds.
pub const CLOSE_SECONDS: f32 = 0.1;
/// How quickly the fog opens up as chunks are drawn, likewise.
pub const OPEN_SECONDS: f32 = 0.6;
/// The fog counts as having reached its size within this fraction of it,
/// and snaps there.
const SNAP: f32 = 0.002;

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        // On the camera from the start, so its first frame is fogged and its
        // pipelines are only built with fog.
        app.register_required_components_with::<EyeView, DistanceFog>(|| DistanceFog {
            falloff: falloff(&ChunkFog::default()),
            ..default()
        })
        .init_resource::<ChunkFog>()
        .add_systems(
            PostUpdate,
            // After transforms propagate, so the fog is measured from
            // where the camera is this frame.
            (follow_the_drawn_world, apply_fog)
                .chain()
                .in_set(FogSystems)
                .after(TransformSystems::Propagate),
        )
        .add_systems(PostUpdate, fog_inside_fluid.after(FogSystems));
    }
}

/// Where the fog is put on the cameras, in [`PostUpdate`]. Anything that
/// changes it, such as the view from inside water, runs after.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FogSystems;

/// How far the fog reaches from the camera at its full size, in blocks: the
/// cylinder that ends it.
#[derive(Reflect, Debug, Clone, Copy, PartialEq)]
pub struct FogShape {
    /// Sideways: the cylinder's radius.
    pub sideways: f32,
    /// Up to its top.
    pub up: f32,
    /// Down to its bottom.
    pub down: f32,
}

impl FogShape {
    /// The biggest cylinder that stays inside what `settings` draw wherever
    /// the camera is in its chunk, with a chunk to spare all round. It's
    /// never less than a chunk each way, so with tiny settings (a radius of
    /// 4 or less, or 2 or fewer levels up or down) it doesn't quite fit.
    ///
    /// Drawn chunks are those within `radius - 1` chunks of the camera's
    /// chunk sideways (see [`StreamingSettings::drawn`]). A point within
    /// `d` blocks of the camera is in a chunk less than `d / 16 + 1` chunks
    /// away on each axis, so up to `d / 16 + √2` chunks away diagonally:
    /// `d` up to `radius - 1 - √2` chunks fits. The spare chunk keeps the
    /// chunks that start loading when the camera moves into the next chunk
    /// outside: they're at least a chunk nearer the edge than those were.
    pub fn new(settings: &StreamingSettings) -> Self {
        let size = CHUNK_SIZE as f32;
        let levels = settings.drawn_levels();
        Self {
            sideways: (settings.radius as f32 - 2.0 - SQRT_2).max(1.0) * size,
            // The camera's chunk is drawn up to its top, `above - 1` chunks
            // up, so `above - 1` chunks above the camera are always drawn,
            // less one to spare; likewise below.
            up: (*levels.end() as f32 - 1.0).max(1.0) * size,
            down: (-*levels.start() as f32 - 1.0).max(1.0) * size,
        }
    }

    /// How far `offset` from the camera is through the cylinder: 1 on its
    /// surface, more outside.
    pub fn depth(&self, offset: Vec3) -> f32 {
        (offset.xz().length() / self.sideways)
            .max(offset.y / self.up)
            .max(-offset.y / self.down)
    }
}

/// The fog's size and shape.
#[derive(Resource, Reflect, Debug, Clone, Copy, PartialEq)]
#[reflect(Resource)]
pub struct ChunkFog {
    /// Its full size.
    pub shape: FogShape,
    /// How big it is now, as a fraction of [`Self::shape`].
    pub scale: f32,
    /// How big it's heading for: as far as the world is drawn.
    pub target: f32,
}

impl Default for ChunkFog {
    /// As close as it gets: nothing's drawn yet.
    fn default() -> Self {
        Self {
            shape: FogShape::new(&StreamingSettings::default()),
            scale: MIN_SCALE,
            target: MIN_SCALE,
        }
    }
}

impl ChunkFog {
    /// Whether the fog has reached the edge of the drawn world.
    pub fn settled(&self) -> bool {
        self.scale == self.target
    }

    /// Moves the fog toward `target` over `seconds`.
    pub fn approach(&mut self, target: f32, seconds: f32) {
        self.target = target;
        let tau = if target < self.scale {
            CLOSE_SECONDS
        } else {
            OPEN_SECONDS
        };
        self.scale += (target - self.scale) * (1.0 - (-seconds / tau).exp());
        if (self.scale - target).abs() < SNAP {
            self.scale = target;
        }
    }
}

/// How far the drawn world reaches from `camera`, as a fraction of `shape`
/// (at most 1): how far through it the nearest chunk that the settings draw
/// but isn't drawn yet is (by its nearest point).
pub fn drawn_reach(
    camera: Vec3,
    shape: &FogShape,
    settings: &StreamingSettings,
    drawn: impl Fn(IVec3) -> bool,
) -> f32 {
    let size = CHUNK_SIZE as f32;
    // How far `at` is from chunk `low`'s span along one axis: negative
    // below it, positive above, zero inside.
    let gap = |low: i32, at: f32| {
        let (low, high) = (low as f32 * size, (low + 1) as f32 * size);
        if at < low {
            low - at
        } else if at > high {
            high - at
        } else {
            0.0
        }
    };
    let (center, _) = ChunkCoord::split(camera.floor().as_ivec3());
    let mut reach = 1.0f32;
    let radius = settings.radius;
    for x in -radius..=radius {
        for z in -radius..=radius {
            if !settings.drawn(IVec3::new(x, 0, z)) {
                continue;
            }
            let column = center.0 + IVec3::new(x, 0, z);
            let sideways = Vec2::new(gap(column.x, camera.x), gap(column.z, camera.z));
            if shape.depth(sideways.extend(0.0).xzy()) >= reach {
                continue;
            }
            for y in settings.drawn_levels() {
                let chunk = column + IVec3::new(0, y, 0);
                let offset = Vec3::new(sideways.x, gap(chunk.y, camera.y), sideways.y);
                let depth = shape.depth(offset);
                if depth < reach && !drawn(chunk) {
                    reach = depth;
                }
            }
        }
    }
    reach
}

/// Moves [`ChunkFog`] toward the edge of the drawn world.
///
/// Once the fog is fully open it stays open until the camera moves into
/// another chunk, a chunk goes or the streaming settings change (chunks
/// are only ever drawn more, never less, while they're loaded), so the scan
/// is skipped until then.
fn follow_the_drawn_world(
    camera: Query<&GlobalTransform, With<EyeView>>,
    settings: Res<StreamingSettings>,
    map: Res<ChunkMap>,
    meshed: Query<(), With<Meshed>>,
    mut unmeshed: RemovedComponents<Meshed>,
    time: Res<Time>,
    mut fog: ResMut<ChunkFog>,
    mut last_chunk: Local<Option<IVec3>>,
) {
    let Some(camera) = camera.iter().next() else {
        return;
    };
    let chunk = ChunkCoord::split(camera.translation().floor().as_ivec3())
        .0
        .0;
    let removed = unmeshed.read().count() > 0;
    let moved = last_chunk.replace(chunk) != Some(chunk);
    if fog.scale == 1.0 && fog.settled() && !moved && !removed && !settings.is_changed() {
        return;
    }
    fog.shape = FogShape::new(&settings);
    let reach = drawn_reach(camera.translation(), &fog.shape, &settings, |chunk| {
        map.get(ChunkCoord(chunk))
            .is_some_and(|entity| meshed.contains(entity))
    });
    fog.approach(reach.max(MIN_SCALE), time.delta_secs());
}

/// The [`DistanceFog`] falloff that carries `fog` to the shader.
pub fn falloff(fog: &ChunkFog) -> FogFalloff {
    let FogShape { sideways, up, down } = fog.shape;
    cylinder_falloff(Vec3::new(sideways, up, down) * fog.scale, FOG_START)
}

/// The [`DistanceFog`] falloff for fog that's clear up to `start` blocks
/// from the camera and thick from `end`, in every direction.
pub fn falloff_between(start: f32, end: f32) -> FogFalloff {
    cylinder_falloff(Vec3::splat(end), start / end)
}

/// The falloff for fog ending at a cylinder `end` blocks out sideways, up
/// and down, and starting `start` of the way there. Bevy passes the
/// atmospheric falloff's two vectors to the shader unchanged, where they're
/// read as `be` and `bi` (see `assets/shaders/voxel.wesl`): this is that
/// encoding, not Bevy's atmospheric fog.
fn cylinder_falloff(end: Vec3, start: f32) -> FogFalloff {
    FogFalloff::Atmospheric {
        extinction: end,
        inscattering: Vec3::new(start, 0.0, 0.0),
    }
}

/// Puts [`ChunkFog`] on the cameras, in the sky's colour.
fn apply_fog(
    fog: Res<ChunkFog>,
    sky: Res<ClearColor>,
    mut views: Query<&mut DistanceFog, With<EyeView>>,
) {
    for mut view in &mut views {
        view.color = sky.0;
        view.falloff = falloff(&fog);
    }
}

/// With the eyes inside a fluid, the fog is the fluid's: its color, from
/// right at the eyes out to its `view.distance`, in place of the fog at the
/// edge of the world.
fn fog_inside_fluid(
    fluids: FluidView,
    mut views: Query<(&GlobalTransform, &mut DistanceFog), With<EyeView>>,
) {
    for (transform, mut fog) in &mut views {
        let inside = fluids.fluid_at(transform.translation());
        let Some(fluid) = inside.and_then(|kind| fluids.fluid(kind)) else {
            continue;
        };
        fog.color = fluid.view_color;
        fog.falloff = falloff_between(0.0, fluid.view_distance);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::{platform::collections::HashSet, time::TimeUpdateStrategy};

    use super::*;
    use defaria_core::{
        content::{FULL_FLUID, base, block},
        testing::insert_chunk,
        world::{Chunk, VoxelFluid},
    };

    fn settings() -> StreamingSettings {
        StreamingSettings {
            radius: 6,
            above: 4,
            below: 5,
            ..default()
        }
    }

    /// Every chunk the settings draw around chunk `center`.
    fn everything(settings: &StreamingSettings, center: IVec3) -> HashSet<IVec3> {
        let r = settings.radius;
        let mut chunks = HashSet::new();
        for x in -r..=r {
            for z in -r..=r {
                for y in settings.drawn_levels() {
                    let offset = IVec3::new(x, y, z);
                    if settings.drawn(offset) {
                        chunks.insert(center + offset);
                    }
                }
            }
        }
        chunks
    }

    #[test]
    fn the_shape_fits_inside_the_drawn_world() {
        let settings = settings();
        let shape = FogShape::new(&settings);
        let size = CHUNK_SIZE as f32;
        assert_eq!(shape.up, 2.0 * size);
        assert_eq!(shape.down, 3.0 * size);
        assert_eq!(shape.sideways, (4.0 - SQRT_2) * size);

        // Every point inside it is in a drawn chunk, wherever the camera
        // is in its chunk.
        let drawn = everything(&settings, IVec3::ZERO);
        for camera in [Vec3::ZERO, Vec3::splat(8.0), Vec3::splat(15.99)] {
            for step in 0..64 {
                let angle = step as f32 / 64.0 * std::f32::consts::TAU;
                for offset in [
                    Vec3::new(angle.cos(), 0.0, angle.sin()) * shape.sideways,
                    Vec3::new(angle.cos() * shape.sideways, shape.up, 0.0),
                    Vec3::new(0.0, -shape.down, angle.sin() * shape.sideways),
                ] {
                    let point = (camera + offset * 0.999).floor().as_ivec3();
                    let (chunk, _) = ChunkCoord::split(point);
                    assert!(drawn.contains(&chunk.0), "{camera} + {offset}");
                }
            }
        }
    }

    #[test]
    fn with_everything_drawn_the_fog_is_full_size() {
        let settings = settings();
        let shape = FogShape::new(&settings);
        let drawn = everything(&settings, IVec3::ZERO);
        let reach = drawn_reach(Vec3::splat(8.0), &shape, &settings, |c| drawn.contains(&c));
        assert_eq!(reach, 1.0);
    }

    #[test]
    fn moving_into_the_next_chunk_doesnt_bring_the_fog_in() {
        let settings = settings();
        let shape = FogShape::new(&settings);
        let size = CHUNK_SIZE as f32;
        // Drawn around chunk (0, 0, 0), but the camera has just moved into
        // the next chunk up, east, or down, where more is wanted.
        let drawn = everything(&settings, IVec3::ZERO);
        for camera in [
            Vec3::new(8.0, size + 0.01, 8.0),
            Vec3::new(size + 0.01, 8.0, 8.0),
            Vec3::new(8.0, -0.01, 8.0),
        ] {
            let reach = drawn_reach(camera, &shape, &settings, |c| drawn.contains(&c));
            assert_eq!(reach, 1.0, "at {camera}");
        }
    }

    #[test]
    fn the_fog_stops_at_the_nearest_chunk_not_drawn() {
        let settings = settings();
        let shape = FogShape::new(&settings);
        let mut drawn = everything(&settings, IVec3::ZERO);
        // A chunk two columns east, at the camera's level, isn't drawn yet:
        // 24 blocks away sideways.
        drawn.remove(&IVec3::new(2, 0, 0));
        let camera = Vec3::splat(8.0);
        let reach = drawn_reach(camera, &shape, &settings, |c| drawn.contains(&c));
        assert_eq!(reach, 24.0 / shape.sideways);

        // One straight above, a level up, is 8 blocks up.
        drawn.insert(IVec3::new(2, 0, 0));
        drawn.remove(&IVec3::new(0, 1, 0));
        let reach = drawn_reach(camera, &shape, &settings, |c| drawn.contains(&c));
        assert_eq!(reach, 8.0 / shape.up);
    }

    #[test]
    fn with_nothing_drawn_the_fog_is_at_the_camera() {
        let settings = settings();
        let shape = FogShape::new(&settings);
        let reach = drawn_reach(Vec3::splat(8.0), &shape, &settings, |_| false);
        assert_eq!(reach, 0.0);
    }

    #[test]
    fn the_fog_closes_quickly_and_opens_slowly() {
        let shape = FogShape::new(&settings());
        let at = |scale: f32| ChunkFog {
            shape,
            scale,
            target: scale,
        };

        // One step, against the exponential approach it should follow.
        let mut fog = at(1.0);
        fog.approach(0.5, 0.1);
        let closed = 1.0 - 0.5 * (1.0 - (-0.1 / CLOSE_SECONDS).exp());
        assert!((fog.scale - closed).abs() < 1e-5, "{}", fog.scale);
        assert!(!fog.settled());
        let mut fog = at(0.5);
        fog.approach(1.0, 0.1);
        let opened = 0.5 + 0.5 * (1.0 - (-0.1 / OPEN_SECONDS).exp());
        assert!((fog.scale - opened).abs() < 1e-5, "{}", fog.scale);

        // The same however the time is split into frames.
        for target in [0.5, 1.0] {
            let (mut once, mut twice) = (at(0.75), at(0.75));
            once.approach(target, 0.1);
            twice.approach(target, 0.05);
            twice.approach(target, 0.05);
            assert!((once.scale - twice.scale).abs() < 1e-6);
        }

        // It gets there in the end, exactly.
        for _ in 0..600 {
            fog.approach(1.0, 1.0 / 60.0);
        }
        assert!(fog.settled());
        assert_eq!(fog.scale, 1.0);

        // Where it already is, nothing happens.
        let mut fog = at(0.4);
        fog.approach(0.4, 0.1);
        assert_eq!(fog, at(0.4));
    }

    /// The fog plugin around a camera in chunk (0, 0, 0), with `settings`,
    /// stepping 16 ms a frame; and its eye view.
    fn fog_app(settings: StreamingSettings) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin))
            .insert_resource(settings)
            .insert_resource(base().clone())
            .init_resource::<ChunkMap>()
            .insert_resource(ClearColor(Color::srgb(0.1, 0.2, 0.3)))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                16,
            )))
            .add_plugins(FogPlugin);
        // The eyes the view is a child of.
        let eyes = app
            .world_mut()
            .spawn(Transform::from_xyz(8.0, 8.0, 8.0))
            .id();
        let view = app.world_mut().spawn((EyeView, ChildOf(eyes))).id();
        (app, view)
    }

    /// Spawns a drawn chunk entity at each of `chunks`, in the chunk map.
    fn draw(app: &mut App, chunks: impl IntoIterator<Item = IVec3>) {
        for chunk in chunks {
            insert_chunk(app, chunk, Meshed);
        }
    }

    fn view_fog(app: &App, view: Entity) -> (Color, Vec3, f32) {
        let fog = app.world().get::<DistanceFog>(view).unwrap();
        let FogFalloff::Atmospheric {
            extinction,
            inscattering,
        } = fog.falloff
        else {
            panic!("the shape travels in the atmospheric falloff");
        };
        (fog.color, extinction, inscattering.x)
    }

    #[test]
    fn the_camera_has_fog_from_the_start() {
        let (app, view) = fog_app(settings());
        // Before any update: a required component, closed in.
        let shape = FogShape::new(&StreamingSettings::default());
        let (_, extinction, start) = view_fog(&app, view);
        assert_eq!(
            extinction,
            Vec3::new(shape.sideways, shape.up, shape.down) * MIN_SCALE
        );
        assert_eq!(start, FOG_START);
    }

    #[test]
    fn the_fog_opens_as_chunks_are_drawn_and_closes_on_a_gap() {
        let settings = settings();
        let shape = FogShape::new(&settings);
        let (mut app, view) = fog_app(settings.clone());
        app.update();
        app.update();
        // Nothing's drawn: as close as it gets, in the sky's colour.
        let fog = *app.world().resource::<ChunkFog>();
        assert_eq!((fog.scale, fog.target), (MIN_SCALE, MIN_SCALE));
        let (color, extinction, _) = view_fog(&app, view);
        assert_eq!(color, Color::srgb(0.1, 0.2, 0.3));
        let full = Vec3::new(shape.sideways, shape.up, shape.down);
        assert_eq!(extinction, full * MIN_SCALE);

        // Everything's drawn: it opens all the way, a little each frame,
        // and the camera's fog follows.
        draw(&mut app, everything(&settings, IVec3::ZERO));
        app.update();
        let first = app.world().resource::<ChunkFog>().scale;
        assert!(first > MIN_SCALE && first < 1.0, "{first}");
        assert_eq!(view_fog(&app, view).1, full * first);
        for _ in 0..600 {
            app.update();
        }
        let fog = *app.world().resource::<ChunkFog>();
        assert_eq!(fog.scale, 1.0);
        assert!(fog.settled());
        assert_eq!(view_fog(&app, view).1, full);

        // A chunk a level up isn't drawn any more (loaded, but not meshed):
        // the fog closes in to it, 8 blocks up.
        let gap = IVec3::new(0, 1, 0);
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(gap))
            .unwrap();
        app.world_mut().entity_mut(entity).remove::<Meshed>();
        app.update();
        let fog = *app.world().resource::<ChunkFog>();
        assert_eq!(fog.target, 8.0 / shape.up);
        assert!(fog.scale < 1.0);

        // And the sky's colour is followed.
        app.insert_resource(ClearColor(Color::srgb(0.5, 0.5, 0.5)));
        app.update();
        assert_eq!(view_fog(&app, view).0, Color::srgb(0.5, 0.5, 0.5));
    }

    /// Opens the fog all the way around a camera in chunk (0, 0, 0).
    fn opened(settings: &StreamingSettings) -> (App, Entity) {
        let (mut app, view) = fog_app(settings.clone());
        draw(&mut app, everything(settings, IVec3::ZERO));
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(app.world().resource::<ChunkFog>().scale, 1.0);
        (app, view)
    }

    /// Moving on faster than the world loads: three chunks east, nothing
    /// there is drawn, and the fog closes in.
    #[test]
    fn the_fog_looks_again_when_the_camera_changes_chunk() {
        let (mut app, view) = opened(&settings());
        let eyes = app.world().get::<ChildOf>(view).unwrap().parent();
        app.world_mut()
            .get_mut::<Transform>(eyes)
            .unwrap()
            .translation
            .x += 48.0;
        app.update();
        let fog = *app.world().resource::<ChunkFog>();
        assert!(fog.target < 1.0, "{fog:?}");
    }

    /// Loading further sends the fog further, once that's drawn.
    #[test]
    fn the_fog_follows_the_settings() {
        let settings = settings();
        let (mut app, _) = opened(&settings);
        let further = StreamingSettings {
            radius: settings.radius + 2,
            ..settings
        };
        app.insert_resource(further.clone());
        app.update();
        let fog = *app.world().resource::<ChunkFog>();
        assert_eq!(fog.shape, FogShape::new(&further));
        assert!(fog.target < 1.0, "the new ring isn't drawn yet: {fog:?}");
    }

    /// Fog between two distances ends at the far one in every direction,
    /// and starts the near one's share of the way there.
    #[test]
    fn fog_between_two_distances() {
        let FogFalloff::Atmospheric {
            extinction,
            inscattering,
        } = falloff_between(2.0, 8.0)
        else {
            panic!("not the cylinder encoding");
        };
        assert_eq!(extinction, Vec3::splat(8.0));
        assert_eq!(inscattering.x, 0.25);
    }

    /// With the eyes under a fluid's surface, the fog is the fluid's; above
    /// it, or in a voxel's empty top, it's left alone.
    #[test]
    fn the_fog_inside_fluid_is_the_fluids() {
        let mut app = defaria_core::testing::chunk_app(base().clone(), Duration::from_millis(50));
        app.add_systems(Update, fog_inside_fluid);
        let mut chunk = Chunk::default();
        let half = VoxelFluid::of(block("water"), FULL_FLUID / 2);
        chunk.set_fluid_at(IVec3::splat(4), half, FULL_FLUID);
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        let fog_at = |app: &mut App, y: f32| {
            let eye = app
                .world_mut()
                .spawn((
                    EyeView,
                    GlobalTransform::from_translation(Vec3::new(4.5, y, 4.5)),
                    DistanceFog::default(),
                ))
                .id();
            app.update();
            let fog = app.world().get::<DistanceFog>(eye).unwrap().clone();
            app.world_mut().despawn(eye);
            (fog.color, fog.falloff)
        };
        let water = base().state(block("water")).fluid.clone().unwrap();
        assert_eq!(fog_at(&mut app, 4.25).0, water.view_color);
        assert_eq!(
            format!("{:?}", fog_at(&mut app, 4.25).1),
            format!("{:?}", falloff_between(0.0, water.view_distance))
        );
        assert_eq!(fog_at(&mut app, 4.75).0, DistanceFog::default().color);
        assert_eq!(fog_at(&mut app, 3.5).0, DistanceFog::default().color);
    }
}
