//! The shape of the land: how high the ground is in each column.
//!
//! Like Minecraft's terrain, the height comes from a few kinds of noise,
//! each mapped through a [`Spline`]:
//!
//! - **Continentalness**, the largest: how high the land is overall, from
//!   lowland basins up through plains to high plateaus.
//! - **Erosion**: how rugged it is. Eroded land stays smooth; where it isn't
//!   eroded, the peaks rise.
//! - **Peaks and valleys**: ridged noise (one minus the noise's size), so
//!   its highs are sharp ridgelines and crests, and between them valleys.
//!   How much of it shows is the erosion's ruggedness times how inland the
//!   land is (higher continentalness has bigger mountains).
//! - **Hills**: the small rolling hills everywhere, as before
//!   ([`TerrainSettings::scale`], [`TerrainSettings::amplitude`]).
//!
//! Fractal noise rarely strays far from 0, so each kind is stretched by
//! [`STRETCH`] (and clamped to -1 to 1) to use the splines' whole range.
//!
//! The land around the origin is kept to the plain rolling hills the start
//! and the scenarios were built on: the bigger shapes fade in between
//! [`CALM_RADIUS`] and [`FULL_RADIUS`] blocks out.

#[cfg(test)]
use std::f32::consts::SQRT_2;

use crate::{
    perlin::Perlin,
    worldgen::{
        TerrainSettings,
        spline::{Spline, knot},
    },
};

/// How much the continentalness, erosion and peaks noise are stretched:
/// fractal noise's spread is about ±0.19, and this brings it to about ±0.5.
pub const STRETCH: f32 = 2.6;
/// Within this many blocks of the origin, sideways, the land is plain hills.
pub const CALM_RADIUS: f32 = 200.0;
/// From this many blocks out the land is fully shaped.
pub const FULL_RADIUS: f32 = 500.0;

/// The land at and above this height (from the base height) is bare rock.
pub const ROCK_LINE: i32 = 100;
/// Ground this steep, in blocks up per block across, is a cliff, with no soil
/// on it. Measured across each column, from its neighbours' unrounded
/// heights either side, so it changes smoothly and the edges of cliffs
/// aren't speckled. The plain
/// hills step at most 3 a block each way, at most 4.24 diagonally, so
/// they're never cliffs.
pub const CLIFF: f32 = 4.5;
/// The columns next to one, in the order [`TerrainShape::is_rock`] takes
/// their heights: east, west, south, north.
pub const NEIGHBOURS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Works out the ground's height in any column.
pub struct TerrainShape {
    settings: TerrainSettings,
    hills: Perlin,
    continents: Perlin,
    erosion: Perlin,
    peaks: Perlin,
    /// Continentalness to height, from the base height.
    continental_height: Spline,
    /// Continentalness to how big mountains get there, 0 to 1.
    mountainous: Spline,
    /// Erosion to ruggedness, 0 (worn smooth) to 1.
    ruggedness: Spline,
    /// Peaks-and-valleys noise (0 in valleys, 1 on crests) to height, at full
    /// ruggedness.
    peak_height: Spline,
}

impl TerrainShape {
    /// The world's seed.
    pub fn seed(&self) -> u64 {
        self.settings.seed
    }

    pub fn new(settings: TerrainSettings) -> Self {
        let seed = settings.seed;
        Self {
            // The hills keep the world's own seed, so they're as they were.
            hills: Perlin::new(seed),
            continents: Perlin::new(seed ^ 0xC0A7_1E47),
            erosion: Perlin::new(seed ^ 0xE205_1011),
            peaks: Perlin::new(seed ^ 0x9EA6_5A11),
            continental_height: Spline::new([
                knot(-1.0, -45.0, 0.0),
                knot(-0.5, -25.0, 40.0),
                // Plains, flat across the middle.
                knot(-0.15, 0.0, 0.0),
                knot(0.1, 0.0, 0.0),
                knot(0.4, 40.0, 120.0),
                knot(0.7, 85.0, 100.0),
                knot(1.0, 110.0, 0.0),
            ]),
            mountainous: Spline::new([
                knot(-0.6, 0.25, 0.0),
                knot(0.0, 0.6, 0.8),
                knot(0.6, 1.0, 0.0),
            ]),
            ruggedness: Spline::new([
                knot(-0.7, 1.0, 0.0),
                knot(-0.35, 0.75, -1.5),
                knot(0.0, 0.0, -0.5),
                // Worn smooth from here up.
                knot(0.3, 0.0, 0.0),
            ]),
            peak_height: Spline::new([
                knot(0.0, -35.0, 0.0),
                knot(0.4, -10.0, 50.0),
                knot(0.7, 30.0, 150.0),
                // The crest: steep on both sides, so it comes to a point.
                knot(1.0, 150.0, 350.0),
            ]),
            settings,
        }
    }

