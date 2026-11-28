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

/// 32-bit words a table entry takes in the shader's `ModelFace`: each
/// corner's position and `u`, then the four `v`s, then the normal and
/// texture.
pub const FACE_WORDS: usize = 24;

/// One entry: a model face as the shader reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelFace {
    /// Each corner's position in its voxel; a triangle's fourth repeats
    /// its third.
    pub positions: [Vec3; 4],
    pub uvs: [Vec2; 4],
    pub normal: Vec3,
    /// The texture's index.
    pub texture: u32,
    /// Whether it has three corners rather than four. The shader needn't
    /// know: a triangle has no twist (see [`super::packed`]).
    pub triangle: bool,
}

impl ModelFace {
    fn of(face: &BakedFace) -> Self {
        let corner = |i: usize| &face.vertices[i.min(face.vertices.len() - 1)];
        Self {
            positions: std::array::from_fn(|i| corner(i).pos),
            uvs: std::array::from_fn(|i| corner(i).uv),
            normal: face.normal,
            texture: face.layer,
            triangle: face.vertices.len() == 3,
        }
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
        words[23] = self.texture;
        words
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
    /// The entry of face `face` of state `state`: by its index in the
    /// state's faces, or past them, in its `next_faces`.
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
    pub words: Vec<u32>,
}

impl ModelFaces {
    pub fn new(content: &Content) -> Self {
        Self::with_limit(content, MAX_MODEL_FACES)
    }

    /// The table, with at most `limit` entries: faces past it get none,
    /// and aren't drawn.
    fn with_limit(content: &Content, limit: usize) -> Self {
        let mut index = FaceIndex::default();
        let mut found: HashMap<([u32; FACE_WORDS], bool), u16> = HashMap::new();
        let mut words = Vec::new();
        let mut left_out = 0;
        for state in &content.states {
            index.starts.push(index.ids.len() as u32);
            // Its parts drawn by their neighbours after its own faces.
            for face in state.faces.iter().chain(&state.next_faces) {
                let entry = ModelFace::of(face);
                let entry_words = entry.words();
                let key = (entry_words, entry.triangle);
                let id = match found.get(&key) {
                    Some(&id) => Some(id),
                    None if index.faces.len() < limit => {
                        let id = index.faces.len() as u16;
                        found.insert(key, id);
                        index.faces.push(entry);
                        words.extend_from_slice(&entry_words);
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
                "the blocks' models have more distinct faces than the {limit} a chunk mesh can \
                 name; {left_out} faces aren't drawn"
            );
        }
        Self {
            index: Arc::new(index),
            words,
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
    /// The faces of parts drawn by their neighbours are numbered after
    /// the state's own, and each has its entry.
    #[test]
    fn parts_drawn_by_neighbours_have_entries() {
        let content = defaria_core::content::vines();
        let faces = ModelFaces::new(&content);
        let vine = content.default_state("test:vine").unwrap();
        let state = content.state(vine);
        assert!(!state.next_faces.is_empty());
        for (f, face) in state.next_faces.iter().enumerate() {
            let id = faces.index.id(vine, state.faces.len() + f).unwrap();
            let entry = faces.index.face(id);
            assert_eq!(*entry, ModelFace::of(face));
            assert_eq!(entry.texture, face.layer);
        }
    }

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
                assert_eq!(entry.texture, face.layer);
                assert_eq!(entry.triangle, face.vertices.len() == 3);
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

    /// Faces past the table's limit get no entry, while faces that share
    /// an entry already made still find it; and a mesh leaves the faces
    /// without one out.
    #[test]
    fn faces_past_the_limit_get_no_entry_and_arent_drawn() {
        use crate::meshing::build::Neighborhood;
        use defaria_core::light::Light;
        let content = base();
        let full = ModelFaces::new(content);
        let limit = 6;
        let small = ModelFaces::with_limit(content, limit);
        assert_eq!(small.index.len(), limit);
        assert_eq!(small.words.len(), limit * FACE_WORDS);
        let mut kept = 0;
        for (i, state) in content.states.iter().enumerate() {
            for f in 0..state.faces.len() {
                let state = BlockState(i as u16);
                match small.index.id(state, f) {
                    Some(id) => {
                        kept += 1;
                        let entry = full.index.face(full.index.id(state, f).unwrap());
                        assert_eq!(small.index.face(id), entry);
                    }
                    None => assert!(full.index.id(state, f).unwrap() as usize >= limit),
                }
            }
        }
        assert!(
            kept > limit,
            "states sharing the first entries still find them"
        );
        // A block whose faces are all past the limit draws nothing; one
        // within it draws all six.
        let mesh = |block: BlockState| {
            let pos = IVec3::splat(4);
            Neighborhood::from_fn(|p| (if p == pos { block } else { BlockState::AIR }, Light::DARK))
                .build(content, &small.index)
                .opaque
                .count_vertices()
        };
        let cube =
            |s: &BlockState| content.state(*s).faces.len() == 6 && content.state(*s).is_opaque();
        let states = || {
            (1..content.states.len() as u16)
                .map(BlockState)
                .filter(cube)
        };
        let drawn = states()
            .find(|&s| (0..6).all(|f| small.index.id(s, f).is_some()))
            .unwrap();
        let left_out = states()
            .find(|&s| (0..6).all(|f| small.index.id(s, f).is_none()))
            .unwrap();
        assert_eq!(mesh(drawn), 24);
        assert_eq!(mesh(left_out), 0);
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
            texture: 7,
            triangle: false,
        };
        let words = face.words();
        let f = |w: u32| f32::from_bits(w);
        assert_eq!([f(words[4]), f(words[5]), f(words[6])], [0.0, 1.0, 0.0]);
        assert_eq!(f(words[7]), 0.3);
        assert_eq!(f(words[17]), 0.4);
        assert_eq!([f(words[20]), f(words[21]), f(words[22])], [0.0, -1.0, 0.0]);
        assert_eq!(words[23], 7);
    }
}
