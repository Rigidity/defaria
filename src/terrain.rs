//! Heightmap terrain from Perlin noise, in chunks, and the meshing of
//! chunks.
//!
//! Each column's height comes from noise mapped through splines, with
//! plains, mountains, ridges and valleys (see [`shape`]). Below the surface
//! is stone, then a few layers of dirt, then one layer of grass on top, except
//! high up and on cliffs, which are bare stone. Here and there a tuft of
//! grass grows on the grass. Hollows below [`SEA_LEVEL`] fill with water,
//! and the ground at and under the water's edge is sand. The world
//! goes on forever in every direction, up and down too, in 16×16×16 chunks:
//! stone all the way down, air all the way up. Chunks are loaded and
//! unloaded around the player (see `streaming`), so [`ChunkMap`] only holds
//! the loaded ones.
//!
//! Chunks are meshed by [`remesh_changed_chunks`] whenever they or their
//! light change, including when they're spawned, so editing a chunk through
//! [`Voxels`] is all it takes to update what's drawn. Meshes are built from
//! copies of a chunk's blocks and light and the voxels around it: a chunk's
//! first mesh on the async task pool, over as many frames as it takes, and
//! later ones (after edits, which should show at once) in parallel on the
//! same frame. A chunk marked [`AwaitingNeighbours`] isn't meshed until the
//! mark comes off, since its faces against the missing chunks would be
//! wrong.

use std::sync::Arc;

use bevy::{
    ecs::system::SystemParam,
    mesh::Indices,
    platform::collections::{HashMap, HashSet},
    prelude::*,
    tasks::{AsyncComputeTaskPool, ComputeTaskPool, Task, futures::check_ready},
};

use crate::{
    content::{BlockState, Content, Render},
    fly_camera::FlyCamera,
    light::ChunkLight,
    voxel::{CHUNK_SIZE, Chunk, ChunkMeshes, Neighborhood, TranslucentFaces, touching_offsets},
    voxel_material::{BlockMaterial, VoxelMaterial},
};

pub mod shape;

use shape::{NEIGHBOURS, TerrainShape};

/// Layers of dirt under the grass.
const DIRT_DEPTH: i32 = 3;
/// Air below this height is water: the water's surface is the top of the
/// voxels at `SEA_LEVEL - 1`.
pub const SEA_LEVEL: i32 = 12;
/// How many grass columns in 1000 grow a tuft of grass.
const TUFTS_PER_THOUSAND: u64 = 80;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainSettings>()
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_systems(
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

/// How the terrain is shaped. Read once at startup. A saved world keeps
/// its own seed (see `save`).
#[derive(Resource, Reflect, Debug, Clone)]
#[reflect(Resource)]
pub struct TerrainSettings {
    pub seed: u64,
    /// Horizontal size of the largest rolling hills, in voxels.
    pub scale: f32,
    /// Noise layers in the hills; each adds detail at half the size of the
    /// last.
    pub octaves: u32,
    /// Surface height where all the noise is 0.
    pub base_height: f32,
    /// How far the hills move the surface up or down at the noise's
    /// extremes.
    pub amplitude: f32,
    /// Horizontal size of the continentalness noise: lowlands to plateaus.
    pub continent_scale: f32,
    /// Horizontal size of the erosion noise: smooth land to rugged.
    pub erosion_scale: f32,
    /// Horizontal size of the peaks-and-valleys noise.
    pub peak_scale: f32,
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            seed: 0x0DEF_A41A,
            scale: 80.0,
            octaves: 3,
            base_height: 22.0,
            amplitude: 36.0,
            continent_scale: 900.0,
            erosion_scale: 500.0,
            peak_scale: 300.0,
        }
    }
}

/// Which chunk of the world grid this is. Chunk `c` covers voxels
/// `c * CHUNK_SIZE` up to (not including) `(c + 1) * CHUNK_SIZE`.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[reflect(Component)]
pub struct ChunkCoord(pub IVec3);

impl ChunkCoord {
    /// The world position of the chunk's `(0, 0, 0)` voxel.
    pub fn origin(self) -> IVec3 {
        self.0 * CHUNK_SIZE as i32
    }

    /// The chunk holding world voxel `pos`, and `pos` within that chunk.
    pub fn split(pos: IVec3) -> (Self, IVec3) {
        let size = IVec3::splat(CHUNK_SIZE as i32);
        (Self(pos.div_euclid(size)), pos.rem_euclid(size))
    }

    /// The chunks whose meshes show world voxel `pos`: its own, and every
    /// chunk across the edges and corners it touches (a voxel shades the
    /// corners of faces next to it, and those can be in any of them).
    pub fn touching(pos: IVec3) -> impl Iterator<Item = ChunkCoord> {
        let (coord, local) = Self::split(pos);
        touching_offsets(local).map(move |offset| ChunkCoord(coord.0 + offset))
    }

    /// The 26 chunks around this one.
    pub fn neighbours(self) -> impl Iterator<Item = ChunkCoord> {
        (-1..=1)
            .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| IVec3::new(x, y, z))))
            .filter(|&offset| offset != IVec3::ZERO)
            .map(move |offset| ChunkCoord(self.0 + offset))
    }
}