    /// The height of the ground in the column at `(x, z)`: its top solid
    /// voxel is at `y = height - 1`. Chunks round [`Self::surface`]
    /// themselves, since they also need it unrounded.
    #[cfg(test)]
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.surface(x, z).round() as i32
    }

    /// The ground's height at `(x, z)` before it's rounded to whole blocks,
    /// for measuring slopes smoothly.
    pub fn surface(&self, x: i32, z: i32) -> f32 {
        let s = &self.settings;
        // Sample at voxel centers so lattice points (where noise is 0) don't
        // line up with voxel edges.
        let (x, z) = (x as f32 + 0.5, z as f32 + 0.5);
        let hills = self.hills.fbm(x / s.scale, z / s.scale, s.octaves);
        let stretched = |noise: &Perlin, scale: f32, octaves: u32| {
            (noise.fbm(x / scale, z / scale, octaves) * STRETCH).clamp(-1.0, 1.0)
        };
        let continents = stretched(&self.continents, s.continent_scale, 4);
        let erosion = stretched(&self.erosion, s.erosion_scale, 3);
        let ridges = 1.0 - stretched(&self.peaks, s.peak_scale, 4).abs();

        let rugged = self.ruggedness.at(erosion).clamp(0.0, 1.0)
            * self.mountainous.at(continents).clamp(0.0, 1.0);
        let shaped = self.continental_height.at(continents) + rugged * self.peak_height.at(ridges);
        let calm = smoothstep(CALM_RADIUS, FULL_RADIUS, x.hypot(z));
        s.base_height + calm * shaped + hills * s.amplitude
    }

    /// Whether a column `height` tall is bare rock: high up, or a cliff.
    /// `neighbours` are the unrounded heights ([`Self::surface`]) of the
    /// columns next to it, in [`NEIGHBOURS`] order.
    pub fn is_rock(&self, height: i32, neighbours: [f32; 4]) -> bool {
        let [east, west, south, north] = neighbours;
        let slope = (east - west).hypot(south - north) / 2.0;
        height >= self.settings.base_height as i32 + ROCK_LINE || slope >= CLIFF
    }
}

