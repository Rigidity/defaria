//! Drawing chunks: building their meshes and showing them, plus the
//! world's sky color.
//!
//! Chunks are meshed by [`remesh_changed_chunks`] whenever they or their
//! light change, including when they're spawned, so editing a chunk through
//! [`Voxels`](crate::world::Voxels) is all it takes to update what's drawn. Meshes are built from
//! copies of a chunk's blocks and light and the voxels around it: a chunk's
//! first mesh on the async task pool, over as many frames as it takes, and
//! later ones (after edits, which should show at once) in parallel on the
//! same frame. A chunk marked [`AwaitingNeighbours`] isn't meshed until the
//! mark comes off, since its faces against the missing chunks would be
//! wrong.
//!
//! A mesh is built from a [`Neighborhood`] (see [`build`]). Opaque faces
//! are drawn on the chunk's own entity, cut-out and translucent ones on
//! child entities ([`ChunkParts`]), with the translucent faces sorted back
//! to front ([`sort_translucent_faces`]).
//!
//! The main world keeps no copy of a chunk's mesh once it's on the GPU
//! ([`RenderAssetUsages::RENDER_WORLD`]): nothing reads a chunk's vertices
//! back after it's drawn, and the copies were the largest memory a loaded
//! chunk cost. The exception is a translucent mesh whose faces need
//! sorting: putting them in order rewrites its indices, and Bevy can only
//! change a mesh's data in the main world, so that mesh keeps its copy.

use std::sync::Arc;

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    platform::collections::HashSet,
    prelude::*,
    tasks::{AsyncComputeTaskPool, ComputeTaskPool, Task, futures::check_ready},
};

use crate::{
    content::Content,
    light::ChunkLight,
    perlin::splitmix64,
    player::{Eyes, LocalPlayer},
    voxel_material::{BlockMaterial, VoxelMaterial},
    world::{CHUNK_SIZE, Chunk, ChunkCoord, ChunkMap},
};

mod build;

pub use build::{ATTRIBUTE_CORNER, ATTRIBUTE_FACE, ATTRIBUTE_FLOW, ATTRIBUTE_LIGHT};
pub use build::{BACK_FACE, MARGIN};
use build::{ChunkMeshes, Neighborhood, TranslucentFaces};

pub struct MeshingPlugin;

impl Plugin for MeshingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, set_sky_color).add_systems(
            Update,
            (
                remesh_changed_chunks,
                apply_finished_meshes,
                sort_translucent_faces,
            )
                .chain()
                .in_set(RemeshChunks),
        );
    }
}

/// Where changed chunks are relit and remeshed. Systems that edit chunks
/// run before it, so their edits show on the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemeshChunks;

/// A chunk that isn't meshed yet because a chunk around it isn't loaded.
/// Whatever loads the chunks takes it off once they are, and the chunk is
/// meshed then. Chunks without it are meshed as soon as they spawn.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct AwaitingNeighbours;

/// A chunk that's been meshed at least once (it may have no mesh, if
/// nothing in it shows).
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Meshed;

/// A mesh being built on the async task pool. Replacing or removing it
/// cancels the build.
#[derive(Component)]
pub struct MeshTask(Task<ChunkMeshes>);