/// A voxel was set to a different block, through [`Voxels`].
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelChanged {
    pub pos: IVec3,
}

/// Every loaded chunk's entity, by its coordinate.
#[derive(Resource, Debug, Default)]
pub struct ChunkMap(HashMap<ChunkCoord, Entity>);

impl ChunkMap {
    pub fn get(&self, coord: ChunkCoord) -> Option<Entity> {
        self.0.get(&coord).copied()
    }

    pub fn insert(&mut self, coord: ChunkCoord, chunk: Entity) {
        self.0.insert(coord, chunk);
    }

    pub fn remove(&mut self, coord: ChunkCoord) -> Option<Entity> {
        self.0.remove(&coord)
    }
}

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

/// Reads and writes voxels by world position, across every chunk.
#[derive(SystemParam)]
pub struct Voxels<'w, 's> {
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, &'static mut Chunk>,
    changes: MessageWriter<'w, VoxelChanged>,
}

impl Voxels<'_, '_> {
    /// The voxel at a world position; `None` outside the world or in a
    /// chunk that isn't loaded.
    pub fn get(&self, pos: IVec3) -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some(chunk.at(local))
    }

    /// Sets the voxel at a world position, and returns whether it's there to
    /// set: there's nothing outside the world or the loaded chunks. A voxel on a chunk's edge also
    /// shows, hides or shades faces of the chunks across that edge, so they
    /// are marked changed too and remeshed with it. A change to a different
    /// block sends [`VoxelChanged`].
    pub fn set(&mut self, pos: IVec3, state: BlockState) -> bool {
        let (coord, local) = ChunkCoord::split(pos);
        let Some(mut chunk) = self
            .map
            .get(coord)
            .and_then(|entity| self.chunks.get_mut(entity).ok())
        else {
            return false;
        };
        if chunk.at(local) == state {
            return true;
        }
        chunk.set_at(local, state);
        for touching in ChunkCoord::touching(pos) {
            if let Some(mut chunk) = self
                .map
                .get(touching)
                .and_then(|entity| self.chunks.get_mut(entity).ok())
            {
                chunk.set_changed();
            }
        }
        self.changes.write(VoxelChanged { pos });
        true
    }
}

/// The blocks the terrain is made of.
#[derive(Debug, Clone, Copy)]
pub struct TerrainBlocks {
    pub grass: BlockState,
    pub dirt: BlockState,
    pub stone: BlockState,
    pub sand: BlockState,
    pub water: BlockState,
    pub tuft: BlockState,
}

impl TerrainBlocks {
    const IDS: [&str; 6] = [
        "defaria:grass",
        "defaria:dirt",
        "defaria:stone",
        "defaria:sand",
        "defaria:water",
        "defaria:tuft",
    ];

    /// The ids of the blocks the terrain needs that didn't load.
    pub fn missing(content: &Content) -> Vec<&'static str> {
        Self::IDS
            .into_iter()
            .filter(|id| content.block(id).is_none())
            .collect()
    }

    /// The base game's blocks the terrain is made of. One that didn't load
    /// is reported and left as air.
    pub fn from_content(content: &Content) -> Self {
        let block = |name: &str| {
            let id = format!("defaria:{name}");
            content.default_state(&id).unwrap_or_else(|| {
                error!("the terrain needs block `{id}`, which didn't load");
                BlockState::AIR
            })
        };
        Self {
            grass: block("grass"),
            dirt: block("dirt"),
            stone: block("stone"),
            sand: block("sand"),
            water: block("water"),
            tuft: block("tuft"),
        }
    }
}

/// Answers "what voxel is at this world position?" for the whole world.
/// Shared with the tasks that generate chunks.
pub struct Terrain {
    blocks: TerrainBlocks,
    shape: TerrainShape,
}

impl Terrain {
    pub fn new(settings: TerrainSettings, blocks: TerrainBlocks) -> Self {
        Self {
            shape: TerrainShape::new(settings),
            blocks,
        }
    }

