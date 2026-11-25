//! The block texture the chunk shader samples its tiles from.
//!
//! Each tile is a [`Texture`] from the mods: a stack of layers, bottom
//! first, each a grayscale 16×16 PNG plus a [`Palette`] that gives each of
//! its gray levels a color (see [`layer`]). One image can appear in several
//! tiles with different palettes.
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

use crate::content::Texture;
pub use layer::Palette;

/// Width and height of one tile, and of every layer image, in pixels.
pub const TILE_SIZE: u32 = 16;
/// Anisotropic filtering samples at most. Keeps textures sharp on surfaces
/// seen at a grazing angle, where plain mipmapping would blur them.
const MAX_ANISOTROPY: u16 = 16;

/// Builds the block texture: layer `i` is `textures[i]`. `image` looks up a
/// loaded layer image by its asset path. Returns the texture, the tiles it
/// was built from (see [`tiles`]) and one message per texture that failed.
pub fn build<'a>(
    textures: &[Texture],
    image: impl Fn(&str) -> Option<&'a Image>,
) -> (Image, Vec<Vec<u8>>, Vec<String>) {
    let (tiles, errors) = tiles(textures, image);
    (array_texture(&tiles), tiles, errors)
}

/// Colorizes and stacks every texture: tile `i` is `textures[i]`, as
/// [`TILE_SIZE`]-square sRGB RGBA8 pixels, rows top first.
///
/// A texture that can't be built (an image missing, or breaking the layer
/// rules) gets [`placeholder`] instead, so one bad image only marks its own
/// tiles, and the other tiles keep their places. Returns the tiles and one
/// message per texture that failed.
pub fn tiles<'a>(
    textures: &[Texture],
    image: impl Fn(&str) -> Option<&'a Image>,
) -> (Vec<Vec<u8>>, Vec<String>) {
    let mut errors = Vec::new();
    let tiles = textures
        .iter()
        .map(|texture| {
            compose(texture, &image).unwrap_or_else(|error| {
                errors.push(format!("{}: {error}", texture.id));
                placeholder()
            })
        })
        .collect();
    (tiles, errors)
}

/// The tile shown for a texture that couldn't be built: a magenta and black
/// checkerboard, which no real texture looks like.
pub fn placeholder() -> Vec<u8> {
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    (0..TILE_SIZE)
        .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            if (x / 4 + y / 4).is_multiple_of(2) {
                MAGENTA
            } else {
                BLACK
            }
        })
        .collect()
}

