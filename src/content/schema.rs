//! The JSON5 file formats, as they're written. `docs/modding.md` documents
//! them for modders; keep the two in step.
//!
//! Every struct denies unknown fields, so a misspelt key is an error rather
//! than silently ignored. References to other definitions are plain strings
//! here; [`super::bake`] resolves them.

use std::{collections::BTreeMap, fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer,
    de::{
        self, MapAccess, SeqAccess, Unexpected, Visitor,
        value::{MapAccessDeserializer, SeqAccessDeserializer},
    },
};

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
    /// Shown in names that use the material, such as "{wood} Log".
    #[serde(default)]
    pub name: Option<String>,
    /// Groups the material belongs to, which properties choose from
    /// (`"#wood"`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Two to eight colors, darkest first, spread over the gray levels.
    pub palette: Vec<String>,
    /// Other palettes for particular uses, by role: a layer with
    /// `role: "leaves"` takes the material's `leaves` palette, or its main
    /// one if it has none.
    #[serde(default)]
    pub palettes: BTreeMap<String, Vec<String>>,
    /// How hard it is to break, unitless: dirt is about 0.5, stone 1.5.
    pub hardness: f32,
    /// What a whole block of it weighs, in kilograms (its density in
    /// kg/m³).
    pub weight: f32,
}

/// `textures/<name>.json5`.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextureDef {
    /// Bottom first.
    pub layers: Vec<LayerDef>,
}

/// One grayscale image and what colors it: a material, or a palette given
/// right here. A material written `$name` is one of the texture's inputs,
/// which whoever uses the texture fills in.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LayerDef {
    pub image: String,
    #[serde(default)]
    pub material: Option<String>,
    /// Which of the material's `palettes` to use, falling back to its main
    /// one.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub palette: Option<Vec<String>>,
}

