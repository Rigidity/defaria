//! 16×16×16 voxel chunks, each rendered as one custom mesh, plus the world's
//! sky color. The terrain module decides what goes in the chunks, and
//! `voxel_material` draws them.
//!
//! A chunk holds a [`BlockState`] per voxel; what each state looks like
//! comes from the mods (see `content`). Each face's vertices carry a UV
//! within its tile (UV_0) and the tile's layer in the block texture array
//! (UV_1.x).
//!
//! Meshing copies each block state's baked model faces into the chunk mesh,
//! leaving out the faces covered by the neighbour on their `cull` side,
//! including across chunk borders (the caller says what lies outside the
//! chunk). Greedy meshing is the next step if vertex counts become a
//! problem.

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::content::{BlockState, Content};

/// Voxels along each axis of a chunk.
pub const CHUNK_SIZE: usize = 16;
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

pub struct VoxelPlugin;

impl Plugin for VoxelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, set_sky_color);
    }
}

/// A cube of voxels. Voxel `(x, y, z)` occupies the unit cube from
/// `(x, y, z)` to `(x + 1, y + 1, z + 1)` in the chunk's local space.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct Chunk {
    voxels: Vec<BlockState>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            voxels: vec![BlockState::AIR; CHUNK_VOLUME],
        }
    }
}

impl Chunk {
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        self.voxels[Self::index(x, y, z)]
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) {
        self.voxels[Self::index(x, y, z)] = state;
    }

    /// [`Chunk::get`] for a chunk-local position given as an `IVec3`.
    pub fn at(&self, pos: IVec3) -> BlockState {
        self.get(pos.x as usize, pos.y as usize, pos.z as usize)
    }

    /// [`Chunk::set`] for a chunk-local position given as an `IVec3`.
    pub fn set_at(&mut self, pos: IVec3, state: BlockState) {
        self.set(pos.x as usize, pos.y as usize, pos.z as usize, state);
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

    /// The voxel at a chunk-local position that may lie outside the chunk:
    /// inside, it comes from the chunk; outside, from `outside`.
    fn get_or(&self, pos: IVec3, outside: &impl Fn(IVec3) -> BlockState) -> BlockState {
        let size = CHUNK_SIZE as i32;
        if pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(size)).all() {
            self.at(pos)
        } else {
            outside(pos)
        }
    }

    /// Builds a mesh of every voxel's model faces, leaving out each face
    /// whose neighbour on its `cull` side covers the side facing it.
    /// `outside` gives the voxel at a chunk-local position just beyond the
    /// chunk's edge, so faces buried against a neighbouring chunk are
    /// skipped too.
    pub fn mesh(&self, content: &Content, outside: impl Fn(IVec3) -> BlockState) -> Mesh {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut layers = Vec::new();
        let mut indices = Vec::new();

        for (x, y, z) in Self::positions() {
            let voxel = self.get(x, y, z);
            if voxel.is_air() {
                continue;
            }
            let origin = Vec3::new(x as f32, y as f32, z as f32);
            let pos = IVec3::new(x as i32, y as i32, z as i32);
            for face in &content.state(voxel).faces {
                if let Some(side) = face.cull {
                    let neighbour = self.get_or(pos + side.normal(), &outside);
                    if content.state(neighbour).occludes(side.opposite()) {
                        continue;
                    }
                }
                let base = positions.len() as u32;
                for vertex in &face.vertices {
                    positions.push((origin + vertex.pos).to_array());
                    normals.push(face.normal.to_array());
                    uvs.push(vertex.uv.to_array());
                    layers.push([face.layer as f32, 0.0]);
                }
                // A triangle, or a quad as two counter-clockwise triangles.
                let corners: &[u32] = if face.vertices.len() == 3 {
                    &[0, 1, 2]
                } else {
                    &[0, 1, 2, 0, 2, 3]
                };
                indices.extend(corners.iter().map(|i| base + i));
            }
        }

        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        // The shader reads the face's texture array layer from UV_1.x.
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, layers)
        .with_inserted_indices(Indices::U32(indices))
    }
}

