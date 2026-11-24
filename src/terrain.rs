//! Heightmap terrain from Perlin noise, split into a fixed grid of chunks.
//!
//! Each column's height comes from fractal Perlin noise. Below the surface is
//! stone, then a few layers of dirt, then one layer of grass on top. The world
//! is [`WORLD_CHUNKS`] chunks; everything outside it is air.
//!
//! Chunks are meshed by [`remesh_changed_chunks`] whenever they or their
//! light change, including when they're spawned, so editing a chunk through
//! [`Voxels`] is all it takes to update what's drawn. Chunks are meshed in
//! parallel, from copies of their blocks and light and the voxels around
//! them.

use bevy::{
    ecs::system::SystemParam,
    platform::collections::{HashMap, HashSet},
    prelude::*,
    tasks::ComputeTaskPool,
};

use crate::{
    content::{BlockState, Content},
    light::ChunkLight,
    perlin::Perlin,
    voxel::{CHUNK_SIZE, Chunk, Neighborhood},
    voxel_material::{BlockMaterial, VoxelMaterial},
};

/// Chunks along each axis of the world.
pub const WORLD_CHUNKS: IVec3 = IVec3::new(8, 4, 8);
/// The world's size in voxels.
pub const WORLD_SIZE: IVec3 = IVec3::new(
    WORLD_CHUNKS.x * CHUNK_SIZE as i32,
    WORLD_CHUNKS.y * CHUNK_SIZE as i32,
    WORLD_CHUNKS.z * CHUNK_SIZE as i32,
);
/// Layers of dirt under the grass.
const DIRT_DEPTH: i32 = 3;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainSettings>()
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_systems(Startup, spawn_chunks)
            .add_systems(Update, remesh_changed_chunks.in_set(RemeshChunks));
    }
}

/// Where changed chunks are relit and remeshed. Systems that edit chunks
/// run before it, so their edits show on the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemeshChunks;

/// How the terrain is shaped. Read once at startup.
#[derive(Resource, Reflect, Debug, Clone)]
#[reflect(Resource)]
pub struct TerrainSettings {
    pub seed: u64,
    /// Horizontal size of the largest hills, in voxels.
    pub scale: f32,
    /// Noise layers; each adds detail at half the size of the last.
    pub octaves: u32,
    /// Surface height where the noise is 0.
    pub base_height: f32,
    /// How far the surface moves up or down at the noise's extremes.
    pub amplitude: f32,
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            seed: 0x0DEF_A41A,
            scale: 80.0,
            octaves: 3,
            base_height: 22.0,
            amplitude: 36.0,
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
        let last = CHUNK_SIZE as i32 - 1;
        let edge = local.map(|v| match v {
            0 => -1,
            v if v == last => 1,
            _ => 0,
        });
        let steps = |axis: usize| {
            if edge[axis] == 0 {
                vec![0]
            } else {
                vec![0, edge[axis]]
            }
        };
        let (xs, ys, zs) = (steps(0), steps(1), steps(2));
        xs.into_iter().flat_map(move |x| {
            let zs = zs.clone();
            ys.clone().into_iter().flat_map(move |y| {
                zs.clone()
                    .into_iter()
                    .map(move |z| ChunkCoord(coord.0 + IVec3::new(x, y, z)))
            })
        })
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

/// Every chunk entity, by its coordinate. Filled as the chunks spawn.
#[derive(Resource, Debug, Default)]
pub struct ChunkMap(HashMap<ChunkCoord, Entity>);

impl ChunkMap {
    pub fn get(&self, coord: ChunkCoord) -> Option<Entity> {
        self.0.get(&coord).copied()
    }

    pub fn insert(&mut self, coord: ChunkCoord, chunk: Entity) {
        self.0.insert(coord, chunk);
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
    /// The voxel at a world position; `None` outside the world.
    pub fn get(&self, pos: IVec3) -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some(chunk.at(local))
    }

    /// Sets the voxel at a world position, and returns whether it's there to
    /// set: there's nothing outside the world. A voxel on a chunk's edge also
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
}

impl TerrainBlocks {
    /// The base game's grass, dirt and stone. One that didn't load is
    /// reported and left as air.
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
        }
    }
}

/// Answers "what voxel is at this world position?" for the whole world.
pub struct Terrain {
    settings: TerrainSettings,
    blocks: TerrainBlocks,
    noise: Perlin,
}

impl Terrain {
    pub fn new(settings: TerrainSettings, blocks: TerrainBlocks) -> Self {
        Self {
            noise: Perlin::new(settings.seed),
            settings,
            blocks,
        }
    }

