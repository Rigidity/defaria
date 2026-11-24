//! A 16×16×16 voxel grid rendered as one custom mesh.
//!
//! Meshing is deliberately naive: every solid voxel emits all six faces, even
//! the ones buried against a neighbour. Culling hidden faces (and later greedy
//! meshing) is the obvious next step once the grid gets bigger.

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

/// Voxels along each axis of a chunk.
pub const CHUNK_SIZE: usize = 16;
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

pub struct VoxelPlugin;

impl Plugin for VoxelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_chunk, spawn_lights));
    }
}

#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Voxel {
    #[default]
    Air,
    Stone,
    Dirt,
    Grass,
}

impl Voxel {
    pub fn is_solid(self) -> bool {
        self != Voxel::Air
    }

    fn color(self) -> Color {
        match self {
            Voxel::Air => Color::NONE,
            Voxel::Stone => Color::srgb(0.5, 0.5, 0.52),
            Voxel::Dirt => Color::srgb(0.45, 0.3, 0.18),
            Voxel::Grass => Color::srgb(0.3, 0.62, 0.22),
        }
    }
}

/// A cube of voxels. Voxel `(x, y, z)` occupies the unit cube from
/// `(x, y, z)` to `(x + 1, y + 1, z + 1)` in the chunk's local space.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct Chunk {
    voxels: Vec<Voxel>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            voxels: vec![Voxel::Air; CHUNK_VOLUME],
        }
    }
}

impl Chunk {
    /// A completely full chunk: stone, topped with dirt, topped with grass.
    pub fn filled() -> Self {
        let mut chunk = Self::default();
        for (x, y, z) in Self::positions() {
            let voxel = match y {
                y if y == CHUNK_SIZE - 1 => Voxel::Grass,
                y if y >= CHUNK_SIZE - 4 => Voxel::Dirt,
                _ => Voxel::Stone,
            };
            chunk.set(x, y, z, voxel);
        }
        chunk
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> Voxel {
        self.voxels[Self::index(x, y, z)]
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, voxel: Voxel) {
        self.voxels[Self::index(x, y, z)] = voxel;
    }

    fn index(x: usize, y: usize, z: usize) -> usize {
        assert!(
            x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE,
            "voxel ({x}, {y}, {z}) is outside the chunk"
        );
        (y * CHUNK_SIZE + z) * CHUNK_SIZE + x
    }

    fn positions() -> impl Iterator<Item = (usize, usize, usize)> {
        (0..CHUNK_SIZE).flat_map(|y| {
            (0..CHUNK_SIZE).flat_map(move |z| (0..CHUNK_SIZE).map(move |x| (x, y, z)))
        })
    }

    /// Builds a mesh with all six faces of every solid voxel, colored per
    /// voxel through vertex colors.
    pub fn mesh(&self) -> Mesh {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut colors = Vec::new();
        let mut indices = Vec::new();

        for (x, y, z) in Self::positions() {
            let voxel = self.get(x, y, z);
            if !voxel.is_solid() {
                continue;
            }
            let origin = Vec3::new(x as f32, y as f32, z as f32);
            let color = voxel.color().to_linear().to_f32_array();
            for face in &FACES {
                let base = positions.len() as u32;
                positions.extend(face.corners.map(|corner| (origin + corner).to_array()));
                normals.extend([face.normal.to_array(); 4]);
                colors.extend([color; 4]);
                // Two counter-clockwise triangles per quad.
                indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
            }
        }

        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
    }
}

struct Face {
    normal: Vec3,
    /// Corners in counter-clockwise order when viewed from outside the cube.
    corners: [Vec3; 4],
}

const FACES: [Face; 6] = [
    // +X
    Face {
        normal: Vec3::X,
        corners: [
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        ],
    },
    // -X
    Face {
        normal: Vec3::NEG_X,
        corners: [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(0.0, 1.0, 0.0),
        ],
    },
    // +Y
    Face {
        normal: Vec3::Y,
        corners: [
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ],
    },
    // -Y
    Face {
        normal: Vec3::NEG_Y,
        corners: [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
    },
    // +Z
    Face {
        normal: Vec3::Z,
        corners: [
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ],
    },
    // -Z
    Face {
        normal: Vec3::NEG_Z,
        corners: [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ],
    },
];

fn spawn_chunk(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let chunk = Chunk::filled();
    commands.spawn((
        Name::new("Chunk"),
        Mesh3d(meshes.add(chunk.mesh())),
        // White base color so the vertex colors show through unchanged.
        MeshMaterial3d(materials.add(StandardMaterial {
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::default(),
        chunk,
    ));
}

fn spawn_lights(mut commands: Commands) {
    commands.insert_resource(ClearColor(Color::srgb(0.53, 0.75, 0.95)));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 400.0,
        ..default()
    });
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 8_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::default().looking_to(Vec3::new(-0.4, -1.0, -0.6), Vec3::Y),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    #[test]
    fn filled_chunk_is_layered() {
        let chunk = Chunk::filled();
        assert_eq!(chunk.get(3, CHUNK_SIZE - 1, 7), Voxel::Grass);
        assert_eq!(chunk.get(3, CHUNK_SIZE - 2, 7), Voxel::Dirt);
        assert_eq!(chunk.get(3, 0, 7), Voxel::Stone);
    }

    #[test]
    fn every_solid_voxel_gets_six_faces() {
        let mut chunk = Chunk::filled();
        chunk.set(0, 0, 0, Voxel::Air);
        let solid = CHUNK_VOLUME - 1;

        let mesh = chunk.mesh();
        assert_eq!(mesh.count_vertices(), solid * 6 * 4);
        assert_eq!(mesh.indices().unwrap().len(), solid * 6 * 6);
    }

    #[test]
    fn empty_chunk_has_empty_mesh() {
        let mesh = Chunk::default().mesh();
        assert_eq!(mesh.count_vertices(), 0);
        assert_eq!(mesh.indices().unwrap().len(), 0);
    }

    #[test]
    fn single_voxel_spans_its_unit_cube() {
        let mut chunk = Chunk::default();
        chunk.set(2, 3, 4, Voxel::Stone);
        let mesh = chunk.mesh();
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("missing positions");
        };
        let (min, max) = positions
            .iter()
            .map(|&p| Vec3::from(p))
            .fold((Vec3::MAX, Vec3::MIN), |(min, max), p| {
                (min.min(p), max.max(p))
            });
        assert_eq!(min, Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(max, Vec3::new(3.0, 4.0, 5.0));
    }

    /// Each face's winding must point the same way as its normal, otherwise
    /// backface culling hides the outside of the cube.
    #[test]
    fn faces_wind_counter_clockwise_around_their_normal() {
        for face in &FACES {
            let [a, b, c, d] = face.corners;
            for (p, q, r) in [(a, b, c), (a, c, d)] {
                let winding = (q - p).cross(r - p).normalize();
                assert_eq!(winding, face.normal);
            }
        }
    }
}
