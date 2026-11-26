//! Heightmap terrain from Perlin noise, in chunks.
//!
//! Each column's height comes from noise mapped through splines, with
//! plains, mountains, ridges and valleys (see [`shape`]). Below the surface
//! is stone, then a few layers of dirt, then one layer of dirt with grass on it, except
//! high up and on cliffs, which are bare stone. Here and there a tuft of
//! grass grows on the grass, and trees grow on it, in forests and alone
//! (see [`trees`]). Hollows below [`SEA_LEVEL`] fill with water,
//! and the ground at and under the water's edge is sand. It goes on
//! forever in every direction, up and down too: stone all the way down,
//! air all the way up. `streaming` runs it, generating each chunk the first
//! time it loads ([`Terrain::chunk`]).

use bevy::prelude::*;

use crate::{
    content::{BlockState, Content, StateName},
    perlin::splitmix64,
    world::{CHUNK_SIZE, Chunk, ChunkCoord},
};

mod shape;
mod spline;
pub mod trees;

use shape::{NEIGHBOURS, TerrainShape};
use trees::{Forests, Part, Tree, Wood};

/// Layers of dirt under the grass.
const DIRT_DEPTH: i32 = 3;
/// Air below this height is water: the water's surface is the top of the
/// voxels at `SEA_LEVEL - 1`.
const SEA_LEVEL: i32 = 12;
/// How many grass columns in 1000 grow a tuft of grass.
const TUFTS_PER_THOUSAND: u64 = 80;

/// How the terrain is shaped. Read once at startup. A saved world keeps
/// its own seed (see `save`).
#[derive(Resource, Reflect, Debug, Clone)]
#[reflect(Resource)]
pub struct TerrainSettings {
    pub seed: u64,
    /// Horizontal size of the largest rolling hills, in voxels.
    pub scale: f32,
    /// Noise layers in the hills; each adds detail at half the size of the
    /// last.
    pub octaves: u32,
    /// Surface height where all the noise is 0.
    pub base_height: f32,
    /// How far the hills move the surface up or down at the noise's
    /// extremes.
    pub amplitude: f32,
    /// Horizontal size of the continentalness noise: lowlands to plateaus.
    pub continent_scale: f32,
    /// Horizontal size of the erosion noise: smooth land to rugged.
    pub erosion_scale: f32,
    /// Horizontal size of the peaks-and-valleys noise.
    pub peak_scale: f32,
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            seed: 0x0DEF_A41A,
            scale: 80.0,
            octaves: 3,
            base_height: 22.0,
            amplitude: 36.0,
            continent_scale: 900.0,
            erosion_scale: 500.0,
            peak_scale: 300.0,
        }
    }
}

/// The blocks the terrain is made of.
#[derive(Debug, Clone, Copy)]
pub struct TerrainBlocks {
    /// Dirt with grass on it.
    pub grass: BlockState,
    pub dirt: BlockState,
    pub stone: BlockState,
    pub sand: BlockState,
    pub water: BlockState,
    pub tuft: BlockState,
    /// Upright logs of each wood, in [`Wood::ALL`] order.
    pub logs: [BlockState; 3],
    /// Leaves of each wood, in [`Wood::ALL`] order.
    pub leaves: [BlockState; 3],
}

impl TerrainBlocks {
    /// Each block state the ground is made of: its block and its property
    /// values, in field order. Soil names every value, so a mod whose soil
    /// sorts before dirt can't change the ground.
    const GROUND: [(&str, &[(&str, &str)]); 6] = [
        (
            "defaria:soil",
            &[("soil", "defaria:dirt"), ("turf", "defaria:grass")],
        ),
        (
            "defaria:soil",
            &[("soil", "defaria:dirt"), ("turf", "none")],
        ),
        ("defaria:stone", &[]),
        (
            "defaria:soil",
            &[("soil", "defaria:sand"), ("turf", "none")],
        ),
        ("defaria:water", &[]),
        ("defaria:tuft", &[]),
    ];

    /// The upright log of a wood: its block and property values.
    fn log(wood: Wood) -> (&'static str, Vec<(&'static str, &'static str)>) {
        (
            "defaria:log",
            vec![("wood", wood.material()), ("axis", "y")],
        )
    }

    /// The leaves of a wood: their block and property values.
    fn leaves(wood: Wood) -> (&'static str, Vec<(&'static str, &'static str)>) {
        ("defaria:leaves", vec![("wood", wood.material())])
    }

    /// Every block state the terrain is made of: the ground's, then each
    /// wood's log and leaves.
    fn wanted() -> impl Iterator<Item = (&'static str, Vec<(&'static str, &'static str)>)> {
        let ground = Self::GROUND
            .iter()
            .map(|(id, values)| (*id, values.to_vec()));
        ground.chain(
            Wood::ALL
                .into_iter()
                .flat_map(|wood| [Self::log(wood), Self::leaves(wood)]),
        )
    }

