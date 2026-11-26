// Chunk shader: the block texture array, lit by the light baked into the
// mesh and by the dynamic lights.
//
// Bindings must match `VoxelMaterial` in src/voxel_material.rs, and the
// vertex attributes the locations its `specialize` gives them. Every vertex
// of a face carries all four of the face's corners' light and ambient
// occlusion, and where it sits on the face (`corner`), so the fragment stage
// can blend the corners bilinearly. Letting the GPU interpolate per vertex
// instead would blend across each of the quad's two triangles separately,
// which shows the diagonal as bands and creases.
//
// Distance fog comes from the camera's `DistanceFog` (see src/fog.rs), in the
// view's bindings, when it has one. It's a cylinder around the camera, like
// the loaded world: `be` holds how far it reaches sideways, up and down, and
// `bi.x` where it starts thickening, as a fraction of that.

import bevy_pbr::render::mesh_functions::{get_world_from_local, mesh_position_local_to_world, mesh_normal_local_to_world};
import bevy_pbr::render::view_transformations::position_world_to_clip;

const MAX_DYNAMIC_LIGHTS: u32 = 16u;

struct DynamicLight {
    position: vec4<f32>,
    // Red, green and blue levels, 0 to 15.
    levels: vec4<f32>,
};

struct Lighting {
    // Sky light's linear colour at full strength.
    sky: vec4<f32>,
    dynamic_count: u32,
    dynamic: array<DynamicLight, MAX_DYNAMIC_LIGHTS>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var blocks: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var blocks_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var<uniform> lighting: Lighting;

// Light levels run from 0 to this.
const MAX_LIGHT: f32 = 15.0;
// Each level down is this much dimmer.
const LEVEL_FALLOFF: f32 = 0.85;
// The least light anything gets, so unlit caves aren't pure black.
const AMBIENT: f32 = 0.025;
// Brightness of a fully occluded corner (ambient occlusion level 0); level 3
// is open and fully bright.
const OCCLUDED: f32 = 0.3;
// Set on the layer of a double-sided block's back face (see BACK_FACE in
// src/voxel.rs). A back is drawn this much farther away, as a share of its
// distance, so a neighbour's face in the same plane (a slab's side beside
// water or lava) wins the depth test instead of flickering with it.
const BACK_FACE: u32 = 0x80000000u;
const BACK_DEPTH_BIAS: f32 = 2e-5;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // (0, 0), (1, 0), (1, 1) or (0, 1): which corner of the face this is.
    @location(3) corner: vec2<f32>,
    // Each corner's light: sky, red, green and blue as bytes, 255 = level 15.
    @location(4) light: vec4<u32>,
    // The texture layer (with BACK_FACE on a back face), then each corner's
    // ambient occlusion as bytes, 0-3.
    @location(5) face: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) corner: vec2<f32>,
    @location(4) @interpolate(flat) light: vec4<u32>,
    @location(5) @interpolate(flat) face: vec2<u32>,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = get_world_from_local(vertex.instance_index);
    let world_position = mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    out.position = position_world_to_clip(world_position.xyz);
    if (vertex.face.x & BACK_FACE) != 0u {
        // Reversed depth: smaller is farther.
        out.position.z = out.position.z * (1.0 - BACK_DEPTH_BIAS);
    }
    out.world_position = world_position.xyz;
    out.world_normal = mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
    out.uv = vertex.uv;
    out.corner = vertex.corner;
    out.light = vertex.light;
    out.face = vertex.face;
    return out;
}

// Blends the four corners' values at `corner` on the face.
fn bilinear(c: array<vec4<f32>, 4>, corner: vec2<f32>) -> vec4<f32> {
    let bottom = mix(c[0], c[1], corner.x);
    let top = mix(c[3], c[2], corner.x);
    return mix(bottom, top, corner.y);
}

// Sky, red, green and blue light levels here, 0 to 15.
fn baked_light(light: vec4<u32>, corner: vec2<f32>) -> vec4<f32> {
    let corners = array<vec4<f32>, 4>(
        unpack4x8unorm(light.x),
        unpack4x8unorm(light.y),
        unpack4x8unorm(light.z),
        unpack4x8unorm(light.w),
    );
    return bilinear(corners, corner) * MAX_LIGHT;
}