    /// The height of the ground in the column at `(x, z)`: its top solid
    /// voxel is at `y = height - 1`.
    #[cfg(test)]
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.shape.height(x, z)
    }

    /// Whether the column at `(x, z)` is bare rock, without soil.
    #[cfg(test)]
    pub fn is_rock(&self, x: i32, z: i32) -> bool {
        let neighbours = NEIGHBOURS.map(|(dx, dz)| self.shape.surface(x + dx, z + dz));
        self.shape.is_rock(self.height(x, z), neighbours)
    }

    /// The voxel at a world position.
    #[cfg(test)]
    pub fn voxel(&self, pos: IVec3) -> BlockState {
        let column = self.column(
            pos.x,
            pos.z,
            self.height(pos.x, pos.z),
            self.is_rock(pos.x, pos.z),
        );
        column.voxel(pos.y, &self.blocks)
    }

    /// What decides a column's voxels, besides its height: whether it's
    /// bare rock, and whether a tuft grows on it.
    fn column(&self, x: i32, z: i32, height: i32, rock: bool) -> Column {
        let tuft = hash(self.shape.seed(), x, z) % 1000 < TUFTS_PER_THOUSAND;
        Column { height, rock, tuft }
    }

    /// Fills one chunk of the world.
    pub fn chunk(&self, coord: ChunkCoord) -> Chunk {
        let origin = coord.origin();
        let size = CHUNK_SIZE as i32;
        // One noise lookup per column, not per voxel, with a column more on
        // each side for telling cliffs.
        let padded = size + 2;
        let surfaces: Vec<f32> = (0..padded * padded)
            .map(|i| {
                self.shape
                    .surface(origin.x + i % padded - 1, origin.z + i / padded - 1)
            })
            .collect();
        let surface = |x: i32, z: i32| surfaces[((z + 1) * padded + x + 1) as usize];
        let at = |x: i32, z: i32| surface(x, z).round() as i32;
        let columns = || (0..size * size).map(|i| (i % size, i / size));
        let (lowest, highest) = columns().fold((i32::MAX, i32::MIN), |(low, high), (x, z)| {
            (low.min(at(x, z)), high.max(at(x, z)))
        });
        // Most chunks are all sky or all stone; skip the voxel loop. (A
        // tuft can stand on the highest column, at `highest`.)
        if origin.y > highest && origin.y >= SEA_LEVEL {
            return Chunk::default();
        }
        if origin.y + size <= lowest - 1 - DIRT_DEPTH {
            return Chunk::filled(self.blocks.stone);
        }
        let mut chunk = Chunk::default();
        for (x, z) in columns() {
            let height = at(x, z);
            let rock = self
                .shape
                .is_rock(height, NEIGHBOURS.map(|(dx, dz)| surface(x + dx, z + dz)));
            let column = self.column(origin.x + x, origin.z + z, height, rock);
            let (x, z) = (x as usize, z as usize);
            for y in 0..CHUNK_SIZE {
                chunk.set(x, y, z, column.voxel(origin.y + y as i32, &self.blocks));
            }
        }
        chunk
    }
}

/// One column of the terrain.
struct Column {
    /// Its top solid voxel is at `height - 1`.
    height: i32,
    /// Bare rock, without soil.
    rock: bool,
    /// A tuft of grass grows on it, if it's grass.
    tuft: bool,
}

impl Column {
    /// The voxel at height `y`.
    fn voxel(&self, y: i32, blocks: &TerrainBlocks) -> BlockState {
        let height = self.height;
        // The top at the water's edge or under the water.
        let shore = height <= SEA_LEVEL;
        match y {
            y if y >= height && y < SEA_LEVEL => blocks.water,
            y if y == height && self.tuft && !self.rock && !shore => blocks.tuft,
            y if y >= height => BlockState::AIR,
            _ if self.rock => blocks.stone,
            y if y >= height - 1 - DIRT_DEPTH && shore => blocks.sand,
            y if y == height - 1 => blocks.grass,
            y if y >= height - 1 - DIRT_DEPTH => blocks.dirt,
            _ => blocks.stone,
        }
    }
}

/// A well-mixed number from a seed and a column, the same every time
/// (SplitMix64's finalizer).
fn hash(seed: u64, x: i32, z: i32) -> u64 {
    let mut h = seed ^ (x as u32 as u64) << 32 ^ z as u32 as u64;
    h = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
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
#[derive(Component, Debug, Clone, Copy)]
pub struct ChunkParts {
    pub cutout: Entity,
    pub translucent: Entity,
}

/// Where the camera was, in its chunk-local [`TranslucentFaces`] sort's
/// steps, when a translucent mesh's faces were last put in order. `None`
/// until they first are.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct SortedFrom(Option<IVec3>);

fn show_meshes(
    commands: &mut Commands,
    entity: Entity,
    built: ChunkMeshes,
    parts: Option<&ChunkParts>,
    meshes: &mut Assets<Mesh>,
    material: &BlockMaterial,
) {
    commands.entity(entity).insert(Meshed);
    show_mesh(commands, entity, &built.opaque, meshes, &material.material);
    let see_through = [Render::Cutout, Render::Translucent]
        .iter()
        .any(|&render| built.get(render).count_vertices() > 0);
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
        &built.cutout,
        meshes,
        &material.cutout,
    );
    show_mesh(
        commands,
        parts.translucent,
        &built.translucent,
        meshes,
        &material.translucent,
    );
    commands
        .entity(parts.translucent)
        .insert((built.translucent_faces, SortedFrom::default()));
}

/// Draws `mesh` on `entity`, or nothing if it has no faces: chunks with
/// nothing visible (all air, or buried) get no mesh, since the renderer
/// warns about meshes with no vertices.
fn show_mesh(
    commands: &mut Commands,
    entity: Entity,
    mesh: &Mesh,
    meshes: &mut Assets<Mesh>,
    material: &Handle<VoxelMaterial>,
) {
    let mut part = commands.entity(entity);
    if mesh.count_vertices() > 0 {
        part.insert((
            Mesh3d(meshes.add(mesh.clone())),
            MeshMaterial3d(material.clone()),
        ));
    } else {
        part.remove::<(Mesh3d, MeshMaterial3d<VoxelMaterial>)>();
    }
}