impl MeshTask {
    /// The mesh is built and waiting to be shown.
    #[cfg(test)]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

/// Rebuilds the mesh of every chunk that was spawned or changed, or whose
/// light changed, since the last run, and of the chunks around each one that
/// was spawned (their faces against it are culled and shaded differently
/// now). Chunks [`AwaitingNeighbours`] are left for when they aren't.
///
/// A chunk that's been meshed before is remeshed right away, all of them in
/// parallel, so edits show on the frame they're made. A chunk's first mesh
/// (or its first since its neighbours came back) is built on the async task
/// pool instead, so loading a lot of chunks doesn't hold up the frame;
/// [`apply_finished_meshes`] picks the results up.
pub fn remesh_changed_chunks(
    mut commands: Commands,
    chunks: Query<(
        Entity,
        &ChunkCoord,
        Ref<Chunk>,
        Ref<ChunkLight>,
        Has<Meshed>,
        Has<AwaitingNeighbours>,
    )>,
    mut completed: RemovedComponents<AwaitingNeighbours>,
    map: Res<ChunkMap>,
    parts: Query<&ChunkParts>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<BlockMaterial>,
    content: Res<Content>,
    mut shared: Local<Option<Arc<Content>>>,
) {
    let mut dirty = HashSet::new();
    // Chunks to mesh in the background even though they've been meshed.
    let mut background = HashSet::new();
    for entity in completed.read() {
        if let Ok((_, coord, ..)) = chunks.get(entity) {
            dirty.insert(*coord);
            background.insert(*coord);
        }
    }
    for (_, coord, chunk, light, ..) in &chunks {
        if chunk.is_changed() || light.is_changed() {
            dirty.insert(*coord);
        }
        if chunk.is_added() {
            dirty.extend(coord.neighbours().filter(|n| map.get(*n).is_some()));
        }
    }
    if dirty.is_empty() {
        return;
    }
    let mut now = Vec::new();
    let mut later = Vec::new();
    for coord in dirty {
        let Some(entity) = map.get(coord) else {
            continue;
        };
        let Ok((_, _, _, _, meshed, awaiting)) = chunks.get(entity) else {
            continue;
        };
        if awaiting {
            continue;
        }
        let neighborhood = Neighborhood::gather(|offset| {
            let (_, _, chunk, light, ..) =
                chunks.get(map.get(ChunkCoord(coord.0 + offset))?).ok()?;
            Some((chunk.into_inner(), light.into_inner()))
        });
        if meshed && !background.contains(&coord) {
            now.push((entity, neighborhood));
        } else {
            later.push((entity, neighborhood));
        }
    }

    if !later.is_empty() {
        let shared = shared.get_or_insert_with(|| Arc::new(content.clone()));
        let pool = AsyncComputeTaskPool::get();
        for (entity, neighborhood) in later {
            let content = shared.clone();
            let task = pool.spawn(async move { neighborhood.meshes(&content) });
            commands.entity(entity).insert(MeshTask(task));
        }
    }

    let built = ComputeTaskPool::get().scope(|scope| {
        for (entity, neighborhood) in &now {
            let content = &content;
            scope.spawn(async move { (*entity, neighborhood.meshes(content)) });
        }
    });
    for (entity, built) in built {
        commands.entity(entity).remove::<MeshTask>();
        let parts = parts.get(entity).ok();
        show_meshes(&mut commands, entity, built, parts, &mut meshes, &material);
    }
}

/// Shows the meshes finished on the async task pool.
pub fn apply_finished_meshes(
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut MeshTask, Option<&ChunkParts>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<BlockMaterial>,
) {
    for (entity, mut task, parts) in &mut tasks {
        if let Some(built) = check_ready(&mut task.0) {
            commands.entity(entity).remove::<MeshTask>();
            show_meshes(&mut commands, entity, built, parts, &mut meshes, &material);
        }
    }
}

/// The child entities that draw a chunk's see-through faces: its own
/// entity draws the opaque ones. Spawned the first time the chunk has any.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct ChunkParts {
    pub cutout: Entity,
    pub translucent: Entity,
}

/// Where the eyes were, in its chunk-local [`TranslucentFaces`] sort's
/// steps, when a translucent mesh's faces were last put in order. `None`
/// until they first are.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq)]
#[reflect(Component)]
pub struct SortedFrom(Option<IVec3>);

fn show_meshes(
    commands: &mut Commands,
    entity: Entity,
    built: ChunkMeshes,
    parts: Option<&ChunkParts>,
    meshes: &mut Assets<Mesh>,
    material: &BlockMaterial,
) {
    let ChunkMeshes {
        opaque,
        cutout,
        translucent,
        translucent_faces,
    } = built;
    commands.entity(entity).insert(Meshed);
    show_mesh(
        commands,
        entity,
        render_only(opaque),
        meshes,
        &material.material,
    );
    let see_through = cutout.count_vertices() + translucent.count_vertices() > 0;
    let parts = match parts {
        Some(parts) => *parts,
        // Most chunks never need them.
        None if !see_through => return,
        None => {
            let mut part = |name: &str| {
                commands
                    .spawn((
                        Name::new(name.to_string()),
                        Transform::default(),
                        Visibility::default(),
                        ChildOf(entity),
                    ))
                    .id()
            };
            let parts = ChunkParts {
                cutout: part("Cut-out faces"),
                translucent: part("Translucent faces"),
            };
            commands.entity(entity).insert(parts);
            parts
        }
    };
    show_mesh(
        commands,
        parts.cutout,
        render_only(cutout),
        meshes,
        &material.cutout,
    );
    // Sorting rewrites the indices in the main world's copy.
    let translucent = if translucent_faces.needs_sorting() {
        translucent
    } else {
        render_only(translucent)
    };
    show_mesh(
        commands,
        parts.translucent,
        translucent,
        meshes,
        &material.translucent,
    );
    commands
        .entity(parts.translucent)
        .insert((translucent_faces, SortedFrom::default()));
}

