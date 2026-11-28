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

use defaria_core::content::{Animation, MAX_TILES, Texture};

/// Width and height of one tile, and of every layer image, in pixels.
pub const TILE_SIZE: u32 = 16;
/// Anisotropic filtering samples at most. Keeps textures sharp on surfaces
/// seen at a grazing angle, where plain mipmapping would blur them.
const MAX_ANISOTROPY: u16 = 16;

/// The most frames one texture can step through.
pub const MAX_FRAMES: usize = defaria_core::content::load::MAX_IMAGE_FRAMES as usize;

/// The block texture as [`build`] makes it.
pub struct Built {
    /// The array texture: layer `i` is texture `i`'s first frame, and the
    /// rest of animated textures' frames come after every texture's first
    /// (see [`Animations`]).
    pub image: Image,
    /// Each texture's first frame, [`TILE_SIZE`]-square sRGB RGBA8 pixels.
    pub tiles: Vec<Vec<u8>>,
    /// Where each texture's frames are, and how it moves.
    pub animations: Animations,
    /// One message per texture that failed.
    pub errors: Vec<String>,
}

/// Builds the block texture from `textures`. `image` looks up a loaded
/// layer image by its asset path.
///
/// An animated texture whose frames don't fit in the array (it holds at
/// most [`MAX_TILES`] layers) shows only its first frame, and is reported.
pub fn build<'a>(textures: &[Texture], image: impl Fn(&str) -> Option<&'a Image>) -> Built {
    let (frames, mut errors) = frames(textures, image);
    let animations = Animations::lay_out(textures, &frames, MAX_TILES, &mut errors);
    let cutout = |i: usize| textures.get(i).is_some_and(|t| t.cutout);
    // Each texture's first frame at its own index, then the rest in order.
    let mut layers: Vec<(&[u8], bool)> = frames
        .iter()
        .enumerate()
        .map(|(i, frames)| (&frames[0][..], cutout(i)))
        .collect();
    for (i, frames) in frames.iter().enumerate() {
        if animations.frames(i) > 1 {
            layers.extend(frames[1..].iter().map(|frame| (&frame[..], cutout(i))));
        }
    }
    let image = array_texture(&layers);
    let tiles = frames.into_iter().map(|mut f| f.swap_remove(0)).collect();
    Built {
        image,
        tiles,
        animations,
        errors,
    }
}

