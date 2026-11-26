//! The JSON5 file formats, as they're written. `docs/modding.md` documents
//! them for modders; keep the two in step.
//!
//! Every struct denies unknown fields, so a misspelt key is an error rather
//! than silently ignored. References to other definitions are plain strings
//! here; [`super::bake`] resolves them.

use std::{collections::BTreeMap, fmt, marker::PhantomData};

use bevy::reflect::Reflect;

use crate::block_entity::Value;

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
    /// Other named numbers, such as `conductivity`, which attributes read
    /// as they read `hardness` and `weight`.
    #[serde(default)]
    pub stats: BTreeMap<String, f32>,
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

/// A block or item property: a list of values, or the materials it can be.
/// Its default is `default`, or else its first value.
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
    /// tag, in order of id. A material listed again is skipped.
    #[serde(default)]
    pub materials: Option<OneOrMany>,
    /// Adds the value `none` before the materials.
    #[serde(default)]
    pub optional: bool,
    /// The value the block or item has unless something sets it: one of
    /// `values`, or a material (or `none`). Left out, it's the first value,
    /// or `none` for an optional material.
    #[serde(default)]
    pub default: Option<String>,
    /// Whether the property is part of the block's item, so each of its
    /// values is a different item. On by default for materials, off for
    /// plain values. Only for blocks.
    #[serde(default)]
    pub item: Option<bool>,
    /// How a placed block picks the value, from where you click or look.
    /// Only for plain block properties.
    #[serde(default)]
    pub place: Option<PlaceDef>,
    /// What the block connects to its neighbours by: a rule's name, or
    /// `true` for a conduit, which connects by its network kind and group.
    /// The property becomes six: `north`, `south`, `east`, `west`, `up` and
    /// `down`, each `"false"` or `"true"`, set by the engine to whether the
    /// neighbour on that side accepts it. Nothing else goes with it. Only
    /// for blocks, once per block.
    #[serde(default)]
    pub connections: Option<ConnectionsDef>,
}

/// What a `connections` property connects by.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionsDef {
    /// A rule, by name: blocks with the same rule join.
    Rule(String),
    /// `true`: the block's `conduit` says, by its network kind and group.
    Conduit,
}

/// How a property is set when the block is placed: `"face"` or `"look"`,
/// or `{ from, values }` to say which value each direction gives.
#[derive(Debug, Clone)]
pub struct PlaceDef {
    pub from: PlaceFrom,
    /// Direction to value. Left out, each direction gives the value named
    /// after its axis (`x`, `y`, `z`) or after itself (`up`, `north`, ...).
    pub values: Option<BTreeMap<Direction, String>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct PlaceSpec {
    from: PlaceFrom,
    #[serde(default)]
    values: Option<BTreeMap<Direction, String>>,
}

/// Where a placed block's direction comes from.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlaceFrom {
    /// The side of the block you click: placing on top of a block is `up`.
    Face,
    /// The way you look, along its strongest axis: looking down is `down`.
    Look,
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
#[derive(Deserialize, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    /// Each property's values and default.
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
    /// How its faces are drawn: opaque (the default), cut out or
    /// translucent.
    #[serde(default)]
    pub render: Option<Render>,
    /// A fluid: nothing bumps into it, the crosshair passes through it, and
    /// placing a block replaces it. It flows, and the player swims in it,
    /// as this says. It has no `model`: it's drawn as a box as high as it's
    /// full.
    #[serde(default)]
    pub fluid: Option<FluidDef>,
    /// Its faces show from behind as well as in front.
    #[serde(default)]
    pub double_sided: bool,
    /// How hard each state is to break. Left out, 0.
    #[serde(default)]
    pub hardness: Option<Attribute>,
    /// What each state weighs, in kilograms. Left out, 0.
    #[serde(default)]
    pub weight: Option<Attribute>,
    /// Named numbers for each state, for systems and scripts to read.
    #[serde(default)]
    pub stats: BTreeMap<String, Attribute>,
    /// Applied in order on top of the block's own look, each where its
    /// `when` matches the state.
    #[serde(default)]
    pub variants: Vec<VariantDef>,
    /// Models added to the state's own, each where its `when` matches the
    /// state (or always, without one): a pipe's core, and an arm for each
    /// side it joins.
    #[serde(default)]
    pub parts: Vec<PartDef>,
    /// Whether the block gets an item that places it. On by default.
    #[serde(default = "default_true")]
    pub item: bool,
    /// The category its item is listed under in the inventory.
    #[serde(default)]
    pub category: Option<String>,
    /// It's part of a network: a wire, pipe or tube.
    #[serde(default)]
    pub conduit: Option<ConduitDef>,
    /// Each voxel of the block keeps data of its own, beyond its state,
    /// saved with its chunk.
    #[serde(default)]
    pub block_entity: Option<BlockEntityDef>,
    /// It's a machine: its ports onto networks, its energy buffer, tanks
    /// and item slots. A machine has a block entity, which keeps them.
    #[serde(default)]
    pub machine: Option<MachineDef>,
}

