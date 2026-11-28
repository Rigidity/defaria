//! Building a chunk's meshes, one custom mesh per render (opaque, cut-out
//! and translucent). `light` lights the chunks, and `voxel_material`
//! draws the meshes.
//!
//! Meshing copies each block state's baked model faces into the chunk mesh,
//! leaving out the faces covered by the neighbour on their `cull` side,
//! including across chunk borders, and the faces of its parts drawn by
//! their neighbours (a part's `next`) where the blocks around match. It reads a [`Neighborhood`]: a copy of the
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
//! arrangement of light and shadow comes out smooth. Each vertex carries its
//! own corner's values and the face's twist, from which the shader gets
//! the bilinear blend back exactly (see [`packed`]).
//!
//! # The vertices
//!
//! A vertex is packed into 20 bytes ([`packed`]): its voxel in the chunk,
//! which corner of which model face it is (an index into the model face
//! table, [`model_faces`], which holds where the corners sit in the voxel,
//! their texture coordinates, the face's normal and its texture), its
//! light and ambient occlusion, and a fluid face's height and flow.
//!
//! Faces of blocks that give off light are drawn at full brightness, so
//! light sources glow.

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
    shape::Aabb3d,
};

use crate::meshing::{
    model_faces::FaceIndex,
    packed::{self, ATTRIBUTE_FLOW, ATTRIBUTE_PACKED},
    scroll,
};
use defaria_core::{
    content::{
        Around, BakedFace, BlockState, Content, Direction, FULL_FLUID, Fluid, MAX_LIGHT, Render,
        Vertex,
    },
    fluid,
    light::{CHANNELS, ChunkLight, Light, part_full_dim},
    world::{CHUNK_SIZE, Chunk, FLUID_STEPS, FluidTop, VoxelFluid, axis_of, fluid_surface},
};

/// What lies outside every loaded chunk, as the mesh sees it: open air in
/// full sky light, so faces at the edge of the world are lit.
const OPEN_SKY: Light = Light::new(MAX_LIGHT, [0; 3]);

/// Where `face` (of a block from 0 to 1 high) is once squeezed to run from
/// the bottom up to `top`, as the shader draws a fluid's faces: for
/// shading it, sorting it and its mesh's bounds. Its texture coordinates
/// are left alone; a fluid's come from where it is.
fn cut_face(face: &BakedFace, top: &FluidTop) -> BakedFace {
    let vertices = face
        .vertices
        .iter()
        .map(|vertex| Vertex {
            pos: vertex.pos.with_y(vertex.pos.y * top.at_vertex(vertex.pos)),
            uv: vertex.uv,
        })
        .collect();
    BakedFace {
        vertices,
        ..face.clone()
    }
}

pub use defaria_core::world::MARGIN;
/// Each voxel's flow in a [`Neighborhood`], by its index, worked out once:
/// the corners of several faces average it. Made with the first fluid
/// face, so chunks without fluid don't pay for it.
type FlowCache = Option<Vec<Option<scroll::Flow>>>;

/// Voxels along each axis of a [`Neighborhood`].
const PADDED: usize = CHUNK_SIZE + 2 * MARGIN as usize;

