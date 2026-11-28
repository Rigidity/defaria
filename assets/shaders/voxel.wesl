// Chunk shader: the block texture array, lit by the light baked into the
// mesh and by the dynamic lights.
//
// Bindings must match `VoxelMaterial` in crates/defaria_client/src/voxel_material.rs, and the
// vertex attributes the locations its `specialize` gives them.
//
// Vertices are packed (see crates/defaria_client/src/meshing/packed.rs): four words naming
// the voxel in the chunk, a corner of a face in the model face table
// (`faces`, which holds where the corner sits in the voxel, its texture
// coordinates, the face's normal and texture), the corner's light and
// ambient occlusion and the face's twist, and a fluid face's height; and a
// fluid face's flow. The fragment stage blends a face's four corners'
// light and occlusion bilinearly: the GPU interpolates each vertex's own
// corner across its triangle, and `untwist` takes off what that differs
// from the bilinear blend, from the face's twist. Interpolating alone
// would blend across each of the quad's two triangles separately, which
// shows the diagonal as bands and creases.
//
// Animated textures: a texture's first frame is the array layer of its own
// index, and its other frames come after every texture's first. The
// `animations` table (see `block_texture::Animations` in
// crates/defaria_client/src/block_texture.rs) says, per texture, how many frames it has, where
// the rest are, and how it moves; the time comes from Bevy's globals, so
// animating needs no remeshing or uploads.
//
// Fluid faces take their texture coordinates from where they are, wrapping
// every block (WRAPPED), and flowing ones (FLOWING) move their texture by a
// flow map: a flow vector at each vertex, which the mesher averages over
// the voxels sharing the corner (see crates/defaria_client/src/meshing/scroll.rs), blended across
// the face, so the flow runs on continuously from tile to tile. Two
// samples half a cycle apart, each pushed along the flow and pulled back
// once a cycle, cross-fade so neither's reset or stretching shows.
//
// Distance fog comes from the camera's `DistanceFog` (see crates/defaria_client/src/fog.rs), in the
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

// One texture's entry in the animation table. Must match
// `Animations::words` in crates/defaria_client/src/block_texture.rs.
struct Animation {
    // How many frames it steps through: 1 for a still texture.
    frames: u32,
    // The array layer of its second frame; the rest follow it.
    rest: u32,
    // Seconds a frame shows for.
    frame_time: f32,
    // Blocks a second it scrolls along a flowing face.
    scroll: f32,
    // 1 if each frame fades into the next.
    blend: u32,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<storage, read> animations: array<Animation>;

// One face of a block's model, wherever it's drawn. Must match
// `ModelFace::words` in crates/defaria_client/src/meshing/model_faces.rs.
struct ModelFace {
    // Each corner's position in its voxel, then its texture's u.
    corners: array<vec4<f32>, 4>,
    // Each corner's v.
    v: vec4<f32>,
    normal: vec3<f32>,
    // The texture's index, with TRIANGLE on a triangle.
    layer: u32,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(4) var<storage, read> faces: array<ModelFace>;

const TRIANGLE: u32 = 0x80000000u;

// The packed vertex's fields: word, lowest bit and width. Must match
// crates/defaria_client/src/meshing/packed.rs.
const LIGHT: vec3<u32> = vec3<u32>(0u, 0u, 32u);
const LIGHT_TWIST_SKY: vec3<u32> = vec3<u32>(1u, 0u, 10u);
const LIGHT_TWIST_RED: vec3<u32> = vec3<u32>(1u, 10u, 10u);
const LIGHT_TWIST_GREEN: vec3<u32> = vec3<u32>(1u, 20u, 10u);
const LIGHT_TWIST_BLUE: vec3<u32> = vec3<u32>(2u, 0u, 10u);
const AO: vec3<u32> = vec3<u32>(1u, 30u, 2u);
const AO_TWIST: vec3<u32> = vec3<u32>(2u, 10u, 4u);
const VOXEL_X: vec3<u32> = vec3<u32>(2u, 14u, 4u);
const VOXEL_Y: vec3<u32> = vec3<u32>(2u, 18u, 4u);
const VOXEL_Z: vec3<u32> = vec3<u32>(2u, 22u, 4u);
const CORNER: vec3<u32> = vec3<u32>(2u, 26u, 2u);
const BACK: vec3<u32> = vec3<u32>(2u, 28u, 1u);
const FLOWING: vec3<u32> = vec3<u32>(2u, 29u, 1u);
const WRAPPED: vec3<u32> = vec3<u32>(2u, 30u, 1u);
const FACE: vec3<u32> = vec3<u32>(3u, 0u, 16u);
const HEIGHT: vec3<u32> = vec3<u32>(3u, 16u, 16u);
// A block's height in HEIGHT's steps.
const HEIGHT_STEPS: f32 = 49152.0;

fn field(words: vec4<u32>, at: vec3<u32>) -> u32 {
    return extractBits(words[at.x], at.y, at.z);
}

// A field holding a signed number.
fn signed_field(words: vec4<u32>, at: vec3<u32>) -> f32 {
    return f32(extractBits(bitcast<i32>(words[at.x]), at.y, at.z));
}

// Light levels run from 0 to this.
const MAX_LIGHT: f32 = 15.0;
// Each level down is this much dimmer.
const LEVEL_FALLOFF: f32 = 0.85;
// The least light anything gets, so unlit caves aren't pure black.
const AMBIENT: f32 = 0.025;
// Brightness of a fully occluded corner (ambient occlusion level 0); level 3
// is open and fully bright.
const OCCLUDED: f32 = 0.3;
// A double-sided block's back face (BACK) is drawn this much farther away,
// as a share of its distance, so a neighbour's face in the same plane (a
// slab's side beside water or lava) wins the depth test instead of
// flickering with it.
const BACK_DEPTH_BIAS: f32 = 2e-5;
// The fragment stage's flags: a fluid face's texture coordinates wrap
// every block, and a flowing one's move.
const WRAPS: u32 = 1u;
const FLOWS: u32 = 2u;
// Seconds each of the flow map's two phases takes to push its sample along
// and start again.
const FLOW_CYCLE: f32 = 1.0;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) packed: vec4<u32>,
    // A fluid face's flow here, in its texture coordinates; zero elsewhere.
    @location(1) flow: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Where on the face: (0, 0) at its first corner, (1, 0) at the second,
    // (1, 1) at the third and (0, 1) at the fourth.
    @location(3) corner: vec2<f32>,
    // Sky, red, green and blue light levels, 0 to 15, interpolated between
    // the vertices' own corners, and the face's twist.
    @location(4) light: vec4<f32>,
    @location(5) @interpolate(flat) light_twist: vec4<f32>,
    // Ambient occlusion, 0 (fully occluded) to 3 (open), the same way.
    @location(6) occlusion: f32,
    @location(7) @interpolate(flat) occlusion_twist: f32,
    // The texture, then WRAPS and FLOWS.
    @location(8) @interpolate(flat) face: vec2<u32>,
    @location(9) flow: vec2<f32>,
};