/// 0 up to `low`, 1 from `high`, and a smooth S between.
pub(super) fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> TerrainShape {
        TerrainShape::new(TerrainSettings::default())
    }

    /// Columns sampled every `step` blocks over a square `size` blocks
    /// across, around `(x0, z0)`.
    fn sample(x0: i32, z0: i32, size: i32, step: usize) -> impl Iterator<Item = (i32, i32)> {
        (x0 - size / 2..x0 + size / 2)
            .step_by(step)
            .flat_map(move |x| {
                (z0 - size / 2..z0 + size / 2)
                    .step_by(step)
                    .map(move |z| (x, z))
            })
    }

    #[test]
    fn there_are_mountains_and_valleys() {
        let shape = shape();
        let base = TerrainSettings::default().base_height as i32;
        let heights: Vec<i32> = sample(0, 0, 8000, 16)
            .map(|(x, z)| shape.height(x, z))
            .collect();
        let peaks = heights.iter().filter(|&&h| h >= base + 150).count();
        let lowlands = heights.iter().filter(|&&h| h <= base - 20).count();
        let plains = heights
            .iter()
            .filter(|&&h| (base - 10..=base + 30).contains(&h))
            .count();
        let n = heights.len();
        // Some of each, and the plains are common.
        assert!(peaks * 200 > n, "{peaks} of {n} columns are peaks");
        assert!(lowlands * 50 > n, "{lowlands} of {n} columns are lowlands");
        assert!(plains * 4 > n, "{plains} of {n} columns are plains");
        let (min, max) = (heights.iter().min().unwrap(), heights.iter().max().unwrap());
        assert!(max - min >= 250, "heights only span {min}..={max}");
    }

    /// Mountainsides are steep but the ground never jumps a whole chunk
    /// between neighbouring columns.
    #[test]
    fn slopes_are_steep_but_not_walls() {
        let shape = shape();
        let mut steepest = 0;
        for (x, z) in sample(1500, -400, 1200, 3) {
            let h = shape.height(x, z);
            for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                steepest = steepest.max((shape.height(nx, nz) - h).abs());
            }
        }
        assert!(steepest >= 8, "the steepest step is {steepest}");
        assert!(steepest <= 16, "a step of {steepest}");
    }

    /// The land around the start is the plain hills it always was.
    #[test]
    fn the_start_is_calm() {
        let shape = shape();
        let s = TerrainSettings::default();
        let hills = Perlin::new(s.seed);
        let calm = (CALM_RADIUS / SQRT_2) as i32;
        for (x, z) in sample(0, 0, calm * 2, 7) {
            let (fx, fz) = (x as f32 + 0.5, z as f32 + 0.5);
            let plain =
                s.base_height + hills.fbm(fx / s.scale, fz / s.scale, s.octaves) * s.amplitude;
            assert_eq!(shape.height(x, z), plain.round() as i32, "at ({x}, {z})");
        }
    }

    #[test]
    fn peaks_and_cliffs_are_bare_rock() {
        let shape = shape();
        let base = TerrainSettings::default().base_height as i32;
        let b = base as f32;
        let flat = |h: i32| [h as f32; 4];
        assert!(!shape.is_rock(base, flat(base)));
        assert!(shape.is_rock(base + ROCK_LINE, flat(base + ROCK_LINE)));
        assert!(!shape.is_rock(base + ROCK_LINE - 1, flat(base + ROCK_LINE - 1)));
        // East to west across the column: 9 blocks over 2 is a cliff, 8.9
        // isn't; and the plain hills' steepest, 3 a block each way, isn't.
        assert!(shape.is_rock(base, [b + 5.0, b - 4.0, b, b]));
        assert!(!shape.is_rock(base, [b + 4.9, b - 4.0, b, b]));
        assert!(!shape.is_rock(base, [b + 3.0, b - 3.0, b + 3.0, b - 3.0]));
        // Diagonal: 3.5 a block both ways, about 4.95.
        assert!(shape.is_rock(base, [b + 7.0, b, b + 7.0, b]));
    }

    /// The land eases from the plain hills into the shaped terrain: walking
    /// out from the start in any direction, no step between neighbouring
    /// columns is a wall.
    #[test]
    fn the_edge_of_the_calm_land_is_smooth() {
        let shape = shape();
        let mut steepest = 0;
        for step in 0..720 {
            let angle = (step as f32 * 0.5).to_radians();
            let (dx, dz) = (angle.cos(), angle.sin());
            let mut last: Option<((i32, i32), i32)> = None;
            for r in (CALM_RADIUS as i32 - 50)..(FULL_RADIUS as i32 + 50) {
                let column = ((dx * r as f32) as i32, (dz * r as f32) as i32);
                if last.is_some_and(|(at, _)| at == column) {
                    continue;
                }
                let h = shape.height(column.0, column.1);
                if let Some((_, before)) = last {
                    steepest = steepest.max((h - before).abs());
                }
                last = Some((column, h));
            }
        }
        assert!(steepest <= 16, "a step of {steepest}");
    }

    #[test]
    fn smoothstep_eases_between_its_edges() {
        assert_eq!(smoothstep(1.0, 3.0, 0.0), 0.0);
        assert_eq!(smoothstep(1.0, 3.0, 2.0), 0.5);
        assert_eq!(smoothstep(1.0, 3.0, 5.0), 1.0);
    }
}