    /// The states the terrain needs that didn't load: each block's id with
    /// the values it sets, in brackets
    /// (`defaria:soil[soil=defaria:sand,turf=none]`).
    pub fn missing(content: &Content) -> Vec<String> {
        Self::wanted()
            .filter(|(id, values)| content.block_state(id, values).is_none())
            .map(|(id, values)| StateName::new(id, values).to_string())
            .collect()
    }

    /// The base game's blocks the terrain is made of. One that didn't load
    /// is reported and left as air.
    pub fn from_content(content: &Content) -> Self {
        let missing = Self::missing(content);
        if !missing.is_empty() {
            error!("the terrain needs {missing:?}, which didn't load; they're left as air");
        }
        let state = |id: &str, values: &[(&str, &str)]| {
            content.block_state(id, values).unwrap_or(BlockState::AIR)
        };
        let [grass, dirt, stone, sand, water, tuft] =
            Self::GROUND.map(|(id, values)| state(id, values));
        Self {
            grass,
            dirt,
            stone,
            sand,
            water,
            tuft,
            logs: Wood::ALL.map(|wood| {
                let (id, values) = Self::log(wood);
                state(id, &values)
            }),
            leaves: Wood::ALL.map(|wood| {
                let (id, values) = Self::leaves(wood);
                state(id, &values)
            }),
        }
    }

    /// The block of a tree's part.
    fn tree_part(&self, wood: Wood, part: Part) -> BlockState {
        match part {
            Part::Log => self.logs[wood.index()],
            Part::Leaves => self.leaves[wood.index()],
        }
    }

    /// Which block wins where trees and the ground meet: a tree's part goes
    /// where it ranks higher than what's there. Leaves fill only air and
    /// tufts, trunks go through leaves too, and nothing goes into the
    /// ground or water. Between two trees' parts of the same rank, the
    /// first tree's stays.
    fn rank(&self, state: BlockState) -> u8 {
        if state.is_air() || state == self.tuft {
            0
        } else if self.leaves.contains(&state) {
            1
        } else if self.logs.contains(&state) {
            2
        } else {
            3
        }
    }

    /// The block of `wood`'s `part`, if it goes where `there` is (see
    /// [`Self::rank`]).
    fn planted(&self, wood: Wood, part: Part, there: BlockState) -> Option<BlockState> {
        let state = self.tree_part(wood, part);
        (self.rank(state) > self.rank(there)).then_some(state)
    }
}

/// Answers "what voxel is at this world position?" for the whole world.
/// Shared with the tasks that generate chunks.
pub struct Terrain {
    blocks: TerrainBlocks,
    shape: TerrainShape,
    forests: Forests,
}

impl Terrain {
    pub fn new(settings: TerrainSettings, blocks: TerrainBlocks) -> Self {
        Self {
            forests: Forests::new(settings.seed, settings.base_height),
            shape: TerrainShape::new(settings),
            blocks,
        }
    }

    /// The column at `(x, z)`. `surface` gives the unrounded heights
    /// ([`TerrainShape::surface`]) of it and the columns next to it.
    fn column(&self, x: i32, z: i32, surface: impl Fn(i32, i32) -> f32) -> Column {
        let height = surface(x, z).round() as i32;
        let rock = self
            .shape
            .is_rock(height, NEIGHBOURS.map(|(dx, dz)| surface(x + dx, z + dz)));
        let tuft = hash(self.shape.seed(), x, z) % 1000 < TUFTS_PER_THOUSAND;
        Column { height, rock, tuft }
    }

    /// The column at `(x, z)`, worked out on its own.
    fn column_alone(&self, x: i32, z: i32) -> Column {
        self.column(x, z, |x, z| self.shape.surface(x, z))
    }

    /// Every tree reaching into the columns from `min` to `max`, both
    /// included, in the order they're placed.
    #[cfg(test)]
    pub fn trees(&self, min: IVec2, max: IVec2) -> Vec<Tree> {
        self.forests
            .reaching(min, max, |x, z| self.column_alone(x, z).grass_height())
    }

