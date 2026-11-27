//! Trees on the terrain's grass: where they grow, and their shapes.
//!
//! The world is split into square cells [`CELL`] blocks across, and each
//! cell grows at most one tree, somewhere inside it, with a chance that
//! comes from the forest noise: dense in forests, a tree here and there
//! elsewhere, none in clearings. Everything about a tree (whether it grows,
//! where, its wood, its size and its leaves) comes from the seed and its
//! cell alone, so any chunk can work out every tree that reaches into it,
//! from the cells around it, without the chunks around it: a tree split
//! across a chunk border is the same tree on both sides, whichever chunk is
//! generated first.
//!
//! A tree is [`REACH`] blocks at most sideways from its trunk.
//!
//! Three shapes: rounded oaks (some of them big), slim birches with small
//! crowns, and tall, narrow, tiered spruces. Which wood grows where follows
//! a noise of its own, with spruce more common higher up, until there are
//! biomes.

use bevy::prelude::*;

use crate::worldgen::shape::{STRETCH, smoothstep};
use defaria_core::{
    perlin::{Perlin, splitmix64},
    player::START_POSITION,
};

/// The size of the cells trees grow in, in blocks.
const CELL: i32 = 6;
/// How far a tree's leaves reach sideways from its trunk, at most.
pub const REACH: i32 = 3;
/// Within this many blocks of the start, sideways, no tree grows, so the
/// player starts in a meadow.
pub const START_CLEARING: f32 = 40.0;
/// Trees come back to their full density this many blocks past the
/// clearing.
const CLEARING_EDGE: f32 = 24.0;
/// Horizontal size of the forest noise, in blocks.
const FOREST_SCALE: f32 = 160.0;
/// Horizontal size of the noise that picks the woods, in blocks.
const WOOD_SCALE: f32 = 300.0;
/// The chance a cell grows a tree in the thick of a forest.
const DENSE: f32 = 0.85;
/// The chance a cell grows a tree away from forests (but not in clearings).
const SPARSE: f32 = 0.05;
/// One tree in this many is a random wood, so forests aren't all one kind.
const STRAYS: u64 = 8;

/// The woods trees grow of, which are the base game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wood {
    Oak,
    Birch,
    Spruce,
}

impl Wood {
    pub const ALL: [Wood; 3] = [Wood::Oak, Wood::Birch, Wood::Spruce];

    /// Where it is in [`Self::ALL`].
    pub fn index(self) -> usize {
        match self {
            Wood::Oak => 0,
            Wood::Birch => 1,
            Wood::Spruce => 2,
        }
    }

    /// The wood's material id.
    pub fn material(self) -> &'static str {
        match self {
            Wood::Oak => "defaria:oak",
            Wood::Birch => "defaria:birch",
            Wood::Spruce => "defaria:spruce",
        }
    }
}

/// A block of a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// The trunk, upright.
    Log,
    Leaves,
}

/// One tree, standing on the ground.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    /// The bottom of the trunk: the voxel just above the ground.
    pub root: IVec3,
    pub wood: Wood,
    /// Its blocks, by world position, each position once.
    pub parts: Vec<(IVec3, Part)>,
}

impl Tree {
    /// The tree of `wood` standing at `root`, its size and leaves from `rng`.
    fn grow(root: IVec3, wood: Wood, rng: &mut u64) -> Self {
        let mut parts = Vec::new();
        let trunk = match wood {
            Wood::Oak => oak(rng, &mut parts),
            Wood::Birch => birch(rng, &mut parts),
            Wood::Spruce => spruce(rng, &mut parts),
        };
        // The trunk takes the place of any leaves in its way.
        parts.retain(|(pos, _)| pos.x != 0 || pos.z != 0 || pos.y >= trunk);
        parts.extend((0..trunk).map(|y| (IVec3::new(0, y, 0), Part::Log)));
        for (pos, _) in &mut parts {
            *pos += root;
        }
        Self { root, wood, parts }
    }

    /// The height of its highest block.
    pub fn top(&self) -> i32 {
        self.parts
            .iter()
            .map(|(pos, _)| pos.y)
            .max()
            .unwrap_or(self.root.y)
    }
}

