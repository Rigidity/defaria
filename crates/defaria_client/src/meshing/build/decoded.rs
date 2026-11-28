//! Chunk meshes unpacked as the shader unpacks them, for tests: each
//! vertex's position, normal, texture coordinates, where it is on its
//! face, its face's four corners' light, its texture and flags and its
//! corners' ambient occlusion, as attributes of their own beside the
//! packed ones. Unpacking also checks that every vertex of a face agrees
//! on what's the same across it, the twist among them.
//!
//! The attributes and flag bits here are a view for tests only, laid out
//! as the tests read them; nothing draws with them. The mesh itself has
//! only [`ATTRIBUTE_PACKED`] and [`ATTRIBUTE_FLOW`].

use bevy::mesh::{MeshVertexAttribute, VertexAttributeValues, VertexFormat};

use super::*;
use crate::meshing::packed::{Vertex, corner_position, twist};

/// Where the vertex is on its face, for blending: see
/// [`corner_position`].
pub const ATTRIBUTE_CORNER: MeshVertexAttribute =
    MeshVertexAttribute::new("Corner", 0x0DEF_A41A_1001, VertexFormat::Float32x2);
/// The light at each of the face's four corners: sky, red, green and
/// blue, a byte each from the lowest.
pub const ATTRIBUTE_LIGHT: MeshVertexAttribute =
    MeshVertexAttribute::new("Light", 0x0DEF_A41A_1002, VertexFormat::Uint32x4);
/// The face's texture with [`BACK_FACE`], [`FLOWING`] and [`WRAPPED`], then
/// its four corners' ambient occlusion, a byte each from the lowest.
pub const ATTRIBUTE_FACE: MeshVertexAttribute =
    MeshVertexAttribute::new("Face", 0x0DEF_A41A_1003, VertexFormat::Uint32x2);

pub const BACK_FACE: u32 = 1 << 31;
pub const WRAPPED: u32 = 1 << 29;
pub const FLOWING: u32 = 1 << 28;
pub const TEXTURE_BITS: u32 = (1 << 24) - 1;

/// `mesh` with its vertices unpacked into the attributes above and Bevy's
/// position, normal and UV ones.
pub fn with_attributes(mut mesh: Mesh, faces: &FaceIndex) -> Mesh {
    let Some(VertexAttributeValues::Uint32x4(words)) = mesh.attribute(ATTRIBUTE_PACKED) else {
        panic!("no packed vertices");
    };
    let vertices: Vec<Vertex> = words.iter().map(|w| Vertex::unpack(*w)).collect();
    let (mut positions, mut normals, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
    let (mut corners, mut lights, mut layers) = (Vec::new(), Vec::new(), Vec::new());
    let mut start = 0;
    while start < vertices.len() {
        let first = vertices[start];
        let face = faces.face(first.face);
        let triangle = face.triangle;
        let group = &vertices[start..start + if triangle { 3 } else { 4 }];
        let mut light = [0u32; 4];
        let mut occlusion = [0u8; 4];
        for vertex in group {
            light[vertex.corner as usize] = u32::from_le_bytes(vertex.light);
            occlusion[vertex.corner as usize] = vertex.ao;
        }
        let expected = if triangle {
            ([0; 4], 0)
        } else {
            (
                std::array::from_fn(|channel| {
                    twist(light.map(|corner| corner.to_le_bytes()[channel] as i32)) as i16
                }),
                twist(occlusion.map(i32::from)) as i8,
            )
        };
        for vertex in group {
            let same = |v: &Vertex| {
                (v.face, v.voxel, v.back, v.flowing, v.wrapped)
                    == (
                        first.face,
                        first.voxel,
                        first.back,
                        first.flowing,
                        first.wrapped,
                    )
            };
            assert!(same(vertex), "a face's vertices disagree: {group:?}");
            assert_eq!(
                (vertex.light_twist, vertex.ao_twist),
                expected,
                "the twist of {group:?}"
            );
            let position = faces.position(vertex);
            positions.push(position.to_array());
            let normal = if vertex.back {
                -face.normal
            } else {
                face.normal
            };
            normals.push(normal.to_array());
            let uv = if vertex.wrapped {
                scroll::uv_at(face.normal, position)
            } else {
                face.uvs[vertex.corner as usize]
            };
            uvs.push(uv.to_array());
            corners.push(corner_position(vertex.corner).to_array());
            lights.push(light);
            let flags = [
                (vertex.back, BACK_FACE),
                (vertex.wrapped, WRAPPED),
                (vertex.flowing, FLOWING),
            ]
            .into_iter()
            .filter(|(set, _)| *set)
            .fold(0, |bits, (_, flag)| bits | flag);
            layers.push([face.texture | flags, u32::from_le_bytes(occlusion)]);
        }
        start += group.len();
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(ATTRIBUTE_CORNER, corners);
    mesh.insert_attribute(ATTRIBUTE_LIGHT, lights);
    mesh.insert_attribute(ATTRIBUTE_FACE, layers);
    mesh
}
