//! Drawing item icons on the CPU from the block texture's tiles.
//!
//! A block's icon is its default state's baked model drawn in isometric
//! view from above the south-east corner, so the top, south and east faces
//! show, each shaded by a fixed amount (top brightest) the way the world's
//! sun would. Faces are drawn into a depth buffer with nearest-texel
//! sampling, [`SUPERSAMPLE`]² samples a pixel, so any model works, slabs
//! and other partial shapes included. An item with its own texture shows
//! that tile, flat.
//!
//! Every function here is plain data in, pixels out: sRGB RGBA8, rows top
//! first.

use bevy::prelude::*;

use crate::{
    block_texture::TILE_SIZE,
    content::{BakedFace, Content, ItemStateInfo},
};

/// Width and height of a block icon, in pixels: at least the icon's size
/// on a scale-2 (retina) window, so it's only ever scaled down.
pub const BLOCK_ICON_SIZE: u32 = 128;
/// Samples per pixel along each axis, averaged, to smooth the edges.
const SUPERSAMPLE: u32 = 2;
/// The share of the icon's height the whole cube takes up.
const FILL: f32 = 0.94;

/// What an icon shows.
#[derive(Debug, Clone, PartialEq)]
pub struct IconPixels {
    pub size: u32,
    /// sRGB RGBA8, rows top first.
    pub data: Vec<u8>,
    /// Whether it's a flat tile, to be scaled up crisply, rather than a
    /// drawing to be scaled smoothly.
    pub flat: bool,
}

/// The icon for `item`. An item with a texture of its own shows that tile;
/// one that places a block shows the block; anything else (or a tile
/// that isn't there) is fully transparent.
pub fn item_icon(content: &Content, item: &ItemStateInfo, tiles: &[Vec<u8>]) -> IconPixels {
    if let Some(layer) = item.texture {
        if let Some(tile) = tiles.get(layer as usize) {
            return IconPixels {
                size: TILE_SIZE,
                data: tile.clone(),
                flat: true,
            };
        }
    } else if let Some(block) = item.block {
        return IconPixels {
            size: BLOCK_ICON_SIZE,
            data: block_icon(&content.state(block).faces, tiles, BLOCK_ICON_SIZE),
            flat: false,
        };
    }
    IconPixels {
        size: TILE_SIZE,
        data: vec![0; (TILE_SIZE * TILE_SIZE * 4) as usize],
        flat: true,
    }
}

/// The direction the icon is seen from, toward the viewer: up, south (+Z)
/// and east (+X) equally.
fn toward_viewer() -> Vec3 {
    Vec3::ONE.normalize()
}

/// Where a point of the block, in block coordinates (a full block is 0 to
/// 1), lands on an icon `size` pixels square: x right, y down, in pixels,
/// and z its nearness to the viewer.
pub fn project(pos: Vec3, size: f32) -> Vec3 {
    let q = pos - Vec3::splat(0.5);
    let right = Vec3::new(1.0, 0.0, -1.0).normalize();
    let up = toward_viewer().cross(right);
    // A cube is `2 * (0.5 + 1 + 0.5) / sqrt(6)` tall from corner to corner.
    let scale = size * FILL / (4.0 / 6f32.sqrt());
    Vec3::new(
        size / 2.0 + q.dot(right) * scale,
        size / 2.0 - q.dot(up) * scale,
        q.dot(toward_viewer()),
    )
}

/// How bright a face pointing along `normal` is drawn: the top fully lit,
/// south a little darker, east darker still, the bottom darkest.
pub fn shade(normal: Vec3) -> f32 {
    let n = normal.normalize_or_zero();
    let vertical = if n.y > 0.0 { 1.0 } else { 0.5 };
    n.x * n.x * 0.62 + n.z * n.z * 0.8 + n.y * n.y * vertical
}

/// Draws `faces` (a block state's baked faces) in isometric view, `size`
/// pixels square. Faces turned away from the viewer are skipped; the rest
/// go through a depth buffer, so any shape draws right.
pub fn block_icon(faces: &[BakedFace], tiles: &[Vec<u8>], size: u32) -> Vec<u8> {
    let samples = size * SUPERSAMPLE;
    let count = (samples * samples) as usize;
    // Linear, premultiplied RGBA per sample, and nearness.
    let mut color = vec![Vec4::ZERO; count];
    let mut depth = vec![f32::NEG_INFINITY; count];
    for face in faces {
        if face.normal.dot(toward_viewer()) <= 1e-4 || face.vertices.len() < 3 {
            continue;
        }
        let Some(tile) = tiles.get(face.layer as usize) else {
            continue;
        };
        let brightness = shade(face.normal);
        let points: Vec<(Vec3, Vec2)> = face
            .vertices
            .iter()
            .map(|v| (project(v.pos, samples as f32), v.uv))
            .collect();
        // A fan of triangles from the first vertex.
        for i in 1..points.len() - 1 {
            let triangle = [points[0], points[i], points[i + 1]];
            fill_triangle(triangle, samples, |index, uv, near| {
                if near < depth[index] - 1e-5 {
                    return;
                }
                let texel = sample(tile, uv);
                if texel[3] < 128 {
                    return;
                }
                depth[index] = near;
                let rgb = Vec3::from_array(std::array::from_fn(|c| to_linear(texel[c])));
                color[index] = (rgb * brightness).extend(1.0);
            });
        }
    }
    downsample(&color, size)
}