// The horizontal axis along a side facing `normal`, a quarter turn from
// it. As `across` in crates/defaria_client/src/meshing/scroll.rs.
fn across(normal: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(-round(normal.z), 0.0, round(normal.x));
}

// A fluid face's texture coordinates at `position`: where it is along its
// plane. As `uv_at` in crates/defaria_client/src/meshing/scroll.rs.
fn uv_at(normal: vec3<f32>, position: vec3<f32>) -> vec2<f32> {
    if abs(normal.y) > 0.5 {
        return position.xz;
    }
    return vec2<f32>(dot(position, across(normal)), -position.y);
}

// Where corner `corner` of a face is: see `corner` in VertexOutput. A
// triangle's third vertex is the fourth corner.
fn corner_position(corner: u32, triangle: bool) -> vec2<f32> {
    let x = corner == 1u || (corner == 2u && !triangle);
    let y = corner >= 2u;
    return vec2<f32>(select(0.0, 1.0, x), select(0.0, 1.0, y));
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let words = vertex.packed;
    let face = field(words, FACE);
    let corner = field(words, CORNER);
    let model = faces[face].corners[corner];
    let normal = faces[face].normal;
    let layer = faces[face].layer;
    let voxel = vec3<f32>(vec3<u32>(field(words, VOXEL_X), field(words, VOXEL_Y), field(words, VOXEL_Z)));
    let wrapped = field(words, WRAPPED) != 0u;
    var local = model.xyz;
    if wrapped {
        // A fluid's surface, at the height its neighbours set.
        local.y = local.y * f32(field(words, HEIGHT)) / HEIGHT_STEPS;
    }
    let position = voxel + local;
    let back = field(words, BACK) != 0u;

    let world_from_local = get_world_from_local(vertex.instance_index);
    let world_position = mesh_position_local_to_world(world_from_local, vec4<f32>(position, 1.0));
    out.position = position_world_to_clip(world_position.xyz);
    if back {
        // Reversed depth: smaller is farther.
        out.position.z = out.position.z * (1.0 - BACK_DEPTH_BIAS);
    }
    out.world_position = world_position.xyz;
    out.world_normal = mesh_normal_local_to_world(select(normal, -normal, back), vertex.instance_index);
    out.uv = select(vec2<f32>(model.w, faces[face].v[corner]), uv_at(normal, position), wrapped);
    out.corner = corner_position(corner, (layer & TRIANGLE) != 0u);
    out.light = unpack4x8unorm(field(words, LIGHT)) * MAX_LIGHT;
    out.light_twist = vec4<f32>(
        signed_field(words, LIGHT_TWIST_SKY),
        signed_field(words, LIGHT_TWIST_RED),
        signed_field(words, LIGHT_TWIST_GREEN),
        signed_field(words, LIGHT_TWIST_BLUE),
    ) / 255.0 * MAX_LIGHT;
    out.occlusion = f32(field(words, AO));
    out.occlusion_twist = signed_field(words, AO_TWIST);
    let flags = select(0u, WRAPS, wrapped) | select(0u, FLOWS, field(words, FLOWING) != 0u);
    out.face = vec2<u32>(layer & ~TRIANGLE, flags);
    out.flow = vertex.flow;
    return out;
}

