//! Turns the block atlas PNG into the texture the chunk shader samples: a 2D
//! array texture with one layer per tile and a full mip chain per layer.
//!
//! Layers instead of one big atlas, so smaller mips never blend a tile with
//! its neighbour. Mips are box-filtered in linear color, so a distant
//! checkerboard of black and white averages to the right mid grey instead of
//! a too-dark one.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor,
        TextureViewDimension,
    },
};

/// Width and height of one tile, in pixels.
pub const TILE_SIZE: u32 = 16;
/// Anisotropic filtering samples at most. Keeps textures sharp on surfaces
/// seen at a grazing angle, where plain mipmapping would blur them.
const MAX_ANISOTROPY: u16 = 16;

/// Builds the block texture from `atlas`, a single row of `layers` square
/// tiles of [`TILE_SIZE`] pixels in sRGB RGBA8.
pub fn build(atlas: &Image, layers: u32) -> Result<Image, String> {
    let format = atlas.texture_descriptor.format;
    if format != TextureFormat::Rgba8UnormSrgb {
        return Err(format!("atlas must be Rgba8UnormSrgb, not {format:?}"));
    }
    let (width, height) = (atlas.width(), atlas.height());
    if (width, height) != (TILE_SIZE * layers, TILE_SIZE) {
        return Err(format!(
            "atlas is {width}×{height}, expected {layers} tiles of {TILE_SIZE}×{TILE_SIZE} in a row"
        ));
    }
    let pixels = atlas.data.as_deref().ok_or("atlas has no pixel data")?;

    let mut data = Vec::new();
    for layer in 0..layers {
        let mut level = tile(pixels, width, layer);
        let mut size = TILE_SIZE;
        // Layer-major: every mip of this layer, largest first.
        data.extend_from_slice(&level);
        while size > 1 {
            level = downsample(&level, size);
            size /= 2;
            data.extend_from_slice(&level);
        }
    }

    let mut image = Image::new_uninit(
        Extent3d {
            width: TILE_SIZE,
            height: TILE_SIZE,
            depth_or_array_layers: layers,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.data_order = TextureDataOrder::LayerMajor;
    image.texture_descriptor.mip_level_count = mip_levels(TILE_SIZE);
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = ImageSampler::Descriptor(sampler());
    Ok(image)
}

/// Mip levels for a square texture of `size` pixels, down to 1×1.
pub fn mip_levels(size: u32) -> u32 {
    size.ilog2() + 1
}

/// Trilinear and anisotropic filtering, clamped at tile edges. Linear
/// magnification would blur close-up pixels, so the shader snaps its UVs to
/// texel centres and only blends across the one screen pixel at each texel
/// edge.
fn sampler() -> ImageSamplerDescriptor {
    let mut sampler = ImageSamplerDescriptor::default();
    sampler
        .set_address_mode(ImageAddressMode::ClampToEdge)
        .set_anisotropic_filter(MAX_ANISOTROPY);
    sampler
}

/// Copies tile `index` out of a row of tiles `width` pixels wide.
fn tile(pixels: &[u8], width: u32, index: u32) -> Vec<u8> {
    let row_bytes = (TILE_SIZE * 4) as usize;
    (0..TILE_SIZE)
        .flat_map(|y| {
            let start = ((y * width + index * TILE_SIZE) * 4) as usize;
            &pixels[start..start + row_bytes]
        })
        .copied()
        .collect()
}

/// Halves a square sRGB RGBA8 image, averaging each 2×2 block in linear
/// color. Alpha is linear already.
fn downsample(pixels: &[u8], size: u32) -> Vec<u8> {
    let half = size / 2;
    let mut out = Vec::with_capacity((half * half * 4) as usize);
    for y in 0..half {
        for x in 0..half {
            for channel in 0..4 {
                let mut sum = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (((2 * y + dy) * size + 2 * x + dx) * 4 + channel) as usize;
                    let value = pixels[i] as f32 / 255.0;
                    sum += if channel == 3 {
                        value
                    } else {
                        srgb_to_linear(value)
                    };
                }
                let mean = sum / 4.0;
                let value = if channel == 3 {
                    mean
                } else {
                    linear_to_srgb(mean)
                };
                out.push((value * 255.0).round() as u8);
            }
        }
    }
    out
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An atlas whose tile `i` is filled by `paint(i, x, y)`.
    fn atlas(layers: u32, paint: impl Fn(u32, u32, u32) -> [u8; 4]) -> Image {
        let width = TILE_SIZE * layers;
        let data = (0..TILE_SIZE)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| paint(x / TILE_SIZE, x % TILE_SIZE, y))
            .collect();
        Image::new(
            Extent3d {
                width,
                height: TILE_SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::all(),
        )
    }

    /// Byte offset of (layer, mip) in layer-major data.
    fn offset(layer: u32, mip: u32) -> usize {
        let per_layer: u32 = (0..mip_levels(TILE_SIZE))
            .map(|m| (TILE_SIZE >> m).pow(2) * 4)
            .sum();
        let before: u32 = (0..mip).map(|m| (TILE_SIZE >> m).pow(2) * 4).sum();
        (layer * per_layer + before) as usize
    }

    #[test]
    fn sixteen_pixels_make_five_mips() {
        assert_eq!(mip_levels(16), 5);
        assert_eq!(mip_levels(1), 1);
    }

    #[test]
    fn layers_hold_their_own_tile_and_full_mip_chains() {
        let image = build(&atlas(3, |i, _, _| [i as u8 * 50, 7, 9, 255]), 3).unwrap();
        let size = image.texture_descriptor.size;
        assert_eq!(
            (size.width, size.height, size.depth_or_array_layers),
            (16, 16, 3)
        );
        assert_eq!(image.texture_descriptor.mip_level_count, 5);
        let data = image.data.as_ref().unwrap();
        assert_eq!(data.len(), offset(3, 0));
        for layer in 0..3 {
            for mip in 0..5 {
                let at = offset(layer, mip);
                assert_eq!(data[at..at + 4], [layer as u8 * 50, 7, 9, 255]);
            }
        }
    }

    #[test]
    fn tile_pixels_keep_their_positions() {
        let paint = |i: u32, x: u32, y: u32| [i as u8, x as u8, y as u8, 255];
        let image = build(&atlas(2, paint), 2).unwrap();
        let data = image.data.as_ref().unwrap();
        let (x, y) = (5, 11);
        let at = offset(1, 0) + ((y * TILE_SIZE + x) * 4) as usize;
        assert_eq!(data[at..at + 4], [1, 5, 11, 255]);
    }

    /// Black and white average to 50% linear light, which is about 188 in
    /// sRGB, not the naive 128.
    #[test]
    fn mips_average_in_linear_color() {
        let checker = |_, x: u32, y: u32| {
            let v = if (x + y).is_multiple_of(2) { 255 } else { 0 };
            [v, v, v, 255]
        };
        let image = build(&atlas(1, checker), 1).unwrap();
        let data = image.data.as_ref().unwrap();
        for mip in 1..5 {
            let at = offset(0, mip);
            assert_eq!(data[at..at + 4], [188, 188, 188, 255], "mip {mip}");
        }
    }

    #[test]
    fn samples_with_mipmaps_and_anisotropy() {
        let image = build(&atlas(1, |_, _, _| [0; 4]), 1).unwrap();
        let ImageSampler::Descriptor(sampler) = image.sampler else {
            panic!("no sampler");
        };
        assert_eq!(sampler.anisotropy_clamp, MAX_ANISOTROPY);
        assert!(sampler.lod_max_clamp >= mip_levels(TILE_SIZE) as f32);
        let view = image.texture_view_descriptor.unwrap();
        assert_eq!(view.dimension, Some(TextureViewDimension::D2Array));
    }

    #[test]
    fn rejects_atlases_of_the_wrong_size() {
        assert!(build(&atlas(3, |_, _, _| [0; 4]), 4).is_err());
    }

    #[test]
    fn srgb_round_trips() {
        for v in 0..=255u8 {
            let c = v as f32 / 255.0;
            assert_eq!((linear_to_srgb(srgb_to_linear(c)) * 255.0).round() as u8, v);
        }
    }
}