/// A chunk's blocks, fluid and light and the [`MARGIN`] voxels around it,
/// copied out for meshing. Positions are chunk-local, from `-MARGIN` to
/// [`CHUNK_SIZE`]` + MARGIN - 1`.
#[derive(Debug, Clone)]
pub(super) struct Neighborhood {
    blocks: Vec<BlockState>,
    light: Vec<Light>,
    /// Each voxel's fluid, and the height it's drawn at (see
    /// [`Chunk::fluid_step_at`]).
    fluids: Vec<(Option<VoxelFluid>, u16)>,
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
        Self::with_steps(|pos| {
            let offset = pos.div_euclid(size) + IVec3::ONE;
            match chunks[(offset.x + offset.y * 3 + offset.z * 9) as usize] {
                Some((chunk, light)) => {
                    let local = pos.rem_euclid(size);
                    let fluid = (chunk.fluid_at(local), chunk.fluid_step_at(local));
                    (chunk.at(local), light.at(local), fluid)
                }
                None => (BlockState::AIR, OPEN_SKY, (None, 0)),
            }
        })
    }

    /// Fills every position from `voxel`, with no fluid.
    #[cfg(test)]
    pub fn from_fn(voxel: impl Fn(IVec3) -> (BlockState, Light)) -> Self {
        Self::with_fluid(|pos| {
            let (block, light) = voxel(pos);
            (block, light, None)
        })
    }

    /// Fills every position from `voxel`: its block (of the base game),
    /// light and fluid, drawn at the amount's own height up the block's
    /// room.
    #[cfg(test)]
    pub fn with_fluid(voxel: impl Fn(IVec3) -> (BlockState, Light, Option<VoxelFluid>)) -> Self {
        Self::with_steps(|pos| {
            let (block, light, fluid) = voxel(pos);
            let room = defaria_core::content::base().state(block).fluid_room;
            let step = fluid.map_or(0, |fluid| {
                defaria_core::world::fluid_step(defaria_core::world::filled(fluid.amount, room))
            });
            (block, light, (fluid, step))
        })
    }

    /// Fills every position from `voxel`: its block, light, and its fluid
    /// with the height it's drawn at.
    fn with_steps(voxel: impl Fn(IVec3) -> (BlockState, Light, (Option<VoxelFluid>, u16))) -> Self {
        let mut blocks = Vec::with_capacity(PADDED.pow(3));
        let mut light = Vec::with_capacity(PADDED.pow(3));
        let mut fluids = Vec::with_capacity(PADDED.pow(3));
        let range = -MARGIN..CHUNK_SIZE as i32 + MARGIN;
        for y in range.clone() {
            for z in range.clone() {
                for x in range.clone() {
                    let (block, level, fluid) = voxel(IVec3::new(x, y, z));
                    blocks.push(block);
                    light.push(level);
                    fluids.push(fluid);
                }
            }
        }
        Self {
            blocks,
            light,
            fluids,
        }
    }

    fn index(pos: IVec3) -> usize {
        let p = (pos + IVec3::splat(MARGIN)).as_uvec3();
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

    /// The light a block face in front of `pos` is shaded with: the
    /// voxel's, less what fluid that doesn't fill it dims by how full it is
    /// drawn (see [`part_full_dim`]). Light leaves a part-full voxel's own
    /// light alone, so what sticks out of the fluid is lit as the air, but
    /// a face under it is under that much fluid; and it's as dim under a
    /// voxel nearly full as under a full one.
    fn face_light(&self, content: &Content, pos: IVec3) -> Light {
        let mut light = self.light(pos);
        let (Some(fluid), step) = self.fluids[Self::index(pos)] else {
            return light;
        };
        let room = content.state(self.block(pos)).fluid_room;
        if fluid.amount >= room {
            return light;
        }
        let filter = content.state(fluid.kind).light_filter;
        let dim = part_full_dim(filter, step, FLUID_STEPS);
        for channel in 0..CHANNELS {
            light.set(channel, light.get(channel).saturating_sub(dim));
        }
        light
    }

    /// Which fluid is at `pos`, if any.
    pub fn fluid(&self, pos: IVec3) -> Option<BlockState> {
        self.fluids[Self::index(pos)].0.map(|fluid| fluid.kind)
    }

    /// Whether the voxel at `next`, on the `side` of a face of `own`,
    /// hides it (see [`Content::hides`]): its block does, or its fluid.
    fn hides(&self, content: &Content, own: BlockState, next: IVec3, side: Direction) -> bool {
        content.hides(own, self.block(next), side)
            || self
                .fluid(next)
                .is_some_and(|fluid| content.hides(own, fluid, side))
    }

    /// The top of the fluid of block `fluid` at `pos` (see [`FluidTop`]),
    /// from the voxels around it, which are all in the neighbourhood for a
    /// voxel of the middle chunk. Each counts by how high its fluid is
    /// drawn, up the room its block leaves (see [`fluid_surface`]), so a
    /// slab in a pond slopes and meets the water around it like any voxel.
    fn fluid_top(&self, content: &Content, fluid: BlockState, pos: IVec3) -> FluidTop {
        FluidTop::of(fluid, pos, |at| {
            let (here, step) = self.fluids[Self::index(at)];
            let room = content.state(self.block(at)).fluid_room;
            here.map(|here| (here.kind, fluid_surface(step, room)))
        })
    }

    /// How high the fluid at `pos` stands, as flow measures it: its amount
    /// on top of the part of the voxel its block takes.
    fn fluid_level(&self, content: &Content, pos: IVec3) -> u16 {
        let (fluid, _) = self.fluids[Self::index(pos)];
        let room = content.state(self.block(pos)).fluid_room;
        fluid.map_or(0, |fluid| fluid.amount) + FULL_FLUID.saturating_sub(room)
    }

    /// Which way the fluid `kind` at `pos` flows (see [`scroll`]). Whether
    /// it flows at all goes by the amounts, as flow itself does, not by the
    /// heights drawn, which are sticky (a settled pool can be drawn a
    /// little uneven for good): it falls while the voxel below has room
    /// for it, and flows across toward neighbours holding the same fluid
    /// (waterlogged blocks among them) whose level is lower by as much as
    /// flow moves sideways (a `ripple`, and at least [`fluid::SHARE`]
    /// units). Flowing across, it runs down its drawn surface's slope, or
    /// toward those neighbours where the surface is level.
    fn fluid_flow(
        &self,
        content: &Content,
        kind: BlockState,
        fluid: &Fluid,
        pos: IVec3,
    ) -> scroll::Flow {
        let below = pos - IVec3::Y;
        let room_below = content.state(self.block(below)).fluid_room;
        let falling = match self.fluids[Self::index(below)].0 {
            None => room_below > 0,
            Some(there) => there.kind == kind && there.amount < room_below,
        };
        let level = fluid.ripple.max(fluid::SHARE) as f32;
        let here = self.fluid_level(content, pos) as f32;
        let toward: Vec3 = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
            .into_iter()
            .filter(|&side| self.fluid(pos + side) == Some(kind))
            .map(|side| (side, here - self.fluid_level(content, pos + side) as f32))
            .filter(|(_, drop)| drop.abs() >= level)
            .map(|(side, drop)| side.as_vec3() * drop / FULL_FLUID as f32)
            .sum();
        let downhill = if toward == Vec3::ZERO {
            Vec3::ZERO
        } else {
            // Down the slope: the corners' heights fall this much across
            // the voxel in X and in Z.
            let top = self.fluid_top(content, kind, pos);
            let slope = Vec3::new(
                top.corner(0, 0) + top.corner(0, 1) - top.corner(1, 0) - top.corner(1, 1),
                0.0,
                top.corner(0, 0) + top.corner(1, 0) - top.corner(0, 1) - top.corner(1, 1),
            ) / 2.0;
            if slope.length() > 1e-3 { slope } else { toward }
        };
        scroll::Flow { falling, downhill }
    }

    /// The faces of the fluid `fluid` at `pos`, and their backs if it's
    /// double-sided, cut to its surface (see [`FluidTop`]):
    ///
    /// - its top, unless the same fluid is above it; a full block high,
    ///   it's hidden as any face is ([`Content::hides`]);
    /// - each side up to the corners of its top, unless the same fluid is
    ///   next to it, whose top meets this one's at the same corners;
    ///   anything else hides it as usual, the block there or its fluid;
    /// - its bottom, unless the same fluid is below it.
    ///
    /// A side the voxel's own block covers (a slab's bottom) has no face:
    /// the block's is there.
    fn fluid_faces(
        &self,
        content: &Content,
        fluid: BlockState,
        pos: IVec3,
        faces: &FaceIndex,
        builder: &mut MeshBuilder,
        opaque: &impl Fn(IVec3) -> bool,
        flows: &mut FlowCache,
    ) {
        let state = content.state(fluid);
        let block = content.state(self.block(pos));
        let Some(definition) = &state.fluid else {
            return;
        };
        // Built only once a face shows: most of a sea shows none.
        let mut top = None;
        for (index, face) in state.faces.iter().enumerate() {
            let Some(side) = face.cull else {
                continue;
            };
            let Some(id) = faces.id(fluid, index) else {
                continue;
            };
            let next = pos + side.normal();
            if block.occludes(side) || self.fluid(next) == Some(fluid) {
                continue;
            }
            let top = *top.get_or_insert_with(|| self.fluid_top(content, fluid, pos));
            let model = face;
            let (face, shading) = if side == Direction::Up && !top.is_full() {
                // A lowered top lies inside the voxel, so nothing beside it
                // can hide it. It's still the surface the light above falls
                // on, so it's shaded from the voxel above, as a full top is
                // (the fluid's own voxel is darker by its light filter, and
                // the surface would darken the moment it dropped a step),
                // unless that voxel is solid.
                let cut = cut_face(face, &top);
                let shading = if opaque(next) {
                    BakedFace {
                        cull: None,
                        ..cut.clone()
                    }
                } else {
                    cut.clone()
                };
                (BakedFace { cull: None, ..cut }, shading)
            } else {
                if self.hides(content, fluid, next, side) {
                    continue;
                }
                let cut = cut_face(face, &top);
                (cut.clone(), cut)
            };
            // Each corner's flow, averaged over the voxels around it in
            // the face's plane holding the same fluid (waterlogged blocks
            // among them), in the face's texture coordinates.
            let normal = face.normal;
            let (a, b) = scroll::plane_axes(normal);
            let corners: Vec<Vec2> = model
                .vertices
                .iter()
                .map(|vertex| {
                    let around = |offset: IVec2| {
                        let at = pos + a * offset.x + b * offset.y;
                        (self.fluid(at) == Some(fluid)).then(|| {
                            let slot = &mut flows.get_or_insert_with(|| vec![None; PADDED.pow(3)])
                                [Self::index(at)];
                            let flow = *slot.get_or_insert_with(|| {
                                self.fluid_flow(content, fluid, definition, at)
                            });
                            scroll::face_vector(flow, normal)
                        })
                    };
                    let toward = scroll::corner_toward(normal, vertex.pos);
                    scroll::to_uv(normal, scroll::corner(around, toward))
                })
                .collect();
            let surface = FluidSurface {
                heights: model
                    .vertices
                    .iter()
                    .map(|v| top.at_vertex(v.pos))
                    .collect(),
                flowing: corners.iter().any(|flow| flow.length() > 1e-3),
                flows: corners,
            };
            let shading = self.shade_face(content, pos, &shading, state.emits_light(), opaque);
            for back in [false, true]
                .into_iter()
                .take(1 + state.double_sided as usize)
            {
                builder.push(pos, id, &face, &shading, back, Some(&surface));
            }
        }
    }

    /// Builds the meshes of every voxel's model faces in the middle chunk,
    /// and its fluid's, one per way faces are drawn (see [`Render`]),
    /// leaving out each face its neighbour on its `cull` side hides (see
    /// [`Content::hides`]), with ambient occlusion and light at every
    /// corner. `faces` is the model face table built from `content`.
    pub fn build(&self, content: &Content, faces: &FaceIndex) -> ChunkMeshes {
        let opaque: Vec<bool> = self
            .blocks
            .iter()
            .zip(&self.fluids)
            .map(|(&block, (fluid, _))| {
                content.state(block).is_opaque()
                    || fluid.is_some_and(|fluid| content.state(fluid.kind).is_opaque())
            })
            .collect();
        let opaque_at = |pos: IVec3| opaque[Self::index(pos)];

        let mut builders: [MeshBuilder; 3] = Default::default();
        let mut flows = None;
        // Only translucent faces are sorted.
        builders[Render::Translucent as usize].sorted = true;
        for (x, y, z) in Chunk::positions() {
            let pos = IVec3::new(x as i32, y as i32, z as i32);
            if let Some(fluid) = self.fluid(pos) {
                let builder = &mut builders[content.state(fluid).render as usize];
                self.fluid_faces(content, fluid, pos, faces, builder, &opaque_at, &mut flows);
            }
            let voxel = self.block(pos);
            if voxel.is_air() {
                continue;
            }
            let state = content.state(voxel);
            let glows = state.emits_light();
            let builder = &mut builders[state.render as usize];
            // Its own faces, then those of its parts drawn by the blocks
            // next to it, numbered after its own in the face table.
            let own = state.faces.iter().enumerate();
            let around = Around::read(|side| self.block(pos + side.normal()));
            let by_next = content
                .next_faces(voxel, &around)
                .map(|(index, face)| (state.faces.len() + index, face));
            for (index, face) in own.chain(by_next) {
                if face.hidden_by(|side| self.hides(content, voxel, pos + side.normal(), side)) {
                    continue;
                }
                let Some(id) = faces.id(voxel, index) else {
                    continue;
                };
                let shading = self.shade_face(content, pos, face, glows, &opaque_at);
                for back in [false, true]
                    .into_iter()
                    .take(1 + state.double_sided as usize)
                {
                    builder.push(pos, id, face, &shading, back, None);
                }
            }
        }
        let [opaque, cutout, mut translucent] = builders;
        let faces = TranslucentFaces::new(
            std::mem::take(&mut translucent.sort_faces),
            translucent.vertices.len(),
        );
        ChunkMeshes {
            opaque: opaque.build(),
            cutout: cutout.build(),
            translucent: translucent.build(),
            translucent_faces: faces,
        }
    }

    /// The meshes, built with a model face table of their own, with what
    /// the shader would unpack from each vertex added as attributes of its
    /// own, for tests to read (see [`decoded`]).
    #[cfg(test)]
    pub fn meshes(&self, content: &Content) -> ChunkMeshes {
        let faces = super::model_faces::ModelFaces::new(content);
        let built = self.build(content, &faces.index);
        let decode = |mesh: Mesh| decoded::with_attributes(mesh, &faces.index);
        ChunkMeshes {
            opaque: decode(built.opaque),
            cutout: decode(built.cutout),
            translucent: decode(built.translucent),
            translucent_faces: built.translucent_faces,
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
        content: &Content,
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
        for (corner, vertex) in face.vertices.iter().enumerate() {
            corners[corner] = if axis_aligned {
                self.shade_corner(content, front, normal, vertex.pos, opaque)
            } else {
                // Inside its voxel at an angle, as a plant's quads are: lit
                // as the voxel, even where fluid stands part way up it.
                CornerShading::from_light(self.light(pos), 1, OPEN)
            };
        }
        // A triangle has no fourth corner: it's blended flat (see
        // `packed`), and corner 3 is left dark.
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
        content: &Content,
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
            let light = self.face_light(content, *voxel);
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += light.get(channel) as u32;
            }
            count += 1;
        }
        if count == 0 {
            // Every voxel around the corner is opaque: use the one in front.
            return CornerShading::from_light(self.face_light(content, front), 1, occlusion);
        }
        CornerShading::from_sum(sum, count, occlusion)
    }
}

