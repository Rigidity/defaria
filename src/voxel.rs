//! 16×16×16 voxel chunks, each rendered as one custom mesh, plus the world's
//! sky color. The terrain module decides what goes in the chunks, `light`
//! lights them, and `voxel_material` draws them.
//!
//! A chunk holds a [`BlockState`] per voxel; what each state looks like
//! comes from the mods (see `content`).
//!
//! Meshing copies each block state's baked model faces into the chunk mesh,
//! leaving out the faces covered by the neighbour on their `cull` side,
//! including across chunk borders. It reads a [`Neighborhood`]: a copy of the
//! chunk's blocks and light plus the one-voxel layer around it from the
//! neighbouring chunks, so meshing never looks anything up in other chunks.
//! Greedy meshing is the next step if vertex counts become a problem.
//!
//! # Lighting in the mesh
//!
//! Each corner of a face gets an ambient occlusion level and a light level
//! per channel, Minecraft's "smooth lighting":
//!
//! - **Ambient occlusion** looks at the three voxels touching the corner in
//!   the layer in front of the face: the two along its edges and the one
//!   diagonally across. Each opaque one darkens the corner a step; with both
//!   edges blocked the corner is fully dark, whatever the diagonal is.
//! - **Light** is the average of those voxels and the one right in front of
//!   the face, leaving out opaque ones (they hold no light) and the
//!   diagonal when both edges are blocked (light can't get through there).
//!
//! The shader blends the four corners bilinearly across the face, rather
//! than letting the GPU interpolate across the quad's two triangles. That
//! interpolation depends on which way the diagonal runs and shows up as
//! diagonal bands and creases; bilinear blending is symmetric, so every
//! arrangement of light and shadow comes out smooth. So every vertex of a
//! face carries all four corners' values ([`ATTRIBUTE_LIGHT`],
//! [`ATTRIBUTE_FACE`]) plus where it sits on the face ([`ATTRIBUTE_CORNER`]).
//!
//! Faces of blocks that give off light are drawn at full brightness, so
//! light sources glow.

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, MeshVertexAttribute, PrimitiveTopology, VertexFormat},
    prelude::*,
};

use crate::{
    content::{BakedFace, BlockState, Content, MAX_LIGHT, Render},
    light::{ChunkLight, Light},
};

/// Voxels along each axis of a chunk.
pub const CHUNK_SIZE: usize = 16;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Where a vertex sits on its face, for blending the corners: `(0, 0)` at
/// the first vertex, `(1, 0)` at the second, `(1, 1)` at the third and
/// `(0, 1)` at the fourth.
pub const ATTRIBUTE_CORNER: MeshVertexAttribute =
    MeshVertexAttribute::new("Corner", 0x0DEF_A41A_0001, VertexFormat::Float32x2);
/// The light at each of the face's four corners, in the order of
/// [`ATTRIBUTE_CORNER`]: sky, red, green and blue, one byte each from the
/// lowest, where 255 is [`MAX_LIGHT`]. The same on every vertex of a face.
pub const ATTRIBUTE_LIGHT: MeshVertexAttribute =
    MeshVertexAttribute::new("Light", 0x0DEF_A41A_0002, VertexFormat::Uint32x4);
/// The face's layer in the block texture array, then its four corners'
/// ambient occlusion, one byte each from the lowest: 3 is open, 0 is fully
/// occluded. The same on every vertex of a face.
pub const ATTRIBUTE_FACE: MeshVertexAttribute =
    MeshVertexAttribute::new("Face", 0x0DEF_A41A_0003, VertexFormat::Uint32x2);

/// What lies outside every loaded chunk, as the mesh sees it: open air in
/// full sky light, so faces at the edge of the world are lit.
pub const OPEN_SKY: Light = Light::new(MAX_LIGHT, [0; 3]);

pub struct VoxelPlugin;

impl Plugin for VoxelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, set_sky_color);
    }
}

/// A cube of voxels. Voxel `(x, y, z)` occupies the unit cube from
/// `(x, y, z)` to `(x + 1, y + 1, z + 1)` in the chunk's local space.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Component)]
#[require(ChunkLight)]
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
    /// A chunk with these voxels, in [`Chunk::index_of`] order. `None`
    /// unless there are exactly [`CHUNK_VOLUME`].
    pub fn from_voxels(voxels: Vec<BlockState>) -> Option<Self> {
        (voxels.len() == CHUNK_VOLUME).then_some(Self { voxels })
    }

    /// A chunk of one block throughout.
    pub fn filled(state: BlockState) -> Self {
        Self {
            voxels: vec![state; CHUNK_VOLUME],
        }
    }

    /// Every voxel, in [`Chunk::index_of`] order.
    pub fn voxels(&self) -> &[BlockState] {
        &self.voxels
    }

    #[cfg(test)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        self.voxels[Self::index(x, y, z)]
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) {
        self.voxels[Self::index(x, y, z)] = state;
    }

    /// [`Chunk::get`] for a chunk-local position given as an `IVec3`.
    pub fn at(&self, pos: IVec3) -> BlockState {
        self.voxels[Self::index_of(pos)]
    }

    /// [`Chunk::set`] for a chunk-local position given as an `IVec3`.
    pub fn set_at(&mut self, pos: IVec3, state: BlockState) {
        self.voxels[Self::index_of(pos)] = state;
    }

    /// The voxel at an index from [`Chunk::index_of`].
    pub fn get_index(&self, index: usize) -> BlockState {
        self.voxels[index]
    }

    /// Where the voxel at a chunk-local position is stored. Chunk light
    /// uses the same order.
    pub fn index_of(pos: IVec3) -> usize {
        Self::index(pos.x as usize, pos.y as usize, pos.z as usize)
    }

    /// The chunk-local position of the voxel stored at `index`.
    pub fn position_of(index: usize) -> IVec3 {
        let size = CHUNK_SIZE;
        IVec3::new(
            (index % size) as i32,
            (index / (size * size)) as i32,
            (index / size % size) as i32,
        )
    }

    fn index(x: usize, y: usize, z: usize) -> usize {
        assert!(
            x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE,
            "voxel ({x}, {y}, {z}) is outside the chunk"
        );
        (y * CHUNK_SIZE + z) * CHUNK_SIZE + x
    }

    pub(crate) fn positions() -> impl Iterator<Item = (usize, usize, usize)> {
        (0..CHUNK_SIZE).flat_map(|y| {
            (0..CHUNK_SIZE).flat_map(move |z| (0..CHUNK_SIZE).map(move |x| (x, y, z)))
        })
    }

    /// Meshes the chunk with `outside` giving the voxels around it, all in
    /// full sky light.
    #[cfg(test)]
    pub fn mesh(&self, content: &Content, outside: impl Fn(IVec3) -> BlockState) -> Mesh {
        let inside = |pos: IVec3| pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(16)).all();
        Neighborhood::from_fn(|pos| {
            let block = if inside(pos) {
                self.at(pos)
            } else {
                outside(pos)
            };
            (block, OPEN_SKY)
        })
        .mesh(content)
    }
}

