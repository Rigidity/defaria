//! Heightmap terrain from Perlin noise, split into a fixed grid of chunks.
//!
//! Each column's height comes from fractal Perlin noise. Below the surface is
//! stone, then a few layers of dirt, then one layer of grass on top. The world
//! is [`WORLD_CHUNKS`] chunks; everything outside it is air.

use bevy::prelude::*;

use crate::{
    perlin::Perlin,
    voxel::{CHUNK_SIZE, Chunk, Voxel},
    voxel_material::BlockMaterial,
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
            .add_systems(Startup, spawn_chunks);
    }
}

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
}

/// Answers "what voxel is at this world position?" for the whole world.
pub struct Terrain {
    settings: TerrainSettings,
    noise: Perlin,
}

impl Terrain {
    pub fn new(settings: TerrainSettings) -> Self {
        Self {
            noise: Perlin::new(settings.seed),
            settings,
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
    pub fn voxel(&self, pos: IVec3) -> Voxel {
        if pos.cmplt(IVec3::ZERO).any() || pos.cmpge(WORLD_SIZE).any() {
            return Voxel::Air;
        }
        column_voxel(pos.y, self.height(pos.x, pos.z))
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
                    chunk.set(x, y, z, column_voxel(origin.y + y as i32, height));
                }
            }
        }
        chunk
    }

    /// Meshes a chunk, culling faces against its neighbours in the world.
    pub fn mesh(&self, coord: ChunkCoord, chunk: &Chunk) -> Mesh {
        let origin = coord.origin();
        chunk.mesh(|local| self.voxel(origin + local))
    }
}

/// The voxel at height `y` in a column whose surface is `height` voxels tall.
fn column_voxel(y: i32, height: i32) -> Voxel {
    match y {
        y if y >= height => Voxel::Air,
        y if y == height - 1 => Voxel::Grass,
        y if y >= height - 1 - DIRT_DEPTH => Voxel::Dirt,
        _ => Voxel::Stone,
    }
}

/// Every chunk coordinate in the world grid.
pub fn chunk_coords() -> impl Iterator<Item = ChunkCoord> {
    (0..WORLD_CHUNKS.x).flat_map(|x| {
        (0..WORLD_CHUNKS.y)
            .flat_map(move |y| (0..WORLD_CHUNKS.z).map(move |z| ChunkCoord(IVec3::new(x, y, z))))
    })
}

fn spawn_chunks(
    mut commands: Commands,
    settings: Res<TerrainSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<BlockMaterial>,
) {
    let terrain = Terrain::new(settings.clone());
    for coord in chunk_coords() {
        let chunk = terrain.chunk(coord);
        let mesh = terrain.mesh(coord, &chunk);
        let IVec3 { x, y, z } = coord.0;
        let mut entity = commands.spawn((
            Name::new(format!("Chunk ({x}, {y}, {z})")),
            Transform::from_translation(coord.origin().as_vec3()),
            coord,
            chunk,
        ));
        // Chunks with nothing visible (all air, or buried) get no mesh; the
        // renderer warns about meshes with no vertices.
        if mesh.count_vertices() > 0 {
            entity.insert((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.material.clone()),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain() -> Terrain {
        Terrain::new(TerrainSettings::default())
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
            assert_eq!(at(height - 1), Voxel::Grass);
            for y in height..WORLD_SIZE.y {
                assert_eq!(at(y), Voxel::Air);
            }
            for y in 0..height - 1 {
                let expected = if y >= height - 1 - DIRT_DEPTH {
                    Voxel::Dirt
                } else {
                    Voxel::Stone
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
            assert_eq!(terrain.voxel(pos), Voxel::Air);
        }
        assert_eq!(terrain.voxel(IVec3::ZERO), Voxel::Stone);
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
        let b = Terrain::new(TerrainSettings {
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

    /// Faces between two solid voxels in different chunks are culled, so the
    /// whole world's faces equal the faces of the terrain as one solid.
    #[test]
    fn no_faces_hidden_between_chunks() {
        let terrain = terrain();
        let total: usize = chunk_coords()
            .map(|coord| terrain.mesh(coord, &terrain.chunk(coord)).count_vertices() / 4)
            .sum();

        // Count exposed faces directly from the heightmap.
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
    fn plugin_spawns_every_chunk_at_its_origin() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), TerrainPlugin))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial {
                material: Handle::default(),
                atlas: Handle::default(),
                blocks: Handle::default(),
            });
        app.update();

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
