//! Grayscale layer images, the palettes that colorize them, and stacking
//! colorized layers into one tile.
//!
//! A layer image uses only the [`GRAY_LEVELS`] shades of [`gray`], plus
//! fully or partly transparent pixels. Each shade is an index into a
//! [`Palette`]: the darkest gray takes the palette's first color, the
//! lightest its last. So one grayscale image can be dirt in one tile and
//! sand in another, and art from different sources shares one shading
//! scheme.

use bevy::{
    color::{ColorToPacked, Mix},
    prelude::*,
    render::render_resource::TextureFormat,
};

use super::TILE_SIZE;

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

/// Colors the grayscale `image` with `palette`, in linear color. Fails if
/// the image isn't [`TILE_SIZE`] wide and a whole number of tiles high (a
/// strip of animation frames, the first on top), or uses a color that
/// isn't one of the gray levels. Fully transparent pixels may hold any
/// color. Each [`TILE_SIZE`]-square run of the result is one frame.
pub fn colorize(image: &Image, palette: &Palette) -> Result<Vec<LinearRgba>, String> {
    let format = image.texture_descriptor.format;
    if !matches!(
        format,
        TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm
    ) {
        return Err(format!("must decode to RGBA8, not {format:?}"));
    }
    let (width, height) = (image.width(), image.height());
    if width != TILE_SIZE || height == 0 || !height.is_multiple_of(TILE_SIZE) {
        return Err(format!(
            "is {width}×{height}, expected {TILE_SIZE}×{TILE_SIZE}, or {TILE_SIZE} wide \
             with frames {TILE_SIZE} high stacked"
        ));
    }
    let pixels = image.data.as_deref().ok_or("has no pixel data")?;
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(i, &[r, g, b, a])| {
            if a == 0 {
                return Ok(LinearRgba::NONE);
            }
            let (x, y) = (i as u32 % width, i as u32 / width);
            if r != g || g != b {
                return Err(format!("pixel ({x}, {y}) is ({r}, {g}, {b}), not gray"));
            }
            let level = level_of(r).ok_or_else(|| {
                let levels: Vec<u8> = (0..GRAY_LEVELS).map(gray).collect();
                format!("pixel ({x}, {y}) is gray {r}, not one of the levels {levels:?}")
            })?;
            let color = LinearRgba::from(palette.color(level));
            Ok(color.with_alpha(color.alpha * a as f32 / 255.0))
        })
        .collect()
}

/// Stacks colorized layers (one frame of each), bottom first, with
/// ordinary "over" blending in linear color. Returns sRGB RGBA8 pixels.
pub fn stack(layers: &[&[LinearRgba]]) -> Vec<u8> {
    let len = (TILE_SIZE * TILE_SIZE) as usize;
    // Premultiplied while blending.
    let mut out = vec![LinearRgba::NONE; len];
    for layer in layers {
        for (dst, src) in out.iter_mut().zip(layer.iter()) {
            let premultiplied = (*src * src.alpha).with_alpha(src.alpha);
            *dst = premultiplied + *dst * (1.0 - src.alpha);
        }
    }
    out.iter()
        .flat_map(|&pixel| {
            let straight = if pixel.alpha > 0.0 {
                (pixel / pixel.alpha).with_alpha(pixel.alpha)
            } else {
                LinearRgba::NONE
            };
            Srgba::from(straight).to_u8_array()
        })
        .collect()
}

/// Test images in the layer format, for this module's tests and others'.
#[cfg(test)]
pub(crate) mod test_images {
    use super::{TILE_SIZE, gray};
    use bevy::{
        asset::RenderAssetUsages,
        prelude::*,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    };