/// The axis an axis-aligned vector points along: 0, 1 or 2 for X, Y or Z.
pub fn axis_of(direction: IVec3) -> usize {
    if direction.x != 0 {
        0
    } else if direction.y != 0 {
        1
    } else {
        2
    }
}

/// The offsets of the chunks whose meshes show the voxel at a chunk-local
/// position: zero (its own chunk), and one for every chunk across the faces,
/// edges and corners it lies on, since a voxel shades the corners of faces
/// next to it.
pub fn touching_offsets(local: IVec3) -> impl Iterator<Item = IVec3> {
    let last = CHUNK_SIZE as i32 - 1;
    let edge = local.map(|v| match v {
        0 => -1,
        v if v == last => 1,
        _ => 0,
    });
    // One bit per axis: step across that axis's edge or not. Steps along an
    // axis the voxel isn't on an edge of would repeat an offset.
    (0..8)
        .filter(move |bits| (0..3).all(|axis| bits & (1 << axis) == 0 || edge[axis] != 0))
        .map(move |bits| {
            IVec3::new(
                if bits & 1 != 0 { edge.x } else { 0 },
                if bits & 2 != 0 { edge.y } else { 0 },
                if bits & 4 != 0 { edge.z } else { 0 },
            )
        })
}

/// Voxels along each axis of a [`Neighborhood`].
const PADDED: usize = CHUNK_SIZE + 2;

/// A chunk's blocks and light and the layer of voxels around it, copied out
/// for meshing. Positions are chunk-local, from -1 to [`CHUNK_SIZE`].
#[derive(Debug, Clone)]
pub struct Neighborhood {
    blocks: Vec<BlockState>,
    light: Vec<Light>,
}

impl Neighborhood {
    /// Copies the chunk at offset zero from `chunk` and the edges of the
    /// chunks around it, by their offset (each coordinate -1, 0 or 1). A
    /// chunk that isn't there reads as open sky.
    pub fn gather<'a>(chunk: impl Fn(IVec3) -> Option<(&'a Chunk, &'a ChunkLight)>) -> Self {
        let chunks: [_; 27] = std::array::from_fn(|i| {
            let i = i as i32;
            chunk(IVec3::new(i % 3, i / 3 % 3, i / 9) - IVec3::ONE)
        });
        let size = IVec3::splat(CHUNK_SIZE as i32);
        Self::from_fn(|pos| {
            let offset = pos.div_euclid(size) + IVec3::ONE;
            match chunks[(offset.x + offset.y * 3 + offset.z * 9) as usize] {
                Some((chunk, light)) => {
                    let local = pos.rem_euclid(size);
                    (chunk.at(local), light.at(local))
                }
                None => (BlockState::AIR, OPEN_SKY),
            }
        })
    }

    /// Fills every position from `voxel`.
    pub fn from_fn(voxel: impl Fn(IVec3) -> (BlockState, Light)) -> Self {
        let mut blocks = Vec::with_capacity(PADDED.pow(3));
        let mut light = Vec::with_capacity(PADDED.pow(3));
        let range = -1..=CHUNK_SIZE as i32;
        for y in range.clone() {
            for z in range.clone() {
                for x in range.clone() {
                    let (block, level) = voxel(IVec3::new(x, y, z));
                    blocks.push(block);
                    light.push(level);
                }
            }
        }
        Self { blocks, light }
    }

    fn index(pos: IVec3) -> usize {
        let p = (pos + IVec3::ONE).as_uvec3();
        debug_assert!(
            p.cmplt(UVec3::splat(PADDED as u32)).all(),
            "{pos} is outside"
        );
        ((p.y as usize * PADDED) + p.z as usize) * PADDED + p.x as usize
    }

    pub fn block(&self, pos: IVec3) -> BlockState {
        self.blocks[Self::index(pos)]
    }

    pub fn light(&self, pos: IVec3) -> Light {
        self.light[Self::index(pos)]
    }

    /// Builds the meshes of every voxel's model faces in the middle chunk,
    /// one per way faces are drawn (see [`Render`]), leaving out each face
    /// its neighbour on its `cull` side hides (see [`Content::hides`]),
    /// with ambient occlusion and light at every corner.
    pub fn meshes(&self, content: &Content) -> ChunkMeshes {
        let opaque: Vec<bool> = self
            .blocks
            .iter()
            .map(|&block| content.state(block).is_opaque())
            .collect();
        let opaque_at = |pos: IVec3| opaque[Self::index(pos)];

        let mut builders: [MeshBuilder; 3] = Default::default();
        // Only translucent faces are sorted.
        builders[Render::Translucent as usize].sorted = true;
        for (x, y, z) in Chunk::positions() {
            let pos = IVec3::new(x as i32, y as i32, z as i32);
            let voxel = self.block(pos);
            if voxel.is_air() {
                continue;
            }
            let state = content.state(voxel);
            let glows = state.emits_light();
            let builder = &mut builders[state.render as usize];
            for face in &state.faces {
                if face
                    .hidden_by(|side| content.hides(voxel, self.block(pos + side.normal()), side))
                {
                    continue;
                }
                let shading = self.shade_face(pos, face, glows, &opaque_at);
                builder.push(pos.as_vec3(), face, &shading, false);
                if state.double_sided {
                    builder.push(pos.as_vec3(), face, &shading, true);
                }
            }
        }
        let [opaque, cutout, mut translucent] = builders;
        let faces = TranslucentFaces::new(std::mem::take(&mut translucent.sort_faces));
        ChunkMeshes {
            opaque: opaque.build(),
            cutout: cutout.build(),
            translucent: translucent.build(),
            translucent_faces: faces,
        }
    }

    /// The opaque mesh alone, for tests of meshes with nothing see-through.
    #[cfg(test)]
    pub fn mesh(&self, content: &Content) -> Mesh {
        self.meshes(content).opaque
    }

    /// The light and ambient occlusion at a face's corners, packed for the
    /// vertex attributes.
    fn shade_face(
        &self,
        pos: IVec3,
        face: &BakedFace,
        glows: bool,
        opaque: &impl Fn(IVec3) -> bool,
    ) -> FaceShading {
        let mut corners = [CornerShading::default(); 4];
        let normal = face.normal.as_ivec3();
        let axis_aligned = normal.abs().element_sum() == 1 && normal.as_vec3() == face.normal;
        // A face on the block's boundary sees the voxel it faces; one inside
        // the block (no `cull`) sees the block's own voxel.
        let front = if face.cull.is_some() {
            pos + normal
        } else {
            pos
        };
        for (i, vertex) in face.vertices.iter().enumerate() {
            let corner = if face.vertices.len() == 3 && i == 2 {
                3
            } else {
                i
            };
            corners[corner] = if axis_aligned {
                self.shade_corner(front, normal, vertex.pos, opaque)
            } else {
                CornerShading::from_light(self.light(pos), 1, OPEN)
            };
        }
        if face.vertices.len() == 3 {
            // Extend the triangle's blend to the made-up corner, so it stays
            // flat (affine) over the triangle.
            corners[2] = corners[1].extend(corners[3], corners[0]);
        }
        if glows {
            for corner in &mut corners {
                corner.light[1..].fill(u8::MAX);
                corner.occlusion = OPEN;
            }
        }
        FaceShading {
            light: corners.map(|c| u32::from_le_bytes(c.light)),
            occlusion: u32::from_le_bytes(corners.map(|c| c.occlusion)),
        }
    }

    /// The light and ambient occlusion at one corner of a face whose front
    /// layer is at `front` (the voxel right in front of it), facing along
    /// the axis-aligned `normal`. `vertex` is the corner's position in its
    /// block.
    fn shade_corner(
        &self,
        front: IVec3,
        normal: IVec3,
        vertex: Vec3,
        opaque: &impl Fn(IVec3) -> bool,
    ) -> CornerShading {
        let axis = axis_of(normal);
        // Toward the corner along each of the face's two directions.
        let toward = |along: usize| {
            let mut step = IVec3::ZERO;
            step[along] = if vertex[along] < 0.5 { -1 } else { 1 };
            step
        };
        let (a, b) = (toward((axis + 1) % 3), toward((axis + 2) % 3));
        let (side_a, side_b, diagonal) = (front + a, front + b, front + a + b);
        let (blocked_a, blocked_b) = (opaque(side_a), opaque(side_b));
        let blocked_diagonal = opaque(diagonal);
        let occlusion = if blocked_a && blocked_b {
            0
        } else {
            OPEN - blocked_a as u8 - blocked_b as u8 - blocked_diagonal as u8
        };
        let lit = [
            (front, !opaque(front)),
            (side_a, !blocked_a),
            (side_b, !blocked_b),
            (diagonal, !blocked_diagonal && !(blocked_a && blocked_b)),
        ];
        let mut sum = [0u32; 4];
        let mut count = 0;
        for (voxel, _) in lit.iter().filter(|(_, open)| *open) {
            let light = self.light(*voxel);
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += light.get(channel) as u32;
            }
            count += 1;
        }
        if count == 0 {
            // Every voxel around the corner is opaque: use the one in front.
            return CornerShading::from_light(self.light(front), 1, occlusion);
        }
        CornerShading::from_sum(sum, count, occlusion)
    }
}

