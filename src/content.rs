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
pub mod load;
pub mod schema;

use std::{collections::BTreeMap, path::Path};

use bevy::{asset::io::file::FileAssetReader, platform::collections::HashMap, prelude::*};

use crate::block_texture::Palette;
pub use schema::Direction;

/// The mods folder, relative to `assets/`.
pub const MODS_DIR: &str = "mods";
/// The id of the block that's nothing at all. Not defined by any mod.
pub const AIR: &str = "air";

pub struct ContentPlugin;

impl Plugin for ContentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Content>();
    }
}

/// A block in one particular state: an index into [`Content::states`].
/// State 0 is always air.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

    fn bit(self) -> u8 {
        1 << self as u8
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
    pub id: String,
    pub layers: Vec<TextureLayer>,
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
    pub faces: Vec<ModelFace>,
}

#[derive(Debug, Clone)]
pub struct ModelFace {
    pub slot: String,
    pub cull: Option<Direction>,
    /// Three or four, counter-clockwise seen from the front.
    pub vertices: Vec<Vertex>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub pos: Vec3,
    /// Within the face's tile, `v` pointing down the image.
    pub uv: Vec2,
}

/// A block and the range of [`BlockState`]s it owns.
#[derive(Debug, Clone)]
pub struct Block {
    pub id: String,
    pub name: String,
    /// Each property with its values, the default first, sorted by name.
    pub properties: Vec<(String, Vec<String>)>,
    pub first_state: u16,
    pub state_count: u16,
}

impl Block {
    /// Every property at its first value.
    pub fn default_state(&self) -> BlockState {
        BlockState(self.first_state)
    }

    /// The state with the given property values, the rest at their
    /// defaults. `None` for a property or value the block doesn't have.
    pub fn state(&self, values: &[(&str, &str)]) -> Option<BlockState> {
        let mut index = vec![0; self.properties.len()];
        for &(name, value) in values {
            let p = self.properties.iter().position(|(n, _)| n == name)?;
            index[p] = self.properties[p].1.iter().position(|v| v == value)?;
        }
        Some(BlockState(self.first_state + self.state_offset(&index)))
    }

    /// The offset of the state with these value indices: the last property
    /// counts fastest.
    fn state_offset(&self, index: &[usize]) -> u16 {
        let mut offset = 0;
        for ((_, values), &i) in self.properties.iter().zip(index) {
            offset = offset * values.len() + i;
        }
        offset as u16
    }
}

/// What a block state looks like, ready to mesh.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// Index into [`Content::blocks`].
    pub block: usize,
    /// Property name and value, in the block's property order.
    pub values: Vec<(String, String)>,
    pub faces: Vec<BakedFace>,
    /// Bit per [`Direction`]: sides this state covers completely.
    occludes: u8,
}

impl State {
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

#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub name: String,
    /// The icon's layer in the block texture array.
    pub texture: Option<u32>,
    /// The block it places.
    pub block: Option<BlockState>,
    pub max_stack: u32,
}

/// Everything the mods define, resolved.
#[derive(Resource, Debug, Clone)]
pub struct Content {
    /// In load order.
    pub mods: Vec<ModInfo>,
    pub materials: BTreeMap<String, Palette>,
    /// In block texture array order: texture `i` is layer `i`.
    pub textures: Vec<Texture>,
    pub models: BTreeMap<String, Model>,
    /// Air first, then in load order.
    pub blocks: Vec<Block>,
    pub states: Vec<State>,
    pub items: Vec<Item>,
    /// What went wrong while loading, one message per problem.
    pub errors: Vec<String>,
    texture_index: HashMap<String, u32>,
    block_index: HashMap<String, usize>,
    item_index: HashMap<String, usize>,
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
            "loaded {} mods: {} blocks ({} states), {} items, {} textures",
            content.mods.len(),
            content.blocks.len() - 1,
            content.states.len() - 1,
            content.items.len(),
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

    /// What `state` looks like. Air for a state that doesn't exist.
    pub fn state(&self, state: BlockState) -> &State {
        self.states.get(state.0 as usize).unwrap_or(&self.states[0])
    }

    /// The block `state` is a state of.
    pub fn block_of(&self, state: BlockState) -> &Block {
        &self.blocks[self.state(state).block]
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.item_index.get(id).map(|&i| &self.items[i])
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

/// The default state of base game block `defaria:<name>`.
#[cfg(test)]
pub fn block(name: &str) -> BlockState {
    base()
        .default_state(&format!("defaria:{name}"))
        .unwrap_or_else(|| panic!("no block defaria:{name}"))
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
            r##"{ palette: ["#000000", "#ffffff"] }"##,
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
        for name in ["stone", "dirt", "grass", "sand", "gravel", "log", "planks"] {
            let id = format!("defaria:{name}");
            let state = content.default_state(&id).unwrap();
            assert_eq!(content.state(state).faces.len(), 6, "{id}");
            // Every block gets an item that places it.
            assert_eq!(content.item(&id).unwrap().block, Some(state), "{id}");
        }
        let pickaxe = content.item("defaria:pickaxe").unwrap();
        assert_eq!(pickaxe.block, None);
        assert_eq!(pickaxe.max_stack, 1);
        assert_eq!(pickaxe.texture, content.texture("defaria:pickaxe"));
    }

    #[test]
    fn states_count_properties_last_fastest() {
        let content = base();
        let log = content.block("defaria:log").unwrap();
        assert_eq!(log.state_count, 3);
        assert_eq!(log.state(&[]), Some(log.default_state()));
        let x = log.state(&[("axis", "x")]).unwrap();
        assert_eq!(x.0, log.first_state + 1);
        assert_eq!(content.block_of(x).id, "defaria:log");
        assert_eq!(content.state(x).values, [("axis".into(), "x".into())]);
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

    /// Every image the base game's textures use is in the image set, once.
    #[test]
    fn image_paths_are_listed_once() {
        let paths = base().image_paths();
        assert!(paths.contains(&"mods/defaria/images/dirt.png".to_string()));
        // Dirt is under both dirt and grass_side but loads once.
        assert_eq!(paths.iter().filter(|p| p.ends_with("/dirt.png")).count(), 1);
    }
}
