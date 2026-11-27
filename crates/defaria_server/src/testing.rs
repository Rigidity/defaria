//! Test helpers for the server's features (and other crates' tests,
//! through `test-support`): apps that stream a small world around a
//! loader ([`world_app`]) or hold the fixed test patch of terrain
//! ([`patch_app`]), and waiting for streaming to settle ([`settle`]).
//! The ones that need no server are `defaria_core::testing`.
//!
//! When a plugin these apps add starts needing a resource or message,
//! add it here, once, rather than in each test.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use bevy::{app::Plugins, prelude::*};
use defaria_core::{
    content::Content,
    save::WorldFolder,
    streaming::{SETTLE_TIMEOUT_FRAMES, StreamingSettings, StreamingStatus},
    world::WorldPlugin,
};

use crate::{
    light::LightPlugin,
    streaming::{ChunkLoader, StreamingPlugin},
    worldgen::testing::spawn_patch,
};

/// Where [`world_app`]'s loader stands unless it's given another place.
pub const HOME: Vec3 = Vec3::new(8.0, 40.0, 8.0);

/// The world, light and streaming with no rendering, loading a
/// small world (radius 2, a chunk above and below) around a
/// [`ChunkLoader`] at [`HOME`]. Add the feature's plugins and whatever
/// else the test needs, then [`WorldApp::build`] it.
pub fn world_app(content: Content) -> WorldApp {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .insert_resource(content)
        .add_plugins((WorldPlugin, LightPlugin, StreamingPlugin))
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
/// generate or save. Panics if it never does within
/// [`SETTLE_TIMEOUT_FRAMES`]. It doesn't wait for the fog to open, as a
/// script's `settle` does.
pub fn settle(app: &mut App) {
    // Two frames first: a moved loader's transform reaches the plan a
    // frame late.
    app.update();
    app.update();
    for _ in 0..SETTLE_TIMEOUT_FRAMES {
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

/// The world and light, over the test patch of terrain
/// ([`spawn_patch`]), lit.
pub fn patch_app(content: Content) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .insert_resource(content)
        .add_plugins((WorldPlugin, LightPlugin));
    spawn_patch(app.world_mut());
    app.update();
    app
}