/// Calls `plot(sample index, uv, nearness)` for each sample center inside
/// `triangle` (projected position and uv at each corner), on a square grid
/// `samples` wide.
fn fill_triangle(
    triangle: [(Vec3, Vec2); 3],
    samples: u32,
    mut plot: impl FnMut(usize, Vec2, f32),
) {
    let [(a, ua), (b, ub), (c, uc)] = triangle;
    let edge = |p: Vec2, q: Vec2, r: Vec2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let area = edge(a.xy(), b.xy(), c.xy());
    if area.abs() < 1e-6 {
        return;
    }
    let min = a.xy().min(b.xy()).min(c.xy()).floor().max(Vec2::ZERO);
    let max = a
        .xy()
        .max(b.xy())
        .max(c.xy())
        .ceil()
        .min(Vec2::splat(samples as f32));
    for y in min.y as u32..max.y as u32 {
        for x in min.x as u32..max.x as u32 {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            // Barycentric weights, the same sign as the area inside.
            let wa = edge(b.xy(), c.xy(), p) / area;
            let wb = edge(c.xy(), a.xy(), p) / area;
            let wc = edge(a.xy(), b.xy(), p) / area;
            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                continue;
            }
            let uv = ua * wa + ub * wb + uc * wc;
            let near = a.z * wa + b.z * wb + c.z * wc;
            plot((y * samples + x) as usize, uv, near);
        }
    }
}

/// The tile's texel at `uv`, nearest, clamped to the tile.
fn sample(tile: &[u8], uv: Vec2) -> [u8; 4] {
    let texel = |t: f32| ((t * TILE_SIZE as f32).floor() as i32).clamp(0, TILE_SIZE as i32 - 1);
    let i = ((texel(uv.y) * TILE_SIZE as i32 + texel(uv.x)) * 4) as usize;
    tile.get(i..i + 4)
        .map_or([0; 4], |t| [t[0], t[1], t[2], t[3]])
}

/// Averages each [`SUPERSAMPLE`]² block of linear premultiplied samples into
/// one sRGB RGBA8 pixel.
fn downsample(color: &[Vec4], size: u32) -> Vec<u8> {
    let samples = size * SUPERSAMPLE;
    let per_pixel = (SUPERSAMPLE * SUPERSAMPLE) as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut sum = Vec4::ZERO;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let index = (y * SUPERSAMPLE + sy) * samples + x * SUPERSAMPLE + sx;
                    sum += color[index as usize];
                }
            }
            let average = sum / per_pixel;
            let alpha = average.w;
            let rgb = if alpha > 0.0 {
                average.xyz() / alpha
            } else {
                Vec3::ZERO
            };
            out.extend([
                to_srgb(rgb.x),
                to_srgb(rgb.y),
                to_srgb(rgb.z),
                (alpha * 255.0).round() as u8,
            ]);
        }
    }
    out
}