/// A random number below `n`.
fn below(rng: &mut u64, n: u64) -> u64 {
    splitmix64(rng) % n
}

/// A random number from `low` to `high`, both included.
fn between(rng: &mut u64, low: i32, high: i32) -> i32 {
    low + below(rng, (high - low + 1) as u64) as i32
}

/// Adds a layer of leaves at height `y` (from the root), `radius` blocks
/// out from the trunk, with its corners rounded off. At radius 1 and 2, each
/// corner is kept or not at random, so crowns aren't all the same.
fn layer(parts: &mut Vec<(IVec3, Part)>, rng: &mut u64, y: i32, radius: i32) {
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            let corner = dx.abs() == radius && dz.abs() == radius;
            let keep = if corner && (1..=2).contains(&radius) {
                below(rng, 2) == 0
            } else {
                dx * dx + dz * dz <= radius * radius + 1
            };
            if keep {
                parts.push((IVec3::new(dx, y, dz), Part::Leaves));
            }
        }
    }
}

/// Adds a cross of five leaves at height `y`, the top of most crowns.
fn cross(parts: &mut Vec<(IVec3, Part)>, y: i32) {
    for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        parts.push((IVec3::new(dx, y, dz), Part::Leaves));
    }
}

/// A round crown on a short trunk; one in four is a big one. Returns the
/// trunk's height.
fn oak(rng: &mut u64, parts: &mut Vec<(IVec3, Part)>) -> i32 {
    if below(rng, 4) == 0 {
        let trunk = between(rng, 6, 7);
        for (y, radius) in [(-3, 2), (-2, 3), (-1, 3), (0, 2), (1, 1)] {
            layer(parts, rng, trunk + y, radius);
        }
        trunk
    } else {
        let trunk = between(rng, 4, 6);
        for (y, radius) in [(-3, 2), (-2, 2), (-1, 1)] {
            layer(parts, rng, trunk + y, radius);
        }
        cross(parts, trunk);
        trunk
    }
}

/// A small, narrow crown on a tall trunk. Returns the trunk's height.
fn birch(rng: &mut u64, parts: &mut Vec<(IVec3, Part)>) -> i32 {
    let trunk = between(rng, 5, 7);
    for (y, radius) in [(-3, 1), (-2, 2), (-1, 1)] {
        layer(parts, rng, trunk + y, radius);
    }
    cross(parts, trunk);
    trunk
}

/// A tall cone of tiers, wider and narrower in turn going down, with a
/// leaf on top. The lowest tier is high enough to walk under on level
/// ground. Returns the trunk's height.
fn spruce(rng: &mut u64, parts: &mut Vec<(IVec3, Part)>) -> i32 {
    let trunk = between(rng, 7, 10);
    // Only the tallest get the widest tiers.
    let widest = if trunk >= 9 { 3 } else { 2 };
    let bottom = between(rng, 3, 4);
    parts.push((IVec3::new(0, trunk + 1, 0), Part::Leaves));
    cross(parts, trunk);
    for (i, y) in (bottom..trunk).rev().enumerate() {
        let i = i as i32;
        let radius = ((i + 2) / 2).min(widest) - i % 2;
        if radius > 0 {
            layer(parts, rng, y, radius);
        }
    }
    trunk
}

/// Where trees grow, from the seed.
pub struct Forests {
    seed: u64,
    density: Perlin,
    woods: Perlin,
    /// The height of the ground where the noise is 0, for leaning to spruce
    /// higher up.
    base_height: f32,
}

impl Forests {
    pub fn new(seed: u64, base_height: f32) -> Self {
        Self {
            seed: seed ^ 0x7EE5_F0E5,
            density: Perlin::new(seed ^ 0xF0E5_7000),
            woods: Perlin::new(seed ^ 0x3D0D_5000),
            base_height,
        }
    }

    /// The chance that a cell around column `(x, z)` grows a tree.
    fn density(&self, x: i32, z: i32) -> f32 {
        let (x, z) = (x as f32 + 0.5, z as f32 + 0.5);
        let forest =
            (self.density.fbm(x / FOREST_SCALE, z / FOREST_SCALE, 3) * STRETCH).clamp(-1.0, 1.0);
        let chance = SPARSE * smoothstep(-0.45, -0.25, forest)
            + (DENSE - SPARSE) * smoothstep(0.05, 0.35, forest);
        let start = (x - START_POSITION.x).hypot(z - START_POSITION.z);
        chance * smoothstep(START_CLEARING, START_CLEARING + CLEARING_EDGE, start)
    }

