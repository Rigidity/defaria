//! Game content from mods: materials, textures, models, blocks and items,
//! declared in JSON5 files. The format is documented in `docs/modding.md`.
//!
//! Every mod is a folder in `assets/mods/` with a `mod.json5` manifest and
//! a folder per kind of definition. The base game is the `defaria` mod. A
//! definition's id is its mod's id and its path in the kind's folder, so
//! `blocks/ores/iron.json5` in mod `defaria` is block `defaria:ores/iron`.
//! References inside a file may leave out the mod (`stone` means
//! `<this mod>:stone`).
//!
//! Loading reads every mod once, at startup ([`Content`]'s `FromWorld`),
//! in three steps:
//!
//! 1. [`load`] reads the folders and orders the mods by dependency.
//! 2. [`bake`] parses each file and resolves the references between them,
//!    working out up front everything the game needs per frame: each block
//!    state's faces, already rotated and with their texture's layer in the
//!    block texture array.
//! 3. The result is the [`Content`] resource. Chunks store [`BlockState`]s,
//!    which index [`Content::states`].
//!
//! A broken file is reported (logged, and kept in [`Content::errors`]) and
//! left out, along with anything that refers to it; everything else still
//! loads. The layer images themselves load through the asset server (see
//! `voxel_material`), so they hot reload; the JSON5 files don't yet.

mod bake;
pub(crate) use bake::title_case;
mod connect;
pub mod load;
mod machine;
mod place;
mod recipe;
pub mod schema;
mod state_name;

use std::{collections::BTreeMap, ops::Range, path::Path};

use bevy::{asset::io::file::FileAssetReader, platform::collections::HashMap, prelude::*};

use crate::block_texture::Palette;
pub use connect::{Connections, Joins, Rule, SIDE_VALUES};
pub use machine::{
    Emits, EnergySpec, FACING, ItemFilter, MAX_SLOTS, Machine, MachinePort, MachineRecipes,
    SlotGroupSpec, TankSpec, resolve_face,
};
pub use place::Placement;
pub use recipe::{Recipe, RecipeInput, RecipeOutput, share};
pub use schema::{Direction, ItemAction, PlaceFrom, PortRole, Render, Solver};
pub use state_name::StateName;

/// The mods folder, relative to `assets/`.
pub const MODS_DIR: &str = "mods";
/// The id of the block that's nothing at all. Not defined by any mod.
pub const AIR: &str = "air";
/// The brightest light level, in each colour channel and the sky's.
pub const MAX_LIGHT: u8 = 15;
/// [`State::occludes`] bits for every side.
const ALL_SIDES: u8 = (1 << Direction::ALL.len()) - 1;

pub struct ContentPlugin;

impl Plugin for ContentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Content>();
    }
}

/// A block in one particular state: an index into [`Content::states`].
/// State 0 is always air.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct BlockState(pub u16);

impl BlockState {
    pub const AIR: Self = Self(0);

    pub fn is_air(self) -> bool {
        self == Self::AIR
    }
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::Up,
        Direction::Down,
        Direction::North,
        Direction::South,
        Direction::East,
        Direction::West,
    ];

    /// The unit vector pointing out of this side of a block.
    pub fn normal(self) -> IVec3 {
        match self {
            Direction::Up => IVec3::Y,
            Direction::Down => IVec3::NEG_Y,
            Direction::North => IVec3::NEG_Z,
            Direction::South => IVec3::Z,
            Direction::East => IVec3::X,
            Direction::West => IVec3::NEG_X,
        }
    }

    pub fn from_normal(normal: IVec3) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.normal() == normal)
    }

    pub fn opposite(self) -> Self {
        match self {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::East => Direction::West,
            Direction::West => Direction::East,
        }
    }

    /// This side's bit in a set of sides kept as a `u8`.
    pub(crate) fn bit(self) -> u8 {
        1 << self as u8
    }

    /// The direction nearest `dir`: the one along its largest component.
    /// Ties go to Y, then X.
    pub fn along(dir: Vec3) -> Self {
        let a = dir.abs();
        if a.y >= a.x && a.y >= a.z {
            if dir.y < 0.0 {
                Direction::Down
            } else {
                Direction::Up
            }
        } else if a.x >= a.z {
            if dir.x < 0.0 {
                Direction::West
            } else {
                Direction::East
            }
        } else if dir.z < 0.0 {
            Direction::North
        } else {
            Direction::South
        }
    }

    /// The name of the axis the direction runs along.
    pub fn axis_name(self) -> &'static str {
        match self {
            Direction::Up | Direction::Down => "y",
            Direction::East | Direction::West => "x",
            Direction::North | Direction::South => "z",
        }
    }

    /// The name it's written with in mod files.
    pub fn name(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
            Direction::North => "north",
            Direction::South => "south",
            Direction::East => "east",
            Direction::West => "west",
        }
    }
}

/// A loaded mod's manifest.
#[derive(Debug, Clone)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
    /// The mod folder's asset path, such as `mods/defaria`.
    pub dir: String,
}

/// A tile: grayscale images colored by palettes and stacked, bottom first.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    /// The texture file's id, with the materials for its inputs if it has
    /// any: `defaria:log_side(wood=defaria:birch)`.
    pub id: String,
    /// The texture file's id alone: `defaria:log_side`.
    pub template: String,
    pub layers: Vec<TextureLayer>,
    /// Whether a cut-out block's face shows it, so its mipmaps keep its
    /// share of solid pixels (see `block_texture`).
    pub cutout: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextureLayer {
    /// The image's asset path, relative to `assets/`.
    pub image: String,
    pub palette: Palette,
}

/// Mesh data with named texture slots. Blocks use models now; entities
/// can later.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    /// Each slot and the slot it falls back to.
    pub slots: BTreeMap<String, Option<String>>,
    pub occludes: Vec<Direction>,
    /// What things bump into, in the block's own coordinates.
    pub collision: Vec<BlockBox>,
    pub faces: Vec<ModelFace>,
}

/// A box inside a block, in the block's own coordinates: a full block runs
/// from `(0, 0, 0)` to `(1, 1, 1)`. Always within that, and never empty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockBox {
    pub min: Vec3,
    pub max: Vec3,
}

impl BlockBox {
    pub const FULL: Self = Self {
        min: Vec3::ZERO,
        max: Vec3::ONE,
    };
}

