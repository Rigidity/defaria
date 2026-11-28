//! The model face table: every distinct face of every block state's model
//! ([`BakedFace`]), once, for the chunk shader to read. A chunk mesh's
//! vertices name their face by its index here (see [`super::packed`]), so
//! where the face's corners sit in their voxel, their texture
//! coordinates, its normal and its texture aren't repeated in every
//! vertex of every chunk.
//!
//! Faces are told apart by their corners, texture coordinates, normal and
//! texture, so the many states that share a model and texture (a log's
//! every property but its axis, a pipe's arms in each combination) share
//! their entries. The base game's 25 thousand baked faces come to about
//! 1,500 entries.
//!
//! [`ModelFaces`] is built again whenever [`Content`] changes (a server's
//! mods swapped in, or single player's again), before chunks are meshed
//! with it, and `voxel_material` hands the new table to the shader.

use std::sync::Arc;

use bevy::{platform::collections::HashMap, prelude::*};
use defaria_core::content::{BakedFace, BlockState, Content};

use super::packed::{FACE_BITS, Vertex};

/// The most entries the table can have: as many as a vertex's face index
/// can name.
pub const MAX_MODEL_FACES: usize = 1 << FACE_BITS;

/// Set in an entry's `layer` when the face is a triangle: its third
/// vertex is the fourth corner, for blending light.
pub const TRIANGLE: u32 = 1 << 31;

/// 32-bit words a table entry takes in the shader's `ModelFace`: each
/// corner's position and `u`, then the four `v`s, then the normal and
/// layer.
pub const FACE_WORDS: usize = 24;

/// One entry: a model face as the shader reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelFace {
    /// Each corner's position in its voxel; a triangle's fourth repeats
    /// its third.
    pub positions: [Vec3; 4],
    pub uvs: [Vec2; 4],
    pub normal: Vec3,
    /// The texture's index, with [`TRIANGLE`] on a triangle.
    pub layer: u32,
}

impl ModelFace {
    fn of(face: &BakedFace) -> Self {
        let corner = |i: usize| &face.vertices[i.min(face.vertices.len() - 1)];
        let triangle = face.vertices.len() == 3;
        Self {
            positions: std::array::from_fn(|i| corner(i).pos),
            uvs: std::array::from_fn(|i| corner(i).uv),
            normal: face.normal,
            layer: face.layer | if triangle { TRIANGLE } else { 0 },
        }
    }

    pub fn is_triangle(&self) -> bool {
        self.layer & TRIANGLE != 0
    }

    pub fn texture(&self) -> u32 {
        self.layer & !TRIANGLE
    }

    /// The entry's words, as the shader's `ModelFace` lays them out.
    fn words(&self) -> [u32; FACE_WORDS] {
        let mut words = [0; FACE_WORDS];
        for (i, (pos, uv)) in self.positions.iter().zip(&self.uvs).enumerate() {
            words[i * 4..i * 4 + 3].copy_from_slice(&pos.to_array().map(f32::to_bits));
            words[i * 4 + 3] = uv.x.to_bits();
            words[16 + i] = uv.y.to_bits();
        }
        words[20..23].copy_from_slice(&self.normal.to_array().map(f32::to_bits));
        words[23] = self.layer;
        words
    }

    /// The bits that tell entries apart.
    fn key(&self) -> [u32; FACE_WORDS] {
        self.words()
    }
}

/// Which table entry each block state's faces are, for the mesher.
#[derive(Debug, Default)]
pub struct FaceIndex {
    /// Where each state's faces start in `ids`, by state.
    starts: Vec<u32>,
    /// Each face's entry, state by state in their faces' order: `None` for
    /// one that didn't fit in the table, which isn't drawn.
    ids: Vec<Option<u16>>,
    /// The entries, for decoding meshes on the CPU.
    faces: Vec<ModelFace>,
}

impl FaceIndex {
    /// The entry of face `face` (by its index in the state's faces) of
    /// state `state`.
    pub fn id(&self, state: BlockState, face: usize) -> Option<u16> {
        let start = *self.starts.get(state.0 as usize)? as usize;
        self.ids.get(start + face).copied().flatten()
    }