/// Colorizes and stacks one texture's layers into sRGB RGBA8 pixels.
fn compose<'a>(
    texture: &Texture,
    image: impl Fn(&str) -> Option<&'a Image>,
) -> Result<Vec<u8>, String> {
    let layers = texture
        .layers
        .iter()
        .map(|layer| {
            let path = &layer.image;
            let image = image(path).ok_or_else(|| format!("{path} isn't loaded"))?;
            layer::colorize(image, &layer.palette).map_err(|error| format!("{path} {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(layer::stack(&layers))
}

/// A 2D array texture with a full mip chain, one array layer per entry of
/// `tiles` (each [`TILE_SIZE`] square, sRGB RGBA8).
/// An empty list gets one [`placeholder`] layer, since an array texture
/// needs at least one.
fn array_texture(tiles: &[Vec<u8>]) -> Image {
    let placeholder = [placeholder()];
    let tiles = if tiles.is_empty() {
        &placeholder[..]
    } else {
        tiles
    };
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
    use crate::content::{BlockState, base};
    use bevy::{
        color::ColorToPacked,
        image::{CompressedImageFormats, ImageFilterMode, ImageType},
    };
    use layer::test_images::flat_gray;
    use std::collections::HashMap;

    /// Array layers in the test textures made by [`tiles`].
    const TILES: u32 = 4;

    /// Tiles filled by `paint(tile, x, y)`, for [`array_texture`].
    fn tiles(paint: impl Fn(u32, u32, u32) -> [u8; 4]) -> Vec<Vec<u8>> {
        (0..TILES)
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

    /// The array layer of base game texture `defaria:<name>`.
    fn tile(name: &str) -> u32 {
        base().texture(&format!("defaria:{name}")).unwrap()
    }

    /// The asset path of base game image `<name>`.
    fn image_path(name: &str) -> String {
        format!("mods/defaria/images/{name}.png")
    }

    fn material(name: &str) -> Palette {
        base().materials[&format!("defaria:{name}")]
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

    /// Every layer image the base game uses, decoded, by asset path.
    fn layers_on_disk() -> HashMap<String, Image> {
        base()
            .image_paths()
            .into_iter()
            .map(|path| {
                let bytes = std::fs::read(format!("assets/{path}"))
                    .unwrap_or_else(|error| panic!("assets/{path}: {error}"));
                let image = decode(&bytes);
                (path, image)
            })
            .collect()
    }

    fn build_base(layers: &HashMap<String, Image>) -> Image {
        let (image, _, errors) = build(&base().textures, |path| layers.get(path));
        assert!(errors.is_empty(), "{errors:?}");
        image
    }

    /// Every PNG in every mod's images, including ones no texture uses yet
    /// (leaves), decodes to a valid layer: 16×16, gray levels only.
    #[test]
    fn every_layer_file_is_valid() {
        fn pngs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pngs(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "png") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        pngs(std::path::Path::new("assets/mods"), &mut files);
        assert!(files.len() >= base().image_paths().len());
        let palette = material("stone");
        for file in files {
            let image = decode(&std::fs::read(&file).unwrap());
            if let Err(error) = layer::colorize(&image, &palette) {
                panic!("{}: {error}", file.display());
            }
        }
    }

    #[test]
    fn sixteen_pixels_make_five_mips() {
        assert_eq!(mip_levels(16), 5);
        assert_eq!(mip_levels(1), 1);
    }

    /// The real layer images decode the way the game loads them, are
    /// 16×16, use only the gray levels, and build, one array layer per
    /// texture.
    #[test]
    fn layers_on_disk_build() {
        let image = build_base(&layers_on_disk());
        assert_eq!(
            image.texture_descriptor.size.depth_or_array_layers,
            base().textures.len() as u32
        );
    }

    /// The grass overhang hangs from the top edge of the side face and lets
    /// the dirt show below it: every column is opaque from the top down to
    /// some depth and transparent below, and the bottom row is all dirt.
    #[test]
    fn grass_side_overhang_hangs_from_the_top() {
        let layers = layers_on_disk();
        let side = &layers[&image_path("grass_side")];
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
        let image = build_base(&layers);
        let bottom = TILE_SIZE - 1;
        for x in 0..TILE_SIZE {
            assert_eq!(
                pixel(&image, tile("grass_side"), 0, x, bottom),
                pixel(&image, tile("dirt"), 0, x, bottom),
            );
        }
    }

    /// The log's cut end shows its bark rim over the rings: the corner
    /// comes from the bark layer (`log_top_bark`, on top) and the middle from
    /// the rings (`log_top`), each colored by its own palette.
    #[test]
    fn log_top_has_bark_around_the_rings() {
        let layers = layers_on_disk();
        let pixels = |name: &str| layers[&image_path(name)].data.clone().unwrap();
        let (rings, bark) = (pixels("log_top"), pixels("log_top_bark"));
        let at = |x: u32, y: u32| ((y * TILE_SIZE + x) * 4) as usize;
        let (corner, middle) = (at(0, 0), at(8, 8));
        assert_eq!(bark[corner + 3], 255, "the bark covers the corner");
        assert_eq!(bark[middle + 3], 0, "the bark leaves the middle open");

        let image = build_base(&layers);
        let color = |palette: Palette, gray: u8| {
            palette.color(layer::level_of(gray).unwrap()).to_u8_array()
        };
        assert_eq!(
            pixel(&image, tile("log_top"), 0, 0, 0),
            color(material("bark"), bark[corner])
        );
        assert_eq!(
            pixel(&image, tile("log_top"), 0, 8, 8),
            color(material("log_end"), rings[middle])
        );
    }

    /// Every pixel of every tile a block shows is opaque: the shader
    /// ignores alpha, so a hole would show the color hiding behind it.
    /// (Item icons may be transparent.)
    #[test]
    fn block_tiles_are_opaque() {
        let image = build_base(&layers_on_disk());
        let content = base();
        let mut layers: Vec<u32> = (0..content.states.len())
            .flat_map(|i| {
                content
                    .state(BlockState(i as u16))
                    .faces
                    .iter()
                    .map(|f| f.layer)
            })
            .collect();
        layers.sort();
        layers.dedup();
        assert!(layers.len() >= 9, "{layers:?}");
        for layer in layers {
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let alpha = pixel(&image, layer, 0, x, y)[3];
                    let id = &content.textures[layer as usize].id;
                    assert_eq!(alpha, 255, "{id} ({x}, {y})");
                }
            }
        }
    }

    /// A tile's pixels come from its palettes: with every layer a flat
    /// gray, a one-layer tile is exactly that level's palette color.
    #[test]
    fn tiles_are_colored_by_their_palettes() {
        let images: HashMap<String, Image> = base()
            .image_paths()
            .into_iter()
            .map(|path| (path, flat_gray(3)))
            .collect();
        let image = build_base(&images);
        let expected = material("stone").color(3).to_u8_array();
        assert_eq!(pixel(&image, tile("stone"), 0, 7, 7), expected);
        // Opaque grass over dirt hides the dirt.
        let grass = material("grass").color(3).to_u8_array();
        assert_eq!(pixel(&image, tile("grass_side"), 0, 0, 0), grass);
    }

    /// The pixels of array layer `layer`'s full-size mip.
    fn tile_pixels(image: &Image, layer: u32) -> Vec<u8> {
        let at = offset(layer, 0);
        image.data.as_ref().unwrap()[at..at + (TILE_SIZE * TILE_SIZE * 4) as usize].to_vec()
    }

    /// A missing or bad image shows the placeholder in the tiles that use
    /// it, names the texture and file, and leaves every other tile as it
    /// should be.
    #[test]
    fn a_missing_or_bad_layer_marks_only_its_own_tiles() {
        let good = layers_on_disk();
        let expected = build_base(&good);
        let stone = image_path("stone");

        let mut bad = good.clone();
        bad.get_mut(&stone).unwrap().data.as_mut().unwrap()[0] = 200;
        let missing = {
            let mut layers = good.clone();
            layers.remove(&stone);
            layers
        };
        for (layers, message) in [(&bad, "not gray"), (&missing, "isn't loaded")] {
            let (image, _, errors) = build(&base().textures, |path| layers.get(path));
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(
                errors[0].contains("defaria:stone")
                    && errors[0].contains(&stone)
                    && errors[0].contains(message),
                "{errors:?}"
            );
            for (layer, texture) in base().textures.iter().enumerate() {
                let layer = layer as u32;
                let wanted = if texture.id == "defaria:stone" {
                    placeholder()
                } else {
                    tile_pixels(&expected, layer)
                };
                assert_eq!(tile_pixels(&image, layer), wanted, "{}", texture.id);
            }
        }
    }

    /// With no textures at all there's still a valid one-layer texture.
    #[test]
    fn no_textures_build_a_placeholder_layer() {
        let (image, _, errors) = build(&[], |_| None);
        assert!(errors.is_empty());
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 1);
        assert_eq!(tile_pixels(&image, 0), placeholder());
    }

    #[test]
    fn layers_hold_their_own_tile_and_full_mip_chains() {
        let image = array_texture(&tiles(|i, _, _| [i as u8 * 20, 7, 9, 255]));
        let count = TILES;
        let size = image.texture_descriptor.size;
        assert_eq!(
            (size.width, size.height, size.depth_or_array_layers),
            (16, 16, count)
        );
        assert_eq!(image.texture_descriptor.mip_level_count, 5);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(image.data_order, TextureDataOrder::LayerMajor);
        assert_eq!(image.data.as_ref().unwrap().len(), offset(count, 0));
        for layer in 0..count {
            for mip in 0..5 {
                assert_eq!(
                    pixel(&image, layer, mip, 0, 0),
                    [layer as u8 * 20, 7, 9, 255]
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