#[derive(Debug, Clone)]
pub struct ModelFace {
    pub slot: String,
    pub cull: Option<Direction>,
    /// Worked out from the first three vertices.
    pub normal: Vec3,
    /// Three or four, counter-clockwise seen from the front.
    pub vertices: Vec<Vertex>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub pos: Vec3,
    /// Within the face's tile, `v` pointing down the image.
    pub uv: Vec2,
}

/// A material: what a texture layer is colored with, and a value that
/// properties can take, such as a log's wood.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub id: String,
    /// Shown in names that use it, such as "Birch Log".
    pub name: String,
    pub tags: Vec<String>,
    pub palette: Palette,
    /// Palettes for particular uses, by role.
    pub palettes: BTreeMap<String, Palette>,
    /// How hard it is to break; see [`State::hardness`].
    pub hardness: f32,
    /// What a whole block of it weighs, in kilograms: its density in
    /// kg/m³, a block being a metre on each side.
    pub weight: f32,
    /// Its other named numbers, such as `conductivity`. Not `hardness` or
    /// `weight`, which [`Material::stat`] reads from their fields.
    pub stats: BTreeMap<String, f32>,
}

/// The stats every material has, read from fields rather than `stats`.
pub const BUILT_IN_STATS: [&str; 2] = ["hardness", "weight"];

/// The stat called `name` of a material, block state or item state: one
/// of the [`BUILT_IN_STATS`] from its field, or one of its `stats`.
fn stat_of(name: &str, hardness: f32, weight: f32, stats: &BTreeMap<String, f32>) -> Option<f32> {
    match name {
        "hardness" => Some(hardness),
        "weight" => Some(weight),
        _ => stats.get(name).copied(),
    }
}

impl Material {
    /// The stat called `name`: `hardness`, `weight` or one of its `stats`.
    pub fn stat(&self, name: &str) -> Option<f32> {
        stat_of(name, self.hardness, self.weight, &self.stats)
    }

    /// The palette for `role`, or the main one.
    pub fn palette_for(&self, role: Option<&str>) -> Palette {
        role.and_then(|role| self.palettes.get(role))
            .copied()
            .unwrap_or(self.palette)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// The value of an optional material property that has no material.
pub const NONE: &str = "none";

/// A block or item property: its values, the default first.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    /// Plain values, or material ids (and maybe [`NONE`]) for a material
    /// property.
    pub values: Vec<String>,
    /// Whether the values are materials.
    pub materials: bool,
    /// Whether the block's item carries it, so each value is a different
    /// item. Only for block properties.
    pub item: bool,
    /// How a placed block picks the value. Only for block properties.
    pub place: Option<Placement>,
}

/// The index of the state with these property values, the rest at their
/// defaults, counting the last property fastest. `None` for a property or
/// value that isn't there.
fn state_offset(properties: &[Property], values: &[(&str, &str)]) -> Option<usize> {
    let mut index = vec![0; properties.len()];
    for &(name, value) in values {
        let p = properties.iter().position(|prop| prop.name == name)?;
        index[p] = properties[p].values.iter().position(|v| v == value)?;
    }
    Some(offset_of(properties, &index))
}

/// The offset of the state with these value indices: the last property
/// counts fastest.
fn offset_of(properties: &[Property], index: &[usize]) -> usize {
    let mut offset = 0;
    for (property, &i) in properties.iter().zip(index) {
        offset = offset * property.values.len() + i;
    }
    offset
}

/// The value indices of the state at `offset`; undoes [`offset_of`].
fn index_of(properties: &[Property], offset: usize) -> Vec<usize> {
    let mut rest = offset;
    let mut index = vec![0; properties.len()];
    for (p, property) in properties.iter().enumerate().rev() {
        index[p] = rest % property.values.len();
        rest /= property.values.len();
    }
    index
}

/// How many states the properties make.
fn state_count(properties: &[Property]) -> usize {
    properties
        .iter()
        .fold(1usize, |count, p| count.saturating_mul(p.values.len()))
}

/// A block and the range of [`BlockState`]s it owns.
#[derive(Debug, Clone)]
pub struct Block {
    pub id: String,
    /// Sorted by name.
    pub properties: Vec<Property>,
    pub first_state: u16,
    pub state_count: u16,
    /// The properties the engine sets from the neighbours, if it has them.
    pub connections: Option<Connections>,
    /// Its voxels' block entity, if they have one.
    pub block_entity: Option<schema::BlockEntityDef>,
    /// The id of the script whose hooks run for it.
    pub script: Option<String>,
}

impl Block {
    /// Whether each voxel of the block has a block entity: if it asks
    /// for one, or it's a machine or a conduit (which keeps what it
    /// carries and its blocked faces there), which get one when baked.
    pub fn has_block_entity(&self) -> bool {
        self.block_entity.is_some()
    }

    /// Whether it's a conduit: it connects by its `conduit`.
    pub fn is_conduit(&self) -> bool {
        self.connections
            .as_ref()
            .is_some_and(|connections| matches!(connections.rule, Rule::Conduit))
    }

    /// Every property at its first value.
    pub fn default_state(&self) -> BlockState {
        BlockState(self.first_state)
    }