    /// The wood a tree standing on ground `height` high at `(x, z)` is.
    fn wood(&self, x: i32, z: i32, height: i32, rng: &mut u64) -> Wood {
        if below(rng, STRAYS) == 0 {
            return Wood::ALL[below(rng, Wood::ALL.len() as u64) as usize];
        }
        let (fx, fz) = (x as f32 / WOOD_SCALE, z as f32 / WOOD_SCALE);
        let altitude = (height as f32 - self.base_height) / 80.0;
        let pick = (self.woods.fbm(fx, fz, 2) * STRETCH).clamp(-1.0, 1.0) + altitude;
        match pick {
            p if p < -0.25 => Wood::Birch,
            p if p > 0.3 => Wood::Spruce,
            _ => Wood::Oak,
        }
    }

    /// Where in `cell` a tree grows, if one does, and the random numbers
    /// the rest of it comes from.
    fn sprout(&self, cell: IVec2) -> Option<(IVec2, u64)> {
        let mut rng = self.seed ^ (cell.x as u32 as u64) << 32 ^ cell.y as u32 as u64;
        let chance = below(&mut rng, 1 << 16) as f32 / (1 << 16) as f32;
        // Anywhere in the cell but its first row and column, so there's
        // always a column between two cells' trunks.
        let x = cell.x * CELL + between(&mut rng, 1, CELL - 1);
        let z = cell.y * CELL + between(&mut rng, 1, CELL - 1);
        (chance < self.density(x, z)).then_some((IVec2::new(x, z), rng))
    }

    /// The tree rooted at `at`, sprouted with `rng`, if the ground there is
    /// grass. `grass` is the height of the ground in a column, if it's
    /// grass, which is all trees grow on.
    fn grow(
        &self,
        at: IVec2,
        mut rng: u64,
        grass: &impl Fn(i32, i32) -> Option<i32>,
    ) -> Option<Tree> {
        let height = grass(at.x, at.y)?;
        let wood = self.wood(at.x, at.y, height, &mut rng);
        Some(Tree::grow(IVec3::new(at.x, height, at.y), wood, &mut rng))
    }

    /// Every tree that reaches into the columns from `min` to `max` (both
    /// included), in the same order whatever the area: by cell, `x` then
    /// `z`. `grass` is the height of the ground in a column, if it's grass.
    pub fn reaching(
        &self,
        min: IVec2,
        max: IVec2,
        grass: impl Fn(i32, i32) -> Option<i32>,
    ) -> Vec<Tree> {
        let low = (min - REACH).div_euclid(IVec2::splat(CELL));
        let high = (max + REACH).div_euclid(IVec2::splat(CELL));
        let mut trees = Vec::new();
        for cx in low.x..=high.x {
            for cz in low.y..=high.y {
                let Some((at, rng)) = self.sprout(IVec2::new(cx, cz)) else {
                    continue;
                };
                let reaches = (at + REACH).cmpge(min).all() && (at - REACH).cmple(max).all();
                if let Some(tree) = reaches.then(|| self.grow(at, rng, &grass)).flatten() {
                    trees.push(tree);
                }
            }
        }
        trees
    }
}

#[cfg(test)]
mod tests {
    use bevy::platform::collections::HashSet;

    use super::*;

    /// Many trees of `wood`, each from its own random numbers.
    fn grown(wood: Wood) -> Vec<Tree> {
        (0..500)
            .map(|i| Tree::grow(IVec3::new(0, 30, 0), wood, &mut (i * 7919)))
            .collect()
    }