/// How far, in blocks, the camera can move before a translucent mesh this
/// far away is sorted again: every block nearby, where the order changes
/// most, and every chunk farther out.
fn sort_step(distance: f32) -> i32 {
    if distance < 2.0 * CHUNK_SIZE as f32 {
        1
    } else {
        CHUNK_SIZE as i32
    }
}

/// Puts each translucent mesh's faces in order, farthest from the camera
/// first, when it's new or the camera has moved far enough (see
/// [`sort_step`]). Only the indices change.
pub fn sort_translucent_faces(
    camera: Query<&Transform, With<FlyCamera>>,
    mut parts: Query<(&TranslucentFaces, &Mesh3d, &ChildOf, &mut SortedFrom)>,
    coords: Query<&ChunkCoord>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    for (faces, mesh, parent, mut sorted) in &mut parts {
        let Ok(coord) = coords.get(parent.parent()) else {
            continue;
        };
        let eye = camera.translation - coord.origin().as_vec3();
        let middle = Vec3::splat(CHUNK_SIZE as f32 / 2.0);
        let step = sort_step(eye.distance(middle));
        let at = eye.div_euclid(Vec3::splat(step as f32)).as_ivec3() * step;
        if sorted.0 == Some(at) {
            continue;
        }
        sorted.0 = Some(at);
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            mesh.insert_indices(Indices::U32(faces.indices_from(eye)));
        }
    }
}

/// Spawns a chunk entity at its place in the world and adds it to `map`.
pub fn spawn_chunk(
    commands: &mut Commands,
    map: &mut ChunkMap,
    coord: ChunkCoord,
    chunk: Chunk,
    extra: impl Bundle,
) -> Entity {
    let IVec3 { x, y, z } = coord.0;
    let entity = commands
        .spawn((
            Name::new(format!("Chunk ({x}, {y}, {z})")),
            Transform::from_translation(coord.origin().as_vec3()),
            // Its parts (see `ChunkParts`) inherit it.
            Visibility::default(),
            coord,
            chunk,
            extra,
        ))
        .id();
    map.insert(coord, entity);
    entity
}

/// Test helpers: a fixed patch of terrain without the streaming, and
/// running an app until its meshes are built.
#[cfg(test)]
pub mod testing {
    use super::*;

    /// The patch of world the tests use: 8×4×8 chunks from chunk
    /// `(0, 0, 0)`, 128×64×128 voxels, holding the default terrain's
    /// surface around the camera's start.
    pub const PATCH: IVec3 = IVec3::new(8, 4, 8);
    pub const PATCH_SIZE: IVec3 = IVec3::new(128, 64, 128);

    pub fn patch_chunks() -> impl Iterator<Item = ChunkCoord> {
        (0..PATCH.x).flat_map(|x| {
            (0..PATCH.y)
                .flat_map(move |y| (0..PATCH.z).map(move |z| ChunkCoord(IVec3::new(x, y, z))))
        })
    }

    /// Spawns the patch's chunks from the default terrain.
    pub fn spawn_patch(world: &mut World) {
        world
            .run_system_cached(
                |mut commands: Commands,
                 mut map: ResMut<ChunkMap>,
                 settings: Res<TerrainSettings>,
                 content: Res<Content>| {
                    let terrain =
                        Terrain::new(settings.clone(), TerrainBlocks::from_content(&content));
                    for coord in patch_chunks() {
                        spawn_chunk(&mut commands, &mut map, coord, terrain.chunk(coord), ());
                    }
                },
            )
            .unwrap();
    }

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
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::{
        testing::{PATCH_SIZE, finish_meshing, patch_chunks, spawn_patch},
        *,
    };
    use crate::content::{self, base, block};

    fn terrain() -> Terrain {
        terrain_with(TerrainSettings::default())
    }

    fn terrain_with(settings: TerrainSettings) -> Terrain {
        Terrain::new(settings, TerrainBlocks::from_content(base()))
    }

    /// The voxel columns of the test patch.
    fn columns() -> impl Iterator<Item = (i32, i32)> {
        (0..PATCH_SIZE.x).flat_map(|x| (0..PATCH_SIZE.z).map(move |z| (x, z)))
    }