// Ambient occlusion here, 0 (fully occluded) to 3 (open).
fn occlusion(packed: u32, corner: vec2<f32>) -> f32 {
    let levels = unpack4x8unorm(packed) * 255.0;
    let corners = array<vec4<f32>, 4>(
        vec4<f32>(levels.x),
        vec4<f32>(levels.y),
        vec4<f32>(levels.z),
        vec4<f32>(levels.w),
    );
    return bilinear(corners, corner).x;
}

// How bright a light level looks: 0 at level 0, 1 at level 15, each level
// LEVEL_FALLOFF times the one above.
fn brightness(level: vec3<f32>) -> vec3<f32> {
    let darkest = pow(LEVEL_FALLOFF, MAX_LIGHT);
    let scaled = pow(vec3<f32>(LEVEL_FALLOFF), MAX_LIGHT - clamp(level, vec3<f32>(0.0), vec3<f32>(MAX_LIGHT)));
    return (scaled - darkest) / (1.0 - darkest);
}

// Red, green and blue levels from the dynamic lights, measured like block
// light: each light's level minus its distance to the middle of the voxel in
// front of the face. Surfaces facing away from a light don't get it.
fn dynamic_light(position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var levels = vec3<f32>(0.0);
    let front = position + normal * 0.5;
    for (var i = 0u; i < min(lighting.dynamic_count, MAX_DYNAMIC_LIGHTS); i = i + 1u) {
        let light = lighting.dynamic[i];
        let to_light = light.position.xyz - position;
        let facing = saturate(1.0 + 2.0 * dot(normal, to_light) / max(length(to_light), 1e-4));
        let reach = light.levels.xyz - distance(light.position.xyz, front);
        levels = max(levels, max(reach, vec3<f32>(0.0)) * facing);
    }
    return levels;
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

// The alpha a texel is drawn with. Cut-out faces use alpha to coverage
// with multisampling: the alpha is sharpened by how fast it changes across
// the screen, so a hole's edge is about one pixel wide at any distance and
// the samples smooth it. (The block texture's mipmaps keep each tile's
// coverage, so cut-outs don't fade away with distance.) Without
// multisampling they're an alpha test. Opaque and blended faces keep their
// alpha.
@if(ALPHA_TO_COVERAGE)
fn cut_out(alpha: f32) -> f32 {
    return (alpha - 0.5) / max(fwidth(alpha), 1e-4) + 0.5;
}
@if(MAY_DISCARD)
fn cut_out(alpha: f32) -> f32 {
    if alpha < 0.5 {
        discard;
    }
    return 1.0;
}
@if(!ALPHA_TO_COVERAGE && !MAY_DISCARD)
fn cut_out(alpha: f32) -> f32 {
    return alpha;
}

// Fades `color` into the fog with distance from the camera.
@if(DISTANCE_FOG)
fn fogged(color: vec3<f32>, position: vec3<f32>) -> vec3<f32> {
    let fog = bevy_pbr::render::mesh_view_bindings::fog;
    let offset = position - bevy_pbr::render::mesh_view_bindings::view.world_position;
    // How far through the cylinder: 1 on its surface.
    let depth = max(
        length(offset.xz) / fog.be.x,
        max(offset.y / fog.be.y, -offset.y / fog.be.z),
    );
    let amount = smoothstep(fog.bi.x, 1.0, depth);
    return mix(color, fog.base_color.rgb, amount);
}
@else
fn fogged(color: vec3<f32>, position: vec3<f32>) -> vec3<f32> {
    return color;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(blocks));
    // Pass the unsnapped UV's derivatives so mip level and anisotropy are
    // chosen from how the face is actually seen, not from the snapped UV.
    let texel = textureSampleGrad(
        blocks,
        blocks_sampler,
        pixel_art_uv(in.uv, size),
        i32(in.face.x & ~BACK_FACE),
        dpdx(in.uv),
        dpdy(in.uv),
    );

    let baked = baked_light(in.light, in.corner);
    let normal = normalize(in.world_normal);
    // Block and dynamic light combine like two block lights: the brighter
    // wins, per channel. Then the brighter of that and the sky.
    let block = max(baked.yzw, dynamic_light(in.world_position, normal));
    let sky = lighting.sky.rgb * brightness(vec3<f32>(baked.x));
    let light = max(max(sky, brightness(block)), vec3<f32>(AMBIENT));
    let ao = mix(OCCLUDED, 1.0, occlusion(in.face.y, in.corner) / 3.0);
    let alpha = cut_out(texel.a);
    return vec4<f32>(fogged(texel.rgb * light * ao, in.world_position), alpha);
}
