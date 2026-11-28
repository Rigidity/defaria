//! A chunk mesh's vertices, packed: 20 bytes each, against 68 when they
//! carried their own positions, normals and texture coordinates.
//!
//! What's the same wherever a block is (where a face's corners sit in its
//! voxel, their texture coordinates, its normal and texture) is in the
//! model face table ([`super::model_faces`]), once. A vertex carries what
//! isn't:
//!
//! | Word | Bits | What |
//! |---|---|---|
//! | 0 | 32 | its corner's light: sky, red, green, blue, a byte each (255 = level 15) |
//! | 1 | 10, 10, 10, 2 | the face's light [twist](#the-twist) for sky, red and green; its corner's ambient occlusion, 0-3 |
//! | 2 | 10, 4 | the twist for blue and for ambient occlusion |
//! | 2 | 4, 4, 4 | its voxel in the chunk: x, y, z |
//! | 2 | 2 | which corner of the model face it is |
//! | 2 | 1, 1, 1 | flags: [`BACK`], [`FLOWING`], [`WRAPPED`] |
//! | 3 | 16 | the model face's index in the table |
//! | 3 | 16 | a fluid face's height at this corner, in [`HEIGHT_STEPS`] a block (0 elsewhere) |
//! | flow | 2 × 16 | a fluid face's flow here, `Snorm16x2` ([`ATTRIBUTE_FLOW`]) |
//!
//! The voxel position needs no more than 4 bits an axis: everything in a
//! chunk's mesh is in one of its 16³ voxels, and where in the voxel comes
//! from the table, at full precision, so rotated and odd model geometry
//! costs nothing. The one shape the table can't hold is a fluid's surface,
//! whose corners stand at heights its neighbours set: the vertex scales
//! its model corner's height by its own ([`WRAPPED`] faces only), in
//! steps of [`HEIGHT_STEPS`] a block: exact for every multiple of 1/16,
//! 1/48 and 1/64 of a block, so for the averages of up to four fluid
//! steps (see `FluidTop`) of a voxel all fluid can fill, and within a
//! 100,000th of a block otherwise. A fluid face's texture coordinates come from where
//! it is, which the shader works out from the position, as the mesher did.
//!
//! # The twist
//!
//! Light and ambient occlusion blend bilinearly between a face's four
//! corners (see `build`). That used to take all four corners' values in
//! every vertex. Each vertex now carries only its own corner's, which the
//! GPU interpolates across each triangle as usual, plus the face's twist,
//! `t = c₀ − c₁ + c₂ − c₃` (its corners in order round the face), the same
//! in all four. With `(s, t)` where the fragment is on the face (corner 0
//! at `(0, 0)`, 1 at `(1, 0)`, 2 at `(1, 1)`, 3 at `(0, 1)`), and the quad
//! split along its 0-2 diagonal, the bilinear blend is the interpolated
//! value less `min(s, t) · (1 − max(s, t)) · twist`, exactly, in either
//! triangle, whatever the quad's shape. A triangle (corners 0, 1 and 2) is
//! blended flat, with no twist. Light's twist runs from −510 to 510 in
//! light bytes (10 bits, signed), ambient occlusion's from −6 to 6 (4
//! bits).
//!
//! A face's back is the same vertices in the other order, flagged
//! [`BACK`]: the shader turns its normal round and draws it a hair farther
//! away. Its corners' light and the twist are the front's: blending the
//! same values at the same places comes out the same either way round.

use bevy::{
    mesh::{MeshVertexAttribute, VertexFormat},
    prelude::*,
};

/// A vertex's four packed words (see the module's table).
pub const ATTRIBUTE_PACKED: MeshVertexAttribute =
    MeshVertexAttribute::new("Packed", 0x0DEF_A41A_0005, VertexFormat::Uint32x4);

/// A fluid face's flow at each vertex (see `scroll`): which way, and how
/// strongly, its texture moves, in its texture coordinates; zero
/// everywhere else. The shader blends the corners across the face.
pub const ATTRIBUTE_FLOW: MeshVertexAttribute =
    MeshVertexAttribute::new("Flow", 0x0DEF_A41A_0004, VertexFormat::Snorm16x2);