    /// The terrain carries on in every direction, at negative coordinates
    /// and far away, without seams.
    #[test]
    fn terrain_goes_on_in_every_direction() {
        let terrain = terrain();
        for (x0, z0) in [(-200, -200), (100_000, -3_000), (-64, 50)] {
            let mut heights = Vec::new();
            for x in x0..x0 + 64 {
                let h = terrain.height(x, z0);
                let top = if terrain.is_rock(x, z0) {
                    block("stone")
                } else if h <= SEA_LEVEL {
                    block("sand")
                } else {
                    block("grass")
                };
                assert_eq!(terrain.voxel(IVec3::new(x, h - 1, z0)), top);
                assert!(
                    !content::base()
                        .state(terrain.voxel(IVec3::new(x, h, z0)))
                        .is_opaque()
                );
                assert!(
                    terrain
                        .voxel(IVec3::new(x, h.max(SEA_LEVEL) + 1, z0))
                        .is_air()
                );
                if let Some(&last) = heights.last() {
                    let step: i32 = h - last;
                    assert!(step.abs() <= 16, "a wall at ({x}, {z0})");
                }
                heights.push(h);
            }
            assert!(heights.iter().any(|&h| h != heights[0]), "flat near {x0}");
        }
    }

    /// Grass over dirt over stone, or sand at and under the water's edge,
    /// with water up to the sea level, and a tuft on some of the grass.
    #[test]
    fn columns_are_layered_grass_dirt_stone() {
        let terrain = terrain();
        let (mut tufts, mut grass, mut ponds) = (0, 0, 0);
        for (x, z) in columns() {
            let height = terrain.height(x, z);
            assert!(
                (1..=PATCH_SIZE.y).contains(&height),
                "patch holds the surface"
            );
            let at = |y| terrain.voxel(IVec3::new(x, y, z));
            let shore = height <= SEA_LEVEL;
            let (top, soil) = if shore {
                (block("sand"), block("sand"))
            } else {
                (block("grass"), block("dirt"))
            };
            assert_eq!(at(height - 1), top);
            let mut above = height;
            if at(above) == block("tuft") {
                assert!(!shore, "a tuft on the sand at ({x}, {z})");
                tufts += 1;
                above += 1;
            }
            for y in above..SEA_LEVEL {
                assert_eq!(at(y), block("water"), "({x}, {y}, {z})");
            }
            ponds += (height < SEA_LEVEL) as u32;
            grass += !shore as u32;
            for y in above.max(SEA_LEVEL)..height + 100 {
                assert_eq!(at(y), BlockState::AIR, "({x}, {y}, {z})");
            }
            for y in height - 100..height - 1 {
                let expected = if y >= height - 1 - DIRT_DEPTH {
                    soil
                } else {
                    block("stone")
                };
                assert_eq!(at(y), expected, "({x}, {y}, {z}), height {height}");
            }
        }
        // About 8% of the grass has a tuft, and the patch has a pond.
        let share = tufts as f32 / grass as f32;
        assert!(
            (0.06..0.10).contains(&share),
            "{share} of the grass has tufts"
        );
        assert!(ponds > 0, "no water in the patch");
    }

    /// Chunks generated whole match the voxels column by column, water and
    /// tufts included, even where a chunk starts right at a tuft.
    #[test]
    fn chunks_match_the_voxels() {
        let terrain = terrain();
        for coord in patch_chunks() {
            let chunk = terrain.chunk(coord);
            for (x, y, z) in Chunk::positions() {
                let local = IVec3::new(x as i32, y as i32, z as i32);
                assert_eq!(
                    chunk.at(local),
                    terrain.voxel(coord.origin() + local),
                    "{local} in {coord:?}"
                );
            }
        }
    }