/// A chunk's meshes, one per [`Render`]. A mesh with no vertices has
/// nothing to draw.
pub struct ChunkMeshes {
    pub opaque: Mesh,
    pub cutout: Mesh,
    /// Its faces are in no particular order until sorted with
    /// `translucent_faces`.
    pub translucent: Mesh,
    pub translucent_faces: TranslucentFaces,
}

#[cfg(test)]
impl ChunkMeshes {
    /// The mesh drawn the way `render` says.
    pub fn get(&self, render: Render) -> &Mesh {
        match render {
            Render::Opaque => &self.opaque,
            Render::Cutout => &self.cutout,
            Render::Translucent => &self.translucent,
        }
    }
}

/// A translucent mesh's faces, for drawing them back to front: blended
/// faces only look right drawn over everything behind them.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct TranslucentFaces {
    faces: Vec<SortFace>,
    /// Whether the order can matter: false when every face lies in one
    /// plane (open water's surface, front and back), where no face is ever
    /// behind another.
    needs_sorting: bool,
}

/// One face of a translucent mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SortFace {
    /// The middle of the face, chunk-local.
    center: Vec3,
    /// Its plane's normal, either way round (pointing toward +X, or +Y,
    /// or +Z, whichever it has first), so a face and its back match.
    normal: Vec3,
    /// Its first vertex in the mesh.
    first: u32,
    /// Its triangles, from its first vertex.
    indices: &'static [u32],
}

impl TranslucentFaces {
    fn new(faces: Vec<SortFace>) -> Self {
        let plane = |face: &SortFace| (face.normal, face.normal.dot(face.center));
        let needs_sorting = faces.first().is_some_and(|first| {
            let (normal, offset) = plane(first);
            faces.iter().any(|face| {
                let (n, d) = plane(face);
                n.distance(normal) > 1e-4 || (d - offset).abs() > 1e-4
            })
        });
        Self {
            // Open water would keep hundreds of faces it never sorts.
            faces: if needs_sorting { faces } else { Vec::new() },
            needs_sorting,
        }
    }

    /// Whether the faces' order can matter: see `needs_sorting`.
    pub fn needs_sorting(&self) -> bool {
        self.needs_sorting
    }

    /// The mesh's indices with its faces ordered farthest first from
    /// `eye`, chunk-local.
    pub fn indices_from(&self, eye: Vec3) -> Vec<u32> {
        let mut order: Vec<(f32, &SortFace)> = self
            .faces
            .iter()
            .map(|face| (face.center.distance_squared(eye), face))
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        order
            .into_iter()
            .flat_map(|(_, face)| face.indices.iter().map(move |i| face.first + i))
            .collect()
    }
}

/// `normal` or its opposite, whichever points toward +X, or failing that
/// +Y, or failing that +Z.
fn either_way(normal: Vec3) -> Vec3 {
    let first = [normal.x, normal.y, normal.z]
        .into_iter()
        .find(|c| c.abs() > 1e-6)
        .unwrap_or(1.0);
    normal * first.signum()
}

