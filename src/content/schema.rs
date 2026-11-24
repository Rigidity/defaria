//! The JSON5 file formats, as they're written. `docs/modding.md` documents
//! them for modders; keep the two in step.
//!
//! Every struct denies unknown fields, so a misspelt key is an error rather
//! than silently ignored. References to other definitions are plain strings
//! here; [`super::bake`] resolves them.

use std::collections::BTreeMap;

use serde::Deserialize;

/// `mod.json5`, at the root of a mod folder.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The namespace of everything the mod defines: `<id>:<name>`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    /// Mods that must load first.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// `materials/<name>.json5`.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MaterialDef {
    /// Two to eight colors, darkest first, spread over the gray levels.
    pub palette: Vec<String>,
}

/// `textures/<name>.json5`.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextureDef {
    /// Bottom first.
    pub layers: Vec<LayerDef>,
}

/// One grayscale image and what colors it: a material, or a palette given
/// right here.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LayerDef {
    pub image: String,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub palette: Option<Vec<String>>,
}

/// `models/<name>.json5`.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ModelDef {
    /// Texture slots, each with the slot it falls back to (or `null`).
    #[serde(default)]
    pub slots: BTreeMap<String, Option<String>>,
    /// Sides of the block the model covers completely.
    #[serde(default)]
    pub occludes: Vec<Direction>,
    pub faces: Vec<FaceDef>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FaceDef {
    /// The slot whose texture the face shows.
    pub texture: String,
    /// Hidden when the neighbour on this side covers its facing side.
    #[serde(default)]
    pub cull: Option<Direction>,
    /// Three or four, counter-clockwise seen from the front.
    pub vertices: Vec<VertexDef>,
}

#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct VertexDef {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

/// A side of a block. Up is +Y, south is +Z and east is +X.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    North,
    South,
    East,
    West,
}

/// `blocks/<name>.json5`.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BlockDef {
    #[serde(default)]
    pub name: Option<String>,
    /// Each property's values; the first is the default.
    #[serde(default)]
    pub properties: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub model: Option<String>,
    /// Slot to texture. Variants merge into this rather than replace it.
    #[serde(default)]
    pub textures: BTreeMap<String, String>,
    /// Degrees about X, then Y, then Z; multiples of 90.
    #[serde(default)]
    pub rotate: Option<[i32; 3]>,
    /// The light the block gives off: red, green and blue levels, 0 to 15.
    #[serde(default)]
    pub light: Option<[u8; 3]>,
    /// Applied in order on top of the block's own look, each where its
    /// `when` matches the state.
    #[serde(default)]
    pub variants: Vec<VariantDef>,
    /// Whether the block gets an item that places it. On by default.
    #[serde(default = "default_true")]
    pub item: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct VariantDef {
    /// Property to the value, or any of the values, it must have.
    #[serde(default)]
    pub when: BTreeMap<String, OneOrMany>,
    #[serde(default)]
    pub model: Option<String>,
    /// Slot to texture. Variants merge into this rather than replace it.
    #[serde(default)]
    pub textures: BTreeMap<String, String>,
    /// Degrees about X, then Y, then Z; multiples of 90.
    #[serde(default)]
    pub rotate: Option<[i32; 3]>,
    /// The light the block gives off: red, green and blue levels, 0 to 15.
    #[serde(default)]
    pub light: Option<[u8; 3]>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn values(&self) -> &[String] {
        match self {
            OneOrMany::One(value) => std::slice::from_ref(value),
            OneOrMany::Many(values) => values,
        }
    }
}

/// `items/<name>.json5`. An item with the same name as a block replaces
/// that block's own item, keeping whatever it doesn't set.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ItemDef {
    #[serde(default)]
    pub name: Option<String>,
    /// The icon.
    #[serde(default)]
    pub texture: Option<String>,
    /// The block it places, in its default state.
    #[serde(default)]
    pub block: Option<String>,
    #[serde(default)]
    pub max_stack: Option<u32>,
}