    /// Around the start, hills, not a flat plane or random spikes: the
    /// height varies a lot over the patch but only a little between
    /// neighbouring columns. (Further out there are mountains: see `shape`.)
    #[test]
    fn terrain_is_hilly_and_smooth() {
        let terrain = terrain();
        let heights: Vec<_> = columns().map(|(x, z)| terrain.height(x, z)).collect();
        let (min, max) = (heights.iter().min().unwrap(), heights.iter().max().unwrap());
        assert!(max - min >= 8, "heights only span {min}..={max}");
        // Both chunk layers have ground in them somewhere, and the peaks
        // aren't clipped flat against the top of the world.
        assert!(*max > CHUNK_SIZE as i32 && *max < PATCH_SIZE.y);
        for (x, z) in columns() {
            let h = terrain.height(x, z);
            for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                let step = (terrain.height(nx, nz) - h).abs();
                assert!(step <= 3, "cliff of {step} at ({x}, {z})");
            }
        }
    }

    #[test]
    fn sky_all_the_way_up_and_stone_all_the_way_down() {
        let terrain = terrain();
        // By the start, and in the mountains.
        for (x, z) in [(-5, 3), (1576, -352)] {
            for y in [400, 10_000, 1_000_000] {
                assert_eq!(terrain.voxel(IVec3::new(x, y, z)), BlockState::AIR);
                assert_eq!(terrain.voxel(IVec3::new(x, -y, z)), block("stone"));
            }
        }
        for pos in [IVec3::ZERO, IVec3::new(-1, 0, -1), IVec3::new(100, 0, 0)] {
            assert_eq!(terrain.voxel(pos), block("stone"));
        }
        let sky = ChunkCoord(IVec3::new(3, 40, -2));
        let deep = ChunkCoord(IVec3::new(3, -40, -2));
        assert_eq!(terrain.chunk(sky), Chunk::default());
        assert_eq!(terrain.chunk(deep), Chunk::filled(block("stone")));
    }

    /// The camera spawns in open air with clearance above the hills around
    /// it, not inside or skimming the ground.
    #[test]
    fn camera_starts_above_the_ground() {
        use crate::fly_camera::START_POSITION;
        let terrain = terrain();
        let start = START_POSITION.as_ivec3();
        assert!((0..PATCH_SIZE.y).contains(&start.y));
        let clearance = 4;
        for dx in -8..=8 {
            for dz in -8..=8 {
                let height = terrain.height(start.x + dx, start.z + dz);
                assert!(
                    height + clearance <= start.y,
                    "hill of {height} near the start"
                );
            }
        }
    }

    #[test]
    fn missing_terrain_blocks_are_named() {
        assert!(TerrainBlocks::missing(base()).is_empty());
        let mut content = base().clone();
        assert!(content.forget_block("defaria:stone"));
        assert_eq!(TerrainBlocks::missing(&content), ["defaria:stone"]);
    }

    #[test]
    fn different_seeds_make_different_terrain() {
        let a = terrain();
        let b = terrain_with(TerrainSettings {
            seed: 1,
            ..default()
        });
        assert!(columns().any(|(x, z)| a.height(x, z) != b.height(x, z)));
    }

    /// In the mountains, among cliffs and peaks, chunks match the
    /// column-by-column answer voxel for voxel, across their borders and
    /// in the all-air and all-stone shortcuts, and bare-rock columns are
    /// stone right to the top.
    #[test]
    fn mountain_chunks_match_their_columns() {
        let terrain = terrain();
        let (mut rock, mut grass) = (0, 0);
        // East of the start, where the mountains are cliffs below the rock
        // line.
        for cx in 102..106 {
            for cz in 4..7 {
                let (x0, z0) = (cx * 16, cz * 16);
                let surface = terrain.height(x0 + 8, z0 + 8).div_euclid(16);
                for cy in surface - 1..=surface + 1 {
                    let coord = ChunkCoord(IVec3::new(cx, cy, cz));
                    let chunk = terrain.chunk(coord);
                    for (x, y, z) in Chunk::positions() {
                        let world = coord.origin() + IVec3::new(x as i32, y as i32, z as i32);
                        assert_eq!(chunk.get(x, y, z), terrain.voxel(world), "at {world}");
                    }
                }
                for x in x0..x0 + 16 {
                    for z in z0..z0 + 16 {
                        let top = terrain.voxel(IVec3::new(x, terrain.height(x, z) - 1, z));
                        if terrain.is_rock(x, z) {
                            rock += 1;
                            assert_eq!(top, block("stone"), "at ({x}, {z})");
                        } else {
                            grass += 1;
                            assert_eq!(top, block("grass"), "at ({x}, {z})");
                        }
                    }
                }
            }
        }
        assert!(
            rock > 50 && grass > 50,
            "{rock} rock and {grass} grass columns"
        );
    }

    #[test]
    fn chunks_hold_their_slice_of_the_world() {
        let terrain = terrain();
        let coord = ChunkCoord(IVec3::new(2, 1, 3));
        let chunk = terrain.chunk(coord);
        for (x, y, z) in [(0, 0, 0), (5, 3, 9), (15, 15, 15)] {
            let world = coord.origin() + IVec3::new(x as i32, y as i32, z as i32);
            assert_eq!(chunk.get(x, y, z), terrain.voxel(world));
        }
    }

    /// The terrain plugin with the systems it needs and the test patch of
    /// terrain, meshed, and a record of which chunks changed in each frame.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins((AssetPlugin::default(), TerrainPlugin))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial::unloaded(Handle::default()))
            .init_resource::<ChangedChunks>()
            .add_systems(Update, record_changed.in_set(RemeshChunks));
        spawn_patch(app.world_mut());
        finish_meshing(&mut app);
        app
    }

    #[derive(Resource, Default)]
    struct ChangedChunks(Vec<IVec3>);

    fn record_changed(
        chunks: Query<(&ChunkCoord, Ref<Chunk>)>,
        mut changed: ResMut<ChangedChunks>,
    ) {
        changed.0 = chunks
            .iter()
            .filter(|(_, chunk)| chunk.is_changed())
            .map(|(coord, _)| coord.0)
            .collect();
        changed.0.sort_by_key(|c| c.to_array());
    }

    fn meshes(app: &mut App) -> HashMap<ChunkCoord, Option<AssetId<Mesh>>> {
        let mut chunks = app.world_mut().query::<(&ChunkCoord, Option<&Mesh3d>)>();
        chunks
            .iter(app.world())
            .map(|(coord, mesh)| (*coord, mesh.map(|mesh| mesh.id())))
            .collect()
    }

    /// Faces between two solid voxels in different chunks are culled, so the
    /// whole world's faces equal the faces of the terrain as one solid.
    #[test]
    fn no_faces_hidden_between_chunks() {
        let mut app = app();
        let ids = meshes(&mut app);
        let assets = app.world().resource::<Assets<Mesh>>();
        let total: usize = ids
            .values()
            .flatten()
            .map(|&id| assets.get(id).unwrap().count_vertices() / 4)
            .sum();

        // Count exposed faces directly from the heightmap.
        let terrain = terrain();
        let mut expected = 0;
        for (x, z) in columns() {
            let h = terrain.height(x, z);
            expected += 2; // top and bottom of the column
            for (nx, nz) in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)] {
                let outside = nx < 0 || nz < 0 || nx >= PATCH_SIZE.x || nz >= PATCH_SIZE.z;
                let neighbour = if outside { 0 } else { terrain.height(nx, nz) };
                expected += (h - neighbour).max(0) as usize;
            }
        }
        assert_eq!(total, expected);
    }

    #[test]
    fn unchanged_chunks_keep_their_meshes() {
        let mut app = app();
        let before = meshes(&mut app);
        app.update();
        assert!(app.world().resource::<ChangedChunks>().0.is_empty());
        assert_eq!(meshes(&mut app), before);
    }

    /// An edit changes its chunk and every chunk across the edges and
    /// corners it touches (their faces' corners can be shaded by it), and
    /// only those are remeshed. Setting a voxel to what it already is
    /// changes nothing.
    #[test]
    fn edits_on_a_border_remesh_the_chunks_across_it() {
        let mut app = app();
        let set = |app: &mut App, pos: IVec3, state: BlockState| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| {
                    assert!(voxels.set(pos, state));
                })
                .unwrap();
            app.update();
            app.world().resource::<ChangedChunks>().0.clone()
        };
        let around = |chunk: IVec3, steps: IVec3| {
            let mut chunks = Vec::new();
            for x in [0, steps.x] {
                for y in [0, steps.y] {
                    for z in [0, steps.z] {
                        chunks.push(chunk + IVec3::new(x, y, z));
                    }
                }
            }
            chunks.sort_by_key(|c| c.to_array());
            chunks.dedup();
            chunks
        };
        let planks = block("planks");
        let chunk = IVec3::new(2, 1, 2);
        let origin = ChunkCoord(chunk).origin();
        let before = meshes(&mut app);
        let corner = origin + IVec3::new(0, 15, 15);
        let changed = set(&mut app, corner, planks);
        let expected = around(chunk, IVec3::new(-1, 1, 1));
        assert_eq!(changed.len(), 8);
        assert_eq!(changed, expected);
        // Every other chunk kept its mesh.
        let after = meshes(&mut app);
        for (coord, mesh) in &before {
            if !expected.contains(&coord.0) {
                assert_eq!(after[coord], *mesh, "{coord:?}");
            }
        }

        let changed = set(&mut app, origin + IVec3::new(15, 0, 0), planks);
        assert_eq!(changed, around(chunk, IVec3::new(1, -1, -1)));
        let changed = set(&mut app, origin + IVec3::new(0, 5, 7), planks);
        assert_eq!(changed, around(chunk, IVec3::new(-1, 0, 0)));
        let changed = set(&mut app, origin + IVec3::new(4, 5, 7), planks);
        assert_eq!(changed, [chunk]);
        assert!(set(&mut app, corner, planks).is_empty(), "already planks");
    }

    /// A chunk awaiting its neighbours keeps whatever mesh it has through
    /// edits, and is meshed once the mark comes off.
    #[test]
    fn chunks_awaiting_neighbours_wait_to_be_meshed() {
        let mut app = app();
        let coord = ChunkCoord(IVec3::new(3, 0, 3));
        let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
        let before = meshes(&mut app)[&coord];
        app.world_mut()
            .entity_mut(entity)
            .insert(AwaitingNeighbours);
        let pos = coord.origin() + IVec3::new(8, 15, 8);
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                assert!(voxels.set(pos, block("planks")));
            })
            .unwrap();
        finish_meshing(&mut app);
        assert_eq!(meshes(&mut app)[&coord], before, "meshed while awaiting");
        app.world_mut()
            .entity_mut(entity)
            .remove::<AwaitingNeighbours>();
        app.update();
        // Built in the background: still being built, or (if the task pool
        // got through it before the frame's end, as it can on a slow or
        // busy machine) already shown. Either way not left alone.
        let building = app.world().get::<MeshTask>(entity).is_some();
        let shown = meshes(&mut app)[&coord] != before;
        assert!(building || shown, "neither building nor rebuilt");
        finish_meshing(&mut app);
        assert_ne!(meshes(&mut app)[&coord], before);
    }

    #[test]
    fn voxels_are_read_and_written_by_world_position() {
        let mut app = app();
        let terrain = terrain();
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                for pos in [IVec3::new(5, 3, 70), IVec3::new(127, 0, 0)] {
                    assert_eq!(voxels.get(pos), Some(terrain.voxel(pos)));
                }
                // Outside the loaded chunks there's nothing to read or write.
                for pos in [IVec3::new(-1, 0, 0), IVec3::new(0, PATCH_SIZE.y, 0)] {
                    assert_eq!(voxels.get(pos), None);
                    assert!(!voxels.set(pos, block("stone")));
                }
                let pos = IVec3::new(20, 40, 33);
                assert!(voxels.set(pos, block("planks")));
                assert_eq!(voxels.get(pos), Some(block("planks")));
            })
            .unwrap();
    }

    #[test]
    fn split_finds_the_chunk_and_local_position() {
        assert_eq!(
            ChunkCoord::split(IVec3::new(17, 0, 15)),
            (ChunkCoord(IVec3::new(1, 0, 0)), IVec3::new(1, 0, 15))
        );
        assert_eq!(
            ChunkCoord::split(IVec3::new(-1, 0, 0)),
            (ChunkCoord(IVec3::new(-1, 0, 0)), IVec3::new(15, 0, 0))
        );
    }

    #[test]
    fn chunks_are_spawned_at_their_origin() {
        let mut app = app();

        let mut chunks = app
            .world_mut()
            .query::<(&ChunkCoord, &Transform, Option<&Mesh3d>)>();
        let mut meshed = 0;
        let mut seen: Vec<_> = chunks
            .iter(app.world())
            .map(|(coord, transform, mesh)| {
                meshed += mesh.is_some() as usize;
                assert_eq!(transform.translation, coord.origin().as_vec3());
                coord.0.to_array()
            })
            .collect();
        seen.sort();
        let mut expected: Vec<_> = patch_chunks().map(|c| c.0.to_array()).collect();
        expected.sort();
        assert_eq!(seen, expected);
        // Every bottom chunk has ground in it; empty upper ones get no mesh.
        assert!((64..=256).contains(&meshed), "{meshed} chunks have meshes");
    }

    /// A chunk with see-through blocks draws them on two child entities,
    /// cut-out and translucent, each with its own material; most chunks
    /// have none. The translucent faces are sorted farthest first from the
    /// camera, and again when it moves far enough.
    #[test]
    fn see_through_faces_draw_on_their_own_parts_sorted() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins((AssetPlugin::default(), TerrainPlugin))
            .init_asset::<Mesh>()
            .init_asset::<VoxelMaterial>();
        let reserve = || {
            app.world()
                .resource::<Assets<VoxelMaterial>>()
                .reserve_handle()
        };
        let materials = BlockMaterial {
            cutout: reserve(),
            translucent: reserve(),
            ..BlockMaterial::unloaded(reserve())
        };
        app.insert_resource(materials.clone());
        let mut chunk = Chunk::default();
        chunk.set(2, 2, 2, block("stone"));
        chunk.set(4, 2, 2, block("leaves"));
        chunk.set(6, 2, 2, block("glass"));
        chunk.set(6, 2, 12, block("glass"));
        let camera = app
            .world_mut()
            .spawn((FlyCamera::default(), Transform::from_xyz(6.5, 2.5, 30.0)))
            .id();
        let (plain, see_through) = app
            .world_mut()
            .run_system_once(move |mut commands: Commands, mut map: ResMut<ChunkMap>| {
                let mut plain = Chunk::default();
                plain.set(1, 1, 1, block("stone"));
                let a = spawn_chunk(&mut commands, &mut map, ChunkCoord(IVec3::X), plain, ());
                let b = spawn_chunk(
                    &mut commands,
                    &mut map,
                    ChunkCoord(IVec3::ZERO),
                    chunk.clone(),
                    (),
                );
                (a, b)
            })
            .unwrap();
        finish_meshing(&mut app);
        app.update();

        assert!(app.world().get::<ChunkParts>(plain).is_none());
        let parts = *app.world().get::<ChunkParts>(see_through).unwrap();
        let world = app.world();
        let material = |entity| {
            world
                .get::<MeshMaterial3d<VoxelMaterial>>(entity)
                .unwrap()
                .0
                .clone()
        };
        let faces = |entity| {
            let mesh = world.get::<Mesh3d>(entity).unwrap();
            world
                .resource::<Assets<Mesh>>()
                .get(mesh)
                .unwrap()
                .count_vertices()
                / 4
        };
        assert_eq!(faces(see_through), 6);
        assert_eq!(faces(parts.cutout), 6);
        assert_eq!(faces(parts.translucent), 12);
        assert_eq!(material(see_through), materials.material);
        assert_eq!(material(parts.cutout), materials.cutout);
        assert_eq!(material(parts.translucent), materials.translucent);
        for part in [parts.cutout, parts.translucent] {
            assert_eq!(world.get::<ChildOf>(part).unwrap().parent(), see_through);
        }

        // Seen from the south (+Z), the north pane's faces come first.
        let first_face_z = |app: &App| {
            let mesh = app.world().get::<Mesh3d>(parts.translucent).unwrap();
            let mesh = app.world().resource::<Assets<Mesh>>().get(mesh).unwrap();
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("no indices");
            };
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("no positions");
            };
            positions[indices[0] as usize][2]
        };
        assert!(first_face_z(&app) < 4.0);
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .z = -20.0;
        app.update();
        assert!(first_face_z(&app) > 10.0);
    }
}