/// The vertex data of one chunk mesh, as it's built.
#[derive(Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    corners: Vec<[f32; 2]>,
    lights: Vec<[u32; 4]>,
    faces: Vec<[u32; 2]>,
    indices: Vec<u32>,
    /// Whether to record `sort_faces`, for a translucent mesh.
    sorted: bool,
    sort_faces: Vec<SortFace>,
}

impl MeshBuilder {
    /// Adds a face of the block at `origin`, or its back: the same face
    /// facing the other way, with the front's light and shading.
    fn push(&mut self, origin: Vec3, face: &BakedFace, shading: &FaceShading, back: bool) {
        let base = self.positions.len() as u32;
        let count = face.vertices.len();
        let triangle = count == 3;
        let (normal, light, occlusion) = if back {
            (-face.normal, shading.back_light(), shading.back_occlusion())
        } else {
            (face.normal, shading.light, shading.occlusion)
        };
        let mut center = Vec3::ZERO;
        for i in 0..count {
            // The back runs the other way round: the first vertex, then the
            // rest in reverse, which swaps the second and fourth corners.
            let vertex = &face.vertices[if back && i > 0 { count - i } else { i }];
            let pos = origin + vertex.pos;
            center += pos;
            self.positions.push(pos.to_array());
            self.normals.push(normal.to_array());
            self.uvs.push(vertex.uv.to_array());
            // A triangle's third vertex is the fourth corner; the third is
            // made up so the blend is flat across it.
            let corner = if triangle && i == 2 { 3 } else { i };
            self.corners.push(CORNERS[corner]);
            self.lights.push(light);
            self.faces.push([face.layer, occlusion]);
        }
        self.indices.extend(face.indices().iter().map(|i| base + i));
        if self.sorted {
            self.sort_faces.push(SortFace {
                center: center / count as f32,
                normal: either_way(face.normal),
                first: base,
                indices: face.indices(),
            });
        }
    }

    fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_attribute(ATTRIBUTE_CORNER, self.corners)
        .with_inserted_attribute(ATTRIBUTE_LIGHT, self.lights)
        .with_inserted_attribute(ATTRIBUTE_FACE, self.faces)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// [`ATTRIBUTE_CORNER`] for each corner.
const CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
/// The ambient occlusion level of a corner with nothing around it.
const OPEN: u8 = 3;
/// A light level's step in the packed bytes: [`MAX_LIGHT`] packs to 255.
const LEVEL_STEP: f32 = u8::MAX as f32 / MAX_LIGHT as f32;

/// One face's [`ATTRIBUTE_LIGHT`] and the occlusion half of its
/// [`ATTRIBUTE_FACE`].
struct FaceShading {
    light: [u32; 4],
    occlusion: u32,
}

impl FaceShading {
    /// The light for the face's back, whose second and fourth corners are
    /// swapped (see [`MeshBuilder::push`]). The made-up third corner of a
    /// triangle is the same either way round.
    fn back_light(&self) -> [u32; 4] {
        let [a, b, c, d] = self.light;
        [a, d, c, b]
    }