    /// The state with the given property values, the rest at their
    /// defaults. `None` for a property or value the block doesn't have.
    pub fn state(&self, values: &[(&str, &str)]) -> Option<BlockState> {
        let offset = state_offset(&self.properties, values)?;
        Some(BlockState(self.first_state + offset as u16))
    }
}

/// What a block state looks like, ready to mesh.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// Index into [`Content::blocks`].
    pub block: usize,
    /// What it's called, with its property values filled in.
    pub name: String,
    /// Property name and value, in the block's property order.
    pub values: Vec<(String, String)>,
    /// Every part's faces, in part order.
    pub faces: Vec<BakedFace>,
    /// The boxes things bump into and the crosshair picks, rotated with the
    /// model: every part's, in part order. Empty for air and anything you
    /// walk through.
    pub collision: Vec<BlockBox>,
    /// Bit per [`Direction`]: sides this state covers completely, which
    /// is the sides any of its parts covers.
    occludes: u8,
    /// The models the state is made of, in order: its own model, then each
    /// of the block's `parts` that matches it. Meshing, light and physics
    /// use the union in `faces`, `collision` and `occludes`; this keeps
    /// which part each came from.
    pub parts: Vec<StatePart>,
    /// The light it gives off: red, green and blue levels, up to
    /// [`MAX_LIGHT`].
    pub light: [u8; 3],
    /// How many levels light loses coming into it, on top of the level
    /// per block it always loses: water's 2 and leaves' 1 dim what's
    /// under them. Up to [`MAX_LIGHT`].
    pub light_filter: u8,
    /// How its faces are drawn, which also decides whether it stops light
    /// and hides the faces next to it.
    pub render: Render,
    /// Set for a fluid: it has no collision, the crosshair passes through
    /// it, and placing a block replaces it. It flows as this says.
    pub fluid: Option<Fluid>,
    /// Whether its faces show from behind too: from inside a glass block
    /// or under water.
    pub double_sided: bool,
    /// How hard it is to break, unitless: dirt is about 0.5 and stone
    /// about 1.5. Air's is 0.
    pub hardness: f32,
    /// What it weighs, in kilograms. A whole block of stone is about 2600.
    pub weight: f32,
    /// The block's `stats`, worked out for this state.
    pub stats: BTreeMap<String, f32>,
    /// Set for a conduit: the network it's part of, and what it carries.
    pub conduit: Option<Conduit>,
    /// Set for a machine: its ports, buffer, tanks and slots.
    pub machine: Option<Box<Machine>>,
}

/// A block state that's part of a network: a wire, pipe or tube.
#[derive(Debug, Clone, PartialEq)]
pub struct Conduit {
    /// Index into [`Content::networks`].
    pub network: usize,
    /// Conduits join only those with the same group (or both none).
    pub group: Option<String>,
    /// What it holds, in its solver's units.
    pub capacity: f32,
    /// What it passes on a second, in its solver's units.
    pub throughput: f32,
}

impl Conduit {
    /// Whether it joins `other`: the same network kind and group.
    pub fn joins(&self, other: &Conduit) -> bool {
        self.network == other.network && self.group == other.group
    }
}

/// A kind of network, from `networks/<name>.json5`: electric, fluid, item,
/// or one a mod adds on one of the engine's solvers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkKind {
    pub id: String,
    pub name: String,
    pub solver: Solver,
}

/// One model in a block state, turned and textured, and where its share
/// of the state's faces and boxes are.
#[derive(Debug, Clone, PartialEq)]
pub struct StatePart {
    /// The model's id.
    pub model: String,
    /// Degrees about X, then Y, then Z.
    pub rotate: [i32; 3],
    /// Its faces, in [`State::faces`].
    pub faces: Range<usize>,
    /// Its boxes, in [`State::collision`].
    pub collision: Range<usize>,
    /// Bit per [`Direction`]: the sides it covers completely.
    pub occludes: u8,
}

/// How much fluid a full cell holds, in the units fluid amounts are counted
/// in. Amounts in the mod files are in blocks, and are turned into these.
pub const FULL_FLUID: u16 = 1000;

/// A fluid's flow and feel, from its block's `fluid` (see `docs/modding.md`),
/// with amounts in [`FULL_FLUID`] units.
#[derive(Debug, Clone, PartialEq)]
pub struct Fluid {
    /// Flow steps a second.
    pub rate: f32,
    /// The most that falls into the cell below in a step, at least 1.
    pub fall: u16,
    /// The thinnest layer that spreads onto dry ground.
    pub film: u16,
    /// The smallest difference that flows between two cells, at least
    /// [`MIN_RIPPLE`](crate::fluid::flow::MIN_RIPPLE).
    pub ripple: u16,
    /// How fast the player moves in it, as a fraction of walking.
    pub move_speed: f32,
    /// The fog with the eyes inside it.
    pub view_color: Color,
    /// How far you can see from inside it, in blocks.
    pub view_distance: f32,
}

impl State {
    /// The stat called `name`: `hardness`, `weight` or one of the
    /// block's `stats`.
    pub fn stat(&self, name: &str) -> Option<f32> {
        stat_of(name, self.hardness, self.weight, &self.stats)
    }

    /// Whether it's a fluid.
    pub fn is_fluid(&self) -> bool {
        self.fluid.is_some()
    }

    /// Whether light can't pass through it, and it shades the corners of
    /// faces next to it: true when it's drawn opaque and covers every side.
    pub fn is_opaque(&self) -> bool {
        self.render == Render::Opaque && self.occludes == ALL_SIDES
    }

    /// Whether light can't pass through its `side`, in or out: a side it
    /// covers, unless it's see-through.
    pub fn blocks_light(&self, side: Direction) -> bool {
        self.render == Render::Opaque && self.occludes(side)
    }

    /// Whether it gives off any light.
    pub fn emits_light(&self) -> bool {
        self.light != [0; 3]
    }

    /// Whether this state covers its whole `side`, hiding the neighbour's
    /// face against it.
    pub fn occludes(&self, side: Direction) -> bool {
        self.occludes & side.bit() != 0
    }
}

/// A model face with the block's rotation and textures applied.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedFace {
    pub vertices: Vec<Vertex>,
    pub normal: Vec3,
    /// The texture's layer in the block texture array.
    pub layer: u32,
    pub cull: Option<Direction>,
}

impl BakedFace {
    /// Whether the face is hidden, given which sides of its block are
    /// `covered` (see [`Content::covers`]). Faces with no `cull` side are
    /// never hidden.
    pub fn hidden_by(&self, covered: impl FnOnce(Direction) -> bool) -> bool {
        self.cull.is_some_and(covered)
    }

    /// The face's triangles, as indices into its vertices: one triangle, or
    /// a quad as two counter-clockwise triangles.
    pub fn indices(&self) -> &'static [u32] {
        if self.vertices.len() == 3 {
            &[0, 1, 2]
        } else {
            &[0, 1, 2, 0, 2, 3]
        }
    }
}

/// An item in one particular state: an index into
/// [`Content::item_states`]. Items carry their property values this way, as
/// blocks do, so a birch log and an iron-headed pickaxe are each one value.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemState(pub u32);