/// Bytes a vertex takes.
pub const VERTEX_BYTES: usize = 4 * 4 + 2 * 2;

/// A field of a packed vertex: its word, lowest bit and width.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub word: usize,
    pub offset: u32,
    pub bits: u32,
}

impl Field {
    const fn new(word: usize, offset: u32, bits: u32) -> Self {
        Self { word, offset, bits }
    }

    const fn mask(self) -> u32 {
        if self.bits == 32 {
            u32::MAX
        } else {
            ((1 << self.bits) - 1) << self.offset
        }
    }

    /// The field's bits, at the bottom.
    const fn low(self) -> u32 {
        self.mask() >> self.offset
    }

    /// Puts `value` in the field.
    fn put(self, words: &mut [u32; 4], value: u32) {
        debug_assert!(
            self.bits == 32 || value >> self.bits == 0,
            "{value} in {self:?}"
        );
        words[self.word] |= (value << self.offset) & self.mask();
    }

    fn get(self, words: &[u32; 4]) -> u32 {
        (words[self.word] & self.mask()) >> self.offset
    }

    /// The field as a signed number, as the shader's `extractBits` on an
    /// `i32` reads it.
    fn get_signed(self, words: &[u32; 4]) -> i32 {
        let shift = 32 - self.bits;
        ((self.get(words) << shift) as i32) >> shift
    }
}

/// Bits of a light byte's twist.
pub const LIGHT_TWIST_BITS: u32 = 10;
/// Bits of the ambient occlusion twist.
pub const AO_TWIST_BITS: u32 = 4;
/// Bits of a model face's index.
pub const FACE_BITS: u32 = 16;

pub const LIGHT: Field = Field::new(0, 0, 32);
/// Sky, red, green and blue.
pub const LIGHT_TWIST: [Field; 4] = [
    Field::new(1, 0, LIGHT_TWIST_BITS),
    Field::new(1, 10, LIGHT_TWIST_BITS),
    Field::new(1, 20, LIGHT_TWIST_BITS),
    Field::new(2, 0, LIGHT_TWIST_BITS),
];
pub const AO: Field = Field::new(1, 30, 2);
pub const AO_TWIST: Field = Field::new(2, 10, AO_TWIST_BITS);
/// X, Y and Z.
pub const VOXEL: [Field; 3] = [
    Field::new(2, 14, 4),
    Field::new(2, 18, 4),
    Field::new(2, 22, 4),
];
pub const CORNER: Field = Field::new(2, 26, 2);
/// A double-sided block's back face.
pub const BACK: Field = Field::new(2, 28, 1);
/// A fluid face any of whose corners flows: only these pay for the flow
/// map's two samples.
pub const FLOWING: Field = Field::new(2, 29, 1);
/// A fluid face: its height is scaled and its texture coordinates come
/// from where it is, wrapping every block (see `scroll::uv_at`).
pub const WRAPPED: Field = Field::new(2, 30, 1);
pub const FACE: Field = Field::new(3, 0, FACE_BITS);
pub const HEIGHT: Field = Field::new(3, 16, 16);
/// A block's height in [`HEIGHT`]'s steps: 3 × 2¹⁴, so thirds and
/// powers of two divide it.
pub const HEIGHT_STEPS: u32 = 3 << 14;

/// The word's one bit no field uses.
const SPARE: Field = Field::new(2, 31, 1);

/// Every field, by the name the shader gives it, for checking they don't
/// overlap and that the shader reads them where they are.
pub const FIELDS: [(&str, Field); 16] = [
    ("LIGHT", LIGHT),
    ("LIGHT_TWIST_SKY", LIGHT_TWIST[0]),
    ("LIGHT_TWIST_RED", LIGHT_TWIST[1]),
    ("LIGHT_TWIST_GREEN", LIGHT_TWIST[2]),
    ("LIGHT_TWIST_BLUE", LIGHT_TWIST[3]),
    ("AO", AO),
    ("AO_TWIST", AO_TWIST),
    ("VOXEL_X", VOXEL[0]),
    ("VOXEL_Y", VOXEL[1]),
    ("VOXEL_Z", VOXEL[2]),
    ("CORNER", CORNER),
    ("BACK", BACK),
    ("FLOWING", FLOWING),
    ("WRAPPED", WRAPPED),
    ("FACE", FACE),
    ("HEIGHT", HEIGHT),
];