/// A block's `machine`: what it connects to and what it holds.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct MachineDef {
    /// Its faces that conduits join, each on a network kind with a role.
    #[serde(default)]
    pub ports: Vec<PortDef>,
    /// The energy it stores, and how fast it takes and gives it.
    #[serde(default)]
    pub energy: Option<EnergyDef>,
    /// Its fluid tanks, by name.
    #[serde(default)]
    pub tanks: BTreeMap<String, TankDef>,
    /// Its item slots: how many in each named group.
    #[serde(default)]
    pub slots: BTreeMap<String, u32>,
    /// The property values it has while it's working, such as
    /// `{ lit: "true" }`. When it stops, they go back to the block's
    /// defaults.
    #[serde(default)]
    pub active_state: BTreeMap<String, String>,
}

/// A face, or several, of a machine that conduits of one network kind join.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PortDef {
    /// Sides (`up`, `north`, ...), which turn with the block's `rotate`,
    /// or sides relative to its `facing` property (`front`, `back`,
    /// `left`, `right`).
    pub faces: Vec<PortFace>,
    /// The network kind, such as `"electric"`.
    pub network: String,
    /// What it does on the network.
    pub role: PortRole,
    /// The tank a fluid port fills or drains. Left out, the machine's only
    /// tank.
    #[serde(default)]
    pub tank: Option<String>,
    /// The slot group an item port fills or takes from. Left out, the
    /// machine's only group.
    #[serde(default)]
    pub slots: Option<String>,
}

/// A machine face as a port names it.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PortFace {
    Up,
    Down,
    North,
    South,
    East,
    West,
    /// The side its `facing` property names.
    Front,
    /// The side opposite the front.
    Back,
    /// The machine's own left, facing out of its front.
    Left,
    /// The machine's own right.
    Right,
}

/// What a port does on its network. Each solver takes some of them:
/// energy `produce`, `consume` and `store`; fluid and item `input`,
/// `output` and `store`.
#[derive(Deserialize, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PortRole {
    /// Energy: gives the network energy from its buffer (a generator).
    Produce,
    /// Energy: takes energy from the network into its buffer (a lamp).
    Consume,
    /// Takes in and gives out both, as the network has too much or too
    /// little: a battery, a tank, a crate.
    Store,
    /// Fluid or items: takes them from the network (an outlet, a
    /// furnace's input).
    Input,
    /// Fluid or items: gives them to the network (a pump, a furnace's
    /// output).
    Output,
}

impl PortRole {
    /// Whether things go from the machine into the network through it.
    pub fn gives(self) -> bool {
        matches!(self, PortRole::Produce | PortRole::Output | PortRole::Store)
    }

    /// Whether things come from the network into the machine through it.
    pub fn takes(self) -> bool {
        matches!(self, PortRole::Consume | PortRole::Input | PortRole::Store)
    }

    /// Whether a network run by `solver` has ports with this role.
    pub fn fits(self, solver: Solver) -> bool {
        match solver {
            Solver::Energy => matches!(
                self,
                PortRole::Produce | PortRole::Consume | PortRole::Store
            ),
            Solver::Fluid | Solver::Item => {
                matches!(self, PortRole::Input | PortRole::Output | PortRole::Store)
            }
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PortRole::Produce => "produce",
            PortRole::Consume => "consume",
            PortRole::Store => "store",
            PortRole::Input => "input",
            PortRole::Output => "output",
        }
    }
}

