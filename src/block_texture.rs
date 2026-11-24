//! The block tiles, and the texture the chunk shader samples them from.
//!
//! Each tile is a stack of layers, bottom first. A layer is a grayscale
//! 16×16 PNG in [`LAYER_DIR`] plus a [`Palette`] that gives each of its
//! gray levels a color (see [`layer`]). One image can appear in several
//! tiles with different palettes. The stacks are hard-coded in
//! [`Tile::layers`] for now; they'll move to data files later.
//!
//! [`build`] colorizes and stacks every tile's layers, then turns the tiles
//! into a 2D array texture with one layer per tile and a full mip chain per
//! layer. Array layers instead of one big atlas, so smaller mips never blend
//! a tile with its neighbour. Mips are box-filtered in linear color, so a
//! distant checkerboard of black and white averages to the right mid grey
//! instead of a too-dark one. (Bevy 0.20's GPU mip generation needs a
//! storage-capable format, which sRGB RGBA8 isn't, and these mips only need
//! building when a layer changes.)

pub mod layer;

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor,
        TextureViewDimension,
    },
};

pub use layer::Palette;

/// The layer images' folder, relative to `assets/`.
pub const LAYER_DIR: &str = "textures/layers";
/// Width and height of one tile, and of every layer image, in pixels.
pub const TILE_SIZE: u32 = 16;
/// Anisotropic filtering samples at most. Keeps textures sharp on surfaces
/// seen at a grazing angle, where plain mipmapping would blur them.
const MAX_ANISOTROPY: u16 = 16;

/// One tile of the block texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Stone,
    Dirt,
    GrassTop,
    GrassSide,
}

impl Tile {
    /// Every tile, in layer order.
    pub const ALL: [Tile; 4] = [Tile::Stone, Tile::Dirt, Tile::GrassTop, Tile::GrassSide];

    /// The tile's layer in the block texture. Exhaustive, so a new tile
    /// can't compile without one.
    pub fn layer(self) -> u32 {
        match self {
            Tile::Stone => 0,
            Tile::Dirt => 1,
            Tile::GrassTop => 2,
            Tile::GrassSide => 3,
        }
    }

    /// The layers stacked to make the tile, bottom first.
    pub fn layers(self) -> Vec<Layer> {
        match self {
            Tile::Stone => vec![Layer::new("stone", palettes::stone())],
            Tile::Dirt => vec![Layer::new("dirt", palettes::dirt())],
            Tile::GrassTop => vec![Layer::new("grass", palettes::grass())],
            Tile::GrassSide => vec![
                Layer::new("dirt", palettes::dirt()),
                Layer::new("grass_side", palettes::grass()),
            ],
        }
    }
}

/// A grayscale image and the palette that colors it.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The image's file name in [`LAYER_DIR`], without `.png`.
    pub image: &'static str,
    pub palette: Palette,
}

impl Layer {
    pub fn new(image: &'static str, palette: Palette) -> Self {
        Self { image, palette }
    }

    /// The image's asset path, relative to `assets/`.
    pub fn path(&self) -> String {
        layer_path(self.image)
    }
}

/// The asset path of layer image `name`.
pub fn layer_path(name: &str) -> String {
    format!("{LAYER_DIR}/{name}.png")
}