    /// Every tree is a trunk of logs straight up from its root with leaves
    /// around and above it, inside the reach every chunk looks for trees
    /// in, each block once.
    #[test]
    fn trees_are_trunks_with_crowns_inside_their_bounds() {
        for wood in Wood::ALL {
            for tree in grown(wood) {
                let mut seen = HashSet::new();
                for &(pos, part) in &tree.parts {
                    assert!(seen.insert(pos), "{pos} twice in {tree:?}");
                    let offset = pos - tree.root;
                    assert!(
                        offset.x.abs() <= REACH && offset.z.abs() <= REACH,
                        "{offset}"
                    );
                    assert!((0..12).contains(&offset.y), "{offset}");
                    match part {
                        Part::Log => assert_eq!(offset.xz(), IVec2::ZERO, "a branch at {offset}"),
                        // Spruces' lowest tier is high enough to walk under.
                        Part::Leaves if wood == Wood::Spruce => assert!(offset.y >= 3, "{offset}"),
                        Part::Leaves => {}
                    }
                }
                let trunk = tree.parts.iter().filter(|(_, p)| *p == Part::Log).count() as i32;
                for y in 0..trunk {
                    assert!(seen.contains(&(tree.root + IVec3::Y * y)));
                }
                let leaves = tree.parts.len() as i32 - trunk;
                assert!(leaves >= 15, "{wood:?} with {leaves} leaves");
                // Leaves above the trunk, so it doesn't poke out.
                assert!(tree.top() >= tree.root.y + trunk, "{wood:?} pokes out");
            }
        }
    }

    /// Spruces are the tallest, birches slimmer than oaks, and each wood
    /// comes in several sizes.
    #[test]
    fn each_wood_has_its_own_shape() {
        let stats = |wood| {
            let trees = grown(wood);
            let heights: HashSet<i32> = trees.iter().map(|t| t.top() - t.root.y + 1).collect();
            let height = heights.iter().sum::<i32>() as f32 / heights.len() as f32;
            let width = trees
                .iter()
                .map(|t| {
                    t.parts
                        .iter()
                        .map(|(p, _)| (p.x - t.root.x).abs())
                        .max()
                        .unwrap()
                })
                .max()
                .unwrap();
            let leaves =
                trees.iter().map(|t| t.parts.len()).sum::<usize>() as f32 / trees.len() as f32;
            (heights.len(), height, width, leaves)
        };
        let (oak_sizes, oak_height, oak_width, oak_leaves) = stats(Wood::Oak);
        let (birch_sizes, birch_height, birch_width, birch_leaves) = stats(Wood::Birch);
        let (spruce_sizes, spruce_height, _, _) = stats(Wood::Spruce);
        assert!(oak_sizes >= 3 && birch_sizes >= 3 && spruce_sizes >= 4);
        assert!(spruce_height > oak_height && spruce_height > birch_height);
        assert!(
            birch_width < oak_width,
            "birch {birch_width}, oak {oak_width}"
        );
        assert!(birch_leaves < oak_leaves);
    }

    /// The same seed grows the same trees, a different one others.
    #[test]
    fn trees_come_from_the_seed() {
        let flat = |_: i32, _: i32| Some(30);
        let area = (IVec2::splat(-200), IVec2::splat(200));
        let a = Forests::new(1, 22.0).reaching(area.0, area.1, flat);
        let b = Forests::new(1, 22.0).reaching(area.0, area.1, flat);
        let c = Forests::new(2, 22.0).reaching(area.0, area.1, flat);
        assert!(a.len() > 50, "only {} trees", a.len());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    /// Trees grow only where the ground is grass, and every tree that
    /// reaches an area is found, however big the area asked about.
    #[test]
    fn trees_grow_only_where_asked_and_reach_every_area() {
        let forests = Forests::new(5, 22.0);
        // Grass only east of x = 300.
        let grass = |x: i32, _: i32| (x >= 300).then_some(30);
        let wide = forests.reaching(IVec2::new(250, -100), IVec2::new(400, 100), grass);
        assert!(wide.iter().all(|t| t.root.x >= 300));
        assert!(!wide.is_empty());
        // Asking column by column finds the same trees.
        for x in 250..=400 {
            for z in (-100..=100).step_by(7) {
                let column = IVec2::new(x, z);
                let here = forests.reaching(column, column, grass);
                let expected: Vec<_> = wide
                    .iter()
                    .filter(|t| (t.root.xz() - column).abs().max_element() <= REACH)
                    .collect();
                assert_eq!(here.iter().collect::<Vec<_>>(), expected, "at {column}");
            }
        }
    }
}