    /// The height of the ground in the column at `(x, z)`: its top solid
    /// voxel is at `y = height - 1`.
    #[cfg(test)]
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.shape.height(x, z)
    }

    /// Whether the column at `(x, z)` is bare rock, without soil.
    #[cfg(test)]
    pub fn is_rock(&self, x: i32, z: i32) -> bool {
        self.column_alone(x, z).rock
    }

    /// The voxel at a world position, worked out on its own, trees and
    /// all.
    #[cfg(test)]
    pub fn voxel(&self, pos: IVec3) -> BlockState {
        let mut voxel = self.column_alone(pos.x, pos.z).voxel(pos.y, &self.blocks);
        // Trees never go into the ground or water.
        if self.blocks.rank(voxel) == 3 {
            return voxel;
        }
        for tree in self.trees(pos.xz(), pos.xz()) {
            for &(at, part) in &tree.parts {
                if at == pos
                    && let Some(state) = self.blocks.planted(tree.wood, part, voxel)
                {
                    voxel = state;
                }
            }
        }
        voxel
    }

    /// Fills one chunk of the world.
    pub fn chunk(&self, coord: ChunkCoord) -> Chunk {
        let origin = coord.origin();
        let size = CHUNK_SIZE as i32;
        // One noise lookup per column, not per voxel, with a column more on
        // each side for telling cliffs.
        let padded = size + 2;
        let surfaces: Vec<f32> = (0..padded * padded)
            .map(|i| {
                self.shape
                    .surface(origin.x + i % padded - 1, origin.z + i / padded - 1)
            })
            .collect();
        let (min, max) = (origin.xz(), origin.xz() + IVec2::splat(size - 1));
        let inside =
            |x: i32, z: i32| IVec2::new(x, z).cmpge(min).all() && IVec2::new(x, z).cmple(max).all();
        let surface =
            |x: i32, z: i32| surfaces[((z - origin.z + 1) * padded + x - origin.x + 1) as usize];
        // The chunk's columns, `x` fastest; `index` finds one by world
        // position.
        let index = |x: i32, z: i32| ((z - origin.z) * size + x - origin.x) as usize;
        let columns: Vec<Column> = (0..size * size)
            .map(|i| {
                let (x, z) = (origin.x + i % size, origin.z + i / size);
                self.column(x, z, surface)
            })
            .collect();
        let lowest = columns.iter().map(|c| c.height).min().unwrap_or(0);
        // Most chunks are all sky or all stone; skip the voxel loop. Trees
        // only fill air, so an all-stone chunk has none.
        if origin.y + size <= lowest - 1 - DIRT_DEPTH {
            return Chunk::filled(self.blocks.stone);
        }
        // The trees reaching in, with the columns inside from above.
        let trees = self.forests.reaching(min, max, |x, z| {
            if inside(x, z) {
                columns[index(x, z)].grass_height()
            } else {
                self.column_alone(x, z).grass_height()
            }
        });
        // A tuft can stand on the highest column, at its height.
        let top = columns
            .iter()
            .map(|c| c.height)
            .chain(trees.iter().map(Tree::top))
            .max()
            .unwrap_or(0);
        if origin.y > top && origin.y >= SEA_LEVEL {
            return Chunk::default();
        }
        let mut chunk = Chunk::default();
        for (i, column) in columns.iter().enumerate() {
            let (x, z) = (i % CHUNK_SIZE, i / CHUNK_SIZE);
            for y in 0..CHUNK_SIZE {
                chunk.set(x, y, z, column.voxel(origin.y + y as i32, &self.blocks));
            }
        }
        for tree in &trees {
            for &(pos, part) in &tree.parts {
                let local = pos - origin;
                if inside(pos.x, pos.z)
                    && (0..size).contains(&local.y)
                    && let Some(state) = self.blocks.planted(tree.wood, part, chunk.at(local))
                {
                    chunk.set_at(local, state);
                }
            }
        }
        chunk
    }
}

/// One column of the terrain.
struct Column {
    /// Its top solid voxel is at `height - 1`.
    height: i32,
    /// Bare rock, without soil.
    rock: bool,
    /// A tuft of grass grows on it, if it's grass.
    tuft: bool,
}

impl Column {
    /// The ground's height if it's grass, which is what trees grow on: not
    /// bare rock, and not at the water's edge or under the water (sand).
    fn grass_height(&self) -> Option<i32> {
        (!self.rock && self.height > SEA_LEVEL).then_some(self.height)
    }

    /// The voxel at height `y`.
    fn voxel(&self, y: i32, blocks: &TerrainBlocks) -> BlockState {
        let height = self.height;
        // The top at the water's edge or under the water.
        let shore = height <= SEA_LEVEL;
        match y {
            y if y >= height && y < SEA_LEVEL => blocks.water,
            y if y == height && self.tuft && self.grass_height().is_some() => blocks.tuft,
            y if y >= height => BlockState::AIR,
            _ if self.rock => blocks.stone,
            y if y >= height - 1 - DIRT_DEPTH && shore => blocks.sand,
            y if y == height - 1 => blocks.grass,
            y if y >= height - 1 - DIRT_DEPTH => blocks.dirt,
            _ => blocks.stone,
        }
    }
}

