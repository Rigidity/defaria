//! 2D Perlin noise ("improved" Perlin: quintic fade, hashed gradients) plus
//! fractal Brownian motion built from it. Pure math, no Bevy.

/// Seeded 2D Perlin noise. The same seed always gives the same noise.
#[derive(Debug, Clone)]
pub struct Perlin {
    /// The shuffled permutation of `0..256`, repeated twice so lookups of
    /// `perm[perm[x] + y]` never need wrapping.
    perm: [u8; 512],
}

impl Perlin {
    pub fn new(seed: u64) -> Self {
        let mut table: [u8; 256] = std::array::from_fn(|i| i as u8);
        // Fisher-Yates shuffle driven by SplitMix64.
        let mut state = seed;
        for i in (1..table.len()).rev() {
            let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
            table.swap(i, j);
        }
        Self {
            perm: std::array::from_fn(|i| table[i & 255]),
        }
    }

    /// Noise at `(x, y)`, in `[-1, 1]`. It is exactly 0 on integer lattice
    /// points and varies smoothly between them, repeating every 256 units.
    pub fn get(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        // `& 255` on two's complement wraps negatives correctly too.
        let (xi, yi) = ((x0 as i32 & 255) as usize, (y0 as i32 & 255) as usize);

        let hash = |dx: usize, dy: usize| self.perm[self.perm[xi + dx] as usize + yi + dy];
        let n00 = gradient(hash(0, 0), fx, fy);
        let n10 = gradient(hash(1, 0), fx - 1.0, fy);
        let n01 = gradient(hash(0, 1), fx, fy - 1.0);
        let n11 = gradient(hash(1, 1), fx - 1.0, fy - 1.0);

        let (u, v) = (fade(fx), fade(fy));
        lerp(v, lerp(u, n00, n10), lerp(u, n01, n11))
    }

    /// Fractal Brownian motion: `octaves` layers of noise, each at twice the
    /// frequency and half the amplitude of the last. Normalized to `[-1, 1]`.
    pub fn fbm(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let (mut sum, mut total, mut amplitude, mut frequency) = (0.0, 0.0, 1.0, 1.0);
        for _ in 0..octaves {
            sum += amplitude * self.get(x * frequency, y * frequency);
            total += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        if total > 0.0 { sum / total } else { 0.0 }
    }
}

/// Dot product of the offset with one of four diagonal gradients `(±1, ±1)`.
/// Diagonals keep the output range at exactly `[-1, 1]`.
fn gradient(hash: u8, x: f32, y: f32) -> f32 {
    match hash & 3 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        _ => -x - y,
    }
}

/// Perlin's quintic `6t⁵ - 15t⁴ + 10t³`: zero first and second derivatives at
/// 0 and 1, so cells join without visible creases.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

pub(crate) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of sample points that covers negative coordinates and several
    /// lattice cells at sub-cell resolution.
    fn samples() -> impl Iterator<Item = (f32, f32)> {
        (-80..80).flat_map(|i| (-80..80).map(move |j| (i as f32 * 0.137, j as f32 * 0.173)))
    }

    #[test]
    fn permutation_is_a_permutation() {
        let perlin = Perlin::new(7);
        let mut seen = [false; 256];
        for &p in &perlin.perm[..256] {
            seen[p as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(perlin.perm[..256], perlin.perm[256..]);
    }

    #[test]
    fn zero_on_lattice_points() {
        let perlin = Perlin::new(1);
        for x in -5..5 {
            for y in -5..5 {
                assert_eq!(perlin.get(x as f32, y as f32), 0.0);
            }
        }
    }

    #[test]
    fn stays_in_range_and_is_not_flat() {
        let perlin = Perlin::new(2);
        let (mut min, mut max) = (f32::MAX, f32::MIN);
        for (x, y) in samples() {
            let n = perlin.get(x, y);
            assert!((-1.0..=1.0).contains(&n), "noise({x}, {y}) = {n}");
            let f = perlin.fbm(x, y, 4);
            assert!((-1.0..=1.0).contains(&f), "fbm({x}, {y}) = {f}");
            min = min.min(n);
            max = max.max(n);
        }
        assert!(min < -0.4 && max > 0.4, "range was {min}..{max}");
    }

    #[test]
    fn is_continuous() {
        let perlin = Perlin::new(3);
        let step = 1e-3;
        for (x, y) in samples() {
            let n = perlin.get(x, y);
            // The gradient is bounded (under 4 for these gradients), so a tiny
            // step can only move the value a little.
            assert!((perlin.get(x + step, y) - n).abs() < 4.0 * step);
            assert!((perlin.get(x, y + step) - n).abs() < 4.0 * step);
        }
    }

    /// No creases at cell borders: the slope just left of a lattice line
    /// matches the slope just right of it. (A linear fade would leave the
    /// noise continuous but kinked there.)
    #[test]
    fn slope_is_smooth_across_cell_borders() {
        let perlin = Perlin::new(5);
        let (h, eps) = (1e-3_f32, 1e-2_f32);
        for border in -3..3 {
            for y in [0.21, 0.5, 0.77, 1.33] {
                let x = border as f32;
                let left = (perlin.get(x - eps, y) - perlin.get(x - eps - h, y)) / h;
                let right = (perlin.get(x + eps + h, y) - perlin.get(x + eps, y)) / h;
                assert!(
                    (left - right).abs() < 0.1,
                    "kink at x = {x}, y = {y}: slope {left} vs {right}"
                );
            }
        }
    }

    #[test]
    fn same_seed_same_noise_different_seed_different_noise() {
        let (a, b, c) = (Perlin::new(42), Perlin::new(42), Perlin::new(43));
        let mut differs = false;
        for (x, y) in samples() {
            assert_eq!(a.get(x, y), b.get(x, y));
            differs |= a.get(x, y) != c.get(x, y);
        }
        assert!(differs);
    }

    #[test]
    fn repeats_every_256_units() {
        let perlin = Perlin::new(4);
        assert_eq!(perlin.get(3.25, 7.5), perlin.get(3.25 + 256.0, 7.5 - 256.0));
    }
}