/// An item and the range of [`ItemState`]s it owns.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    /// Sorted by name.
    pub properties: Vec<Property>,
    pub first_state: u32,
    pub state_count: u32,
    /// Every one of it is its own entry in an inventory, never counted
    /// together with others.
    pub unique: bool,
    /// Index into [`Content::categories`]; `None` for an item in none.
    pub category: Option<usize>,
    /// What using it does, besides placing its block.
    pub action: Option<ItemAction>,
    /// The id of the script whose `on_use` runs when it's used.
    pub script: Option<String>,
    /// How much fluid one of it holds, in [`FULL_FLUID`] units, if it's a
    /// fluid container (a bucket; see `fluid_container`).
    pub fluid_capacity: Option<u64>,
}

impl Item {
    /// Every property at its first value.
    pub fn default_state(&self) -> ItemState {
        ItemState(self.first_state)
    }

    /// The state with the given property values, the rest at their
    /// defaults. `None` for a property or value the item doesn't have.
    pub fn state(&self, values: &[(&str, &str)]) -> Option<ItemState> {
        let offset = state_offset(&self.properties, values)?;
        Some(ItemState(self.first_state + offset as u32))
    }
}

/// What an item state is: its name, icon and the block it places.
#[derive(Debug, Clone)]
pub struct ItemStateInfo {
    /// Index into [`Content::items`].
    pub item: usize,
    /// Property name and value, in the item's property order.
    pub values: Vec<(String, String)>,
    pub name: String,
    /// The icon's layer in the block texture array.
    pub texture: Option<u32>,
    /// The block it places.
    pub block: Option<BlockState>,
    /// How hard it is, on the same scale as [`State::hardness`]. A block's
    /// item has the hardness of the state it places.
    pub hardness: f32,
    /// What one of it weighs, in kilograms. A block's item weighs what the
    /// state it places does.
    pub weight: f32,
    /// The item's `stats`, worked out for this state, over those of the
    /// block state it places.
    pub stats: BTreeMap<String, f32>,
}

impl ItemStateInfo {
    /// The stat called `name`: `hardness`, `weight`, or one the item or
    /// the block it places has.
    pub fn stat(&self, name: &str) -> Option<f32> {
        stat_of(name, self.hardness, self.weight, &self.stats)
    }
}

/// A heading items are listed under in the inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub order: i32,
}

/// Everything the mods define, resolved.
#[derive(Resource, Debug, Clone)]
pub struct Content {
    /// In load order.
    pub mods: Vec<ModInfo>,
    pub materials: BTreeMap<String, Material>,
    /// In block texture array order: texture `i` is layer `i`.
    pub textures: Vec<Texture>,
    pub models: BTreeMap<String, Model>,
    /// Air first, then in load order.
    pub blocks: Vec<Block>,
    pub states: Vec<State>,
    pub items: Vec<Item>,
    pub item_states: Vec<ItemStateInfo>,
    /// In the order the inventory lists them.
    pub categories: Vec<Category>,
    /// Network kinds, in load order.
    pub networks: Vec<NetworkKind>,
    /// Every recipe, templated ones once for each combination of their
    /// materials, in load order.
    pub recipes: Vec<Recipe>,
    /// What went wrong while loading, one message per problem.
    pub errors: Vec<String>,
    category_index: HashMap<String, usize>,
    network_index: HashMap<String, usize>,
    texture_index: HashMap<String, u32>,
    block_index: HashMap<String, usize>,
    item_index: HashMap<String, usize>,
    recipe_index: HashMap<String, usize>,
    /// Each recipe type's recipes, as indices into `recipes`.
    recipe_types: HashMap<String, Vec<usize>>,
    /// The item states some recipe of each type takes, sorted.
    recipe_items: HashMap<String, Vec<ItemState>>,
}

impl FromWorld for Content {
    fn from_world(_: &mut World) -> Self {
        let root = FileAssetReader::get_base_path()
            .join("assets")
            .join(MODS_DIR);
        let content = Self::load(&root);
        for error in &content.errors {
            error!("{error}");
        }
        info!(
            "loaded {} mods: {} blocks ({} states), {} items ({} states), {} textures",
            content.mods.len(),
            content.blocks.len() - 1,
            content.states.len() - 1,
            content.items.len(),
            content.item_states.len(),
            content.textures.len(),
        );
        content
    }
}

impl Content {
    /// Loads every mod in `root`, the `assets/mods` folder.
    pub fn load(root: &Path) -> Self {
        let (mods, mut errors) = load::read_mods(root, MODS_DIR);
        let mut content = Self::from_mods(mods);
        errors.append(&mut content.errors);
        content.errors = errors;
        content
    }

    /// Loads mods already in memory.
    pub fn from_mods(mods: Vec<load::ModFiles>) -> Self {
        let mut errors = Vec::new();
        let ordered = load::order(mods, &mut errors);
        let mut content = bake::bake(ordered, &mut errors);
        let (mut problems, warnings) = recipe::check_recipe_machines(&content);
        errors.append(&mut problems);
        for warning in warnings {
            warn!("{warning}");
        }
        content.errors = errors;
        content
    }

    pub fn texture(&self, id: &str) -> Option<u32> {
        self.texture_index.get(id).copied()
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.block_index.get(id).map(|&i| &self.blocks[i])
    }

    /// The block's default state.
    pub fn default_state(&self, id: &str) -> Option<BlockState> {
        self.block(id).map(Block::default_state)
    }

    /// Block `id`'s state with these property values, the rest at their
    /// defaults. `None` for a block, property or value that isn't there.
    pub fn block_state(&self, id: &str, values: &[(&str, &str)]) -> Option<BlockState> {
        self.block(id)?.state(values)
    }

    /// What `state` looks like. Air for a state that doesn't exist.
    pub fn state(&self, state: BlockState) -> &State {
        self.states.get(state.0 as usize).unwrap_or(&self.states[0])
    }

    /// Whether `neighbour`, next to `block` on its `side`, hides that
    /// side, so the block's faces culled against it aren't drawn. It must
    /// cover the side facing the block, and then:
    ///
    /// - an opaque block hides it, unless it's a fluid and the face is a
    ///   solid opaque block's: you can be inside a fluid, so the floor and
    ///   walls of a lava pool stay drawn. Any other face against lava
    ///   (water, glass, leaves) is hidden, so only one face is drawn there,
    ///   and between two different opaque fluids the one with the higher
    ///   block number keeps its face;
    /// - a translucent one hides only the same block's translucent faces
    ///   (water against water, glass against glass);
    /// - a cut-out one hides nothing.
    pub fn hides(&self, block: BlockState, neighbour: BlockState, side: Direction) -> bool {
        let (own, next) = (self.state(block), self.state(neighbour));
        next.occludes(side.opposite())
            && match next.render {
                Render::Opaque => {
                    !next.is_fluid()
                        || next.block == own.block
                        || own.render != Render::Opaque
                        || (own.is_fluid() && own.block < next.block)
                }
                Render::Cutout => false,
                Render::Translucent => next.block == own.block && own.render == Render::Translucent,
            }
    }