/// A chunk's meshes, one per [`Render`]. A mesh with no vertices has
/// nothing to draw.
pub(super) struct ChunkMeshes {
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
pub(super) struct TranslucentFaces {
    faces: Vec<SortFace>,
    /// Whether the order can matter: false when every face lies in one
    /// plane (open water's surface, front and back), where no face is ever
    /// behind another.
    needs_sorting: bool,
    /// Vertices in the mesh, which say how wide its indices are.
    vertices: usize,
}

/// One face of a translucent mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SortFace {
    /// The middle of the face, chunk-local.
    center: Vec3,
    /// Where it sorts from: its middle, or a back's a little inside its
    /// block.
    sort_at: Vec3,
    /// Its plane's normal, either way round (pointing toward +X, or +Y,
    /// or +Z, whichever it has first), so a face and its back match.
    normal: Vec3,
    /// Its first vertex in the mesh.
    first: u32,
    /// Its triangles, from its first vertex.
    indices: &'static [u32],
}

impl TranslucentFaces {
    fn new(faces: Vec<SortFace>, vertices: usize) -> Self {
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
            vertices,
        }
    }

    /// Whether the faces' order can matter: see `needs_sorting`.
    pub fn needs_sorting(&self) -> bool {
        self.needs_sorting
    }

    /// The mesh's indices with its faces ordered farthest first from
    /// `eye`, chunk-local.
    pub fn indices_from(&self, eye: Vec3) -> Indices {
        let mut order: Vec<(f32, &SortFace)> = self
            .faces
            .iter()
            .map(|face| (face.sort_at.distance_squared(eye), face))
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        indices(
            order
                .into_iter()
                .flat_map(|(_, face)| face.indices.iter().map(move |i| face.first + i))
                .collect(),
            self.vertices,
        )
    }
}