/// Colorizes and stacks every texture's frames: entry `i` is
/// `textures[i]`'s, each [`TILE_SIZE`]-square sRGB RGBA8 pixels, rows top
/// first.
///
/// A texture that can't be built (an image missing, or breaking the layer
/// rules) gets one frame of [`placeholder`] instead, so one bad image only
/// marks its own tiles, and the other tiles keep their places. Returns the
/// frames and one message per texture that failed.
pub fn frames<'a>(
    textures: &[Texture],
    image: impl Fn(&str) -> Option<&'a Image>,
) -> (Vec<Vec<Vec<u8>>>, Vec<String>) {
    let mut errors = Vec::new();
    let frames = textures
        .iter()
        .map(|texture| {
            compose(texture, &image).unwrap_or_else(|error| {
                errors.push(format!("{}: {error}", texture.id));
                vec![placeholder()]
            })
        })
        .collect();
    (frames, errors)
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

/// Colorizes and stacks one texture's layers into frames of sRGB RGBA8
/// pixels. A layer image may be a strip of frames; the texture has as many
/// frames as the least number every layer's frame count divides, and a
/// layer with fewer repeats (a still layer shows in every frame). Strips
/// need the texture to have an `animation`.
fn compose<'a>(
    texture: &Texture,
    image: impl Fn(&str) -> Option<&'a Image>,
) -> Result<Vec<Vec<u8>>, String> {
    let tile = (TILE_SIZE * TILE_SIZE) as usize;
    let layers = texture
        .layers
        .iter()
        .map(|layer| {
            let path = &layer.image;
            let image = image(path).ok_or_else(|| format!("{path} isn't loaded"))?;
            layer::colorize(image, &layer.palette).map_err(|error| format!("{path} {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut count = 1;
    for (layer, colors) in texture.layers.iter().zip(&layers) {
        let frames = colors.len() / tile;
        count = lcm(count, frames);
        if frames > 1 && texture.animation.and_then(|a| a.frame_time).is_none() {
            return Err(format!(
                "{} has {frames} frames, and the texture's `animation` has no `frame_time`",
                layer.image
            ));
        }
        if count > MAX_FRAMES {
            return Err(format!(
                "its layers' frames make {count} frames, more than the {MAX_FRAMES} a texture can have"
            ));
        }
    }
    Ok((0..count)
        .map(|frame| {
            let frames: Vec<&[LinearRgba]> = layers
                .iter()
                .map(|colors| {
                    let at = frame % (colors.len() / tile) * tile;
                    &colors[at..at + tile]
                })
                .collect();
            layer::stack(&frames)
        })
        .collect())
}

fn lcm(a: usize, b: usize) -> usize {
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    a / gcd(a, b) * b
}

/// Each texture's frames in the block texture, and how it moves, for the
/// shader (see [`Animations::words`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Animations(Vec<Animated>);

/// One texture's entry in [`Animations`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Animated {
    /// How many frames it steps through: 1 for a still texture.
    pub frames: u32,
    /// The array layer of its second frame; the rest follow it. (Its first
    /// is at its own index.)
    pub rest: u32,
    /// How it moves, if it does.
    pub animation: Option<Animation>,
}

/// What the shader samples for texture `texture` at a time: array layer
/// `layer`, faded toward `next` by `fade` (0 is all `layer`).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub layer: u32,
    pub next: u32,
    pub fade: f32,
}

/// 32-bit words per [`Animated`] in the shader's table. Must match the
/// `Animation` struct in `assets/shaders/voxel.wesl`.
pub const ANIMATION_WORDS: usize = 5;

impl Animations {
    /// Places every texture's frames after all the first frames, in
    /// texture order, while they fit in `room` array layers. A texture
    /// whose frames don't fit stays on its first frame, with a message in
    /// `errors`.
    pub fn lay_out(
        textures: &[Texture],
        frames: &[Vec<Vec<u8>>],
        room: usize,
        errors: &mut Vec<String>,
    ) -> Self {
        let mut next = textures.len().max(1);
        let entries = textures
            .iter()
            .zip(frames)
            .map(|(texture, frames)| {
                let mut count = frames.len();
                if next + count - 1 > room {
                    errors.push(format!(
                        "{}: its {count} frames don't fit in the block texture's {room} layers; it shows its first",
                        texture.id
                    ));
                    count = 1;
                }
                let rest = next as u32;
                next += count - 1;
                Animated {
                    frames: count as u32,
                    rest,
                    animation: texture.animation,
                }
            })
            .collect();
        Self(entries)
    }

    /// How many frames texture `texture` steps through.
    pub fn frames(&self, texture: usize) -> u32 {
        self.0.get(texture).map_or(1, |a| a.frames)
    }

    #[cfg(test)]
    pub fn get(&self, texture: usize) -> Option<&Animated> {
        self.0.get(texture)
    }

    /// The array layer of texture `texture`'s frame `frame`.
    #[cfg(test)]
    fn layer(&self, texture: u32, frame: u32) -> u32 {
        if frame == 0 {
            texture
        } else {
            self.0[texture as usize].rest + frame - 1
        }
    }

    /// What texture `texture` shows `time` seconds in: frame after frame,
    /// each for its `frame_time`, round and round, fading into the next if
    /// it blends. The spec of the shader's `animated_frame`, which does
    /// the same on the GPU: tests check the timing against this, and that
    /// the shader still says the same (in `voxel_material`).
    #[cfg(test)]
    pub fn frame_at(&self, texture: u32, time: f32) -> Frame {
        let still = Frame {
            layer: texture,
            next: texture,
            fade: 0.0,
        };
        let Some(animated) = self.0.get(texture as usize).filter(|a| a.frames > 1) else {
            return still;
        };
        let animation = animated.animation.unwrap();
        let steps = time / animation.frame_time.unwrap();
        let frame = (steps.floor() as i64).rem_euclid(animated.frames as i64) as u32;
        Frame {
            layer: self.layer(texture, frame),
            next: self.layer(texture, (frame + 1) % animated.frames),
            fade: if animation.blend {
                steps.fract().abs()
            } else {
                0.0
            },
        }
    }

    /// The table the shader reads, [`ANIMATION_WORDS`] words per texture:
    /// frames, the layer of the second frame, seconds a frame, scroll
    /// speed (both `f32` bits) and whether it blends. Never empty, since a
    /// storage buffer can't be.
    pub fn words(&self) -> Vec<u32> {
        let still = [Animated {
            frames: 1,
            ..default()
        }];
        let entries = if self.0.is_empty() {
            &still[..]
        } else {
            &self.0
        };
        entries
            .iter()
            .flat_map(|a| -> [u32; ANIMATION_WORDS] {
                let animation = a.animation.unwrap_or(Animation {
                    frame_time: None,
                    blend: false,
                    scroll: 0.0,
                });
                [
                    a.frames,
                    a.rest,
                    animation.frame_time.unwrap_or(0.0).to_bits(),
                    animation.scroll.to_bits(),
                    animation.blend as u32,
                ]
            })
            .collect()
    }
}

/// A 2D array texture with a full mip chain, one array layer per entry of
/// `tiles` (each [`TILE_SIZE`] square, sRGB RGBA8, and whether it's a
/// cut-out). An empty list gets one [`placeholder`] layer, since an array
/// texture needs at least one.
fn array_texture(tiles: &[(&[u8], bool)]) -> Image {
    let placeholder = placeholder();
    let fallback = [(&placeholder[..], false)];
    let tiles = if tiles.is_empty() {
        &fallback[..]
    } else {
        tiles
    };
    let mut data = Vec::new();
    for &(tile, cutout) in tiles {
        let mut level = tile.to_vec();
        let mut size = TILE_SIZE;
        // Tiles cut-out blocks show keep their coverage; the rest (blended
        // or opaque) keep their averaged alpha.
        let coverage = cutout.then(|| coverage(tile, 1.0));
        // Layer-major: every mip of this layer, largest first.
        data.extend_from_slice(&level);
        while size > 1 {
            level = downsample(&level, size);
            size /= 2;
            match coverage {
                Some(coverage) => data.extend(keep_coverage(&level, coverage)),
                None => data.extend_from_slice(&level),
            }
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

/// Halves a square sRGB RGBA8 image, averaging each 2×2 block in linear,
/// premultiplied color, so a clear pixel's color (black, usually) doesn't
/// bleed into its solid neighbours. Alpha is linear already.
fn downsample(pixels: &[u8], size: u32) -> Vec<u8> {
    let half = size / 2;
    let mut out = Vec::with_capacity((half * half * 4) as usize);
    for y in 0..half {
        for x in 0..half {
            let mut sum = Vec4::ZERO;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (((2 * y + dy) * size + 2 * x + dx) * 4) as usize;
                let alpha = pixels[i + 3] as f32 / 255.0;
                let rgb = Vec3::from_array(std::array::from_fn(|c| {
                    Srgba::gamma_function(pixels[i + c] as f32 / 255.0)
                }));
                sum += (rgb * alpha).extend(alpha);
            }
            let alpha = sum.w / 4.0;
            let rgb = if sum.w > 0.0 {
                sum.truncate() / sum.w
            } else {
                Vec3::ZERO
            };
            for value in rgb.to_array() {
                out.push((Srgba::gamma_function_inverse(value) * 255.0).round() as u8);
            }
            out.push((alpha * 255.0).round() as u8);
        }
    }
    out
}

/// The alpha a cut-out pixel needs to be drawn: the shader's threshold.
const CUTOFF: f32 = 0.5;

/// The share of pixels drawn (alpha at least [`CUTOFF`]) once every alpha
/// is multiplied by `scale`.
fn coverage(pixels: &[u8], scale: f32) -> f32 {
    let alphas = pixels.iter().skip(3).step_by(4);
    let count = pixels.len() / 4;
    let drawn = alphas
        .filter(|&&a| a as f32 / 255.0 * scale >= CUTOFF)
        .count();
    drawn as f32 / count as f32
}

/// A smaller mip of a cut-out with its alphas scaled so at least `target`
/// of its pixels are drawn, the share drawn at full size, and as few more
/// as it can. Averaging alone shrinks thin parts away: a one-pixel stalk
/// averages to a quarter alpha in the next mip and vanishes from the
/// alpha test. (Castaño's alpha test coverage preservation.)
fn keep_coverage(pixels: &[u8], target: f32) -> Vec<u8> {
    // Coverage only grows with the scale: search for the best one.
    let (mut low, mut high) = (0.0f32, 64.0f32);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if coverage(pixels, middle) < target {
            low = middle;
        } else {
            high = middle;
        }
    }
    // The least scale that reaches the target: a little more coverage
    // rather than less, since thin parts vanishing is the worse mistake.
    let scale = high;
    let mut out = pixels.to_vec();
    for alpha in out.iter_mut().skip(3).step_by(4) {
        *alpha = (*alpha as f32 * scale).round().min(255.0) as u8;
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
    use defaria_core::content::palette::Palette;
    use defaria_core::content::{Render, base};
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

    /// [`array_texture`] of `tiles`, all cut-outs or none.
    fn array(tiles: &[Vec<u8>], cutout: bool) -> Image {
        let tiles: Vec<(&[u8], bool)> = tiles.iter().map(|t| (&t[..], cutout)).collect();
        array_texture(&tiles)
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
        defaria_core::content::base_texture(name)
    }

    /// The asset path of base game image `<name>`.
    fn image_path(name: &str) -> String {
        format!("mods/defaria/images/{name}.png")
    }

    fn material(name: &str) -> Palette {
        base().materials[&format!("defaria:{name}")].palette
    }

    /// Oak's palette for `role`, which the default log and planks use.
    fn wood_palette(role: &str) -> Palette {
        base().materials["defaria:oak"].palette_for(Some(role))
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
                let bytes = std::fs::read(defaria_core::content::assets_dir().join(&path))
                    .unwrap_or_else(|error| panic!("assets/{path}: {error}"));
                let image = decode(&bytes);
                (path, image)
            })
            .collect()
    }

    fn build_base(layers: &HashMap<String, Image>) -> Image {
        let built = build(&base().textures, |path| layers.get(path));
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        built.image
    }

    /// Every PNG in every mod's images, including ones no texture uses
    /// yet, decodes to a valid layer: 16×16, gray levels only.
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
        pngs(&defaria_core::content::mods_dir(), &mut files);
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
    /// 16×16 (or strips of 16×16 frames), use only the gray levels, and
    /// build, one array layer per texture and one per extra frame.
    #[test]
    fn layers_on_disk_build() {
        let built = build(&base().textures, |path| layers_on_disk_cached().get(path));
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let extra: u32 = (0..base().textures.len())
            .map(|i| built.animations.frames(i) - 1)
            .sum();
        assert_eq!(
            built.image.texture_descriptor.size.depth_or_array_layers,
            base().textures.len() as u32 + extra
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
                pixel(&image, tile("soil_turf"), 0, x, bottom),
                pixel(&image, tile("soil"), 0, x, bottom),
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
            color(wood_palette("bark"), bark[corner])
        );
        assert_eq!(
            pixel(&image, tile("log_top"), 0, 8, 8),
            color(wood_palette("heartwood"), rings[middle])
        );
    }

    /// Every pixel of every tile an opaque block shows is opaque: opaque
    /// faces ignore alpha, so a hole would show the color hiding behind it.
    /// (See-through blocks and item icons may be transparent.)
    #[test]
    fn opaque_block_tiles_are_opaque() {
        let image = build_base(&layers_on_disk());
        let content = base();
        let mut layers: Vec<u32> = content
            .states
            .iter()
            .filter(|state| state.render == Render::Opaque)
            .flat_map(|state| state.faces.iter().map(|f| f.layer))
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

    /// Whether a tile is all solid or clear pixels, with some clear.
    fn is_cutout(pixels: &[u8]) -> bool {
        let alphas = || pixels.iter().skip(3).step_by(4);
        alphas().all(|&a| a == 0 || a == 255) && alphas().any(|&a| a == 0)
    }

    /// Cut-out blocks' tiles are marked as cut-outs (so their mips keep
    /// their coverage) and have holes; translucent blocks' are partly
    /// transparent, and not marked.
    #[test]
    fn see_through_tiles_have_the_alpha_their_blocks_need() {
        let Built {
            image,
            tiles,
            errors,
            ..
        } = build(&base().textures, |path| layers_on_disk_cached().get(path));
        assert!(errors.is_empty(), "{errors:?}");
        let tile = |id: &str| &tiles[base().texture(id).unwrap() as usize];
        let marked = |id: &str| base().textures[base().texture(id).unwrap() as usize].cutout;
        for id in ["defaria:leaves(wood=defaria:oak)", "defaria:tuft"] {
            assert!(is_cutout(tile(id)) && marked(id), "{id}");
        }
        assert!(!marked("defaria:stone(rock=defaria:stone)"));
        // The texture `build` makes keeps the leaves' coverage in the mips.
        let leaves = base().texture("defaria:leaves(wood=defaria:oak)").unwrap();
        let full = coverage(tile("defaria:leaves(wood=defaria:oak)"), 1.0);
        for mip in 1..mip_levels(TILE_SIZE) {
            let size = TILE_SIZE >> mip;
            let level: Vec<u8> = (0..size)
                .flat_map(|y| (0..size).map(move |x| (x, y)))
                .flat_map(|(x, y)| pixel(&image, leaves, mip, x, y))
                .collect();
            assert!(coverage(&level, 1.0) >= full, "leaves' mip {mip}");
        }
        for id in ["defaria:glass", "defaria:water"] {
            assert!(!marked(id), "{id}");
            let alphas: Vec<u8> = tile(id).iter().skip(3).step_by(4).copied().collect();
            assert!(!is_cutout(tile(id)), "{id}");
            assert!(alphas.iter().any(|&a| a > 0 && a < 255), "{id}");
        }
    }

    fn layers_on_disk_cached() -> &'static HashMap<String, Image> {
        static LAYERS: std::sync::OnceLock<HashMap<String, Image>> = std::sync::OnceLock::new();
        LAYERS.get_or_init(layers_on_disk)
    }

    /// Smaller mips average in premultiplied color: a clear pixel's color
    /// doesn't bleed into the solid one next to it, and partly clear
    /// pixels count by their alpha.
    #[test]
    fn clear_pixels_dont_bleed_into_mips() {
        let clear = std::iter::repeat_n([0, 255, 0, 0], 3).flatten();
        let pixels: Vec<u8> = [200, 100, 50, 255].into_iter().chain(clear).collect();
        assert_eq!(downsample(&pixels, 2), [200, 100, 50, 64]);
        // Opaque red and quarter-alpha blue: four parts red to one blue.
        let pixels: Vec<u8> = [255, 0, 0, 255, 0, 0, 255, 64, 0, 0, 0, 0, 0, 0, 0, 0].to_vec();
        let [r, g, b, a] = downsample(&pixels, 2)[..] else {
            panic!("one pixel");
        };
        assert_eq!((g, a), (0, 80));
        assert!(r > 220 && (100..140).contains(&b), "({r}, {b})");
    }

    /// A cut-out's mips keep at least the share of pixels drawn that the
    /// full size has, so thin parts don't fade away in the distance: a
    /// grid of one-pixel lines would average to under the cutoff
    /// everywhere and vanish from the third mip on.
    #[test]
    fn cutout_mips_keep_their_coverage() {
        let grid = tiles(|_, x, y| {
            let drawn = x % 4 == 0 || y % 4 == 0;
            [255, 255, 255, if drawn { 255 } else { 0 }]
        });
        let full = coverage(&grid[0], 1.0);
        assert!((full - 7.0 / 16.0).abs() < 1e-6);
        let image = array(&grid, true);
        for mip in 1..mip_levels(TILE_SIZE) {
            let size = TILE_SIZE >> mip;
            let level: Vec<u8> = (0..size)
                .flat_map(|y| (0..size).map(move |x| (x, y)))
                .flat_map(|(x, y)| pixel(&image, 0, mip, x, y))
                .collect();
            let kept = coverage(&level, 1.0);
            assert!(kept >= full, "mip {mip} keeps only {kept} drawn of {full}");
        }
        // Without keeping it, the averages fall under the cutoff.
        let plain = downsample(&downsample(&grid[0], 16), 8);
        assert!(coverage(&plain, 1.0) < full / 2.0);
    }

    /// Tiles that aren't cut-outs keep their averaged alpha, even ones of
    /// only solid and clear pixels (a translucent pane with a clear
    /// middle): scaling it would make them more opaque in the distance.
    #[test]
    fn blended_mips_keep_their_alpha() {
        let glass = tiles(|_, _, _| [255, 255, 255, 64]);
        let image = array(&glass, false);
        for mip in 0..mip_levels(TILE_SIZE) {
            assert_eq!(pixel(&image, 0, mip, 0, 0)[3], 64, "mip {mip}");
        }
        let frame = tiles(|_, x, _| [255, 255, 255, if x % 4 == 0 { 255 } else { 0 }]);
        let image = array(&frame, false);
        assert_eq!(pixel(&image, 0, 2, 0, 0)[3], 64, "a quarter solid");
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
        assert_eq!(pixel(&image, tile("soil_turf"), 0, 0, 0), grass);
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
            let Built { image, errors, .. } = build(&base().textures, |path| layers.get(path));
            // One error per tile that uses the stone image: every rock,
            // with and without ore.
            let using: Vec<&str> = base()
                .textures
                .iter()
                .filter(|t| t.layers.iter().any(|l| l.image == stone))
                .map(|t| t.id.as_str())
                .collect();
            assert!(using.len() > 1, "{using:?}");
            assert_eq!(errors.len(), using.len(), "{errors:?}");
            for (error, id) in errors.iter().zip(&using) {
                assert!(
                    error.contains(id) && error.contains(&stone) && error.contains(message),
                    "{errors:?}"
                );
            }
            for (layer, texture) in base().textures.iter().enumerate() {
                let layer = layer as u32;
                let wanted = if using.contains(&texture.id.as_str()) {
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
        let Built { image, errors, .. } = build(&[], |_| None);
        assert!(errors.is_empty());
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 1);
        assert_eq!(tile_pixels(&image, 0), placeholder());
    }

    #[test]
    fn layers_hold_their_own_tile_and_full_mip_chains() {
        let image = array(&tiles(|i, _, _| [i as u8 * 20, 7, 9, 255]), false);
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
        let image = array(&tiles(|i, x, y| [i as u8, x as u8, y as u8, 255]), false);
        assert_eq!(pixel(&image, 1, 0, 5, 11), [1, 5, 11, 255]);
    }

    /// Mips shrink the tile, they don't flip or transpose it: a tile that's
    /// white on the left and black on the right stays that way.
    #[test]
    fn mips_keep_the_tile_the_right_way_round() {
        let image = array(
            &tiles(|_, x, _| {
                let v = if x < TILE_SIZE / 2 { 255 } else { 0 };
                [v, v, v, 255]
            }),
            false,
        );
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
        let image = array(
            &tiles(|_, x, y| {
                let v = if (x + y).is_multiple_of(2) { 255 } else { 0 };
                [v, v, v, 255]
            }),
            false,
        );
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
        let image = array(
            &tiles(|_, x, y| {
                let a = if (x + y).is_multiple_of(2) { 255 } else { 0 };
                [0, 0, 0, a]
            }),
            false,
        );
        assert_eq!(pixel(&image, 0, 1, 0, 0)[3], 128);
    }

    #[test]
    fn samples_with_mipmaps_and_anisotropy_clamped_to_each_tile() {
        let image = array(&tiles(|_, _, _| [0; 4]), false);
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

    /// A texture of one layer, image `image`, gray palette.
    fn texture(id: &str, images: &[&str], animation: Option<Animation>) -> Texture {
        Texture {
            id: id.into(),
            template: id.into(),
            layers: images
                .iter()
                .map(|image| defaria_core::content::TextureLayer {
                    image: (*image).into(),
                    palette: material("stone"),
                })
                .collect(),
            cutout: false,
            animation,
        }
    }

    fn animation(frame_time: f32, blend: bool) -> Option<Animation> {
        Some(Animation {
            frame_time: Some(frame_time),
            blend,
            scroll: 0.5,
        })
    }

    /// The tile a flat image of gray `level` makes with the stone palette.
    fn flat_tile(level: usize) -> Vec<u8> {
        let colors = layer::colorize(&flat_gray(level), &material("stone")).unwrap();
        layer::stack(&[&colors])
    }

    /// An animated texture's first frame is at its own index (and is its
    /// tile, for item icons); its other frames come after every
    /// texture's first, in order, and the table says where.
    #[test]
    fn frames_go_after_every_first_frame() {
        let images: HashMap<String, Image> = [
            ("still".to_string(), flat_gray(2)),
            ("three".to_string(), layer::test_images::strip(&[1, 4, 6])),
            ("two".to_string(), layer::test_images::strip(&[3, 5])),
        ]
        .into();
        let textures = [
            texture("a", &["three"], animation(0.25, false)),
            texture("b", &["still"], None),
            texture("c", &["two"], animation(1.0, true)),
        ];
        let built = build(&textures, |path| images.get(path));
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let image = &built.image;
        // 3 first frames, then a's other 2 and c's other 1.
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 6);
        for (layer, level) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 6), (5, 5)] {
            assert_eq!(tile_pixels(image, layer), flat_tile(level), "layer {layer}");
        }
        assert_eq!(built.tiles, [flat_tile(1), flat_tile(2), flat_tile(3)]);
        let a = built.animations.get(0).unwrap();
        assert_eq!((a.frames, a.rest), (3, 3));
        assert_eq!(a.animation, animation(0.25, false));
        assert_eq!(built.animations.frames(1), 1);
        let c = built.animations.get(2).unwrap();
        assert_eq!((c.frames, c.rest), (2, 5));
        assert_eq!(c.animation, animation(1.0, true));
        // The shader's table: five words a texture.
        let words = built.animations.words();
        assert_eq!(words.len(), 3 * ANIMATION_WORDS);
        assert_eq!(
            words[..ANIMATION_WORDS],
            [3, 3, 0.25f32.to_bits(), 0.5f32.to_bits(), 0]
        );
        assert_eq!(words[2 * ANIMATION_WORDS + 4], 1);
    }

    /// Picking the frame for a time: each frame for its frame time, round
    /// and round, fading into the next only if the texture blends; a
    /// still texture is always itself.
    #[test]
    fn picks_the_frame_for_a_time() {
        let images: HashMap<String, Image> = [
            ("still".to_string(), flat_gray(2)),
            ("three".to_string(), layer::test_images::strip(&[1, 4, 6])),
        ]
        .into();
        let textures = [
            texture("still", &["still"], None),
            texture("stepped", &["three"], animation(0.25, false)),
            texture("blended", &["three"], animation(0.5, true)),
        ];
        let animations = build(&textures, |path| images.get(path)).animations;
        // stepped: frames at layers 1, 3, 4; blended: 2, 5, 6.
        let frame = |texture, time| {
            let f = animations.frame_at(texture, time);
            (f.layer, f.next, (f.fade * 100.0).round() / 100.0)
        };
        for time in [0.0, 0.3, 100.0] {
            assert_eq!(frame(0, time), (0, 0, 0.0));
        }
        assert_eq!(frame(1, 0.0), (1, 3, 0.0));
        assert_eq!(frame(1, 0.24), (1, 3, 0.0));
        assert_eq!(frame(1, 0.26), (3, 4, 0.0));
        assert_eq!(frame(1, 0.6), (4, 1, 0.0));
        // Round again after three frames.
        assert_eq!(frame(1, 0.8), (1, 3, 0.0));
        assert_eq!(frame(1, 75.5), (4, 1, 0.0));
        assert_eq!(frame(2, 0.25), (2, 5, 0.5));
        assert_eq!(frame(2, 0.6), (5, 6, 0.2));
        assert_eq!(frame(2, 1.4), (6, 2, 0.8));
    }

    /// Layers with different frame counts repeat until they line up: two
    /// frames over three make six. A still layer shows in every frame.
    #[test]
    fn layers_with_different_frame_counts_line_up() {
        let mut top = layer::test_images::strip(&[1, 2]);
        // Clear the top layer's left half so the bottom shows there.
        let data = top.data.as_mut().unwrap();
        for (i, pixel) in data.chunks_mut(4).enumerate() {
            if (i as u32 % TILE_SIZE) < TILE_SIZE / 2 {
                pixel[3] = 0;
            }
        }
        let images: HashMap<String, Image> = [
            ("bottom".to_string(), layer::test_images::strip(&[3, 4, 5])),
            ("top".to_string(), top),
            ("still".to_string(), flat_gray(7)),
        ]
        .into();
        let textures = [texture(
            "a",
            &["still", "bottom", "top"],
            animation(1.0, false),
        )];
        let (frames, errors) = frames(&textures, |path| images.get(path));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(frames[0].len(), 6);
        let left = |frame: usize| frames[0][frame][..4].to_vec();
        let right = |frame: usize| {
            let at = ((TILE_SIZE - 1) * 4) as usize;
            frames[0][frame][at..at + 4].to_vec()
        };
        for frame in 0..6 {
            assert_eq!(left(frame), flat_tile([3, 4, 5][frame % 3])[..4], "{frame}");
            assert_eq!(right(frame), flat_tile([1, 2][frame % 2])[..4], "{frame}");
        }
    }

    /// More than [`MAX_FRAMES`] frames, once layers line up, is reported:
    /// 5, 7 and 3 frames make 105.
    #[test]
    fn too_many_frames_are_reported() {
        let images: HashMap<String, Image> = [
            ("five".to_string(), layer::test_images::strip(&[1; 5])),
            ("seven".to_string(), layer::test_images::strip(&[2; 7])),
            ("three".to_string(), layer::test_images::strip(&[3; 3])),
        ]
        .into();
        let textures = [texture(
            "many",
            &["five", "seven", "three"],
            animation(0.1, false),
        )];
        let built = build(&textures, |path| images.get(path));
        assert_eq!(built.tiles, [placeholder()]);
        assert!(built.errors[0].contains("105 frames"), "{:?}", built.errors);
        // 5 and 7 alone fit.
        let fits = [texture("fits", &["five", "seven"], animation(0.1, false))];
        assert_eq!(
            build(&fits, |path| images.get(path)).animations.frames(0),
            35
        );
    }

    /// A strip in a texture with no `animation` is a mistake: the texture
    /// shows the placeholder and says why.
    #[test]
    fn frames_need_an_animation() {
        let images: HashMap<String, Image> =
            [("two".to_string(), layer::test_images::strip(&[3, 5]))].into();
        let textures = [texture("plain", &["two"], None)];
        let built = build(&textures, |path| images.get(path));
        assert_eq!(built.tiles, [placeholder()]);
        assert_eq!(built.errors.len(), 1);
        assert!(built.errors[0].contains("plain") && built.errors[0].contains("2 frames"));
        assert!(built.errors[0].contains("animation"), "{:?}", built.errors);
    }

    /// A texture whose frames don't fit in the array keeps its first
    /// frame, still; the ones before it keep theirs.
    #[test]
    fn frames_that_dont_fit_stay_on_the_first() {
        let three = flat_tile(1);
        let frames = vec![vec![three.clone(); 3], vec![three.clone(); 3]];
        let textures = [
            texture("a", &[], animation(1.0, false)),
            texture("b", &[], animation(1.0, false)),
        ];
        let mut errors = Vec::new();
        // Room for the two first frames and a's other two.
        let animations = Animations::lay_out(&textures, &frames, 4, &mut errors);
        assert_eq!(animations.frames(0), 3);
        assert_eq!(animations.frames(1), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("b:"), "{errors:?}");
    }

    /// The base game's water and lava are animated, lava blending, and
    /// their tiles (what item icons show) are their first frames.
    #[test]
    fn water_and_lava_are_animated() {
        let layers = layers_on_disk_cached();
        let built = build(&base().textures, |path| layers.get(path));
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let (frames, _) = frames(&base().textures, |path| layers.get(path));
        for (name, blend) in [("water", false), ("lava", true)] {
            let index = tile(name) as usize;
            let animated = built.animations.get(index).unwrap();
            assert!(
                animated.frames >= 8,
                "{name} has {} frames",
                animated.frames
            );
            let animation = animated.animation.unwrap();
            assert_eq!(animation.blend, blend, "{name}");
            assert!(animation.scroll > 0.0, "{name}");
            // The shader's clock wraps every hour: a loop and a scroll
            // that fit an hour a whole number of times never jump.
            let loops = 3600.0 / (animated.frames as f32 * animation.frame_time.unwrap());
            let scrolls = 3600.0 * animation.scroll;
            for count in [loops, scrolls] {
                assert!((count - count.round()).abs() < 1e-3, "{name}: {count}");
            }
            assert_eq!(built.tiles[index], frames[index][0], "{name}");
            // The frames differ: it moves.
            assert_ne!(frames[index][0], frames[index][1], "{name}");
        }
    }
}