/// `mesh`, with its data handed to the GPU and dropped from the main
/// world once it's drawn. Until then (and always, in an app that doesn't
/// render) it's readable as any mesh.
fn render_only(mut mesh: Mesh) -> Mesh {
    mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
    mesh
}

/// Draws `mesh` on `entity`, or nothing if it has no faces: chunks with
/// nothing visible (all air, or buried) get no mesh, since the renderer
/// warns about meshes with no vertices.
fn show_mesh(
    commands: &mut Commands,
    entity: Entity,
    mesh: Mesh,
    meshes: &mut Assets<Mesh>,
    material: &Handle<VoxelMaterial>,
) {
    let mut part = commands.entity(entity);
    if mesh.count_vertices() > 0 {
        part.insert((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone())));
    } else {
        part.remove::<(Mesh3d, MeshMaterial3d<VoxelMaterial>)>();
    }
}

/// How far, in blocks, the eyes can move before a translucent mesh this
/// far away is sorted again: every block nearby, where the order changes
/// most, and every chunk farther out.
fn sort_step(distance: f32) -> i32 {
    if distance < 2.0 * CHUNK_SIZE as f32 {
        1
    } else {
        CHUNK_SIZE as i32
    }
}

/// Where the eyes are, chunk-local (`eye`), rounded to the steps a
/// translucent mesh is sorted at (see [`sort_step`]). Far chunks' steps are
/// staggered by chunk, so crossing a chunk border doesn't sort every one of
/// them on the same frame.
fn sort_position(eye: Vec3, coord: ChunkCoord) -> IVec3 {
    let middle = Vec3::splat(CHUNK_SIZE as f32 / 2.0);
    let step = sort_step(eye.distance(middle));
    let mut state = coord.0.x as u64 ^ (coord.0.y as u64) << 21 ^ (coord.0.z as u64) << 42;
    let bits = splitmix64(&mut state);
    let offset = IVec3::new(bits as i32, (bits >> 16) as i32, (bits >> 32) as i32)
        .rem_euclid(IVec3::splat(step));
    ((eye.floor().as_ivec3() + offset).div_euclid(IVec3::splat(step)) * step) - offset
}

/// Puts each translucent mesh's faces in order, farthest from the local
/// player's eyes first, when it's new or the eyes have moved far enough
/// (see [`sort_position`]). Only the indices change. A mesh whose faces all
/// lie in one plane (open water) never needs it.
///
/// From the eyes, which are where they are this frame by `Update`, rather
/// than the drawn view, whose transform is a frame old here and sways with
/// each step: swaying across a sort boundary would sort again every step.
fn sort_translucent_faces(
    eyes: Query<Eyes, With<LocalPlayer>>,
    mut parts: Query<(&TranslucentFaces, &Mesh3d, &ChildOf, &mut SortedFrom)>,
    coords: Query<&ChunkCoord>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok(eyes) = eyes.single() else {
        return;
    };
    let eyes = eyes.transform().translation;
    for (faces, mesh, parent, mut sorted) in &mut parts {
        if !faces.needs_sorting() {
            continue;
        }
        let Ok(coord) = coords.get(parent.parent()) else {
            continue;
        };
        let eye = eyes - coord.origin().as_vec3();
        let at = sort_position(eye, *coord);
        if sorted.0 == Some(at) {
            continue;
        }
        sorted.0 = Some(at);
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            mesh.insert_indices(Indices::U32(faces.indices_from(eye)));
        }
    }
}

fn set_sky_color(mut commands: Commands) {
    commands.insert_resource(ClearColor(Color::srgb(0.53, 0.75, 0.95)));
}

/// Test helpers: running an app until its meshes are built.
#[cfg(test)]
pub mod testing {
    use super::*;

    /// Updates the app until no mesh is being built in the background.
    pub fn finish_meshing(app: &mut App) {
        for _ in 0..1000 {
            app.update();
            let mut tasks = app.world_mut().query::<&MeshTask>();
            if tasks.iter(app.world()).next().is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("meshes never finished");
    }
}

#[cfg(test)]
mod tests;