    /// Number of solid voxels in the column at `(x, z)`, so the top solid
    /// voxel is at `y = height - 1`. Clamped to `1..=` the world height, so
    /// there are no holes; peaks taller than the world get flattened.
    pub fn height(&self, x: i32, z: i32) -> i32 {
        let s = &self.settings;
        // Sample at voxel centers so lattice points (where noise is 0) don't
        // line up with voxel edges.
        let n = self.noise.fbm(
            (x as f32 + 0.5) / s.scale,
            (z as f32 + 0.5) / s.scale,
            s.octaves,
        );
        ((s.base_height + n * s.amplitude).round() as i32).clamp(1, WORLD_SIZE.y)
    }

    /// The voxel at a world position. Air outside the world.
    #[cfg(test)]
    pub fn voxel(&self, pos: IVec3) -> BlockState {
        if pos.cmplt(IVec3::ZERO).any() || pos.cmpge(WORLD_SIZE).any() {
            return BlockState::AIR;
        }
        self.column_voxel(pos.y, self.height(pos.x, pos.z))
    }

    /// Fills one chunk of the world.
    pub fn chunk(&self, coord: ChunkCoord) -> Chunk {
        let origin = coord.origin();
        let mut chunk = Chunk::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                // One noise lookup per column, not per voxel.
                let height = self.height(origin.x + x as i32, origin.z + z as i32);
                for y in 0..CHUNK_SIZE {
                    chunk.set(x, y, z, self.column_voxel(origin.y + y as i32, height));
                }
            }
        }
        chunk
    }

    /// The voxel at height `y` in a column whose surface is `height`
    /// voxels tall.
    fn column_voxel(&self, y: i32, height: i32) -> BlockState {
        match y {
            y if y >= height => BlockState::AIR,
            y if y == height - 1 => self.blocks.grass,
            y if y >= height - 1 - DIRT_DEPTH => self.blocks.dirt,
            _ => self.blocks.stone,
        }
    }
}

/// Every chunk coordinate in the world grid.
pub fn chunk_coords() -> impl Iterator<Item = ChunkCoord> {
    (0..WORLD_CHUNKS.x).flat_map(|x| {
        (0..WORLD_CHUNKS.y)
            .flat_map(move |y| (0..WORLD_CHUNKS.z).map(move |z| ChunkCoord(IVec3::new(x, y, z))))
    })
}

/// Spawns every chunk of the world. [`remesh_changed_chunks`] meshes them
/// on the same frame.
fn spawn_chunks(
    mut commands: Commands,
    settings: Res<TerrainSettings>,
    content: Res<Content>,
    mut map: ResMut<ChunkMap>,
) {
    let terrain = Terrain::new(settings.clone(), TerrainBlocks::from_content(&content));
    for coord in chunk_coords() {
        let IVec3 { x, y, z } = coord.0;
        let entity = commands
            .spawn((
                Name::new(format!("Chunk ({x}, {y}, {z})")),
                Transform::from_translation(coord.origin().as_vec3()),
                coord,
                terrain.chunk(coord),
            ))
            .id();
        map.insert(coord, entity);
    }
}