    /// The network kind with this id, as an index into
    /// [`Content::networks`].
    pub fn network(&self, id: &str) -> Option<usize> {
        self.network_index.get(id).copied()
    }

    /// What `state` carries, if it's a conduit.
    pub fn conduit(&self, state: BlockState) -> Option<&Conduit> {
        self.state(state).conduit.as_ref()
    }

    /// The block `state` is a state of.
    pub fn block_of(&self, state: BlockState) -> &Block {
        &self.blocks[self.state(state).block]
    }

    /// Forgets a block's id, as if it hadn't loaded. Returns whether it
    /// was there.
    #[cfg(test)]
    pub fn forget_block(&mut self, id: &str) -> bool {
        self.block_index.remove(id).is_some()
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.item_index.get(id).map(|&i| &self.items[i])
    }

    /// The item's default state.
    pub fn default_item(&self, id: &str) -> Option<ItemState> {
        self.item(id).map(Item::default_state)
    }

    /// What item `id`'s default state is.
    #[cfg(test)]
    pub fn default_item_info(&self, id: &str) -> Option<&ItemStateInfo> {
        self.item_state(self.default_item(id)?)
    }

    /// What an [`ItemState`] is. `None` for one past the end, which a stale
    /// state from other content could be.
    pub fn item_state(&self, state: ItemState) -> Option<&ItemStateInfo> {
        self.item_states.get(state.0 as usize)
    }

    /// The item an [`ItemState`] is a state of.
    pub fn item_of(&self, state: ItemState) -> Option<&Item> {
        self.item_state(state).map(|info| &self.items[info.item])
    }

    /// The category an item state is listed under, if any.
    pub fn item_category(&self, state: ItemState) -> Option<&Category> {
        self.categories.get(self.item_of(state)?.category?)
    }

    /// The display name of an [`ItemState`].
    pub fn item_name(&self, state: ItemState) -> Option<&str> {
        self.item_state(state).map(|info| info.name.as_str())
    }

    /// The name `item` is saved under: its id, with the values of its
    /// properties if it has any (`defaria:log[wood=defaria:birch]`).
    /// `None` for a state that doesn't exist.
    pub fn saved_item_name(&self, item: ItemState) -> Option<String> {
        let state = self.item_state(item)?;
        Some(StateName::new(&self.items[state.item].id, state.values.iter().cloned()).to_string())
    }

    /// The item state saved as `name` (see [`Content::saved_item_name`]).
    /// An item whose properties changed, or whose name doesn't parse, comes
    /// back as its default state; one that's gone doesn't come back.
    pub fn saved_item(&self, name: &str) -> Option<ItemState> {
        let Ok(parsed) = StateName::parse(name) else {
            return self.default_item(StateName::id_of(name));
        };
        let item = self.item(&parsed.id)?;
        Some(
            item.state(&parsed.pairs())
                .unwrap_or_else(|| item.default_state()),
        )
    }

    /// Every image some texture uses, as asset paths, each once.
    pub fn image_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .textures
            .iter()
            .flat_map(|texture| texture.layers.iter().map(|layer| layer.image.clone()))
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// Checks a mod id: lowercase ASCII letters, digits and `_`.
pub fn check_namespace(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`{id}` must be lowercase letters, digits and `_` only"
        ))
    }
}

/// Checks the name part of an id: like a mod id, plus `-`, `.` and `/`
/// between path segments.
fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
                })
        });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`{name}` must be `/`-separated lowercase letters, digits, `_`, `-` and `.`"
        ))
    }
}

/// The full id a reference in mod `namespace` means: `name` is
/// `namespace:name`, and `other:name` stays as it is.
pub fn qualify(reference: &str, namespace: &str) -> Result<String, String> {
    let (namespace, name) = reference.split_once(':').unwrap_or((namespace, reference));
    check_namespace(namespace)?;
    check_name(name)?;
    Ok(format!("{namespace}:{name}"))
}

/// The base game's content, loaded from disk once and shared by every
/// test.
#[cfg(test)]
pub fn base() -> &'static Content {
    static BASE: std::sync::OnceLock<Content> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let content = Content::load(Path::new("assets/mods"));
        assert!(content.errors.is_empty(), "{:#?}", content.errors);
        content
    })
}

/// The array layer of base game texture `defaria:<name>`: the first tile
/// baked from it, which for a texture with inputs is the one the first
/// block using it has in its default state.
#[cfg(test)]
pub fn base_texture(name: &str) -> u32 {
    let id = format!("defaria:{name}");
    base()
        .textures
        .iter()
        .position(|t| t.template == id)
        .unwrap_or_else(|| panic!("no texture {id}")) as u32
}

/// The default state of base game block `defaria:<name>`.
#[cfg(test)]
pub fn block(name: &str) -> BlockState {
    block_with(name, &[])
}

/// The state of base game block `defaria:<name>` with these property
/// values, the rest at their defaults.
#[cfg(test)]
pub fn block_with(name: &str, values: &[(&str, &str)]) -> BlockState {
    base()
        .block_state(&format!("defaria:{name}"), values)
        .unwrap_or_else(|| panic!("no block defaria:{name} with {values:?}"))
}

/// Base game dirt with grass on it, the terrain's top.
#[cfg(test)]
pub fn grass() -> BlockState {
    block_with("soil", &[("turf", "defaria:grass")])
}

/// The base game's item for plain dirt: soil, no turf.
#[cfg(test)]
pub fn dirt_item() -> ItemState {
    base()
        .item("defaria:soil")
        .and_then(|item| item.state(&[("soil", "defaria:dirt")]))
        .expect("no dirt item")
}

/// Base game sand.
#[cfg(test)]
pub fn sand() -> BlockState {
    block_with("soil", &[("soil", "defaria:sand")])
}