/// A texture as blocks and items name it: its id, or its id with materials
/// for its `$inputs`.
#[derive(Debug, Clone)]
pub enum TextureRef {
    Id(String),
    Bound(BoundTexture),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BoundTexture {
    pub texture: String,
    /// Input name to material. `{property}` works here as in the id.
    #[serde(default)]
    pub materials: BTreeMap<String, String>,
}

/// A block or item property: a list of values, the first the default, or
/// the materials it can be.
#[derive(Debug, Clone)]
pub enum PropertyDef {
    Values(Vec<String>),
    Full(PropertySpec),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PropertySpec {
    /// Plain values, as in the short form.
    #[serde(default)]
    pub values: Option<Vec<String>>,
    /// Materials, each a material or `#tag` for every material with that
    /// tag, in order of id. A material listed again is skipped, so
    /// `["stone", "#rock"]` puts stone first.
    #[serde(default)]
    pub materials: Option<OneOrMany>,
    /// Adds the value `none` before the materials, as the default.
    #[serde(default)]
    pub optional: bool,
    /// Whether the property is part of the block's item, so each of its
    /// values is a different item. On by default for materials, off for
    /// plain values. Only for blocks.
    #[serde(default)]
    pub item: Option<bool>,
}

/// What a variant's `when` asks of one property: a value, any of several,
/// or none of them. A value may be `#tag`, matching every material with
/// that tag.
#[derive(Debug, Clone)]
pub enum Condition {
    Is(OneOrMany),
    Not(NotCondition),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct NotCondition {
    pub not: OneOrMany,
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
    /// The boxes things bump into, within the block. Left out, it's the box
    /// around every face; `[]` is a block you walk through.
    #[serde(default)]
    pub collision: Option<Vec<BoxDef>>,
    pub faces: Vec<FaceDef>,
}

/// A box inside a block, from one corner to the opposite one.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct BoxDef {
    pub from: [f32; 3],
    pub to: [f32; 3],
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
    /// `{property}` is replaced by its value, or its material's name.
    #[serde(default)]
    pub name: Option<String>,
    /// Each property's values; the first is the default.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyDef>,
    #[serde(default)]
    pub model: Option<String>,
    /// Slot to texture. Variants merge into this rather than replace it.
    #[serde(default)]
    pub textures: BTreeMap<String, TextureRef>,
    /// Degrees about X, then Y, then Z; multiples of 90.
    #[serde(default)]
    pub rotate: Option<[i32; 3]>,
    /// The light the block gives off: red, green and blue levels, 0 to 15.
    #[serde(default)]
    pub light: Option<[u8; 3]>,
    /// How hard each state is to break. Left out, 0.
    #[serde(default)]
    pub hardness: Option<Attribute>,
    /// What each state weighs, in kilograms. Left out, 0.
    #[serde(default)]
    pub weight: Option<Attribute>,
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
    /// What each property must be for the variant to apply.
    #[serde(default)]
    pub when: BTreeMap<String, Condition>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Slot to texture. Variants merge into this rather than replace it.
    #[serde(default)]
    pub textures: BTreeMap<String, TextureRef>,
    /// Degrees about X, then Y, then Z; multiples of 90.
    #[serde(default)]
    pub rotate: Option<[i32; 3]>,
    /// The light the block gives off: red, green and blue levels, 0 to 15.
    #[serde(default)]
    pub light: Option<[u8; 3]>,
}

#[derive(Debug, Clone)]
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
    /// `{property}` is replaced by its value, or its material's name.
    #[serde(default)]
    pub name: Option<String>,
    /// Each property's values; the first is the default. Not for a block's
    /// own item, which has the block's.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyDef>,
    /// The icon.
    #[serde(default)]
    pub texture: Option<TextureRef>,
    /// The block it places: its default state, with any properties it
    /// shares with the item set to the item's values.
    #[serde(default)]
    pub block: Option<String>,
    #[serde(default)]
    pub max_stack: Option<u32>,
    /// How hard each state is. Left out, the block it places' (for a
    /// block's item, the state it places), or 0.
    #[serde(default)]
    pub hardness: Option<Attribute>,
    /// What one of each state weighs, in kilograms. Left out, what the
    /// block it places weighs, or 0.
    #[serde(default)]
    pub weight: Option<Attribute>,
}

/// A block's or item's hardness or weight: a number, or a base times a
/// weighted average of what its material properties' materials say.
#[derive(Debug, Clone)]
pub enum Attribute {
    Fixed(f32),
    Mixed(MixedAttribute),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MixedAttribute {
    /// Multiplies the average. Default 1.
    #[serde(default = "default_one")]
    pub base: f32,
    /// Material property to how much it counts, such as
    /// `{ rock: 3, ore: 1 }`. A property at `none` doesn't count.
    pub materials: BTreeMap<String, f32>,
}

fn default_one() -> f32 {
    1.0
}

/// A value that can be written in more than one form: a string, a list or
/// a map. Picking the form by what's written, rather than trying each
/// (`#[serde(untagged)]`), passes on the chosen form's own error, so a
/// misspelt key in a map is named.
trait Forms: Sized {
    const EXPECTING: &'static str;

    fn from_str<E: de::Error>(_: &str) -> Result<Self, E> {
        Err(E::invalid_type(
            Unexpected::Other("a string"),
            &Self::EXPECTING,
        ))
    }

    fn from_seq<'de, A: SeqAccess<'de>>(_: A) -> Result<Self, A::Error> {
        Err(de::Error::invalid_type(Unexpected::Seq, &Self::EXPECTING))
    }

    fn from_map<'de, A: MapAccess<'de>>(_: A) -> Result<Self, A::Error> {
        Err(de::Error::invalid_type(Unexpected::Map, &Self::EXPECTING))
    }

    fn from_f64<E: de::Error>(value: f64) -> Result<Self, E> {
        Err(E::invalid_type(Unexpected::Float(value), &Self::EXPECTING))
    }

    fn from_i64<E: de::Error>(value: i64) -> Result<Self, E> {
        Err(E::invalid_type(Unexpected::Signed(value), &Self::EXPECTING))
    }