/// A machine's energy buffer, in joules, rates in watts.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EnergyDef {
    /// The most it holds.
    pub capacity: Attribute,
    /// The most it takes in a second.
    #[serde(default = "zero")]
    pub max_in: Attribute,
    /// The most it gives in a second.
    #[serde(default = "zero")]
    pub max_out: Attribute,
    /// What it makes on its own each second, into its buffer while there's
    /// room: a creative generator.
    #[serde(default = "zero")]
    pub generate: Attribute,
    /// What it uses up each second while it's working; it works (and is
    /// in its `active_state`) only while its buffer pays for it: a
    /// powered lamp.
    #[serde(default = "zero", rename = "use")]
    pub uses: Attribute,
}

fn zero() -> Attribute {
    Attribute::Fixed(0.0)
}

/// One of a machine's fluid tanks.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct TankDef {
    /// How much it holds, in blocks of fluid.
    pub capacity: Attribute,
    /// The fluids (blocks with `fluid`) it takes. Left out, any.
    #[serde(default)]
    pub accepts: Vec<String>,
}

/// A block's block entity: what a new one starts with.
#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockEntityDef {
    /// Plain data to start with, by key: numbers, strings, booleans, lists
    /// and tables of them.
    #[serde(default)]
    pub data: Option<BTreeMap<String, Value>>,
}

/// A block's `conduit`: the network kind it carries, and how much.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConduitDef {
    /// The network kind, such as `"electric"` (`networks/electric.json5`).
    pub network: String,
    /// Conduits with a group join only conduits of the same group, and
    /// those without one only others without: coloured cables side by
    /// side. `{property}` is filled per state.
    #[serde(default)]
    pub group: Option<String>,
    /// What one of it holds, in the solver's units.
    pub capacity: Attribute,
    /// What it passes on a second, in the solver's units.
    pub throughput: Attribute,
}

/// `networks/<name>.json5`: a kind of network conduits form.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct NetworkDef {
    pub name: String,
    /// The engine's solver that runs it.
    pub solver: Solver,
}

/// The engine's network solvers.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Solver {
    /// Energy: producers, consumers and storage (#112).
    Energy,
    /// One shared tank of one fluid (#113).
    Fluid,
    /// Items moved between machines (#120).
    Item,
}

/// How a fluid flows and what it's like to be in. Amounts are in blocks: 1
/// is a full block.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FluidDef {
    /// Flow steps a second.
    pub rate: f32,
    /// The most that falls from a cell into the one below in a step.
    pub fall: f32,
    /// The thinnest layer that still spreads onto dry ground. Thinner, it
    /// stays put, unless it can run over an edge.
    pub film: f32,
    /// The smallest difference between two cells that flows from one to
    /// the other. Below it the surface counts as level.
    pub ripple: f32,
    /// How fast the player moves in it, from 0 to 1 of walking speed.
    pub move_speed: f32,
    /// What the view looks like with the eyes in it.
    pub view: FluidViewDef,
}

/// The view from inside a fluid.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FluidViewDef {
    /// The fog's color, as `#rrggbb`.
    pub color: String,
    /// How far you can see, in blocks.
    pub distance: f32,
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
    /// How the variant's faces are drawn, replacing the block's.
    #[serde(default)]
    pub render: Option<Render>,
}

/// A model a block state adds to its own, where `when` matches.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PartDef {
    /// What each property must be for the part to be added, as in a
    /// variant. Left out, it always is.
    #[serde(default)]
    pub when: BTreeMap<String, Condition>,
    /// `{property}` is replaced by its value.
    pub model: String,
    /// Slot to texture, over the state's own textures for this part only.
    #[serde(default)]
    pub textures: BTreeMap<String, TextureRef>,
    /// Degrees about X, then Y, then Z; multiples of 90. Only this part
    /// turns: the block's `rotate` doesn't apply to it.
    #[serde(default)]
    pub rotate: Option<[i32; 3]>,
}

