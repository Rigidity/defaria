//! Test helpers shared by the features' tests: apps that stream a small
//! world around a loader ([`world_app`]), hold the fixed test patch of
//! terrain ([`patch_app`]) or hold chunks the test spawns by hand
//! ([`chunk_app`], [`ChunkMapPlugin`], [`insert_chunk`]), waiting for
//! streaming to settle ([`settle`]), and the base game with a `test` mod
//! ([`test_content`]).
//!
//! When a plugin these apps add starts needing a resource or message,
//! add it here, once, rather than in each test.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use bevy::{app::Plugins, prelude::*, time::TimeUpdateStrategy};

use crate::{
    content::{
        Content, MODS_DIR,
        load::{ModFiles, read_mods},
    },
    light::LightPlugin,
    meshing::{MeshingPlugin, testing::finish_meshing},
    save::WorldFolder,
    streaming::{ChunkLoader, StreamingPlugin, StreamingSettings, StreamingStatus},
    voxel_material::BlockMaterial,
    world::{
        Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated, VoxelChanged, WorldPlugin,
    },
    worldgen::testing::spawn_patch,
};

/// Where [`world_app`]'s loader stands unless it's given another place.
pub const HOME: Vec3 = Vec3::new(8.0, 40.0, 8.0);

/// How many frames [`settle`] waits for streaming before it gives up.
const SETTLE_FRAMES: u32 = 4000;

/// The world, meshing, light and streaming with no rendering, loading a
/// small world (radius 2, a chunk above and below) around a
/// [`ChunkLoader`] at [`HOME`]. Add the feature's plugins and whatever
/// else the test needs, then [`WorldApp::build`] it.
pub fn world_app(content: Content) -> WorldApp {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .insert_resource(content)
        .add_plugins((WorldPlugin, MeshingPlugin, LightPlugin, StreamingPlugin))
        .init_asset::<Mesh>()
        .insert_resource(BlockMaterial::unloaded(Handle::default()))
        .insert_resource(StreamingSettings {
            radius: 2,
            above: 1,
            below: 1,
            ..default()
        });
    WorldApp {
        app,
        home: HOME,
        folder: None,
    }
}

/// A [`world_app`] being put together.
pub struct WorldApp {
    app: App,
    home: Vec3,
    folder: Option<PathBuf>,
}

impl WorldApp {
    /// Adds plugins, after the world's own.
    pub fn plugins<M>(mut self, plugins: impl Plugins<M>) -> Self {
        self.app.add_plugins(plugins);
        self
    }

    /// Inserts a resource, such as [`StreamingSettings`] in place of the
    /// small world's.
    pub fn resource(mut self, resource: impl Resource) -> Self {
        self.app.insert_resource(resource);
        self
    }

    /// Anything else to do to the app before it loads, such as
    /// registering a block entity part.
    pub fn with(mut self, f: impl FnOnce(&mut App)) -> Self {
        f(&mut self.app);
        self
    }

    /// Puts the loader at `home` instead of [`HOME`].
    pub fn home(mut self, home: Vec3) -> Self {
        self.home = home;
        self
    }

    /// Saves the world to `folder`, if there is one; with none, the world
    /// is only in memory.
    pub fn folder(mut self, folder: Option<&Path>) -> Self {
        self.folder = folder.map(Path::to_path_buf);
        self
    }

    /// Spawns the loader and steps the app until the world around it has
    /// loaded.
    pub fn build(self) -> App {
        let Self {
            mut app,
            home,
            folder,
        } = self;
        if let Some(folder) = folder {
            app.insert_resource(WorldFolder(folder));
        }
        app.world_mut()
            .spawn((ChunkLoader, Transform::from_translation(home)));
        settle(&mut app);
        app
    }
}

/// Steps the app until streaming has settled: nothing left to load,
/// generate or save. Panics if it never does.
pub fn settle(app: &mut App) {
    // Two frames first: a moved loader's transform reaches the plan a
    // frame late.
    app.update();
    app.update();
    for _ in 0..SETTLE_FRAMES {
        app.update();
        if app.world().resource::<StreamingStatus>().settled {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("never settled");
}

/// Moves the app's only [`ChunkLoader`] to `to` and [`settle`]s.
pub fn move_loader(app: &mut App, to: Vec3) {
    let mut loaders = app
        .world_mut()
        .query_filtered::<&mut Transform, With<ChunkLoader>>();
    loaders.single_mut(app.world_mut()).unwrap().translation = to;
    settle(app);
}

/// The world, meshing and light with no rendering, over the test patch
/// of terrain ([`spawn_patch`]), meshed.
pub fn patch_app(content: Content) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .insert_resource(content)
        .add_plugins((WorldPlugin, MeshingPlugin, LightPlugin))
        .init_asset::<Mesh>()
        .insert_resource(BlockMaterial::unloaded(Handle::default()));
    spawn_patch(app.world_mut());
    finish_meshing(&mut app);
    app
}

/// The chunk map and the messages voxel edits send: what
/// [`crate::world::Voxels`] needs, without [`WorldPlugin`]'s systems, for
/// tests that spawn their chunks by hand ([`insert_chunk`]).
pub struct ChunkMapPlugin;

impl Plugin for ChunkMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<FluidChanged>()
            .add_message::<PropertiesUpdated>();
    }
}

/// `content`, a [`ChunkMapPlugin`] and no chunks yet, stepping `step` a
/// frame.
pub fn chunk_app(content: Content, step: Duration) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(content)
        .insert_resource(TimeUpdateStrategy::ManualDuration(step))
        .add_plugins(ChunkMapPlugin);
    app
}

/// Spawns chunk `coord`, with `chunk` (a [`Chunk`], and anything that goes
/// with it), and puts it in the [`ChunkMap`].
pub fn insert_chunk(app: &mut App, coord: IVec3, chunk: impl Bundle) -> Entity {
    let world = app.world_mut();
    let entity = world.spawn((ChunkCoord(coord), chunk)).id();
    world
        .resource_mut::<ChunkMap>()
        .insert(ChunkCoord(coord), entity);
    entity
}

/// Unloads chunk `coord` as streaming would, takes it out of the map and
/// despawns it, and runs a frame. Returns its voxels.
pub fn unload_chunk(app: &mut App, coord: IVec3) -> Chunk {
    let entity = app
        .world_mut()
        .resource_mut::<ChunkMap>()
        .remove(ChunkCoord(coord))
        .unwrap();
    let chunk = app.world().get::<Chunk>(entity).unwrap().clone();
    app.world_mut().despawn(entity);
    app.update();
    chunk
}

/// The base game's mods, and mod `test` (depending on it) with `files`:
/// each a path in the mod and what's in it.
pub fn test_mods(files: &[(&str, &str)]) -> Vec<ModFiles> {
    let (mut mods, errors) = read_mods(Path::new("assets/mods"), MODS_DIR);
    assert!(errors.is_empty(), "{errors:?}");
    mods.push(ModFiles::new(
        "mods/test",
        r#"{ id: "test", name: "Test", dependencies: ["defaria"] }"#,
        files,
    ));
    mods
}

/// The content of [`test_mods`], which must load without errors.
pub fn test_content(files: &[(&str, &str)]) -> Content {
    let content = Content::from_mods(test_mods(files));
    assert!(content.errors.is_empty(), "{:#?}", content.errors);
    content
}