const fn fields_fit() -> bool {
    let mut used = [0u32; 4];
    let mut i = 0;
    while i <= FIELDS.len() {
        let field = if i == FIELDS.len() {
            SPARE
        } else {
            FIELDS[i].1
        };
        if field.offset + field.bits > 32 || used[field.word] & field.mask() != 0 {
            return false;
        }
        used[field.word] |= field.mask();
        i += 1;
    }
    used[0] == u32::MAX && used[1] == u32::MAX && used[2] == u32::MAX && used[3] == u32::MAX
}

// The fields fill the four words exactly, without overlapping.
const _: () = assert!(fields_fit());
// A chunk's voxels fit the position's bits, a face's twists theirs, and
// every texture the table's layer.
const _: () = assert!(defaria_core::world::CHUNK_SIZE == 1 << 4);
const _: () = assert!(HEIGHT_STEPS < 1 << HEIGHT.bits);
const _: () = assert!(HEIGHT_STEPS.is_multiple_of(16 * 3 * 4));
const _: () = assert!((2 * u8::MAX as i32) < 1 << (LIGHT_TWIST_BITS - 1));
const _: () = assert!(2 * 3 < 1 << (AO_TWIST_BITS - 1));

/// What one vertex carries, before packing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vertex {
    /// Its voxel in the chunk, each from 0 to 15.
    pub voxel: UVec3,
    /// Which of the model face's corners it is.
    pub corner: u8,
    pub face: u16,
    pub back: bool,
    pub flowing: bool,
    pub wrapped: bool,
    /// A fluid face's height at this corner, from 0 to 1.
    pub height: f32,
    /// Its corner's sky, red, green and blue light, 255 at level 15.
    pub light: [u8; 4],
    /// The face's light twist, per channel.
    pub light_twist: [i16; 4],
    /// Its corner's ambient occlusion, 0 to 3.
    pub ao: u8,
    pub ao_twist: i8,
}

impl Vertex {
    pub fn pack(&self) -> [u32; 4] {
        let mut words = [0; 4];
        LIGHT.put(&mut words, u32::from_le_bytes(self.light));
        for (field, twist) in LIGHT_TWIST.iter().zip(self.light_twist) {
            field.put(&mut words, twist as u32 & field.low());
        }
        AO.put(&mut words, self.ao as u32);
        AO_TWIST.put(&mut words, self.ao_twist as u32 & AO_TWIST.low());
        for (field, at) in VOXEL.iter().zip(self.voxel.to_array()) {
            field.put(&mut words, at);
        }
        CORNER.put(&mut words, self.corner as u32);
        BACK.put(&mut words, self.back as u32);
        FLOWING.put(&mut words, self.flowing as u32);
        WRAPPED.put(&mut words, self.wrapped as u32);
        FACE.put(&mut words, self.face as u32);
        HEIGHT.put(&mut words, height_steps(self.height));
        words
    }

    /// As the shader unpacks it.
    pub fn unpack(words: [u32; 4]) -> Self {
        Self {
            voxel: UVec3::from_array(VOXEL.map(|field| field.get(&words))),
            corner: CORNER.get(&words) as u8,
            face: FACE.get(&words) as u16,
            back: BACK.get(&words) != 0,
            flowing: FLOWING.get(&words) != 0,
            wrapped: WRAPPED.get(&words) != 0,
            height: HEIGHT.get(&words) as f32 / HEIGHT_STEPS as f32,
            light: LIGHT.get(&words).to_le_bytes(),
            light_twist: LIGHT_TWIST.map(|field| field.get_signed(&words) as i16),
            ao: AO.get(&words) as u8,
            ao_twist: AO_TWIST.get_signed(&words) as i8,
        }
    }
}