fn set_sky_color(mut commands: Commands) {
    commands.insert_resource(ClearColor(Color::srgb(0.53, 0.75, 0.95)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Direction, base, block};
    use bevy::mesh::{MeshVertexAttribute, VertexAttributeValues};

    /// A completely full chunk: stone, topped with dirt, topped with grass.
    fn filled() -> Chunk {
        let mut chunk = Chunk::default();
        for (x, y, z) in Chunk::positions() {
            let state = match y {
                y if y == CHUNK_SIZE - 1 => block("grass"),
                y if y >= CHUNK_SIZE - 4 => block("dirt"),
                _ => block("stone"),
            };
            chunk.set(x, y, z, state);
        }
        chunk
    }

    fn air(_: IVec3) -> BlockState {
        BlockState::AIR
    }

    fn mesh(chunk: &Chunk, outside: impl Fn(IVec3) -> BlockState) -> Mesh {
        chunk.mesh(base(), outside)
    }

    fn faces(mesh: &Mesh) -> usize {
        assert_eq!(mesh.indices().unwrap().len(), mesh.count_vertices() / 4 * 6);
        mesh.count_vertices() / 4
    }

    #[test]
    fn lone_voxel_gets_six_faces() {
        let mut chunk = Chunk::default();
        chunk.set(5, 5, 5, block("dirt"));
        assert_eq!(faces(&mesh(&chunk, air)), 6);
    }

    #[test]
    fn faces_between_solid_voxels_are_culled() {
        let mut chunk = Chunk::default();
        chunk.set(5, 5, 5, block("dirt"));
        chunk.set(6, 5, 5, block("stone"));
        assert_eq!(faces(&mesh(&chunk, air)), 10);
    }

    #[test]
    fn filled_chunk_shows_only_its_surface() {
        let side = CHUNK_SIZE * CHUNK_SIZE;
        assert_eq!(faces(&mesh(&filled(), air)), 6 * side);
        // Solid all around: nothing is visible.
        assert_eq!(faces(&mesh(&filled(), |_| block("stone"))), 0);
        // Solid below only (like a chunk stacked on another): the bottom goes.
        let below = |pos: IVec3| {
            if pos.y < 0 {
                block("stone")
            } else {
                BlockState::AIR
            }
        };
        assert_eq!(faces(&mesh(&filled(), below)), 5 * side);
    }

    #[test]
    fn outside_is_queried_in_chunk_local_coordinates() {
        let mut chunk = Chunk::default();
        chunk.set(0, 0, 0, block("stone"));
        let queried = std::cell::RefCell::new(Vec::new());
        mesh(&chunk, |pos| {
            queried.borrow_mut().push(pos);
            BlockState::AIR
        });
        let mut queried = queried.into_inner();
        queried.sort_by_key(|p| p.to_array());
        assert_eq!(queried, [IVec3::NEG_X, IVec3::NEG_Y, IVec3::NEG_Z]);
    }

    #[test]
    fn empty_chunk_has_empty_mesh() {
        let mesh = mesh(&Chunk::default(), air);
        assert_eq!(mesh.count_vertices(), 0);
        assert_eq!(mesh.indices().unwrap().len(), 0);
    }

    fn vec3s(mesh: &Mesh, attribute: MeshVertexAttribute) -> Vec<Vec3> {
        let Some(VertexAttributeValues::Float32x3(values)) = mesh.attribute(attribute) else {
            panic!("missing {attribute:?}");
        };
        values.iter().map(|&v| Vec3::from(v)).collect()
    }

    fn vec2s(mesh: &Mesh, attribute: MeshVertexAttribute) -> Vec<Vec2> {
        let Some(VertexAttributeValues::Float32x2(values)) = mesh.attribute(attribute) else {
            panic!("missing {attribute:?}");
        };
        values.iter().map(|&v| Vec2::from(v)).collect()
    }

    #[test]
    fn single_voxel_spans_its_unit_cube() {
        let mut chunk = Chunk::default();
        chunk.set(2, 3, 4, block("stone"));
        let positions = vec3s(&mesh(&chunk, air), Mesh::ATTRIBUTE_POSITION);
        let (min, max) = positions
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(min, max), &p| {
                (min.min(p), max.max(p))
            });
        assert_eq!(min, Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(max, Vec3::new(3.0, 4.0, 5.0));
    }

    /// The cube model's faces: one per side, each culled against its own
    /// side, with corners starting at the bottom left of the texture and
    /// running counter-clockwise from outside, so the texture is neither
    /// mirrored nor rotated and side faces have their top edge at +Y.
    #[test]
    fn cube_faces_start_bottom_left_and_go_round() {
        let stone = base().state(block("stone"));
        let mut sides: Vec<Direction> = stone.faces.iter().filter_map(|f| f.cull).collect();
        sides.sort_by_key(|d| *d as u8);
        assert_eq!(sides, Direction::ALL);
        for face in &stone.faces {
            let side = face.cull.unwrap();
            assert_eq!(face.normal, side.normal().as_vec3());
            assert!(stone.occludes(side));
            let [bl, br, tr, tl] = [0, 1, 2, 3].map(|i| face.vertices[i].pos);
            let (right, up) = (br - bl, tl - bl);
            assert_eq!(right.cross(up), face.normal);
            assert_eq!(tr, bl + right + up);
            if face.normal.y == 0.0 {
                assert_eq!(up, Vec3::Y, "side face {side:?} isn't upright");
            }
            let uvs = face.vertices.iter().map(|v| v.uv).collect::<Vec<_>>();
            // `v` points down the image, so its top row is the face's top.
            assert_eq!(uvs, [Vec2::Y, Vec2::ONE, Vec2::X, Vec2::ZERO]);
        }
    }

    /// Each face's winding must point the same way as its normal, otherwise
    /// backface culling hides the outside of the cube.
    #[test]
    fn faces_wind_counter_clockwise_around_their_normal() {
        for name in ["stone", "log"] {
            for state in [block(name), BlockState(block(name).0 + 1)] {
                for face in &base().state(state).faces {
                    let [a, b, c, d] = [0, 1, 2, 3].map(|i| face.vertices[i].pos);
                    for (p, q, r) in [(a, b, c), (a, c, d)] {
                        let winding = (q - p).cross(r - p).normalize();
                        assert_eq!(winding, face.normal);
                    }
                }
            }
        }
    }

    /// The tile on each side, by the side's normal.
    fn tiles(state: BlockState) -> Vec<(IVec3, String)> {
        let content = base();
        let mut tiles: Vec<_> = content
            .state(state)
            .faces
            .iter()
            .map(|face| {
                let id = &content.textures[face.layer as usize].id;
                (
                    face.normal.as_ivec3(),
                    id.trim_start_matches("defaria:").to_owned(),
                )
            })
            .collect();
        tiles.sort_by_key(|(normal, _)| normal.to_array());
        tiles
    }

    fn tile_on(state: BlockState, normal: IVec3) -> String {
        tiles(state)
            .into_iter()
            .find(|(n, _)| *n == normal)
            .unwrap()
            .1
    }

    #[test]
    fn grass_has_a_top_sides_and_a_dirt_bottom() {
        let grass = block("grass");
        assert_eq!(tile_on(grass, IVec3::Y), "grass_top");
        assert_eq!(tile_on(grass, IVec3::NEG_Y), "dirt");
        for side in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
            assert_eq!(tile_on(grass, side), "grass_side");
        }
        assert_eq!(tile_on(block("stone"), IVec3::Y), "stone");
    }

    /// A log has rings on the ends of its axis and bark around it.
    #[test]
    fn logs_show_their_rings_along_their_axis() {
        let log = base().block("defaria:log").unwrap();
        for (axis, normal) in [("y", IVec3::Y), ("x", IVec3::X), ("z", IVec3::Z)] {
            let state = log.state(&[("axis", axis)]).unwrap();
            for (side, tile) in tiles(state) {
                let expected = if side.abs() == normal {
                    "log_top"
                } else {
                    "log_side"
                };
                assert_eq!(tile, expected, "axis {axis}, side {side}");
            }
        }
    }

    #[test]
    fn each_face_samples_its_own_tile_layer() {
        let mut chunk = Chunk::default();
        chunk.set(1, 1, 1, block("grass"));
        let mesh = mesh(&chunk, air);
        let normals = vec3s(&mesh, Mesh::ATTRIBUTE_NORMAL);
        let layers = vec2s(&mesh, Mesh::ATTRIBUTE_UV_1);
        let content = base();
        for (normal, layer) in normals.iter().zip(&layers) {
            let tile = tile_on(block("grass"), normal.as_ivec3());
            let expected = content.texture(&format!("defaria:{tile}")).unwrap();
            assert_eq!(*layer, Vec2::new(expected as f32, 0.0));
        }
    }
}