fn to_linear(value: u8) -> f32 {
    let c = value as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(value: f32) -> u8 {
    let c = value.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{BlockState, base, block};

    /// One solid tile per texture: tile `i` is the color `colors(i)`.
    fn solid_tiles(colors: impl Fn(usize) -> [u8; 4]) -> Vec<Vec<u8>> {
        (0..base().textures.len())
            .map(|i| colors(i).repeat((TILE_SIZE * TILE_SIZE) as usize))
            .collect()
    }

    fn pixel(data: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    fn icon_of(state: BlockState, tiles: &[Vec<u8>]) -> Vec<u8> {
        block_icon(&base().state(state).faces, tiles, BLOCK_ICON_SIZE)
    }

    /// Rows of the icon with any opaque pixel in them.
    fn opaque_rows(data: &[u8], size: u32) -> std::ops::RangeInclusive<u32> {
        let rows: Vec<u32> = (0..size)
            .filter(|&y| (0..size).any(|x| pixel(data, size, x, y)[3] == 255))
            .collect();
        rows[0]..=*rows.last().unwrap()
    }

    #[test]
    fn a_cube_shows_its_top_south_and_east_faces_shaded() {
        let white = solid_tiles(|_| [255; 4]);
        let icon = icon_of(block("stone"), &white);
        let s = BLOCK_ICON_SIZE;
        // The top is above the middle, south bottom left, east bottom right.
        let top = pixel(&icon, s, s / 2, s / 4);
        let south = pixel(&icon, s, s / 4, s * 5 / 8);
        let east = pixel(&icon, s, s * 3 / 4, s * 5 / 8);
        assert_eq!(top, [255; 4]);
        assert_eq!(south[..3], [to_srgb(0.8); 3]);
        assert_eq!(south[3], 255);
        assert_eq!(east[..3], [to_srgb(0.62); 3]);
        assert_eq!(east[3], 255);
        // Corners are outside the hexagon.
        assert_eq!(pixel(&icon, s, 0, 0)[3], 0);
        assert_eq!(pixel(&icon, s, s - 1, s - 1)[3], 0);
        // It fills most of the icon's height.
        let rows = opaque_rows(&icon, s);
        assert!(rows.end() - rows.start() + 1 >= s * 9 / 10, "{rows:?}");
    }

    /// The faces show their own textures: grass has a different top.
    #[test]
    fn each_face_shows_its_texture() {
        let content = base();
        let top = content.texture("defaria:grass_top").unwrap() as usize;
        let tiles = solid_tiles(|i| {
            if i == top {
                [0, 255, 0, 255]
            } else {
                [255, 0, 0, 255]
            }
        });
        let icon = icon_of(block("grass"), &tiles);
        let s = BLOCK_ICON_SIZE;
        let top = pixel(&icon, s, s / 2, s / 4);
        let side = pixel(&icon, s, s / 4, s * 5 / 8);
        assert!(top[1] == 255 && top[0] == 0, "{top:?}");
        assert!(side[0] > 0 && side[1] == 0, "{side:?}");
    }

    /// A slab is drawn half as tall as a full block, and sits at the bottom.
    #[test]
    fn a_slab_is_half_height() {
        let white = solid_tiles(|_| [255; 4]);
        let s = BLOCK_ICON_SIZE;
        let cube = opaque_rows(&icon_of(block("stone"), &white), s);
        let slab = opaque_rows(&icon_of(block("stone_slab"), &white), s);
        assert_eq!(slab.end(), cube.end());
        let (cube_h, slab_h) = (
            (cube.end() - cube.start()) as f32,
            (slab.end() - slab.start()) as f32,
        );
        // The slab's top face is as deep as the cube's, so it's a bit more
        // than half the height.
        assert!(
            slab_h < cube_h * 0.8 && slab_h > cube_h * 0.5,
            "{slab_h} of {cube_h}"
        );
    }

    /// Where faces overlap, the nearer one shows, whichever is drawn first.
    #[test]
    fn nearer_faces_hide_farther_ones() {
        use crate::content::Vertex;
        // Two upward quads, one above the other, each its own tile.
        let quad = |y: f32, layer: u32| BakedFace {
            vertices: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]
                .iter()
                .map(|&[x, z]| Vertex {
                    pos: Vec3::new(x, y, z),
                    uv: Vec2::new(x, z),
                })
                .collect(),
            normal: Vec3::Y,
            layer,
            cull: None,
        };
        let tiles = vec![
            [255, 0, 0, 255].repeat((TILE_SIZE * TILE_SIZE) as usize),
            [0, 0, 255, 255].repeat((TILE_SIZE * TILE_SIZE) as usize),
        ];
        let s = BLOCK_ICON_SIZE;
        for faces in [[quad(0.2, 0), quad(0.8, 1)], [quad(0.8, 1), quad(0.2, 0)]] {
            let icon = block_icon(&faces, &tiles, s);
            // The middle of the icon is covered by both; blue is nearer.
            assert_eq!(pixel(&icon, s, s / 2, s / 2)[..3], [0, 0, 255]);
        }
    }

    #[test]
    fn items_with_a_texture_show_their_tile_flat() {
        let content = base();
        let pickaxe = content.default_item_look("defaria:pickaxe").unwrap();
        let layer = pickaxe.texture.unwrap() as usize;
        let tiles = solid_tiles(|i| [i as u8, 1, 2, 255]);
        let icon = item_icon(content, pickaxe, &tiles);
        assert!(icon.flat);
        assert_eq!(icon.size, TILE_SIZE);
        assert_eq!(icon.data, tiles[layer]);

        let stone = content.default_item_look("defaria:stone").unwrap();
        let icon = item_icon(content, stone, &tiles);
        assert!(!icon.flat);
        assert_eq!(icon.size, BLOCK_ICON_SIZE);
    }

    /// Before the tiles are built there's nothing to draw, and nothing
    /// panics.
    #[test]
    fn missing_tiles_draw_nothing() {
        let content = base();
        for item in &content.item_states {
            let icon = item_icon(content, item, &[]);
            assert!(icon.data.chunks(4).all(|p| p[3] == 0), "{}", item.name);
        }
    }
}