    fn back_occlusion(&self) -> u32 {
        let [a, b, c, d] = self.occlusion.to_le_bytes();
        u32::from_le_bytes([a, d, c, b])
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct CornerShading {
    /// Sky, red, green and blue, 255 at [`MAX_LIGHT`].
    light: [u8; 4],
    occlusion: u8,
}

impl CornerShading {
    fn from_light(light: Light, count: u32, occlusion: u8) -> Self {
        Self::from_sum(
            std::array::from_fn(|channel| light.get(channel) as u32),
            count,
            occlusion,
        )
    }

    /// The average of `count` voxels' light, from each channel's total.
    fn from_sum(sum: [u32; 4], count: u32, occlusion: u8) -> Self {
        Self {
            light: sum.map(|total| (total as f32 / count as f32 * LEVEL_STEP).round() as u8),
            occlusion,
        }
    }

    /// `self + other - opposite`, clamped: the fourth corner of a
    /// parallelogram.
    fn extend(self, other: Self, opposite: Self) -> Self {
        let extend = |a: u8, b: u8, c: u8, max: u8| {
            (a as i32 + b as i32 - c as i32).clamp(0, max as i32) as u8
        };
        Self {
            light: std::array::from_fn(|i| {
                extend(self.light[i], other.light[i], opposite.light[i], u8::MAX)
            }),
            occlusion: extend(self.occlusion, other.occlusion, opposite.occlusion, OPEN),
        }
    }
}

fn set_sky_color(mut commands: Commands) {
    commands.insert_resource(ClearColor(Color::srgb(0.53, 0.75, 0.95)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Direction, Render, base, block, test_mod};
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

    /// A slab on a block hides the block's top and its own bottom; a block
    /// beside it keeps its face against the slab, whose upper half shows,
    /// and the slab's side against the block is hidden.
    #[test]
    fn slabs_hide_only_what_they_cover() {
        let mut chunk = Chunk::default();
        chunk.set(5, 5, 5, block("stone"));
        chunk.set(5, 6, 5, block("stone_slab"));
        assert_eq!(faces(&mesh(&chunk, air)), 5 + 5);
        chunk.set(6, 6, 5, block("stone"));
        // The new block: all 6 faces; the slab loses its east side.
        assert_eq!(faces(&mesh(&chunk, air)), 5 + 4 + 6);

        let mesh = mesh(&chunk, air);
        let positions = vec3s(&mesh, Mesh::ATTRIBUTE_POSITION);
        let top = positions
            .chunks(4)
            .find(|quad| quad.iter().all(|p| p.y == 6.5));
        assert!(top.is_some(), "the slab's top is at half height");
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

    /// A neighbourhood copies each neighbouring chunk's edge from that
    /// chunk's own coordinates, and reads missing chunks as open sky.
    #[test]
    fn gather_copies_the_edges_of_the_chunks_around() {
        let mut middle = Chunk::default();
        middle.set(0, 0, 0, block("dirt"));
        let mut west = Chunk::default();
        west.set(15, 0, 0, block("stone"));
        let mut corner = Chunk::default();
        corner.set(0, 15, 15, block("sand"));
        let mut light = ChunkLight::default();
        light.set_at(IVec3::new(15, 0, 0), Light::new(3, [4, 5, 6]));
        let dark = ChunkLight::default();
        let n = Neighborhood::gather(|offset| match offset.to_array() {
            [0, 0, 0] => Some((&middle, &dark)),
            [-1, 0, 0] => Some((&west, &light)),
            [1, -1, -1] => Some((&corner, &dark)),
            _ => None,
        });
        assert_eq!(n.block(IVec3::ZERO), block("dirt"));
        assert_eq!(n.light(IVec3::ZERO), Light::DARK);
        assert_eq!(n.block(IVec3::new(-1, 0, 0)), block("stone"));
        assert_eq!(n.light(IVec3::new(-1, 0, 0)), Light::new(3, [4, 5, 6]));
        assert_eq!(n.block(IVec3::new(16, -1, -1)), block("sand"));
        assert_eq!(n.block(IVec3::new(16, 16, 16)), BlockState::AIR);
        assert_eq!(n.light(IVec3::new(16, 16, 16)), OPEN_SKY);
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
                let id = &content.textures[face.layer as usize].template;
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
        let content = base();
        for (normal, face) in normals.iter().zip(faces_of(&mesh)) {
            let tile = tile_on(block("grass"), normal.as_ivec3());
            let expected = content.texture(&format!("defaria:{tile}")).unwrap();
            assert_eq!(face[0], expected);
        }
    }

    /// Mod `test` plus a `wall` that covers every side but draws nothing, a
    /// `lid` ([`crate::content::QUAD`]: a top face, covering only its top),
    /// a `pair` (a top and a bottom face with no `cull`) and a one-triangle
    /// `tri`.
    fn shapes() -> Content {
        let content = test_mod(&[
            (
                "models/wall.json5",
                r#"{ slots: { all: null }, occludes: ["up", "down", "north", "south", "east", "west"], faces: [] }"#,
            ),
            (
                "models/tri.json5",
                r#"{ slots: { all: null }, faces: [{ texture: "all", cull: "up", vertices: [
                    { pos: [0, 1, 1], uv: [0, 1] }, { pos: [1, 1, 1], uv: [1, 1] }, { pos: [1, 1, 0], uv: [1, 0] },
                ] }] }"#,
            ),
            (
                "blocks/wall.json5",
                r#"{ model: "wall", textures: { all: "a" } }"#,
            ),
            (
                "blocks/lid.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/pair.json5",
                r#"{ model: "pair", textures: { top: "a", bottom: "a" } }"#,
            ),
            (
                "blocks/tri.json5",
                r#"{ model: "tri", textures: { all: "a" } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        content
    }

    fn shape(content: &Content, name: &str) -> BlockState {
        content.default_state(&format!("test:{name}")).unwrap()
    }

    /// A face is hidden by what its neighbour covers on the side facing it,
    /// not by what the neighbour covers on the far side.
    #[test]
    fn faces_are_hidden_by_the_side_of_the_neighbour_facing_them() {
        let content = shapes();
        let (lid, wall) = (shape(&content, "lid"), shape(&content, "wall"));
        // A lid under a lid: the upper one covers only its top, not the
        // bottom facing the lower lid's face, so both faces show.
        let mut chunk = Chunk::default();
        chunk.set(1, 1, 1, lid);
        chunk.set(1, 2, 1, lid);
        assert_eq!(chunk.mesh(&content, air).count_vertices(), 8);
        // A wall covers its bottom, so the lid's face under it goes.
        chunk.set(1, 2, 1, wall);
        assert_eq!(chunk.mesh(&content, air).count_vertices(), 0);
        // A lid on the far side of the chunk border, under a wall outside it.
        let mut edge = Chunk::default();
        edge.set(1, CHUNK_SIZE - 1, 1, lid);
        assert_eq!(edge.mesh(&content, |_| wall).count_vertices(), 0);
        assert_eq!(edge.mesh(&content, air).count_vertices(), 4);
    }

    /// Faces with no `cull` side draw even when buried.
    #[test]
    fn faces_without_a_cull_side_always_draw() {
        let content = shapes();
        let mut chunk = Chunk::default();
        chunk.set(1, 1, 1, shape(&content, "pair"));
        let wall = |_| shape(&content, "wall");
        let mesh = chunk.mesh(&content, wall);
        assert_eq!(mesh.count_vertices(), 8);
        for (x, y, z) in [
            (0, 1, 1),
            (2, 1, 1),
            (1, 0, 1),
            (1, 2, 1),
            (1, 1, 0),
            (1, 1, 2),
        ] {
            chunk.set(x, y, z, shape(&content, "wall"));
        }
        assert_eq!(chunk.mesh(&content, air).count_vertices(), 8);
    }

    /// A three-vertex face is one triangle: three vertices, three indices.
    #[test]
    fn triangles_mesh_as_one_triangle() {
        let content = shapes();
        let mut chunk = Chunk::default();
        chunk.set(2, 2, 2, shape(&content, "tri"));
        chunk.set(4, 2, 2, shape(&content, "lid"));
        let mesh = chunk.mesh(&content, air);
        assert_eq!(mesh.count_vertices(), 3 + 4);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        assert_eq!(indices, &[0, 1, 2, 3, 4, 5, 3, 5, 6]);
    }

    /// One face's shading, read back from the mesh: each corner's ambient
    /// occlusion and light (sky, red, green, blue, 255 at level 15), in
    /// corner order.
    #[derive(Debug, PartialEq)]
    struct Shading {
        occlusion: [u8; 4],
        light: [[u8; 4]; 4],
    }

    fn faces_of(mesh: &Mesh) -> Vec<[u32; 2]> {
        let Some(VertexAttributeValues::Uint32x2(faces)) = mesh.attribute(ATTRIBUTE_FACE) else {
            panic!("no face attribute");
        };
        faces.clone()
    }

    /// The shading of the face of block `pos` facing `normal`.
    fn shading(mesh: &Mesh, pos: IVec3, normal: IVec3) -> Shading {
        let positions = vec3s(mesh, Mesh::ATTRIBUTE_POSITION);
        let normals = vec3s(mesh, Mesh::ATTRIBUTE_NORMAL);
        let Some(VertexAttributeValues::Uint32x4(lights)) = mesh.attribute(ATTRIBUTE_LIGHT) else {
            panic!("no light attribute");
        };
        let faces = faces_of(mesh);
        let center = pos.as_vec3() + Vec3::splat(0.5) + normal.as_vec3() * 0.5;
        let first = (0..positions.len())
            .step_by(4)
            .find(|&i| {
                let middle = (positions[i] + positions[i + 2]) / 2.0;
                normals[i] == normal.as_vec3() && middle.distance(center) < 1e-4
            })
            .unwrap_or_else(|| panic!("no face of {pos} facing {normal}"));
        // Every vertex of a face carries the same corners.
        for i in first..first + 4 {
            assert_eq!((lights[i], faces[i]), (lights[first], faces[first]));
        }
        Shading {
            occlusion: faces[first][1].to_le_bytes(),
            light: lights[first].map(u32::to_le_bytes),
        }
    }

    fn neighborhood(
        blocks: &[(IVec3, BlockState)],
        light: impl Fn(IVec3) -> Light,
    ) -> Neighborhood {
        Neighborhood::from_fn(|pos| {
            let block = blocks
                .iter()
                .find(|(p, _)| *p == pos)
                .map_or(BlockState::AIR, |(_, b)| *b);
            (block, light(pos))
        })
    }

    const FULL: [u8; 4] = [255, 0, 0, 0];

    #[test]
    fn open_faces_have_no_occlusion() {
        let pos = IVec3::new(5, 5, 5);
        let mesh = neighborhood(&[(pos, block("stone"))], |_| OPEN_SKY).mesh(base());
        for normal in Direction::ALL.map(Direction::normal) {
            let shading = shading(&mesh, pos, normal);
            assert_eq!(shading.occlusion, [OPEN; 4], "{normal}");
            assert_eq!(shading.light, [FULL; 4]);
        }
    }

    /// A block beside the face's front voxel darkens the two corners on that
    /// side; one only diagonally across darkens just its corner; two on both
    /// sides of a corner close it completely.
    #[test]
    fn neighbours_occlude_the_corners_they_touch() {
        let stone = block("stone");
        let pos = IVec3::new(5, 5, 5);
        let up = IVec3::Y;
        // The top face's corners run (0,1,1), (1,1,1), (1,1,0), (0,1,0).
        let top = |others: &[IVec3]| {
            let mut blocks = vec![(pos, stone)];
            blocks.extend(others.iter().map(|&o| (pos + up + o, stone)));
            shading(&neighborhood(&blocks, |_| OPEN_SKY).mesh(base()), pos, up).occlusion
        };
        assert_eq!(top(&[IVec3::X]), [3, 2, 2, 3], "east edge");
        assert_eq!(
            top(&[IVec3::new(1, 0, 1)]),
            [3, 2, 3, 3],
            "south-east diagonal"
        );
        assert_eq!(top(&[IVec3::X, IVec3::Z]), [2, 0, 2, 3], "east and south");
        assert_eq!(
            top(&[IVec3::X, IVec3::Z, IVec3::new(1, 0, 1)]),
            [2, 0, 2, 3],
            "the diagonal doesn't matter once both edges are blocked"
        );
    }

    /// A corner's light is the average of the voxels around it that aren't
    /// opaque.
    #[test]
    fn corner_light_averages_the_open_voxels_around_it() {
        let stone = block("stone");
        let pos = IVec3::new(5, 5, 5);
        // Sky 15 on the east half of the layer above, 7 on the west half;
        // a block north-west above blocks a voxel, whose light is ignored.
        let light = |p: IVec3| {
            let sky = if p.x > pos.x { 15 } else { 7 };
            Light::new(sky, [0, 0, 0])
        };
        let blocks = [(pos, stone), (pos + IVec3::new(-1, 1, -1), stone)];
        let shading = shading(&neighborhood(&blocks, light).mesh(base()), pos, IVec3::Y);
        let level = |avg: f32| (avg * LEVEL_STEP).round() as u8;
        // Corners (0,1,1) and (0,1,0) see only west voxels; (1,1,*) see two
        // of each. (0,1,0) has its diagonal blocked, leaving three.
        assert_eq!(
            shading.light.map(|c| c[0]),
            [level(7.0), level(11.0), level(11.0), level(7.0)]
        );
        assert_eq!(shading.occlusion, [3, 3, 3, 2]);
    }

    /// With both edges of a corner blocked, light can't get round to it, so
    /// the diagonal voxel's light is left out even when it's open.
    #[test]
    fn closed_corners_ignore_the_diagonal() {
        let stone = block("stone");
        let pos = IVec3::new(5, 5, 5);
        // Bright only at the open diagonal above the (1, 1, 1) corner.
        let diagonal = pos + IVec3::new(1, 1, 1);
        let light = |p: IVec3| Light::new(if p == diagonal { 15 } else { 3 }, [0; 3]);
        let blocks = [
            (pos, stone),
            (pos + IVec3::new(1, 1, 0), stone),
            (pos + IVec3::new(0, 1, 1), stone),
        ];
        let shading = shading(&neighborhood(&blocks, light).mesh(base()), pos, IVec3::Y);
        let level = |avg: f32| (avg * LEVEL_STEP).round() as u8;
        assert_eq!(shading.occlusion[1], 0, "corner (1, 1, 1) is closed");
        assert_eq!(
            shading.light[1][0],
            level(3.0),
            "only the front voxel counts"
        );
    }

    /// Block light averages per colour channel, like the sky's.
    #[test]
    fn coloured_light_averages_per_channel() {
        let pos = IVec3::new(5, 5, 5);
        let light = |_| Light::new(0, [15, 3, 0]);
        let shading = shading(
            &neighborhood(&[(pos, block("stone"))], light).mesh(base()),
            pos,
            IVec3::X,
        );
        assert_eq!(shading.light, [[0, 255, 51, 0]; 4]);
    }

    /// The faces of a block that gives off light are drawn fully lit.
    #[test]
    fn light_sources_glow() {
        let pos = IVec3::new(5, 5, 5);
        let blocks = [
            (pos, block("red_lamp")),
            (pos + IVec3::new(1, 1, 0), block("stone")),
        ];
        let shading = shading(
            &neighborhood(&blocks, |_| Light::DARK).mesh(base()),
            pos,
            IVec3::Y,
        );
        assert_eq!(shading.occlusion, [OPEN; 4]);
        assert_eq!(shading.light, [[0, 255, 255, 255]; 4]);
    }

    /// A triangle's three corners are its own; the made-up fourth keeps the
    /// blend flat across the triangle.
    #[test]
    fn triangles_extend_their_shading_to_a_fourth_corner() {
        let a = CornerShading {
            light: [10, 20, 30, 40],
            occlusion: 3,
        };
        let b = CornerShading {
            light: [20, 20, 250, 0],
            occlusion: 1,
        };
        let c = CornerShading {
            light: [30, 20, 10, 0],
            occlusion: 3,
        };
        let d = b.extend(c, a);
        assert_eq!(d.light, [40, 20, 230, 0]);
        assert_eq!(d.occlusion, 1);
        let mesh = shapes();
        let mut chunk = Chunk::default();
        chunk.set(2, 2, 2, shape(&mesh, "tri"));
        let corners: Vec<Vec2> = vec2s(&chunk.mesh(&mesh, air), ATTRIBUTE_CORNER);
        assert_eq!(corners, [Vec2::ZERO, Vec2::X, Vec2::Y]);

        // In a mesh, with light differing at each corner: the third packed
        // corner is the made-up one.
        let pos = IVec3::new(2, 2, 2);
        let light = |p: IVec3| Light::new((p.x * 3 + p.z) as u8, [0; 3]);
        let tri = shape(&mesh, "tri");
        let built =
            Neighborhood::from_fn(|p| (if p == pos { tri } else { BlockState::AIR }, light(p)))
                .mesh(&mesh);
        let Some(VertexAttributeValues::Uint32x4(lights)) = built.attribute(ATTRIBUTE_LIGHT) else {
            panic!("no light attribute");
        };
        let sky = lights[0].map(|corner| corner.to_le_bytes()[0] as i32);
        assert_ne!(sky[0], sky[1]);
        assert_eq!(sky[2], (sky[1] + sky[3] - sky[0]).clamp(0, 255));
    }

    /// Faces per mesh, opaque, cut-out and translucent, of `blocks` (by
    /// name, at chunk-local positions) in open air.
    fn split(blocks: &[(IVec3, &str)]) -> [usize; 3] {
        let blocks: Vec<_> = blocks.iter().map(|&(p, b)| (p, block(b))).collect();
        let meshes = neighborhood(&blocks, |_| OPEN_SKY).meshes(base());
        [Render::Opaque, Render::Cutout, Render::Translucent].map(|r| faces(meshes.get(r)))
    }

    const A: IVec3 = IVec3::new(5, 5, 5);
    const B: IVec3 = IVec3::new(6, 5, 5);

    /// Each block's faces go in the mesh for how it's drawn. Glass and
    /// water are double-sided: each face twice, front and back.
    #[test]
    fn faces_go_in_the_mesh_for_their_render() {
        assert_eq!(split(&[(A, "stone")]), [6, 0, 0]);
        assert_eq!(split(&[(A, "leaves")]), [0, 6, 0]);
        assert_eq!(split(&[(A, "glass")]), [0, 0, 12]);
        assert_eq!(split(&[(A, "water")]), [0, 0, 12]);
        // Lava glows, but it's opaque; double-sided like water.
        assert_eq!(split(&[(A, "lava")]), [12, 0, 0]);
    }

    /// You can be inside a fluid, so even opaque lava hides only its own
    /// faces: the floor and walls around a lava pool are still drawn, and
    /// seen from inside it the world doesn't show through them.
    #[test]
    fn fluids_hide_only_their_own_faces() {
        // Stone's face against the lava stays; the lava's against the
        // stone goes.
        assert_eq!(split(&[(A, "stone"), (B, "lava")]), [6 + 10, 0, 0]);
        assert_eq!(split(&[(A, "lava"), (B, "lava")]), [20, 0, 0]);
        assert_eq!(split(&[(A, "stone"), (B, "water")]), [6, 0, 10]);
    }

    /// Nothing is hidden next to a cut-out block, not even another one:
    /// its holes show what's behind. An opaque block still hides a
    /// cut-out's face against it.
    #[test]
    fn faces_next_to_cut_outs_are_not_culled() {
        assert_eq!(split(&[(A, "leaves"), (B, "leaves")]), [0, 12, 0]);
        assert_eq!(split(&[(A, "stone"), (B, "leaves")]), [6, 5, 0]);
    }

    /// Faces between two of the same translucent block are hidden, so
    /// glass walls and water look like one body; against anything else
    /// both faces show.
    #[test]
    fn faces_between_the_same_translucent_block_are_culled() {
        // Double-sided, so two faces each.
        assert_eq!(split(&[(A, "glass"), (B, "glass")]), [0, 0, 20]);
        assert_eq!(split(&[(A, "water"), (B, "water")]), [0, 0, 20]);
        assert_eq!(split(&[(A, "glass"), (B, "water")]), [0, 0, 24]);
        assert_eq!(split(&[(A, "stone"), (B, "glass")]), [6, 0, 10]);
        assert_eq!(split(&[(A, "leaves"), (B, "glass")]), [0, 6, 12]);
    }

    /// See-through blocks cast no ambient occlusion and don't hide light
    /// from a corner: a floor beside and under them is shaded exactly as
    /// it would be with nothing there. An opaque block in the same places
    /// does shade it.
    #[test]
    fn see_through_blocks_cast_no_shadow_on_corners() {
        let floor: Vec<_> = (4..7)
            .flat_map(|x| (4..7).map(move |z| (IVec3::new(x, 4, z), block("stone"))))
            .collect();
        let light = |p: IVec3| Light::new((p.x + 2 * p.z + p.y) as u8 % 16, [0; 3]);
        let top = |extra: &[(IVec3, BlockState)]| {
            let blocks: Vec<_> = floor.iter().chain(extra).copied().collect();
            let mesh = neighborhood(&blocks, light).meshes(base()).opaque;
            shading(&mesh, A - IVec3::Y, IVec3::Y)
        };
        let bare = top(&[]);
        for name in ["leaves", "glass", "water", "tuft"] {
            let around = [A, A + IVec3::X, A + IVec3::Z, A + IVec3::ONE.with_y(0)];
            let around = around.map(|p| (p, block(name)));
            assert_eq!(top(&around), bare, "{name} over and beside");
            assert!(!base().state(block(name)).is_opaque(), "{name}");
        }
        let stone = [(A + IVec3::X, block("stone"))];
        assert_ne!(top(&stone).occlusion, bare.occlusion);
    }

    /// A plant is four faces inside its block, lit by its own voxel, and
    /// it covers no side, so it hides nothing around it and nothing hides
    /// it.
    #[test]
    fn plants_are_crossed_quads_that_hide_nothing() {
        assert_eq!(split(&[(A, "tuft")]), [0, 4, 0]);
        assert_eq!(split(&[(A, "tuft"), (B, "stone")]), [6, 4, 0]);
        let below = A - IVec3::Y;
        assert_eq!(split(&[(A, "tuft"), (below, "grass")]), [6, 4, 0]);
        let blocks = [(A, block("tuft"))];
        let light = |p: IVec3| Light::new(if p == A { 9 } else { 15 }, [0; 3]);
        let meshes = neighborhood(&blocks, light).meshes(base());
        let Some(VertexAttributeValues::Uint32x4(lights)) =
            meshes.cutout.attribute(ATTRIBUTE_LIGHT)
        else {
            panic!("no light attribute");
        };
        let nine = (9.0 * LEVEL_STEP).round() as u8;
        for light in lights {
            assert!(light.iter().all(|c| c.to_le_bytes()[0] == nine));
        }
    }

    /// A double-sided block's back faces are its front faces turned round:
    /// the same corners, wound the other way, facing the other way, each
    /// corner with the front's light and occlusion. So the underside of
    /// water's surface is lit as its top is.
    #[test]
    fn back_faces_mirror_the_front() {
        let blocks = [(A, block("water")), (A - IVec3::X, block("stone"))];
        let light = |p: IVec3| Light::new((p.x + 2 * p.z + 3 * p.y) as u8 % 16, [0; 3]);
        let meshes = neighborhood(&blocks, light).meshes(base());
        let mesh = &meshes.translucent;
        let positions = vec3s(mesh, Mesh::ATTRIBUTE_POSITION);
        let normals = vec3s(mesh, Mesh::ATTRIBUTE_NORMAL);
        let corners = vec2s(mesh, ATTRIBUTE_CORNER);
        let Some(VertexAttributeValues::Uint32x4(lights)) = mesh.attribute(ATTRIBUTE_LIGHT) else {
            panic!("no light attribute");
        };
        let faces = faces_of(mesh);
        // The light and occlusion each vertex gets, blended to its corner.
        let at = |i: usize| {
            let corner = CORNERS
                .iter()
                .position(|c| Vec2::from(*c) == corners[i])
                .unwrap();
            let occlusion = faces[i][1].to_le_bytes()[corner];
            (lights[i][corner], occlusion)
        };
        assert_eq!(positions.len(), 10 * 4, "five faces, front and back");
        for face in positions.chunks(8).enumerate().map(|(n, _)| n * 8) {
            let (front, back) = (face, face + 4);
            assert_eq!(normals[back], -normals[front]);
            let wound =
                |i: usize| (positions[i + 1] - positions[i]).cross(positions[i + 2] - positions[i]);
            assert!(wound(back).dot(normals[back]) > 0.0, "counter-clockwise");
            for i in 0..4 {
                let twin = (0..4)
                    .map(|j| back + j)
                    .find(|&j| positions[j] == positions[front + i])
                    .expect("the back has the front's corners");
                assert_eq!(at(twin), at(front + i));
            }
        }
    }

    /// Faces that all lie in one plane, like open water's surface (front
    /// and back), never need sorting; faces in two planes do.
    #[test]
    fn only_faces_in_several_planes_need_sorting() {
        let needs = |blocks: &[(IVec3, &str)]| {
            let blocks: Vec<_> = blocks.iter().map(|&(p, b)| (p, block(b))).collect();
            // Stone all around below and beside, so only tops show.
            let ground = |p: IVec3| {
                let found = blocks.iter().find(|(q, _)| *q == p).map(|(_, b)| *b);
                let state =
                    found.unwrap_or(if p.y < 5 || p.x < 2 || p.x > 6 || p.z < 2 || p.z > 6 {
                        block("stone")
                    } else {
                        BlockState::AIR
                    });
                (state, OPEN_SKY)
            };
            Neighborhood::from_fn(ground)
                .meshes(base())
                .translucent_faces
                .needs_sorting()
        };
        let sheet: Vec<_> = (2..=6)
            .flat_map(|x| (2..=6).map(move |z| (IVec3::new(x, 5, z), "water")))
            .collect();
        assert!(!needs(&sheet));
        let mut deeper = sheet.clone();
        deeper[0].0.y = 6;
        assert!(needs(&deeper));
        assert!(needs(&[(A, "glass")]));
    }

    /// Faces facing the same way in different planes, like two water
    /// surfaces at different heights, need sorting too.
    #[test]
    fn parallel_planes_need_sorting() {
        let face = |y: f32, first: u32| SortFace {
            center: Vec3::new(0.5, y, 0.5),
            normal: Vec3::Y,
            first,
            indices: &[0, 1, 2, 0, 2, 3],
        };
        assert!(!TranslucentFaces::new(vec![face(5.0, 0), face(5.0, 4)]).needs_sorting());
        assert!(TranslucentFaces::new(vec![face(5.0, 0), face(7.0, 4)]).needs_sorting());
        // A face and its back share their plane.
        assert_eq!(either_way(Vec3::NEG_Y), Vec3::Y);
        assert_eq!(
            either_way(Vec3::new(-1.0, 0.0, 1.0)),
            Vec3::new(1.0, 0.0, -1.0)
        );
    }

    /// Translucent faces sort farthest first from the eye, as whole faces.
    #[test]
    fn translucent_faces_sort_back_to_front() {
        let blocks = [
            (A, block("glass")),
            (A + IVec3::new(0, 0, 3), block("glass")),
        ];
        let meshes = neighborhood(&blocks, |_| OPEN_SKY).meshes(base());
        let positions = vec3s(&meshes.translucent, Mesh::ATTRIBUTE_POSITION);
        let center = |quad: &[u32]| {
            quad.iter().map(|&i| positions[i as usize]).sum::<Vec3>() / quad.len() as f32
        };
        for eye in [
            Vec3::new(5.5, 6.0, 20.0),
            Vec3::new(5.5, 6.0, -10.0),
            Vec3::splat(6.0),
        ] {
            let indices = meshes.translucent_faces.indices_from(eye);
            assert_eq!(indices.len(), 2 * 12 * 6);
            let distances: Vec<f32> = indices
                .chunks(6)
                .map(|quad| {
                    // Two triangles of one face: 0 1 2, 0 2 3.
                    assert_eq!((quad[0], quad[2]), (quad[3], quad[4]));
                    center(&[quad[0], quad[1], quad[2], quad[5]]).distance(eye)
                })
                .collect();
            assert!(
                distances.is_sorted_by(|a, b| a >= b),
                "{eye}: {distances:?}"
            );
        }
    }
}