/// Every layer image some tile uses, as asset paths, each once.
pub fn layer_paths() -> Vec<String> {
    let mut paths: Vec<String> = Tile::ALL
        .iter()
        .flat_map(|tile| tile.layers())
        .map(|layer| layer.path())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The tiles' colors. Each ramp runs dark to light; [`Palette::ramp`]
/// spreads its stops over the gray levels.
mod palettes {
    use super::Palette;
    use bevy::color::Srgba;

    /// Low contrast: the stone layer carries a lot of fine detail, which
    /// looks grainy at full contrast.
    pub fn stone() -> Palette {
        Palette::ramp(&[
            Srgba::rgb_u8(0x33, 0x35, 0x3d),
            Srgba::rgb_u8(0x52, 0x53, 0x5a),
            Srgba::rgb_u8(0x6f, 0x6f, 0x74),
            Srgba::rgb_u8(0x95, 0x93, 0x8f),
        ])
    }

    /// Low contrast, like stone: the dirt layer is fine noise.
    pub fn dirt() -> Palette {
        Palette::ramp(&[
            Srgba::rgb_u8(0x3d, 0x28, 0x18),
            Srgba::rgb_u8(0x57, 0x3a, 0x24),
            Srgba::rgb_u8(0x6f, 0x4c, 0x31),
            Srgba::rgb_u8(0x8a, 0x61, 0x41),
        ])
    }

    /// Bright and saturated, shifting from blue-green shadows to
    /// yellow-green highlights, over a low-contrast range.
    pub fn grass() -> Palette {
        Palette::ramp(&[
            Srgba::rgb_u8(0x3a, 0x7a, 0x28),
            Srgba::rgb_u8(0x51, 0x9a, 0x2d),
            Srgba::rgb_u8(0x6b, 0xb5, 0x35),
            Srgba::rgb_u8(0x8f, 0xce, 0x45),
        ])
    }
}

/// Builds the block texture. `image` looks up a loaded layer image by its
/// asset path; every path in [`layer_paths`] must resolve.
pub fn build<'a>(image: impl Fn(&str) -> Option<&'a Image>) -> Result<Image, String> {
    let tiles = Tile::ALL
        .iter()
        .map(|&tile| compose(tile, &image).map_err(|error| format!("{tile:?}: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(array_texture(&tiles))
}

/// Colorizes and stacks one tile's layers into sRGB RGBA8 pixels.
fn compose<'a>(tile: Tile, image: impl Fn(&str) -> Option<&'a Image>) -> Result<Vec<u8>, String> {
    let layers = tile
        .layers()
        .iter()
        .map(|layer| {
            let path = layer.path();
            let image = image(&path).ok_or_else(|| format!("{path} isn't loaded"))?;
            layer::colorize(image, &layer.palette).map_err(|error| format!("{path} {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(layer::stack(&layers))
}

/// A 2D array texture with a full mip chain, one array layer per entry of
/// `tiles` (each [`TILE_SIZE`] square, sRGB RGBA8).
fn array_texture(tiles: &[Vec<u8>]) -> Image {
    let mut data = Vec::new();
    for tile in tiles {
        let mut level = tile.clone();
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
            depth_or_array_layers: tiles.len() as u32,
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
    image
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
                        Srgba::gamma_function(value)
                    };
                }
                let mean = sum / 4.0;
                let value = if channel == 3 {
                    mean
                } else {
                    Srgba::gamma_function_inverse(mean)
                };
                out.push((value * 255.0).round() as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        color::ColorToPacked,
        image::{CompressedImageFormats, ImageFilterMode, ImageType},
    };
    use layer::gray;
    use std::collections::HashMap;

    /// Tiles filled by `paint(tile, x, y)`, for [`array_texture`].
    fn tiles(paint: impl Fn(u32, u32, u32) -> [u8; 4]) -> Vec<Vec<u8>> {
        (0..Tile::ALL.len() as u32)
            .map(|i| {
                (0..TILE_SIZE)
                    .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
                    .flat_map(|(x, y)| paint(i, x, y))
                    .collect()
            })
            .collect()
    }

    /// Byte offset of (layer, mip) in layer-major data.
    fn offset(layer: u32, mip: u32) -> usize {
        let per_layer: u32 = (0..mip_levels(TILE_SIZE))
            .map(|m| (TILE_SIZE >> m).pow(2) * 4)
            .sum();
        let before: u32 = (0..mip).map(|m| (TILE_SIZE >> m).pow(2) * 4).sum();
        (layer * per_layer + before) as usize
    }

    /// The RGBA bytes of pixel (x, y) of (layer, mip).
    fn pixel(image: &Image, layer: u32, mip: u32, x: u32, y: u32) -> [u8; 4] {
        let at = offset(layer, mip) + ((y * (TILE_SIZE >> mip) + x) * 4) as usize;
        image.data.as_ref().unwrap()[at..at + 4].try_into().unwrap()
    }

    /// A layer image as the game loads it: decoded by Bevy's PNG loader.
    fn decode(bytes: &[u8]) -> Image {
        Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Default,
            RenderAssetUsages::all(),
        )
        .unwrap()
    }

    /// Every layer image on disk, decoded, by asset path.
    fn layers_on_disk() -> HashMap<String, Image> {
        layer_paths()
            .into_iter()
            .map(|path| {
                let bytes = std::fs::read(format!("assets/{path}"))
                    .unwrap_or_else(|error| panic!("assets/{path}: {error}"));
                let image = decode(&bytes);
                (path, image)
            })
            .collect()
    }

    /// A [`TILE_SIZE`] square image of one gray level everywhere.
    fn flat_gray(level: usize) -> Image {
        let v = gray(level);
        Image::new_fill(
            Extent3d {
                width: TILE_SIZE,
                height: TILE_SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[v, v, v, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::all(),
        )
    }

    #[test]
    fn tile_layers_follow_tile_order() {
        for (index, tile) in Tile::ALL.iter().enumerate() {
            assert_eq!(tile.layer(), index as u32);
        }
    }

    #[test]
    fn every_tile_has_layers_and_paths_are_listed_once() {
        for tile in Tile::ALL {
            assert!(!tile.layers().is_empty(), "{tile:?}");
        }
        let paths = layer_paths();
        assert!(paths.contains(&"textures/layers/dirt.png".to_string()));
        // Dirt is under both Dirt and GrassSide but loads once.
        assert_eq!(paths.iter().filter(|p| p.ends_with("/dirt.png")).count(), 1);
    }

    #[test]
    fn sixteen_pixels_make_five_mips() {
        assert_eq!(mip_levels(16), 5);
        assert_eq!(mip_levels(1), 1);
    }

    /// The real layer images decode the way the game loads them, are
    /// 16×16, use only the gray levels, and build.
    #[test]
    fn layers_on_disk_build() {
        let layers = layers_on_disk();
        let image = build(|path| layers.get(path)).unwrap();
        assert_eq!(
            image.texture_descriptor.size.depth_or_array_layers,
            Tile::ALL.len() as u32
        );
    }

    /// The grass overhang hangs from the top edge of the side face and lets
    /// the dirt show below it: every column is opaque from the top down to
    /// some depth and transparent below, and the bottom row is all dirt.
    #[test]
    fn grass_side_overhang_hangs_from_the_top() {
        let layers = layers_on_disk();
        let side = &layers[&layer_path("grass_side")];
        let pixels = side.data.as_ref().unwrap();
        let alpha = |x: u32, y: u32| pixels[((y * TILE_SIZE + x) * 4 + 3) as usize];
        for x in 0..TILE_SIZE {
            let depth = (0..TILE_SIZE).take_while(|&y| alpha(x, y) == 255).count() as u32;
            assert!(depth >= 1, "column {x} has no grass at the top");
            assert!(depth < TILE_SIZE, "column {x} hides the dirt completely");
            for y in depth..TILE_SIZE {
                assert_eq!(
                    alpha(x, y),
                    0,
                    "column {x} has grass below its overhang at {y}"
                );
            }
        }
        let image = build(|path| layers.get(path)).unwrap();
        let bottom = TILE_SIZE - 1;
        for x in 0..TILE_SIZE {
            assert_eq!(
                pixel(&image, Tile::GrassSide.layer(), 0, x, bottom),
                pixel(&image, Tile::Dirt.layer(), 0, x, bottom),
            );
        }
    }

    /// Every pixel of every built tile is opaque: the shader ignores alpha,
    /// so a hole would show the color hiding behind it.
    #[test]
    fn built_tiles_are_opaque() {
        let layers = layers_on_disk();
        let image = build(|path| layers.get(path)).unwrap();
        for tile in Tile::ALL {
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let alpha = pixel(&image, tile.layer(), 0, x, y)[3];
                    assert_eq!(alpha, 255, "{tile:?} ({x}, {y})");
                }
            }
        }
    }

    /// A tile's pixels come from its palettes: with every layer a flat
    /// gray, a one-layer tile is exactly that level's palette color.
    #[test]
    fn tiles_are_colored_by_their_palettes() {
        let images: HashMap<String, Image> = layer_paths()
            .into_iter()
            .map(|path| (path, flat_gray(3)))
            .collect();
        let image = build(|path| images.get(path)).unwrap();
        let expected = palettes::stone().color(3).to_u8_array();
        assert_eq!(pixel(&image, Tile::Stone.layer(), 0, 7, 7), expected);
        // Opaque grass over dirt hides the dirt.
        let grass = palettes::grass().color(3).to_u8_array();
        assert_eq!(pixel(&image, Tile::GrassSide.layer(), 0, 0, 0), grass);
    }

    #[test]
    fn a_missing_or_bad_layer_names_the_tile_and_file() {
        let error = build(|_| None).unwrap_err();
        assert!(
            error.contains("Stone") && error.contains("stone.png"),
            "{error}"
        );

        let mut red = flat_gray(0);
        red.data.as_mut().unwrap()[0] = 200;
        let error = build(|_| Some(&red)).unwrap_err();
        assert!(error.contains("not gray"), "{error}");
    }

    #[test]
    fn layers_hold_their_own_tile_and_full_mip_chains() {
        let image = array_texture(&tiles(|i, _, _| [i as u8 * 50, 7, 9, 255]));
        let size = image.texture_descriptor.size;
        assert_eq!(
            (size.width, size.height, size.depth_or_array_layers),
            (16, 16, 4)
        );
        assert_eq!(image.texture_descriptor.mip_level_count, 5);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(image.data_order, TextureDataOrder::LayerMajor);
        assert_eq!(image.data.as_ref().unwrap().len(), offset(4, 0));
        for layer in 0..4 {
            for mip in 0..5 {
                assert_eq!(
                    pixel(&image, layer, mip, 0, 0),
                    [layer as u8 * 50, 7, 9, 255]
                );
            }
        }
    }

    #[test]
    fn tile_pixels_keep_their_positions() {
        let image = array_texture(&tiles(|i, x, y| [i as u8, x as u8, y as u8, 255]));
        assert_eq!(pixel(&image, 1, 0, 5, 11), [1, 5, 11, 255]);
    }

    /// Mips shrink the tile, they don't flip or transpose it: a tile that's
    /// white on the left and black on the right stays that way.
    #[test]
    fn mips_keep_the_tile_the_right_way_round() {
        let image = array_texture(&tiles(|_, x, _| {
            let v = if x < TILE_SIZE / 2 { 255 } else { 0 };
            [v, v, v, 255]
        }));
        for mip in 1..4 {
            let last = (TILE_SIZE >> mip) - 1;
            assert_eq!(pixel(&image, 0, mip, 0, last)[0], 255, "mip {mip}");
            assert_eq!(pixel(&image, 0, mip, last, 0)[0], 0, "mip {mip}");
        }
    }

    /// Black and white average to 50% linear light, which is about 188 in
    /// sRGB, not the naive 128.
    #[test]
    fn mips_average_in_linear_color() {
        let image = array_texture(&tiles(|_, x, y| {
            let v = if (x + y).is_multiple_of(2) { 255 } else { 0 };
            [v, v, v, 255]
        }));
        for mip in 1..5 {
            assert_eq!(
                pixel(&image, 0, mip, 0, 0),
                [188, 188, 188, 255],
                "mip {mip}"
            );
        }
    }

    /// Alpha is already linear: half-transparent averages to 128, with no
    /// sRGB curve applied.
    #[test]
    fn mips_average_alpha_without_the_srgb_curve() {
        let image = array_texture(&tiles(|_, x, y| {
            let a = if (x + y).is_multiple_of(2) { 255 } else { 0 };
            [0, 0, 0, a]
        }));
        assert_eq!(pixel(&image, 0, 1, 0, 0)[3], 128);
    }

    #[test]
    fn samples_with_mipmaps_and_anisotropy_clamped_to_each_tile() {
        let image = array_texture(&tiles(|_, _, _| [0; 4]));
        let ImageSampler::Descriptor(sampler) = image.sampler else {
            panic!("no sampler");
        };
        assert_eq!(sampler.anisotropy_clamp, MAX_ANISOTROPY);
        assert_eq!(sampler.mipmap_filter, ImageFilterMode::Linear);
        assert_eq!(sampler.address_mode_u, ImageAddressMode::ClampToEdge);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::ClampToEdge);
        assert!(sampler.lod_max_clamp >= mip_levels(TILE_SIZE) as f32);
        let view = image.texture_view_descriptor.unwrap();
        assert_eq!(view.dimension, Some(TextureViewDimension::D2Array));
    }
}