    /// A [`TILE_SIZE`] square RGBA8 image filled by `paint(x, y)`.
    pub fn image(paint: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let data = (0..TILE_SIZE)
            .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
            .flat_map(|(x, y)| paint(x, y))
            .collect();
        Image::new(
            Extent3d {
                width: TILE_SIZE,
                height: TILE_SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::all(),
        )
    }

    /// A [`TILE_SIZE`] square image of one gray level everywhere.
    pub fn flat_gray(level: usize) -> Image {
        let v = gray(level);
        image(|_, _| [v, v, v, 255])
    }

    /// A strip of animation frames, frame `i` all gray level `levels[i]`.
    pub fn strip(levels: &[usize]) -> Image {
        let data = levels
            .iter()
            .flat_map(|&level| {
                let v = gray(level);
                std::iter::repeat_n([v, v, v, 255], (TILE_SIZE * TILE_SIZE) as usize)
            })
            .flatten()
            .collect();
        Image::new(
            Extent3d {
                width: TILE_SIZE,
                height: TILE_SIZE * levels.len() as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::all(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_images::image;
    use super::*;
    use bevy::render::render_resource::Extent3d;

    fn gray_pixel(level: usize) -> [u8; 4] {
        let v = gray(level);
        [v, v, v, 255]
    }

    /// One distinct, easy-to-spot color per level.
    fn test_palette() -> Palette {
        Palette(std::array::from_fn(|level| {
            Srgba::rgb_u8(level as u8 * 30, 255 - level as u8 * 30, 7)
        }))
    }

    #[test]
    fn gray_levels_span_black_to_white_evenly() {
        let levels: Vec<u8> = (0..GRAY_LEVELS).map(gray).collect();
        assert_eq!(levels, [0, 36, 73, 109, 146, 182, 219, 255]);
        for level in 0..GRAY_LEVELS {
            assert_eq!(level_of(gray(level)), Some(level));
        }
        assert_eq!(level_of(100), None);
    }

    #[test]
    fn ramp_hits_its_end_stops_and_passes_through_the_middle() {
        let black = Srgba::rgb_u8(0, 0, 0);
        let red = Srgba::rgb_u8(255, 0, 0);
        let white = Srgba::rgb_u8(255, 255, 255);
        let palette = Palette::ramp(&[black, red, white]);
        let close = |a: Srgba, b: Srgba| (a.to_u8_array()[..3]) == (b.to_u8_array()[..3]);
        assert!(close(palette.color(0), black));
        assert!(close(palette.color(GRAY_LEVELS - 1), white));
        // Each level is lighter than the one below it.
        for level in 1..GRAY_LEVELS {
            let lightness = |l: usize| Oklaba::from(palette.color(l)).lightness;
            assert!(lightness(level) > lightness(level - 1), "level {level}");
        }
        // With 8 levels and 3 stops, the middle stop falls between levels 3
        // and 4, which are both reddish.
        for level in [3, 4] {
            let [r, g, b, _] = palette.color(level).to_u8_array();
            assert!(r > g && r > b, "level {level} is {r} {g} {b}");
        }
    }

    /// Levels are blended in Oklab, not sRGB: a black-to-white ramp steps
    /// evenly in Oklab lightness.
    #[test]
    fn ramp_blends_in_oklab() {
        let palette = Palette::ramp(&[Srgba::BLACK, Srgba::WHITE]);
        for level in 0..GRAY_LEVELS {
            let lightness = Oklaba::from(palette.color(level)).lightness;
            let expected = level as f32 / (GRAY_LEVELS - 1) as f32;
            assert!(
                (lightness - expected).abs() < 1e-3,
                "level {level}: {lightness} != {expected}"
            );
        }
    }

    #[test]
    fn colorize_maps_each_gray_level_to_its_palette_color() {
        let palette = test_palette();
        let layer = image(|x, _| gray_pixel(x as usize % GRAY_LEVELS));
        let colors = colorize(&layer, &palette).unwrap();
        for (x, color) in colors.iter().take(TILE_SIZE as usize).enumerate() {
            assert_eq!(
                Srgba::from(*color).to_u8_array(),
                palette.color(x % GRAY_LEVELS).to_u8_array()
            );
        }
    }

    #[test]
    fn colorize_keeps_transparency() {
        let layer = image(|x, _| {
            if x == 0 {
                [99, 1, 2, 0]
            } else {
                [255, 255, 255, 128]
            }
        });
        let colors = colorize(&layer, &test_palette()).unwrap();
        assert_eq!(colors[0].alpha, 0.0);
        assert!((colors[1].alpha - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn colorize_rejects_colors_and_off_palette_grays() {
        let colored = image(|x, y| {
            if (x, y) == (3, 5) {
                [10, 20, 30, 255]
            } else {
                gray_pixel(0)
            }
        });
        let error = colorize(&colored, &test_palette()).unwrap_err();
        assert!(
            error.contains("(3, 5)") && error.contains("not gray"),
            "{error}"
        );

        let off = image(|x, y| {
            if (x, y) == (1, 2) {
                [100, 100, 100, 255]
            } else {
                gray_pixel(0)
            }
        });
        let error = colorize(&off, &test_palette()).unwrap_err();
        assert!(
            error.contains("(1, 2)") && error.contains("gray 100"),
            "{error}"
        );
    }

    #[test]
    fn colorize_rejects_the_wrong_size() {
        let mut big = image(|_, _| gray_pixel(0));
        big.resize(Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        });
        assert!(
            colorize(&big, &test_palette())
                .unwrap_err()
                .contains("32×32")
        );
        // Tall, but not a whole number of frames.
        big.resize(Extent3d {
            width: 16,
            height: 24,
            depth_or_array_layers: 1,
        });
        assert!(
            colorize(&big, &test_palette())
                .unwrap_err()
                .contains("16×24")
        );
    }

    /// A strip of frames, 16 wide and a whole number of frames high,
    /// colorizes whole: frame after frame, the first on top.
    #[test]
    fn colorize_takes_a_strip_of_frames() {
        let strip = test_images::strip(&[1, 6, 3]);
        let colors = colorize(&strip, &test_palette()).unwrap();
        let len = (TILE_SIZE * TILE_SIZE) as usize;
        assert_eq!(colors.len(), 3 * len);
        for (frame, level) in [1, 6, 3].into_iter().enumerate() {
            let expected = LinearRgba::from(test_palette().color(level));
            assert_eq!(colors[frame * len], expected, "frame {frame}");
            assert_eq!(colors[frame * len + len - 1], expected, "frame {frame}");
        }
    }

    #[test]
    fn upper_layers_cover_lower_ones_where_opaque() {
        let red = LinearRgba::rgb(1.0, 0.0, 0.0);
        let blue = LinearRgba::rgb(0.0, 0.0, 1.0);
        let len = (TILE_SIZE * TILE_SIZE) as usize;
        let bottom = vec![red; len];
        // Blue over the first pixel, half-transparent blue over the second,
        // nothing over the rest.
        let mut top = vec![LinearRgba::NONE; len];
        top[0] = blue;
        top[1] = blue.with_alpha(0.5);
        let out = stack(&[&bottom, &top]);
        assert_eq!(out[0..4], [0, 0, 255, 255]);
        // An even linear mix of red and blue is 188 in sRGB, not 128.
        assert_eq!(out[4..8], [188, 0, 188, 255]);
        assert_eq!(out[8..12], [255, 0, 0, 255]);
    }
}
