//! Test helpers shared by the features' tests (and other crates' tests,
//! through `test-support`): apps that hold chunks the test spawns by hand
//! ([`chunk_app`], [`ChunkMapPlugin`], [`insert_chunk`]), the base game
//! with a `test` mod ([`test_content`]), and a fresh folder to save in
//! ([`temp_folder`]). Apps that stream or generate the world are the
//! server's (`defaria_server::testing`).
//!
//! When a plugin these apps add starts needing a resource or message,
//! add it here, once, rather than in each test.

use std::time::Duration;

use bevy::{prelude::*, time::TimeUpdateStrategy};

use crate::{
    content::{
        Content, MODS_DIR,
        load::{ModFiles, read_mods},
    },
    world::{Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated, VoxelChanged},
};

/// The chunk map and the messages voxel edits send: what
/// [`crate::world::Voxels`] needs, without [`WorldPlugin`](crate::world::WorldPlugin)'s systems, for
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
    let (mut mods, errors) = read_mods(&crate::content::mods_dir(), MODS_DIR);
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

/// A fresh, empty folder under `target/`, unique to this test.
pub fn temp_folder(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("defaria-tests/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}
