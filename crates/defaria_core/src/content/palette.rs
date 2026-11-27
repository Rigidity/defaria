//! Material palettes: the colors that a texture's grayscale layers take,
//! one per gray level (see `block_texture` for the layers themselves).

use bevy::{color::Mix, prelude::*};

/// Shades of gray a layer image may use.
pub const GRAY_LEVELS: usize = 8;

/// The gray value of `level`, from 0 (black) to `GRAY_LEVELS - 1` (white),
/// evenly spaced and rounded to the nearest byte.
pub const fn gray(level: usize) -> u8 {
    let steps = GRAY_LEVELS - 1;
    ((level * 255 + steps / 2) / steps) as u8
}

/// The level whose gray value is `value`, if any.
pub fn level_of(value: u8) -> Option<usize> {
    (0..GRAY_LEVELS).find(|&level| gray(level) == value)
}

/// One color per gray level, darkest level first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette(pub [Srgba; GRAY_LEVELS]);

impl Palette {
    /// A palette that runs through `stops`, spaced evenly from the darkest
    /// level to the lightest and blended in Oklab between them, so the
    /// steps look even and hues stay clean.
    pub fn ramp(stops: &[Srgba]) -> Self {
        assert!(stops.len() >= 2, "a ramp needs at least two stops");
        Self(std::array::from_fn(|level| {
            let at = level as f32 / (GRAY_LEVELS - 1) as f32 * (stops.len() - 1) as f32;
            let from = (at.floor() as usize).min(stops.len() - 2);
            let (a, b) = (Oklaba::from(stops[from]), Oklaba::from(stops[from + 1]));
            Srgba::from(a.mix(&b, at - from as f32))
        }))
    }

    pub fn color(&self, level: usize) -> Srgba {
        self.0[level]
    }
}