/// A well-mixed number from a seed and a column, the same every time.
fn hash(seed: u64, x: i32, z: i32) -> u64 {
    splitmix64(&mut (seed ^ (x as u32 as u64) << 32 ^ z as u32 as u64))
}

/// Test helpers: a fixed patch of terrain without the streaming.
#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::world::{ChunkMap, spawn_chunk};

    /// The patch of world the tests use: 8×4×8 chunks from chunk
    /// `(0, 0, 0)`, 128×64×128 voxels, holding the default terrain's
    /// surface around the camera's start.
    pub const PATCH: IVec3 = IVec3::new(8, 4, 8);
    pub const PATCH_SIZE: IVec3 = IVec3::new(128, 64, 128);

    pub fn patch_chunks() -> impl Iterator<Item = ChunkCoord> {
        (0..PATCH.x).flat_map(|x| {
            (0..PATCH.y)
                .flat_map(move |y| (0..PATCH.z).map(move |z| ChunkCoord(IVec3::new(x, y, z))))
        })
    }

    /// Spawns the patch's chunks from the default terrain.
    pub fn spawn_patch(world: &mut World) {
        world.init_resource::<TerrainSettings>();
        world
            .run_system_cached(
                |mut commands: Commands,
                 mut map: ResMut<ChunkMap>,
                 settings: Res<TerrainSettings>,
                 content: Res<Content>| {
                    let terrain =
                        Terrain::new(settings.clone(), TerrainBlocks::from_content(&content));
                    for coord in patch_chunks() {
                        spawn_chunk(&mut commands, &mut map, coord, terrain.chunk(coord), ());
                    }
                },
            )
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        testing::{PATCH_SIZE, patch_chunks},
        *,
    };
    use bevy::platform::collections::{HashMap, HashSet};

    use crate::content::{self, base, block, grass, sand};

    fn terrain() -> Terrain {
        terrain_with(TerrainSettings::default())
    }

    fn terrain_with(settings: TerrainSettings) -> Terrain {
        Terrain::new(settings, TerrainBlocks::from_content(base()))
    }

    /// The voxel columns of the test patch.
    fn columns() -> impl Iterator<Item = (i32, i32)> {
        (0..PATCH_SIZE.x).flat_map(|x| (0..PATCH_SIZE.z).map(move |z| (x, z)))
    }

    /// The terrain carries on in every direction, at negative coordinates
    /// and far away, without seams.
    #[test]
    fn terrain_goes_on_in_every_direction() {
        let terrain = terrain();
        let blocks = TerrainBlocks::from_content(base());
        for (x0, z0) in [(-200, -200), (100_000, -3_000), (-64, 50)] {
            let mut heights = Vec::new();
            for x in x0..x0 + 64 {
                let h = terrain.height(x, z0);
                let top = if terrain.is_rock(x, z0) {
                    block("stone")
                } else if h <= SEA_LEVEL {
                    sand()
                } else {
                    grass()
                };
                assert_eq!(terrain.voxel(IVec3::new(x, h - 1, z0)), top);
                if top != grass() {
                    let above = terrain.voxel(IVec3::new(x, h, z0));
                    assert_ne!(above, block("tuft"), "a tuft on {top:?} at ({x}, {z0})");
                }
                // Open above the ground, but for trees.
                let above = terrain.voxel(IVec3::new(x, h, z0));
                assert!(!content::base().state(above).is_opaque() || is_tree(&blocks, above));
                let sky = terrain.voxel(IVec3::new(x, h.max(SEA_LEVEL) + 1, z0));
                assert!(
                    sky.is_air() || is_tree(&blocks, sky),
                    "{sky:?} at ({x}, {z0})"
                );
                if let Some(&last) = heights.last() {
                    let step: i32 = h - last;
                    assert!(step.abs() <= 16, "a wall at ({x}, {z0})");
                }
                heights.push(h);
            }
            assert!(heights.iter().any(|&h| h != heights[0]), "flat near {x0}");
        }
    }

    /// Grass over dirt over stone, or sand at and under the water's edge,
    /// with water up to the sea level, a tuft on some of the grass, and
    /// nothing above but trees.
    #[test]
    fn columns_are_layered_grass_dirt_stone() {
        let terrain = terrain();
        let blocks = TerrainBlocks::from_content(base());
        let mut generated = generated(&terrain);
        let (mut tufts, mut grassy, mut ponds, mut trees) = (0, 0, 0, 0);
        for (x, z) in columns() {
            let height = terrain.height(x, z);
            assert!(
                (1..=PATCH_SIZE.y).contains(&height),
                "patch holds the surface"
            );
            let mut at = |y| generated(IVec3::new(x, y, z));
            let shore = height <= SEA_LEVEL;
            let (top, soil) = if shore {
                (sand(), sand())
            } else {
                (grass(), block("soil"))
            };
            assert_eq!(at(height - 1), top);
            let mut above = height;
            if at(above) == block("tuft") {
                assert!(!shore, "a tuft on the sand at ({x}, {z})");
                tufts += 1;
                above += 1;
            }
            for y in above..SEA_LEVEL {
                assert_eq!(at(y), block("water"), "({x}, {y}, {z})");
            }
            ponds += (height < SEA_LEVEL) as u32;
            grassy += !shore as u32;
            for y in above.max(SEA_LEVEL)..height + 100 {
                let there = at(y);
                assert!(
                    there.is_air() || is_tree(&blocks, there),
                    "{there:?} at ({x}, {y}, {z})"
                );
                trees += is_tree(&blocks, there) as u32;
            }
            for y in height - 100..height - 1 {
                let expected = if y >= height - 1 - DIRT_DEPTH {
                    soil
                } else {
                    block("stone")
                };
                assert_eq!(at(y), expected, "({x}, {y}, {z}), height {height}");
            }
        }
        // About 8% of the grass has a tuft, and the patch has a pond.
        let share = tufts as f32 / grassy as f32;
        assert!(
            (0.06..0.10).contains(&share),
            "{share} of the grass has tufts"
        );
        assert!(ponds > 0, "no water in the patch");
        assert!(trees > 0, "no trees in the patch");
    }

    /// Tufts grow only on grass: never on the bare rock of mountains and
    /// cliffs, though the hash picks some of those columns too.
    #[test]
    fn tufts_grow_only_on_grass() {
        let terrain = terrain();
        let mut rock = 0;
        for x in 1500..1564 {
            for z in -400..-336 {
                if !terrain.is_rock(x, z) {
                    continue;
                }
                rock += 1;
                let h = terrain.height(x, z);
                assert_ne!(
                    terrain.voxel(IVec3::new(x, h, z)),
                    block("tuft"),
                    "({x}, {z})"
                );
            }
        }
        assert!(rock > 500, "only {rock} rock columns");
    }

    /// Chunks generated whole match the voxels column by column, water and
    /// tufts included, even where a chunk starts right at a tuft.
    #[test]
    fn chunks_match_the_voxels() {
        let terrain = terrain();
        for coord in patch_chunks() {
            let chunk = terrain.chunk(coord);
            for (x, y, z) in Chunk::positions() {
                let local = IVec3::new(x as i32, y as i32, z as i32);
                assert_eq!(
                    chunk.at(local),
                    terrain.voxel(coord.origin() + local),
                    "{local} in {coord:?}"
                );
            }
        }
    }

    /// Around the start, hills, not a flat plane or random spikes: the
    /// height varies a lot over the patch but only a little between
    /// neighbouring columns. (Further out there are mountains: see `shape`.)
    #[test]
    fn terrain_is_hilly_and_smooth() {
        let terrain = terrain();
        let heights: Vec<_> = columns().map(|(x, z)| terrain.height(x, z)).collect();
        let (min, max) = (heights.iter().min().unwrap(), heights.iter().max().unwrap());
        assert!(max - min >= 8, "heights only span {min}..={max}");
        // Both chunk layers have ground in them somewhere, and the peaks
        // aren't clipped flat against the top of the world.
        assert!(*max > CHUNK_SIZE as i32 && *max < PATCH_SIZE.y);
        for (x, z) in columns() {
            let h = terrain.height(x, z);
            for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                let step = (terrain.height(nx, nz) - h).abs();
                assert!(step <= 3, "cliff of {step} at ({x}, {z})");
            }
        }
    }

    #[test]
    fn sky_all_the_way_up_and_stone_all_the_way_down() {
        let terrain = terrain();
        // By the start, and in the mountains.
        for (x, z) in [(-5, 3), (1576, -352)] {
            for y in [400, 10_000, 1_000_000] {
                assert_eq!(terrain.voxel(IVec3::new(x, y, z)), BlockState::AIR);
                assert_eq!(terrain.voxel(IVec3::new(x, -y, z)), block("stone"));
            }
        }
        for pos in [IVec3::ZERO, IVec3::new(-1, 0, -1), IVec3::new(100, 0, 0)] {
            assert_eq!(terrain.voxel(pos), block("stone"));
        }
        let sky = ChunkCoord(IVec3::new(3, 40, -2));
        let deep = ChunkCoord(IVec3::new(3, -40, -2));
        assert_eq!(terrain.chunk(sky), Chunk::default());
        assert_eq!(terrain.chunk(deep), Chunk::filled(block("stone")));
    }

    /// The camera spawns in open air with clearance above the hills around
    /// it, not inside or skimming the ground.
    #[test]
    fn camera_starts_above_the_ground() {
        use crate::player::START_POSITION;
        let terrain = terrain();
        let start = START_POSITION.as_ivec3();
        assert!((0..PATCH_SIZE.y).contains(&start.y));
        let clearance = 4;
        for dx in -8..=8 {
            for dz in -8..=8 {
                let height = terrain.height(start.x + dx, start.z + dz);
                assert!(
                    height + clearance <= start.y,
                    "hill of {height} near the start"
                );
            }
        }
    }

    #[test]
    fn missing_terrain_blocks_are_named() {
        assert!(TerrainBlocks::missing(base()).is_empty());
        let mut content = base().clone();
        assert!(content.forget_block("defaria:stone"));
        assert_eq!(TerrainBlocks::missing(&content), ["defaria:stone"]);
    }

    /// A mod's soil whose id sorts before `defaria:dirt` changes neither
    /// soil's default nor the ground: the terrain names dirt.
    #[test]
    fn a_modded_soil_doesnt_change_the_ground() {
        let (mut mods, errors) = crate::content::load::read_mods(
            std::path::Path::new("assets/mods"),
            crate::content::MODS_DIR,
        );
        assert!(errors.is_empty(), "{errors:?}");
        mods.push(crate::content::load::ModFiles::new(
            "mods/clay",
            r#"{ id: "clay", name: "Clay", dependencies: ["defaria"] }"#,
            &[
                ("images/clay.png", ""),
                (
                    "materials/clay.json5",
                    r##"{ tags: ["soil"], palette: ["#000000", "#ffffff"], hardness: 1, weight: 1 }"##,
                ),
            ],
        ));
        let content = Content::from_mods(mods);
        let soil = content.block("defaria:soil").unwrap();
        assert!(soil.state(&[("soil", "clay:clay")]).is_some());
        let dirt = |state: BlockState| {
            content
                .state(state)
                .values
                .contains(&("soil".into(), "defaria:dirt".into()))
        };
        assert!(dirt(soil.default_state()));
        let blocks = TerrainBlocks::from_content(&content);
        assert!(dirt(blocks.dirt) && dirt(blocks.grass));
    }

    /// Without the sand material, the soil block still loads but has no
    /// sand state, which the terrain needs for its shores.
    #[test]
    fn missing_terrain_states_are_named() {
        let (mut mods, errors) = crate::content::load::read_mods(
            std::path::Path::new("assets/mods"),
            crate::content::MODS_DIR,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let base = mods
            .iter_mut()
            .find(|m| m.dir.ends_with("defaria"))
            .unwrap();
        assert!(base.files.remove("materials/sand.json5").is_some());
        let content = Content::from_mods(mods);
        assert!(content.block("defaria:soil").is_some());
        assert_eq!(
            TerrainBlocks::missing(&content),
            ["defaria:soil[soil=defaria:sand,turf=none]"]
        );
        assert_eq!(TerrainBlocks::from_content(&content).sand, BlockState::AIR);
    }

    #[test]
    fn different_seeds_make_different_terrain() {
        let a = terrain();
        let b = terrain_with(TerrainSettings {
            seed: 1,
            ..default()
        });
        assert!(columns().any(|(x, z)| a.height(x, z) != b.height(x, z)));
    }

    /// In the mountains, among cliffs and peaks, chunks match the
    /// column-by-column answer voxel for voxel, across their borders and
    /// in the all-air and all-stone shortcuts, and bare-rock columns are
    /// stone right to the top.
    #[test]
    fn mountain_chunks_match_their_columns() {
        let terrain = terrain();
        let (mut rock, mut grassy) = (0, 0);
        // East of the start, where the mountains are cliffs below the rock
        // line.
        for cx in 102..106 {
            for cz in 4..7 {
                let (x0, z0) = (cx * 16, cz * 16);
                let surface = terrain.height(x0 + 8, z0 + 8).div_euclid(16);
                for cy in surface - 1..=surface + 1 {
                    let coord = ChunkCoord(IVec3::new(cx, cy, cz));
                    let chunk = terrain.chunk(coord);
                    for (x, y, z) in Chunk::positions() {
                        let world = coord.origin() + IVec3::new(x as i32, y as i32, z as i32);
                        assert_eq!(chunk.get(x, y, z), terrain.voxel(world), "at {world}");
                    }
                }
                for x in x0..x0 + 16 {
                    for z in z0..z0 + 16 {
                        let top = terrain.voxel(IVec3::new(x, terrain.height(x, z) - 1, z));
                        if terrain.is_rock(x, z) {
                            rock += 1;
                            assert_eq!(top, block("stone"), "at ({x}, {z})");
                        } else {
                            grassy += 1;
                            assert_eq!(top, grass(), "at ({x}, {z})");
                        }
                    }
                }
            }
        }
        assert!(
            rock > 50 && grassy > 50,
            "{rock} rock and {grassy} grass columns"
        );
    }

    #[test]
    fn chunks_hold_their_slice_of_the_world() {
        let terrain = terrain();
        let coord = ChunkCoord(IVec3::new(2, 1, 3));
        let chunk = terrain.chunk(coord);
        for (x, y, z) in [(0, 0, 0), (5, 3, 9), (15, 15, 15)] {
            let world = coord.origin() + IVec3::new(x as i32, y as i32, z as i32);
            assert_eq!(chunk.get(x, y, z), terrain.voxel(world));
        }
    }

    /// Whether a block is any wood's log or leaves.
    fn is_tree(blocks: &TerrainBlocks, state: BlockState) -> bool {
        blocks.logs.contains(&state) || blocks.leaves.contains(&state)
    }

    /// The voxels of generated chunks, generating each the first time it's
    /// asked for.
    fn generated(terrain: &Terrain) -> impl FnMut(IVec3) -> BlockState + '_ {
        let mut chunks = HashMap::<ChunkCoord, Chunk>::new();
        move |pos| {
            let (coord, local) = ChunkCoord::split(pos);
            chunks
                .entry(coord)
                .or_insert_with(|| terrain.chunk(coord))
                .at(local)
        }
    }

    /// A forest, with trees on hills and by ponds, east of the start.
    const FOREST: (IVec2, IVec2) = (IVec2::new(160, 48), IVec2::new(287, 143));

    /// Trees split across chunk borders are whole, and each chunk works out
    /// its share alone: every voxel of every tree, generated chunk by chunk,
    /// is exactly what that voxel is worked out on its own (from the trees
    /// around its column), and leaves never take the place of the ground.
    #[test]
    fn trees_are_whole_across_chunk_borders() {
        let terrain = terrain();
        let blocks = TerrainBlocks::from_content(base());
        let mut at = generated(&terrain);
        let trees = terrain.trees(FOREST.0, FOREST.1);
        let (mut split, mut own_leaves, mut leaves_by_ground, mut on_tufts) = (0, 0, 0, 0);
        for tree in &trees {
            let root = terrain.column_alone(tree.root.x, tree.root.z);
            on_tufts += root.tuft as u32;
            let chunks: HashSet<_> = tree
                .parts
                .iter()
                .map(|&(p, _)| ChunkCoord::split(p).0)
                .collect();
            split += (chunks.len() > 1) as u32;
            for &(pos, part) in &tree.parts {
                let there = at(pos);
                assert_eq!(there, terrain.voxel(pos), "{pos} of {tree:?}");
                if part == Part::Log {
                    assert_eq!(there, blocks.tree_part(tree.wood, part), "{pos}");
                }
                let column = terrain.column_alone(pos.x, pos.z);
                let ground = column.voxel(pos.y, &blocks);
                if pos.y < column.height {
                    leaves_by_ground += 1;
                    assert_eq!(there, ground, "leaves in the ground at {pos}");
                } else if blocks.rank(ground) == 0 {
                    // Open air or a tuft: a tree is there, this one or another.
                    assert!(is_tree(&blocks, there), "{there:?} at {pos} of {tree:?}");
                }
                own_leaves +=
                    (part == Part::Leaves && there == blocks.tree_part(tree.wood, part)) as u32;
            }
        }
        assert!(
            split >= 20,
            "only {split} of {} trees cross a border",
            trees.len()
        );
        assert!(own_leaves >= 1000, "only {own_leaves} leaves");
        assert!(
            leaves_by_ground > 0 && on_tufts > 0,
            "{leaves_by_ground} leaves meet the ground, {on_tufts} trunks stand on tufts"
        );
    }

    /// Trees grow only on grass: never on sand (at or under the water's
    /// edge) or bare rock, though plenty sprout there, in the forest by the
    /// ponds and in the mountains. Every trunk in the generated chunks starts
    /// on grass, where it stands instead of a tuft.
    #[test]
    fn trees_grow_only_on_grass() {
        let terrain = terrain();
        let blocks = TerrainBlocks::from_content(base());
        let (mut sand, mut rock, mut grassy) = (0, 0, 0);
        for (min, max) in [FOREST, (IVec2::new(1500, -400), IVec2::new(1900, 100))] {
            // Every tree that would sprout if the ground didn't matter.
            let sprouts = terrain
                .forests
                .reaching(min, max, |x, z| Some(terrain.column_alone(x, z).height));
            let mut expected = Vec::new();
            for sprout in &sprouts {
                let column = terrain.column_alone(sprout.root.x, sprout.root.z);
                sand += (column.height <= SEA_LEVEL) as u32;
                rock += column.rock as u32;
                if !column.rock && column.height > SEA_LEVEL {
                    grassy += 1;
                    expected.push(sprout.root);
                }
            }
            let grown: Vec<_> = terrain.trees(min, max).iter().map(|t| t.root).collect();
            assert_eq!(grown, expected);
        }
        assert!(
            sand >= 20 && rock >= 20 && grassy >= 200,
            "{sand} sprouts on sand, {rock} on rock, {grassy} on grass"
        );

        let mut at = generated(&terrain);
        let mut trunks = 0;
        for x in FOREST.0.x..=FOREST.1.x {
            for z in FOREST.0.y..=FOREST.1.y {
                let height = terrain.height(x, z);
                if blocks.logs.contains(&at(IVec3::new(x, height, z))) {
                    trunks += 1;
                    assert_eq!(at(IVec3::new(x, height - 1, z)), grass(), "({x}, {z})");
                }
            }
        }
        assert!(trunks >= 50, "only {trunks} trees");
    }

    /// The player starts in a meadow, with no tree nearby, but with forests
    /// in sight.
    #[test]
    fn the_start_is_a_meadow_near_forests() {
        use crate::player::START_POSITION;
        let terrain = terrain();
        let start = START_POSITION.xz();
        let clearing = trees::START_CLEARING;
        let reach = clearing as i32 + trees::REACH;
        let near = terrain.trees(start.as_ivec2() - reach, start.as_ivec2() + reach);
        // Leaves reach out diagonally from a trunk at the clearing's edge.
        let leaves = clearing - trees::REACH as f32 * std::f32::consts::SQRT_2;
        for tree in near {
            let root = tree.root.xz().as_vec2() + 0.5;
            assert!(root.distance(start) >= clearing, "{tree:?} by the start");
            for &(pos, _) in &tree.parts {
                let distance = (pos.xz().as_vec2() + 0.5).distance(start);
                assert!(distance >= leaves, "{pos} of {tree:?}");
            }
        }
        let around = terrain.trees(start.as_ivec2() - 160, start.as_ivec2() + 160);
        assert!(
            around.len() >= 150,
            "only {} trees around the start",
            around.len()
        );
    }

    /// Forests, clearings and a tree here and there between them: squares
    /// of the land hold very different numbers of trees, as the forest
    /// noise says (on grass everywhere, so lakes don't count as clearings).
    #[test]
    fn forests_and_clearings() {
        let terrain = terrain();
        let counts: Vec<usize> = (0..12)
            .flat_map(|i| (0..12).map(move |j| IVec2::new(i * 64 - 2000, j * 64 + 700)))
            .map(|corner| {
                let trees = terrain
                    .forests
                    .reaching(corner, corner + 47, |_, _| Some(30));
                trees.len()
            })
            .collect();
        let forests = counts.iter().filter(|&&n| n >= 30).count();
        let clearings = counts.iter().filter(|&&n| n == 0).count();
        let sparse = counts.iter().filter(|&&n| (1..=8).contains(&n)).count();
        assert!(
            forests >= 10 && clearings >= 10 && sparse >= 10,
            "{counts:?}"
        );
    }

    /// Spruces lean to higher ground than birches.
    #[test]
    fn spruces_grow_higher_up() {
        let terrain = terrain();
        let trees = terrain.trees(IVec2::splat(-1500), IVec2::splat(1500));
        let mean = |wood| {
            let heights: Vec<_> = trees
                .iter()
                .filter(|t| t.wood == wood)
                .map(|t| t.root.y)
                .collect();
            heights.iter().sum::<i32>() as f32 / heights.len() as f32
        };
        let (spruce, birch) = (mean(Wood::Spruce), mean(Wood::Birch));
        assert!(
            spruce > birch + 5.0,
            "spruces at {spruce}, birches at {birch}"
        );
    }

    /// Every wood grows somewhere, of its own logs and leaves.
    #[test]
    fn every_wood_grows() {
        let terrain = terrain();
        let blocks = TerrainBlocks::from_content(base());
        let content = base();
        let trees = terrain.trees(IVec2::splat(-1500), IVec2::splat(1500));
        for wood in Wood::ALL {
            assert_eq!(Wood::ALL[wood.index()], wood);
            let n = trees.iter().filter(|t| t.wood == wood).count();
            assert!(n * 10 >= trees.len(), "{n} {wood:?} of {}", trees.len());
            for (part, id) in [(Part::Log, "defaria:log"), (Part::Leaves, "defaria:leaves")] {
                let state = content.state(blocks.tree_part(wood, part));
                assert_eq!(content.blocks[state.block].id, id);
                let material = ("wood".to_string(), wood.material().to_string());
                assert!(
                    state.values.contains(&material),
                    "{wood:?} {part:?}: {state:?}"
                );
            }
            let log = content.state(blocks.tree_part(wood, Part::Log));
            assert!(log.values.contains(&("axis".into(), "y".into())), "{log:?}");
        }
    }
}