    /// Entry `id`.
    pub fn face(&self, id: u16) -> &ModelFace {
        &self.faces[id as usize]
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// Where `vertex` is in its chunk, as the shader works it out: its
    /// corner of its model face in its voxel, a fluid's at its height.
    pub fn position(&self, vertex: &Vertex) -> Vec3 {
        let mut pos = self.face(vertex.face).positions[vertex.corner as usize];
        if vertex.wrapped {
            pos.y *= vertex.height;
        }
        vertex.voxel.as_vec3() + pos
    }
}

/// The model face table built from the [`Content`] now in place: the
/// index the mesher looks faces up in, and the words the shader reads.
#[derive(Resource, Debug, Default, Clone)]
pub struct ModelFaces {
    pub index: Arc<FaceIndex>,
    /// Every entry's words, for the shader's storage buffer.
    pub words: Arc<Vec<u32>>,
}

impl ModelFaces {
    pub fn new(content: &Content) -> Self {
        let mut index = FaceIndex::default();
        let mut found: HashMap<[u32; FACE_WORDS], u16> = HashMap::new();
        let mut words = Vec::new();
        let mut left_out = 0;
        for state in &content.states {
            index.starts.push(index.ids.len() as u32);
            for face in &state.faces {
                let entry = ModelFace::of(face);
                let id = match found.get(&entry.key()) {
                    Some(&id) => Some(id),
                    None if index.faces.len() < MAX_MODEL_FACES => {
                        let id = index.faces.len() as u16;
                        found.insert(entry.key(), id);
                        index.faces.push(entry);
                        words.extend_from_slice(&entry.words());
                        Some(id)
                    }
                    None => {
                        left_out += 1;
                        None
                    }
                };
                index.ids.push(id);
            }
        }
        if left_out > 0 {
            error!(
                "the blocks' models have more distinct faces than the {MAX_MODEL_FACES} a chunk \
                 mesh can name; {left_out} faces aren't drawn"
            );
        }
        Self {
            index: Arc::new(index),
            words: Arc::new(words),
        }
    }

    /// Bytes the table takes on the GPU.
    pub fn bytes(&self) -> usize {
        self.words.len() * 4
    }
}

/// Builds the table again when the content changes (and first, when it's
/// added).
pub fn update_model_faces(content: Res<Content>, mut faces: ResMut<ModelFaces>) {
    if content.is_changed() {
        *faces = ModelFaces::new(&content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defaria_core::content::{base, block};

    /// Every face of every state has an entry holding what it baked to,
    /// and states sharing a model and texture share entries: the base
    /// game's fit with plenty of room.
    #[test]
    fn every_face_has_its_entry() {
        let content = base();
        let faces = ModelFaces::new(content);
        let mut total = 0;
        for (i, state) in content.states.iter().enumerate() {
            for (f, face) in state.faces.iter().enumerate() {
                let id = faces.index.id(BlockState(i as u16), f).unwrap();
                let entry = faces.index.face(id);
                assert_eq!(*entry, ModelFace::of(face));
                for (corner, vertex) in face.vertices.iter().enumerate() {
                    assert_eq!(entry.positions[corner], vertex.pos);
                    assert_eq!(entry.uvs[corner], vertex.uv);
                }
                assert_eq!(entry.texture(), face.layer);
                assert_eq!(entry.is_triangle(), face.vertices.len() == 3);
                total += 1;
            }
        }
        assert!(
            faces.index.len() * 8 < total,
            "{} of {total}",
            faces.index.len()
        );
        assert!(
            faces.index.len() * 16 < MAX_MODEL_FACES,
            "{}",
            faces.index.len()
        );
        assert_eq!(faces.words.len(), faces.index.len() * FACE_WORDS);
        // A stone block's six faces are six entries, and dirt's are others.
        let stone: Vec<_> = (0..6)
            .map(|f| faces.index.id(block("stone"), f).unwrap())
            .collect();
        let soil = faces.index.id(block("soil"), 0).unwrap();
        assert!(!stone.contains(&soil));
    }

    /// The words are laid out as the shader's `ModelFace` reads them.
    #[test]
    fn words_follow_the_shader_struct() {
        let face = ModelFace {
            positions: [Vec3::X, Vec3::Y, Vec3::Z, Vec3::ONE],
            uvs: [
                Vec2::new(0.1, 0.2),
                Vec2::new(0.3, 0.4),
                Vec2::ZERO,
                Vec2::ONE,
            ],
            normal: Vec3::NEG_Y,
            layer: 7 | TRIANGLE,
        };
        let words = face.words();
        let f = |w: u32| f32::from_bits(w);
        assert_eq!([f(words[4]), f(words[5]), f(words[6])], [0.0, 1.0, 0.0]);
        assert_eq!(f(words[7]), 0.3);
        assert_eq!(f(words[17]), 0.4);
        assert_eq!([f(words[20]), f(words[21]), f(words[22])], [0.0, -1.0, 0.0]);
        assert_eq!(words[23], 7 | TRIANGLE);
    }
}
