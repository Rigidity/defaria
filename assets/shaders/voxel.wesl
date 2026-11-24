// Chunk shader: unlit, textured from the block texture array.
//
// Bindings must match `VoxelMaterial` in src/voxel_material.rs. Uses Bevy's
// default mesh vertex shader, so `in.uv` (UV_0) is the position within the
// face's tile, `in.uv_b.x` (UV_1) is the tile's layer in the array, and
// `in.world_normal` is the face normal.

import bevy_pbr::render::forward_io::VertexOutput;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var blocks: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var blocks_sampler: sampler;

// Fixed brightness per face direction, so neighbouring faces of a block
// never look the same. Not lighting: nothing depends on a light or the view.
const SHADE_TOP: f32 = 1.0;
const SHADE_X: f32 = 0.8;
const SHADE_Z: f32 = 0.65;
const SHADE_BOTTOM: f32 = 0.5;

fn face_shade(normal: vec3<f32>) -> f32 {
    let n = normalize(normal);
    if n.y > 0.5 {
        return SHADE_TOP;
    }
    if n.y < -0.5 {
        return SHADE_BOTTOM;
    }
    if abs(n.x) > 0.5 {
        return SHADE_X;
    }
    return SHADE_Z;
}

// Crisp pixels from a linear-filtering sampler. Close up, each screen pixel
// covers less than a texel: snap the UV to the nearest texel centre, except
// within one screen pixel of a texel edge, where it blends across the edge.
// That gives hard pixel edges without the stair-step aliasing of nearest
// filtering. Far away, a texel is smaller than a pixel, the snap does
// nothing, and the mipmaps take over.
fn pixel_art_uv(uv: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    let texel = uv * size;
    // Capped at 1, where the snap becomes the identity.
    let texels_per_pixel = clamp(fwidth(texel), vec2<f32>(1e-5), vec2<f32>(1.0));
    let edge = floor(texel + 0.5);
    let offset = clamp((texel - edge) / texels_per_pixel, vec2<f32>(-0.5), vec2<f32>(0.5));
    return (edge + offset) / size;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(blocks));
    let layer = i32(round(in.uv_b.x));
    // Pass the unsnapped UV's derivatives so mip level and anisotropy are
    // chosen from how the face is actually seen, not from the snapped UV.
    let texel = textureSampleGrad(
        blocks,
        blocks_sampler,
        pixel_art_uv(in.uv, size),
        layer,
        dpdx(in.uv),
        dpdy(in.uv),
    );
    return vec4<f32>(texel.rgb * face_shade(in.world_normal), 1.0);
}