/// A model with one face, on top of the block, in slot `side`, which
/// falls back to `all`. It covers only its top.
#[cfg(test)]
pub const QUAD: &str = r#"{
    slots: { all: null, side: "all" },
    occludes: ["up"],
    faces: [{
        texture: "side", cull: "up",
        vertices: [
            { pos: [0, 1, 1], uv: [0, 1] },
            { pos: [1, 1, 1], uv: [1, 1] },
            { pos: [1, 1, 0], uv: [1, 0] },
            { pos: [0, 1, 0], uv: [0, 0] },
        ],
    }],
}"#;

/// A model with a top face in slot `top` and a bottom face in slot
/// `bottom`, neither culled. It covers nothing.
#[cfg(test)]
const PAIR: &str = r#"{
    slots: { top: null, bottom: null },
    faces: [
        {
            texture: "top",
            vertices: [
                { pos: [0, 1, 1], uv: [0, 1] },
                { pos: [1, 1, 1], uv: [1, 1] },
                { pos: [1, 1, 0], uv: [1, 0] },
                { pos: [0, 1, 0], uv: [0, 0] },
            ],
        },
        {
            texture: "bottom",
            vertices: [
                { pos: [0, 0, 0], uv: [0, 1] },
                { pos: [1, 0, 0], uv: [1, 1] },
                { pos: [1, 0, 1], uv: [1, 0] },
                { pos: [0, 0, 1], uv: [0, 0] },
            ],
        },
    ],
}"#;