/// `list` as 16-bit indices if a mesh with `vertices` vertices allows, and
/// 32-bit ones otherwise.
fn indices(list: Vec<u32>, vertices: usize) -> Indices {
    if vertices <= 1 << 16 {
        Indices::U16(list.into_iter().map(|i| i as u16).collect())
    } else {
        Indices::U32(list)
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

/// What a fluid face has that a block's doesn't, by its model's vertices:
/// how high its top stands at each (see `FluidTop::at_vertex`), and its
/// flow (see [`scroll`]).
struct FluidSurface {
    heights: Vec<f32>,
    flows: Vec<Vec2>,
    /// Whether any corner flows.
    flowing: bool,
}

/// The vertex data of one chunk mesh, as it's built.
#[derive(Default)]
struct MeshBuilder {
    vertices: Vec<[u32; 4]>,
    flows: Vec<[i16; 2]>,
    indices: Vec<u32>,
    /// The faces' extent, chunk-local, for culling: the mesh has no
    /// positions Bevy could find it from.
    bounds: Option<(Vec3, Vec3)>,
    /// Whether to record `sort_faces`, for a translucent mesh.
    sorted: bool,
    sort_faces: Vec<SortFace>,
}

impl MeshBuilder {
    /// Adds a face of the block at `voxel` (model face `id`, drawn where
    /// `face` says), or its back: the same face facing the other way, with
    /// the front's light and shading, flagged so the shader draws it a hair
    /// farther away. A neighbour's face in the same plane (a slab's side
    /// beside water or lava) then wins the depth test, where they'd flicker
    /// otherwise. A fluid's face has its `surface`.
    fn push(
        &mut self,
        voxel: IVec3,
        id: u16,
        face: &BakedFace,
        shading: &FaceShading,
        back: bool,
        surface: Option<&FluidSurface>,
    ) {
        let base = self.vertices.len() as u32;
        let count = face.vertices.len();
        let triangle = count == 3;
        let light: [[u8; 4]; 4] = shading.light.map(u32::to_le_bytes);
        let occlusion = shading.occlusion.to_le_bytes();
        let (light_twist, ao_twist) = if triangle {
            ([0; 4], 0)
        } else {
            (
                std::array::from_fn(|channel| {
                    packed::twist(light.map(|corner| corner[channel] as i32)) as i16
                }),
                packed::twist(occlusion.map(i32::from)) as i8,
            )
        };
        let origin = voxel.as_vec3();
        let mut center = Vec3::ZERO;
        for i in 0..count {
            // The back runs the other way round: the first vertex, then the
            // rest in reverse.
            let index = if back && i > 0 { count - i } else { i };
            let pos = origin + face.vertices[index].pos;
            center += pos;
            self.bounds = Some(match self.bounds {
                Some((min, max)) => (min.min(pos), max.max(pos)),
                None => (pos, pos),
            });
            let vertex = packed::Vertex {
                voxel: voxel.as_uvec3(),
                corner: index as u8,
                face: id,
                back,
                flowing: surface.is_some_and(|surface| surface.flowing),
                wrapped: surface.is_some(),
                height: surface.map_or(0.0, |surface| surface.heights[index]),
                light: light[index],
                light_twist,
                ao: occlusion[index],
                ao_twist,
            };
            self.vertices.push(vertex.pack());
            let flow = surface.map_or(Vec2::ZERO, |surface| surface.flows[index]);
            self.flows.push(
                flow.to_array()
                    .map(|f| (f.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16),
            );
        }
        self.indices.extend(face.indices().iter().map(|i| base + i));
        if self.sorted {
            let center = center / count as f32;
            self.sort_faces.push(SortFace {
                center,
                // A back sorts as if a little inside its block, nearer than
                // a neighbour's face in its plane, as the block's inside is
                // to someone in it: water blends over the glass it touches.
                sort_at: if back {
                    center - face.normal * BACK_SORT_DEPTH
                } else {
                    center
                },
                normal: either_way(face.normal),
                first: base,
                indices: face.indices(),
            });
        }
    }

    fn build(self) -> Mesh {
        let vertices = self.vertices.len();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(ATTRIBUTE_PACKED, self.vertices)
        .with_inserted_attribute(ATTRIBUTE_FLOW, VertexAttributeValues::Snorm16x2(self.flows))
        .with_inserted_indices(indices(self.indices, vertices));
        mesh.final_aabb = self.bounds.map(|(min, max)| Aabb3d::from_min_max(min, max));
        mesh
    }
}

/// How far into its block a back face sorts from, in blocks (see
/// [`MeshBuilder::push`]).
const BACK_SORT_DEPTH: f32 = 0.01;

/// The ambient occlusion level of a corner with nothing around it.
const OPEN: u8 = 3;
/// A light level's step in the packed bytes: [`MAX_LIGHT`] packs to 255.
const LEVEL_STEP: f32 = u8::MAX as f32 / MAX_LIGHT as f32;

/// The light and ambient occlusion at a face's four corners, in order
/// round it: sky, red, green and blue bytes from the lowest, and the
/// levels a byte each from the lowest. A triangle's fourth corner is dark.
struct FaceShading {
    light: [u32; 4],
    occlusion: u32,
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
}

#[cfg(test)]
mod decoded;
#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod tests;