    fn from_u64<E: de::Error>(value: u64) -> Result<Self, E> {
        Err(E::invalid_type(
            Unexpected::Unsigned(value),
            &Self::EXPECTING,
        ))
    }
}

struct FormsVisitor<T>(PhantomData<T>);

impl<'de, T: Forms> Visitor<'de> for FormsVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(T::EXPECTING)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
        T::from_str(value)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<T, E> {
        T::from_f64(value)
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<T, E> {
        T::from_i64(value)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<T, E> {
        T::from_u64(value)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
        T::from_seq(seq)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::from_map(map)
    }
}

macro_rules! deserialize_forms {
    ($($ty:ty),*) => {$(
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserializer.deserialize_any(FormsVisitor(PhantomData))
            }
        }
    )*};
}

deserialize_forms!(TextureRef, PropertyDef, Condition, OneOrMany, Attribute);

fn seq<'de, T: Deserialize<'de>, A: SeqAccess<'de>>(seq: A) -> Result<T, A::Error> {
    T::deserialize(SeqAccessDeserializer::new(seq))
}

fn map<'de, T: Deserialize<'de>, A: MapAccess<'de>>(map: A) -> Result<T, A::Error> {
    T::deserialize(MapAccessDeserializer::new(map))
}

impl Forms for TextureRef {
    const EXPECTING: &'static str = "a texture id, or `{ texture, materials }`";

    fn from_str<E: de::Error>(value: &str) -> Result<Self, E> {
        Ok(TextureRef::Id(value.into()))
    }

    fn from_map<'de, A: MapAccess<'de>>(m: A) -> Result<Self, A::Error> {
        map(m).map(TextureRef::Bound)
    }
}

impl Forms for PropertyDef {
    const EXPECTING: &'static str = "a list of values, or `{ values }` or `{ materials }`";

    fn from_seq<'de, A: SeqAccess<'de>>(s: A) -> Result<Self, A::Error> {
        seq(s).map(PropertyDef::Values)
    }

    fn from_map<'de, A: MapAccess<'de>>(m: A) -> Result<Self, A::Error> {
        map(m).map(PropertyDef::Full)
    }
}

impl Forms for Condition {
    const EXPECTING: &'static str = "a value, a list of values, or `{ not }`";

    fn from_str<E: de::Error>(value: &str) -> Result<Self, E> {
        OneOrMany::from_str(value).map(Condition::Is)
    }

    fn from_seq<'de, A: SeqAccess<'de>>(s: A) -> Result<Self, A::Error> {
        OneOrMany::from_seq(s).map(Condition::Is)
    }

    fn from_map<'de, A: MapAccess<'de>>(m: A) -> Result<Self, A::Error> {
        map(m).map(Condition::Not)
    }
}

impl Forms for OneOrMany {
    const EXPECTING: &'static str = "a value or a list of values";

    fn from_str<E: de::Error>(value: &str) -> Result<Self, E> {
        Ok(OneOrMany::One(value.into()))
    }

    fn from_seq<'de, A: SeqAccess<'de>>(s: A) -> Result<Self, A::Error> {
        seq(s).map(OneOrMany::Many)
    }
}

impl Forms for Attribute {
    const EXPECTING: &'static str = "a number, or `{ base, materials }`";

    fn from_f64<E: de::Error>(value: f64) -> Result<Self, E> {
        Ok(Attribute::Fixed(value as f32))
    }

    fn from_i64<E: de::Error>(value: i64) -> Result<Self, E> {
        Ok(Attribute::Fixed(value as f32))
    }

    fn from_u64<E: de::Error>(value: u64) -> Result<Self, E> {
        Ok(Attribute::Fixed(value as f32))
    }

    fn from_map<'de, A: MapAccess<'de>>(m: A) -> Result<Self, A::Error> {
        map(m).map(Attribute::Mixed)
    }
}