// The bilinear blend of a face's four corners at `corner`, from the value
// interpolated between the vertices of the triangle it's in and the
// face's twist (c0 - c1 + c2 - c3). As `untwist` in
// crates/defaria_client/src/meshing/packed.rs.
fn untwist(interpolated: vec4<f32>, twist: vec4<f32>, corner: vec2<f32>) -> vec4<f32> {
    return interpolated - min(corner.x, corner.y) * (1.0 - max(corner.x, corner.y)) * twist;
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
// nothing, and the mipmaps take over. `texels_per_pixel` is how many
// texels a screen pixel covers, capped at 1, where the snap becomes the
// identity; it's worked out once (derivatives need uniform control flow)
// and serves the flow map's shifted samples too.
fn pixel_art_uv(uv: vec2<f32>, size: vec2<f32>, texels_per_pixel: vec2<f32>) -> vec2<f32> {
    let texel = uv * size;
    let edge = floor(texel + 0.5);
    let offset = clamp((texel - edge) / texels_per_pixel, vec2<f32>(-0.5), vec2<f32>(0.5));
    return (edge + offset) / size;
}

struct Frame {
    layer: u32,
    next: u32,
    fade: f32,
};

// The array layer of texture `texture`'s frame `frame`.
fn frame_layer(texture: u32, animation: Animation, frame: u32) -> u32 {
    return select(animation.rest + frame - 1u, texture, frame == 0u);
}

// What texture `texture` shows `time` seconds in: frame after frame, each
// for its frame time, fading into the next if it blends. As
// `Animations::frame_at` in crates/defaria_client/src/block_texture.rs.
fn animated_frame(texture: u32, animation: Animation, time: f32) -> Frame {
    var out: Frame;
    out.layer = texture;
    out.next = texture;
    out.fade = 0.0;
    if animation.frames > 1u {
        let steps = time / animation.frame_time;
        let frame = u32(floor(steps)) % animation.frames;
        out.layer = frame_layer(texture, animation, frame);
        out.next = frame_layer(texture, animation, (frame + 1u) % animation.frames);
        out.fade = select(0.0, fract(steps), animation.blend != 0u);
    }
    return out;
}

// Samples `frame` at `uv`, faded into the next frame if it blends. Explicit
// gradients (the unshifted UV's, so mip level and anisotropy follow how
// the face is seen), so sampling needn't be in uniform control flow, and
// only blending textures pay for the second sample.
fn sample_frame(uv: vec2<f32>, frame: Frame, ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    var texel = textureSampleGrad(blocks, blocks_sampler, uv, i32(frame.layer), ddx, ddy);
    if frame.fade > 0.0 {
        let next = textureSampleGrad(blocks, blocks_sampler, uv, i32(frame.next), ddx, ddy);
        texel = mix(texel, next, frame.fade);
    }
    return texel;
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
    let time = bevy_pbr::render::mesh_view_bindings::globals.time;
    let texture = in.face.x;
    let animation = animations[texture];
    let texels_per_pixel = clamp(fwidth(in.uv * size), vec2<f32>(1e-5), vec2<f32>(1.0));
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let frame = animated_frame(texture, animation, time);
    var texel: vec4<f32>;
    if (in.face.y & FLOWS) != 0u {
        // The flow map: two samples pushed along the flow, half a cycle
        // apart, each pulled back to the start once a cycle. Each fades out
        // as it nears its reset, while the other is halfway.
        let phase = fract(time / FLOW_CYCLE);
        let shift = in.flow * animation.scroll * FLOW_CYCLE;
        let first = fract(pixel_art_uv(in.uv - shift * phase, size, texels_per_pixel));
        let second = fract(pixel_art_uv(in.uv - shift * fract(phase + 0.5), size, texels_per_pixel));
        let a = sample_frame(first, frame, ddx, ddy);
        let b = sample_frame(second, frame, ddx, ddy);
        texel = mix(a, b, abs(1.0 - 2.0 * phase));
    } else {
        // Still: one sample, where the face is. A fluid face's coordinates
        // run on past its block, so they wrap.
        let snapped = pixel_art_uv(in.uv, size, texels_per_pixel);
        let wrapped = (in.face.y & WRAPS) != 0u;
        texel = sample_frame(select(snapped, fract(snapped), wrapped), frame, ddx, ddy);
    }

    let baked = untwist(in.light, in.light_twist, in.corner);
    let normal = normalize(in.world_normal);
    // Block and dynamic light combine like two block lights: the brighter
    // wins, per channel. Then the brighter of that and the sky.
    let block = max(baked.yzw, dynamic_light(in.world_position, normal));
    let sky = lighting.sky.rgb * brightness(vec3<f32>(baked.x));
    let light = max(max(sky, brightness(block)), vec3<f32>(AMBIENT));
    let occlusion = untwist(vec4<f32>(in.occlusion), vec4<f32>(in.occlusion_twist), in.corner).x;
    let ao = mix(OCCLUDED, 1.0, occlusion / 3.0);
    let alpha = cut_out(texel.a);
    return vec4<f32>(fogged(texel.rgb * light * ao, in.world_position), alpha);
}