/// Rebuilds the mesh of every chunk that was spawned or changed, or whose
/// light changed, since the last run, and of the chunks around each one that
/// was spawned (their faces against it are culled and shaded differently
/// now). Meshes are built in parallel.
pub fn remesh_changed_chunks(
    mut commands: Commands,
    chunks: Query<(Entity, &ChunkCoord, Ref<Chunk>, Ref<ChunkLight>)>,
    map: Res<ChunkMap>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<BlockMaterial>,
    content: Res<Content>,
) {
    let mut dirty = HashSet::new();
    for (_, coord, chunk, light) in &chunks {
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
    let jobs: Vec<(Entity, Neighborhood)> = dirty
        .into_iter()
        .filter_map(|coord| {
            let entity = map.get(coord)?;
            let neighborhood = Neighborhood::gather(|offset| {
                let (_, _, chunk, light) =
                    chunks.get(map.get(ChunkCoord(coord.0 + offset))?).ok()?;
                Some((chunk.into_inner(), light.into_inner()))
            });
            Some((entity, neighborhood))
        })
        .collect();
    let built = ComputeTaskPool::get().scope(|scope| {
        for (entity, neighborhood) in &jobs {
            let content = &content;
            scope.spawn(async move { (*entity, neighborhood.mesh(content)) });
        }
    });
    for (entity, mesh) in built {
        // Chunks with nothing visible (all air, or buried) get no mesh; the
        // renderer warns about meshes with no vertices.
        if mesh.count_vertices() > 0 {
            commands.entity(entity).insert((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.material.clone()),
            ));
        } else {
            commands
                .entity(entity)
                .remove::<(Mesh3d, MeshMaterial3d<VoxelMaterial>)>();
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::content::{base, block};

    fn terrain() -> Terrain {
        terrain_with(TerrainSettings::default())
    }

    fn terrain_with(settings: TerrainSettings) -> Terrain {
        Terrain::new(settings, TerrainBlocks::from_content(base()))
    }

    fn columns() -> impl Iterator<Item = (i32, i32)> {
        (0..WORLD_SIZE.x).flat_map(|x| (0..WORLD_SIZE.z).map(move |z| (x, z)))
    }

    #[test]
    fn world_is_eight_by_four_by_eight_chunks() {
        let coords: Vec<_> = chunk_coords().collect();
        assert_eq!(coords.len(), 8 * 4 * 8);
        assert_eq!(WORLD_SIZE, IVec3::new(128, 64, 128));
        assert!(coords.contains(&ChunkCoord(IVec3::ZERO)));
        assert!(coords.contains(&ChunkCoord(IVec3::new(7, 3, 7))));
    }

    #[test]
    fn columns_are_layered_grass_dirt_stone() {
        let terrain = terrain();
        for (x, z) in columns() {
            let height = terrain.height(x, z);
            assert!((1..=WORLD_SIZE.y).contains(&height));
            let at = |y| terrain.voxel(IVec3::new(x, y, z));
            assert_eq!(at(height - 1), block("grass"));
            for y in height..WORLD_SIZE.y {
                assert_eq!(at(y), BlockState::AIR);
            }
            for y in 0..height - 1 {
                let expected = if y >= height - 1 - DIRT_DEPTH {
                    block("dirt")
                } else {
                    block("stone")
                };
                assert_eq!(at(y), expected, "({x}, {y}, {z}), height {height}");
            }
        }
    }

    /// Hills, not a flat plane or random spikes: the height varies a lot over
    /// the world but only a little between neighbouring columns.
    #[test]
    fn terrain_is_hilly_and_smooth() {
        let terrain = terrain();
        let heights: Vec<_> = columns().map(|(x, z)| terrain.height(x, z)).collect();
        let (min, max) = (heights.iter().min().unwrap(), heights.iter().max().unwrap());
        assert!(max - min >= 8, "heights only span {min}..={max}");
        // Both chunk layers have ground in them somewhere, and the peaks
        // aren't clipped flat against the top of the world.
        assert!(*max > CHUNK_SIZE as i32 && *max < WORLD_SIZE.y);
        for (x, z) in columns() {
            let h = terrain.height(x, z);
            for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                if nx < WORLD_SIZE.x && nz < WORLD_SIZE.z {
                    let step = (terrain.height(nx, nz) - h).abs();
                    assert!(step <= 3, "cliff of {step} at ({x}, {z})");
                }
            }
        }
    }

    #[test]
    fn outside_the_world_is_air() {
        let terrain = terrain();
        for pos in [
            IVec3::new(-1, 0, 0),
            IVec3::new(0, -1, 0),
            IVec3::new(0, 0, -1),
            IVec3::new(WORLD_SIZE.x, 0, 0),
            IVec3::new(0, 0, WORLD_SIZE.z),
        ] {
            assert_eq!(terrain.voxel(pos), BlockState::AIR);
        }
        assert_eq!(terrain.voxel(IVec3::ZERO), block("stone"));
    }

    /// The camera spawns in open air with clearance above the hills around
    /// it, not inside or skimming the ground.
    #[test]
    fn camera_starts_above_the_ground() {
        use crate::fly_camera::START_POSITION;
        let terrain = terrain();
        let start = START_POSITION.as_ivec3();
        assert!(start.cmpge(IVec3::ZERO).all() && start.cmplt(WORLD_SIZE).all());
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
    fn different_seeds_make_different_terrain() {
        let a = terrain();
        let b = terrain_with(TerrainSettings {
            seed: 1,
            ..default()
        });
        assert!(columns().any(|(x, z)| a.height(x, z) != b.height(x, z)));
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

    /// The terrain plugin with the systems it needs, run for one frame, and
    /// a record of which chunks changed in each frame.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins((AssetPlugin::default(), TerrainPlugin))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial {
                material: Handle::default(),
                layers: default(),
            })
            .init_resource::<ChangedChunks>()
            .add_systems(Update, record_changed.in_set(RemeshChunks));
        app.update();
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
                let outside = nx < 0 || nz < 0 || nx >= WORLD_SIZE.x || nz >= WORLD_SIZE.z;
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

    #[test]
    fn voxels_are_read_and_written_by_world_position() {
        let mut app = app();
        let terrain = terrain();
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                for pos in [IVec3::new(5, 3, 70), IVec3::new(127, 0, 0)] {
                    assert_eq!(voxels.get(pos), Some(terrain.voxel(pos)));
                }
                // Outside the world there's nothing to read or write.
                for pos in [IVec3::new(-1, 0, 0), IVec3::new(0, WORLD_SIZE.y, 0)] {
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
    fn plugin_spawns_every_chunk_at_its_origin() {
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
        let mut expected: Vec<_> = chunk_coords().map(|c| c.0.to_array()).collect();
        expected.sort();
        assert_eq!(seen, expected);
        // Every bottom chunk has ground in it; empty upper ones get no mesh.
        assert!((64..=256).contains(&meshed), "{meshed} chunks have meshes");
    }
}