/// How a block's faces are drawn.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Render {
    /// Every pixel drawn solid. Its covered sides stop light and hide the
    /// faces next to them (a fluid's only some: see `Content::hides`).
    #[default]
    Opaque,
    /// Pixels with alpha under one half are holes; the rest are solid.
    /// Hides no faces next to it, and lets light through.
    Cutout,
    /// Pixels blend with what's behind them by their alpha. Hides only the
    /// same block's translucent faces next to it, and lets light through.
    Translucent,
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
    /// Each property's values and default. Not for a block's own item,
    /// which has the block's.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyDef>,
    /// The icon.
    #[serde(default)]
    pub texture: Option<TextureRef>,
    /// The block it places: its default state, with any properties it
    /// shares with the item set to the item's values.
    #[serde(default)]
    pub block: Option<String>,
    /// Every one of it is unique: copies never merge into one count in an
    /// inventory, as they will carry their own data (such as wear).
    #[serde(default)]
    pub unique: Option<bool>,
    /// How hard each state is. Left out, the block it places' (for a
    /// block's item, the state it places), or 0.
    #[serde(default)]
    pub hardness: Option<Attribute>,
    /// What one of each state weighs, in kilograms. Left out, what the
    /// block it places weighs, or 0.
    #[serde(default)]
    pub weight: Option<Attribute>,
    /// Named numbers for each state. Left out, a stat is the block it
    /// places' (for a block's item, the state it places').
    #[serde(default)]
    pub stats: BTreeMap<String, Attribute>,
    /// The category it's listed under in the inventory. Left out, a
    /// block's item is in the block's.
    #[serde(default)]
    pub category: Option<String>,
}

/// `categories/<name>.json5`: a heading items are listed under in the
/// inventory.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CategoryDef {
    pub name: String,
    /// Where it comes among the categories: lowest first, ties by id.
    #[serde(default)]
    pub order: i32,
}

/// A number a block or item works out per state, such as its hardness or
/// weight: a number, or a base times a weighted average of one stat of its
/// material properties' materials.
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
    /// The materials' stat it averages: `hardness`, `weight` or one of
    /// their `stats`. Left out, the stat named like the field.
    #[serde(default)]
    pub stat: Option<String>,
    /// The value for a material without the stat. Left out, a material
    /// without it is an error.
    #[serde(default)]
    pub default: Option<f32>,
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

    fn from_bool<E: de::Error>(value: bool) -> Result<Self, E> {
        Err(E::invalid_type(Unexpected::Bool(value), &Self::EXPECTING))
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

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<T, E> {
        T::from_bool(value)
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

deserialize_forms!(
    TextureRef,
    PropertyDef,
    Condition,
    OneOrMany,
    Attribute,
    PlaceDef,
    ConnectionsDef
);

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
    const EXPECTING: &'static str = "a number, or `{ base, stat, materials, default }`";

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

impl Forms for PlaceDef {
    const EXPECTING: &'static str = "`\"face\"`, `\"look\"`, or `{ from, values }`";

    fn from_str<E: de::Error>(value: &str) -> Result<Self, E> {
        let from = PlaceFrom::deserialize(de::value::StrDeserializer::<E>::new(value))?;
        Ok(PlaceDef { from, values: None })
    }

    fn from_map<'de, A: MapAccess<'de>>(m: A) -> Result<Self, A::Error> {
        let spec: PlaceSpec = map(m)?;
        Ok(PlaceDef {
            from: spec.from,
            values: spec.values,
        })
    }
}

impl Forms for ConnectionsDef {
    const EXPECTING: &'static str = "a rule's name, or `true` for a conduit";

    fn from_str<E: de::Error>(value: &str) -> Result<Self, E> {
        Ok(ConnectionsDef::Rule(value.into()))
    }

    fn from_bool<E: de::Error>(value: bool) -> Result<Self, E> {
        if value {
            Ok(ConnectionsDef::Conduit)
        } else {
            Err(E::invalid_value(Unexpected::Bool(value), &Self::EXPECTING))
        }
    }
}