/// `height` (from 0 to 1 block) in [`HEIGHT_STEPS`].
fn height_steps(height: f32) -> u32 {
    (height.clamp(0.0, 1.0) * HEIGHT_STEPS as f32).round() as u32
}

/// The twist of a face's corners' values (see the module docs), in their
/// order round the face.
pub fn twist(corners: [i32; 4]) -> i32 {
    corners[0] - corners[1] + corners[2] - corners[3]
}

/// Where corner `corner` of a face is, for blending: `(0, 0)`, `(1, 0)`,
/// `(1, 1)` and `(0, 1)` round it. As the shader's `corner_position`.
pub fn corner_position(corner: u8) -> Vec2 {
    Vec2::new(
        (corner == 1 || corner == 2) as u8 as f32,
        (corner >= 2) as u8 as f32,
    )
}

/// The bilinear blend at `at` on a face, from the value the GPU
/// interpolates between its vertices' own corners (`interpolated`) and
/// the face's twist. As the shader's `untwist`.
pub fn untwist(interpolated: f32, twist: f32, at: Vec2) -> f32 {
    interpolated - at.min_element() * (1.0 - at.max_element()) * twist
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field comes back as it went in, at its extremes.
    #[test]
    fn vertices_round_trip() {
        let vertices = [
            Vertex::default(),
            Vertex {
                voxel: UVec3::new(15, 0, 9),
                corner: 3,
                face: u16::MAX,
                back: true,
                flowing: true,
                wrapped: true,
                height: 1.0,
                light: [255, 0, 17, 254],
                light_twist: [-510, 510, 0, -1],
                ao: 3,
                ao_twist: -6,
            },
            Vertex {
                voxel: UVec3::new(0, 15, 15),
                corner: 1,
                face: 1234,
                height: 0.0,
                light_twist: [511, -512, 7, 255],
                ao: 1,
                ao_twist: 6,
                ..default()
            },
        ];
        for vertex in vertices {
            assert_eq!(Vertex::unpack(vertex.pack()), vertex);
        }
        // Heights fluid steps make come back exactly, and others within
        // half a step.
        for height in (0..=192).map(|i| i as f32 / 192.0).chain([1.0 / 3.0, 0.75]) {
            let vertex = Vertex {
                height,
                ..default()
            };
            assert_eq!(Vertex::unpack(vertex.pack()).height, height);
        }
        let odd = Vertex {
            height: 0.437,
            ..default()
        };
        let error = (Vertex::unpack(odd.pack()).height - 0.437).abs();
        assert!(error <= 0.5 / HEIGHT_STEPS as f32, "{error}");
    }

    /// The bilinear blend of a quad's corners, from each triangle's
    /// interpolation of its own three and the twist, whichever triangle
    /// the point is in.
    #[test]
    fn untwisting_gives_the_bilinear_blend() {
        let corners = [3.0f32, 250.0, 17.0, 120.0];
        let bilinear = |at: Vec2| {
            let bottom = corners[0] + (corners[1] - corners[0]) * at.x;
            let top = corners[3] + (corners[2] - corners[3]) * at.x;
            bottom + (top - bottom) * at.y
        };
        let twist = twist(corners.map(|c| c as i32)) as f32;
        for i in 0..=10 {
            for j in 0..=10 {
                let at = Vec2::new(i as f32, j as f32) / 10.0;
                // Barycentric interpolation in the triangle (0, 1, 2) when
                // below the diagonal, (0, 2, 3) above it.
                let interpolated = if at.x >= at.y {
                    corners[0] * (1.0 - at.x) + corners[1] * (at.x - at.y) + corners[2] * at.y
                } else {
                    corners[0] * (1.0 - at.y) + corners[2] * at.x + corners[3] * (at.y - at.x)
                };
                let blended = untwist(interpolated, twist, at);
                assert!((blended - bilinear(at)).abs() < 1e-3, "{at}: {blended}");
            }
        }
    }
}