/// Mod `test`, in memory: image `a`, material `grey`, textures `a`, `b` and
/// `c`, models `quad` ([`QUAD`]) and `pair`, plus `files`.
#[cfg(test)]
pub fn test_mod(files: &[(&str, &str)]) -> Content {
    let mut all = vec![
        ("images/a.png", ""),
        (
            "materials/grey.json5",
            r##"{ palette: ["#000000", "#ffffff"], hardness: 1, weight: 1000 }"##,
        ),
        (
            "textures/a.json5",
            r#"{ layers: [{ image: "a", material: "grey" }] }"#,
        ),
        (
            "textures/b.json5",
            r##"{ layers: [{ image: "a", palette: ["#ff0000", "#00ff00", "#0000ff"] }] }"##,
        ),
        (
            "textures/c.json5",
            r##"{ layers: [{ image: "a", palette: ["#000000", "#00ff00"] }] }"##,
        ),
        ("models/quad.json5", QUAD),
        ("models/pair.json5", PAIR),
    ];
    all.extend_from_slice(files);
    Content::from_mods(vec![load::ModFiles::new(
        "mods/test",
        r#"{ id: "test", name: "Test" }"#,
        &all,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_item_names_round_trip_and_bad_ones_fall_back() {
        let content = base();
        for i in 0..content.item_states.len() {
            let item = ItemState(i as u32);
            let name = content.saved_item_name(item).unwrap();
            assert_eq!(content.saved_item(&name), Some(item), "{name}");
        }
        let pickaxe = content.default_item("defaria:pickaxe").unwrap();
        for bad in [
            "defaria:pickaxe[head=nope]",
            "defaria:pickaxe[head=defaria:iron",
            "defaria:pickaxe[head]",
        ] {
            assert_eq!(content.saved_item(bad), Some(pickaxe), "{bad}");
        }
        assert_eq!(content.saved_item("gone:thing[a=b]"), None);
    }

    #[test]
    fn along_picks_the_largest_component() {
        assert_eq!(Direction::along(Vec3::new(0.1, -0.9, 0.3)), Direction::Down);
        assert_eq!(Direction::along(Vec3::new(0.8, 0.5, -0.3)), Direction::East);
        assert_eq!(
            Direction::along(Vec3::new(-0.2, 0.1, -0.7)),
            Direction::North
        );
        assert_eq!(Direction::along(Vec3::new(-0.7, 0.0, 0.2)), Direction::West);
        assert_eq!(Direction::along(Vec3::new(0.0, 0.0, 1.0)), Direction::South);
        assert_eq!(Direction::along(Vec3::new(0.0, 0.5, 0.0)), Direction::Up);
        // Ties go to Y, then X.
        assert_eq!(Direction::along(Vec3::new(0.7, -0.7, 0.0)), Direction::Down);
        assert_eq!(Direction::along(Vec3::new(0.0, 0.7, -0.7)), Direction::Up);
        assert_eq!(Direction::along(Vec3::new(-0.7, 0.0, 0.7)), Direction::West);
    }

    #[test]
    fn references_are_qualified_with_their_mod() {
        assert_eq!(qualify("stone", "defaria").unwrap(), "defaria:stone");
        assert_eq!(qualify("ores/iron", "x").unwrap(), "x:ores/iron");
        assert_eq!(qualify("other:stone", "defaria").unwrap(), "other:stone");
        for bad in ["Stone", "a b", "", "x:", ":y", "a//b", "../up", "a:b:c"] {
            assert!(qualify(bad, "defaria").is_err(), "{bad}");
        }
    }

    #[test]
    fn directions_point_out_of_their_side() {
        assert_eq!(Direction::Up.normal(), IVec3::Y);
        assert_eq!(Direction::South.normal(), IVec3::Z);
        assert_eq!(Direction::East.normal(), IVec3::X);
        for d in Direction::ALL {
            assert_eq!(d.opposite().normal(), -d.normal());
            assert_eq!(Direction::from_normal(d.normal()), Some(d));
        }
        assert_eq!(Direction::from_normal(IVec3::ONE), None);
    }

    /// The base game loads with no errors and has what the terrain and the
    /// hotbar expect.
    #[test]
    fn base_game_loads_cleanly() {
        let content = base();
        assert_eq!(content.mods[0].id, "defaria");
        assert_eq!(content.blocks[0].id, AIR);
        assert!(BlockState::AIR.is_air());
        assert!(content.state(BlockState::AIR).faces.is_empty());
        for name in ["stone", "soil", "log", "planks"] {
            let id = format!("defaria:{name}");
            let state = content.default_state(&id).unwrap();
            assert_eq!(content.state(state).faces.len(), 6, "{id}");
            // Every block gets an item that places it.
            assert_eq!(
                content.default_item_info(&id).unwrap().block,
                Some(state),
                "{id}"
            );
        }
        let pickaxe = content.default_item_info("defaria:pickaxe").unwrap();
        assert_eq!(pickaxe.block, None);
        assert_eq!(pickaxe.name, "Iron Pickaxe");
        assert!(content.item("defaria:pickaxe").unwrap().unique);
        assert!(!content.item("defaria:stone").unwrap().unique);
        assert_eq!(
            pickaxe.texture,
            content.texture("defaria:pickaxe(handle=defaria:oak,head=defaria:iron)")
        );
    }

    /// Pickaxes, axes, shovels and knives take a rock head as well as a
    /// metal one, drawn as knapped stone; swords and wrenches stay metal.
    /// Sticks come in every wood. Names, hardness and weight come from the
    /// materials.
    #[test]
    fn tools_take_rock_heads_and_sticks_any_wood() {
        let content = base();
        let info = |item: &str, values: &[(&str, &str)]| {
            let item = content.item(item).unwrap();
            content.item_state(item.state(values).unwrap()).unwrap()
        };
        for tool in ["pickaxe", "axe", "shovel", "knife"] {
            let id = format!("defaria:{tool}");
            let granite = info(
                &id,
                &[("head", "defaria:granite"), ("handle", "defaria:birch")],
            );
            assert_eq!(
                granite.texture,
                content.texture(&format!(
                    "defaria:{tool}_rock(handle=defaria:birch,head=defaria:granite)"
                )),
                "{tool}"
            );
            assert!(granite.name.starts_with("Granite "), "{}", granite.name);
            let iron = info(&id, &[("head", "defaria:iron")]);
            assert_eq!(
                iron.texture,
                content.texture(&format!(
                    "defaria:{tool}(handle=defaria:oak,head=defaria:iron)"
                )),
                "{tool}"
            );
            // Metal or rock heads, by wood handles.
            let item = content.item(&id).unwrap();
            let count = |tag: &str| {
                content
                    .materials
                    .values()
                    .filter(|m| m.has_tag(tag))
                    .count()
            };
            assert_eq!(
                item.state_count as usize,
                (count("metal") + count("rock")) * count("wood"),
                "{tool}"
            );
        }
        let pickaxe = info(
            "defaria:pickaxe",
            &[("head", "defaria:granite"), ("handle", "defaria:oak")],
        );
        assert_eq!(pickaxe.name, "Granite Pickaxe");
        // Stone is softer and lighter than iron, so a stone pickaxe is
        // softer and lighter than an iron one.
        let stone = info("defaria:pickaxe", &[("head", "defaria:stone")]);
        let iron = info("defaria:pickaxe", &[("head", "defaria:iron")]);
        assert_eq!(stone.hardness, (3.0 * 1.5 + 1.0) / 4.0);
        assert!(stone.weight < iron.weight);
        assert!(
            content
                .item("defaria:sword")
                .unwrap()
                .state(&[("head", "defaria:stone")])
                .is_none()
        );

        let stick = info("defaria:stick", &[("wood", "defaria:spruce")]);
        assert_eq!(stick.name, "Spruce Stick");
        assert_eq!(
            stick.texture,
            content.texture("defaria:stick(wood=defaria:spruce)")
        );
        assert!(!content.item("defaria:stick").unwrap().unique);
    }

    #[test]
    fn states_count_properties_last_fastest() {
        let content = base();
        let log = content.block("defaria:log").unwrap();
        // Three axes by three woods; `wood` comes after `axis`, so it
        // counts fastest.
        assert_eq!(log.state_count, 9);
        assert_eq!(log.state(&[]), Some(log.default_state()));
        let x = log.state(&[("axis", "x")]).unwrap();
        assert_eq!(x.0, log.first_state + 3);
        let birch = log
            .state(&[("axis", "x"), ("wood", "defaria:birch")])
            .unwrap();
        assert_eq!(birch.0, x.0 + 1);
        assert_eq!(content.block_of(x).id, "defaria:log");
        assert_eq!(
            content.state(x).values,
            [
                ("axis".into(), "x".into()),
                ("wood".into(), "defaria:oak".into())
            ]
        );
        assert_eq!(content.state(birch).name, "Birch Log");
        assert_eq!(log.state(&[("axis", "w")]), None);
        assert_eq!(log.state(&[("colour", "x")]), None);
    }

    #[test]
    fn unknown_states_read_as_air() {
        let content = base();
        let state = content.state(BlockState(u16::MAX));
        assert!(state.faces.is_empty());
        assert_eq!(content.block_of(BlockState(u16::MAX)).id, AIR);
    }

    /// Soil is every soil material with or without every turf: bare, it's
    /// its own texture all round; with a turf, the turf's on top, hanging
    /// over the soil's sides, and the bottom is bare soil.
    #[test]
    fn soil_is_any_soil_with_or_without_turf() {
        let content = base();
        let soil = content.block("defaria:soil").unwrap();
        // The texture on a face, and its images' names.
        let texture = |state: BlockState, normal: Vec3| {
            let face = content
                .state(state)
                .faces
                .iter()
                .find(|f| f.normal == normal);
            let texture = &content.textures[face.unwrap().layer as usize];
            let images: Vec<&str> = texture
                .layers
                .iter()
                .map(|layer| layer.image.rsplit('/').next().unwrap())
                .collect();
            (texture.id.clone(), images.join(" "))
        };
        for (material, name) in [
            ("dirt", "Dirt"),
            ("sand", "Sand"),
            ("gravel", "Gravel"),
            ("loam", "Loam"),
        ] {
            let id = format!("defaria:{material}");
            let image = format!("{material}.png");
            let bare = soil.state(&[("soil", &id)]).unwrap();
            assert_eq!(content.state(bare).name, name);
            let soil_texture = (format!("defaria:soil(soil={id})"), image.clone());
            for normal in [Vec3::Y, Vec3::NEG_Y, Vec3::X] {
                assert_eq!(texture(bare, normal), soil_texture);
            }
            let turfed = soil
                .state(&[("soil", &id), ("turf", "defaria:grass")])
                .unwrap();
            assert_eq!(content.state(turfed).name, format!("{name} with Grass"));
            assert_eq!(
                texture(turfed, Vec3::Y),
                (
                    "defaria:turf(turf=defaria:grass)".into(),
                    "grass.png".into()
                )
            );
            assert_eq!(
                texture(turfed, Vec3::X),
                (
                    format!("defaria:soil_turf(soil={id},turf=defaria:grass)"),
                    format!("{image} grass_side.png")
                )
            );
            assert_eq!(texture(turfed, Vec3::NEG_Y), soil_texture);
            // Each one is its own item.
            let item = content.item("defaria:soil").unwrap();
            for (values, block) in [
                (vec![("soil", id.as_str())], bare),
                (
                    vec![("soil", id.as_str()), ("turf", "defaria:grass")],
                    turfed,
                ),
            ] {
                let state = item.state(&values).unwrap();
                assert_eq!(content.item_state(state).unwrap().block, Some(block));
            }
        }
        assert_eq!(soil.default_state(), block("soil"));
        assert_eq!(content.state(block("soil")).name, "Dirt");
    }

    /// Shale is a rock, so it reaches every block that takes any rock:
    /// bare, with ore in it, and as a slab, on stone's image with its own
    /// colors and numbers.
    #[test]
    fn shale_is_a_rock() {
        let content = base();
        let shale = &content.materials["defaria:shale"];
        assert_eq!(shale.name, "Shale");
        assert_eq!(shale.tags, ["rock"]);
        let stone = &content.materials["defaria:stone"];
        assert_ne!(shale.palette, stone.palette);
        let bare = block_with("stone", &[("rock", "defaria:shale")]);
        assert_eq!(content.state(bare).name, "Shale");
        assert_eq!(content.state(bare).hardness, shale.hardness);
        assert_eq!(content.state(bare).weight, shale.weight);
        let ore = block_with(
            "stone",
            &[("rock", "defaria:shale"), ("ore", "defaria:coal")],
        );
        assert_eq!(content.state(ore).name, "Shale Coal Ore");
        let slab = block_with("stone_slab", &[("rock", "defaria:shale")]);
        assert_eq!(content.state(slab).name, "Shale Slab");
        assert_eq!(content.state(slab).weight, shale.weight / 2.0);
        let face = &content.state(bare).faces[0];
        let texture = &content.textures[face.layer as usize];
        assert_eq!(texture.id, "defaria:stone(rock=defaria:shale)");
        assert!(texture.layers[0].image.ends_with("/stone.png"));
        assert_eq!(texture.layers[0].palette, shale.palette);
    }

    /// Thatch is a building block, whole and as a slab, drawn with the
    /// thatch image in the thatch material's colors.
    #[test]
    fn thatch_is_a_building_block() {
        let content = base();
        let thatch = &content.materials["defaria:thatch"];
        for (id, name, weight) in [
            ("thatch", "Thatch", thatch.weight),
            ("thatch_slab", "Thatch Slab", thatch.weight / 2.0),
        ] {
            let state = content.state(block(id));
            assert_eq!(state.name, name);
            assert_eq!(state.hardness, thatch.hardness, "{id}");
            assert_eq!(state.weight, weight, "{id}");
            let texture = &content.textures[state.faces[0].layer as usize];
            assert_eq!(texture.id, "defaria:thatch");
            assert!(texture.layers[0].image.ends_with("/thatch.png"));
            assert_eq!(texture.layers[0].palette, thatch.palette);
            let item = content.item(&format!("defaria:{id}")).unwrap();
            let category = content.item_category(item.default_state()).unwrap();
            assert_eq!(category.id, "defaria:building", "{id}");
        }
    }

    /// Cobblestone, bricks and smooth stone, and a slab of each, come in
    /// every rock: named after it, listed under building, as hard as the
    /// rock and weighing what it does (half for a slab), with the
    /// building's own layer colored by the rock all round.
    #[test]
    fn rock_building_blocks_come_in_every_rock() {
        let content = base();
        let rocks: Vec<&Material> = content
            .materials
            .values()
            .filter(|m| m.has_tag("rock"))
            .collect();
        assert!(rocks.len() >= 3, "stone, granite and slate at least");
        // Block, its name, its texture and layer image, and its weight
        // as a share of a whole block of the rock.
        let blocks = [
            ("cobblestone", "Cobbled {}", "cobblestone", 1.0),
            ("cobblestone_slab", "Cobbled {} Slab", "cobblestone", 0.5),
            ("stone_bricks", "{} Bricks", "stone_bricks", 1.0),
            ("stone_brick_slab", "{} Brick Slab", "stone_bricks", 0.5),
            ("smooth_stone", "Smooth {}", "smooth_stone", 1.0),
            ("smooth_stone_slab", "Smooth {} Slab", "smooth_stone", 0.5),
        ];
        for (name, label, texture_name, share) in blocks {
            let id = format!("defaria:{name}");
            let block = content.block(&id).unwrap();
            assert_eq!(block.state_count as usize, rocks.len(), "{id}");
            for rock in &rocks {
                let state = block.state(&[("rock", &rock.id)]).unwrap();
                let info = content.state(state);
                assert_eq!(info.name, label.replace("{}", &rock.name), "{id}");
                assert_eq!(info.hardness, rock.hardness, "{id} of {}", rock.id);
                assert_eq!(info.weight, rock.weight * share, "{id} of {}", rock.id);
                assert_eq!(info.faces.len(), 6, "{id}");
                let tile = format!("defaria:{texture_name}(rock={})", rock.id);
                for face in &info.faces {
                    let texture = &content.textures[face.layer as usize];
                    assert_eq!(texture.id, tile);
                    let images: Vec<&str> = texture
                        .layers
                        .iter()
                        .map(|layer| layer.image.rsplit('/').next().unwrap())
                        .collect();
                    assert_eq!(images, [format!("{texture_name}.png")]);
                }
                let item = content.item(&id).unwrap().state(&[("rock", &rock.id)]);
                let item = item.unwrap();
                assert_eq!(content.item_state(item).unwrap().block, Some(state));
                assert_eq!(content.item_category(item).unwrap().id, "defaria:building");
            }
        }
    }

    /// Every image the base game's textures use is in the image set, once.
    #[test]
    fn image_paths_are_listed_once() {
        let paths = base().image_paths();
        assert!(paths.contains(&"mods/defaria/images/dirt.png".to_string()));
        // Dirt is under both soil and soil_turf but loads once.
        assert_eq!(paths.iter().filter(|p| p.ends_with("/dirt.png")).count(), 1);
    }
}
