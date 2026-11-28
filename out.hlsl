struct DynamicLight {
    float4 position;
    float4 levels;
};

struct Lighting {
    float4 sky;
    uint dynamic_count;
    int _pad2_0;
    int _pad2_1;
    int _pad2_2;
    DynamicLight dynamic[16];
};

struct Animation {
    uint frames;
    uint rest;
    float frame_time;
    float scroll;
    uint blend;
};

struct Vertex {
    uint instance_index : SV_InstanceID;
    float3 position : LOC0;
    float3 normal : LOC1;
    float2 uv : LOC2;
    float2 corner : LOC3;
    uint4 light : LOC4;
    uint2 face : LOC5;
    float2 flow : LOC6;
};

struct VertexOutput {
    float4 position : SV_Position;
    float3 world_position : LOC0;
    float3 world_normal : LOC1;
    float2 uv : LOC2;
    float2 corner : LOC3;
    nointerpolation uint4 light : LOC4;
    nointerpolation uint2 face : LOC5;
    float2 flow : LOC6;
};

struct Frame {
    uint layer;
    uint next;
    float fade;
};

static const uint MAX_DYNAMIC_LIGHTS = 16u;
static const float MAX_LIGHT = 15.0;
static const float LEVEL_FALLOFF = 0.85;
static const float AMBIENT = 0.025;
static const float OCCLUDED = 0.3;
static const uint BACK_FACE = 2147483648u;
static const float BACK_DEPTH_BIAS = 2e-5;
static const uint TEXTURE_BITS = 16777215u;
static const uint FLOWING = 268435456u;
static const uint WRAPPED = 536870912u;
static const float FLOW_CYCLE = 1.0;

cbuffer globals_time : register(b0) { float globals_time; }
ByteAddressBuffer models : register(t1);
Texture2DArray<float4> blocks : register(t0, space2);
SamplerState nagaSamplerHeap[2048]: register(s0, space0);
SamplerComparisonState nagaComparisonSamplerHeap[2048]: register(s0, space1);
StructuredBuffer<uint> nagaGroup2SamplerIndexArray : register(t2, space255);
static const SamplerState blocks_sampler = nagaSamplerHeap[nagaGroup2SamplerIndexArray[1]];
cbuffer lighting : register(b2, space2) { Lighting lighting; }
ByteAddressBuffer animations : register(t3, space2);

struct VertexOutput_vertex {
    float3 world_position : LOC0;
    float3 world_normal : LOC1;
    float2 uv_2 : LOC2;
    float2 corner_3 : LOC3;
    nointerpolation uint4 light_1 : LOC4;
    nointerpolation uint2 face : LOC5;
    float2 flow : LOC6;
    float4 position_2 : SV_Position;
};

struct FragmentInput_fragment {
    float3 world_position_1 : LOC0;
    float3 world_normal_1 : LOC1;
    float2 uv_3 : LOC2;
    float2 corner_4 : LOC3;
    nointerpolation uint4 light_2 : LOC4;
    nointerpolation uint2 face_1 : LOC5;
    float2 flow_1 : LOC6;
    float4 position_3 : SV_Position;
};

float4x4 get_world_from_local(uint i)
{
    float4x4 _e3 = float4x4(asfloat(models.Load4(i*64+0)), asfloat(models.Load4(i*64+16)), asfloat(models.Load4(i*64+32)), asfloat(models.Load4(i*64+48)));
    return _e3;
}

float4 mesh_position_local_to_world(float4x4 m, float4 p)
{
    return mul(p, m);
}

float3 mesh_normal_local_to_world(float3 n, uint i_1)
{
    return n;
}

float4 position_world_to_clip(float3 p_1)
{
    return float4(p_1, 1.0);
}

float4 bilinear(float4 c[4], float2 corner)
{
    float4 bottom = lerp(c[0], c[1], corner.x);
    float4 top = lerp(c[3], c[2], corner.x);
    return lerp(bottom, top, corner.y);
}

typedef float4 ret_Constructarray4_float4_[4];
ret_Constructarray4_float4_ Constructarray4_float4_(float4 arg0, float4 arg1, float4 arg2, float4 arg3) {
    float4 ret[4] = { arg0, arg1, arg2, arg3 };
    return ret;
}

float4 baked_light(uint4 light, float2 corner_1)
{
    uint _e2 = light.x;
    uint _e4 = light.y;
    uint _e6 = light.z;
    uint _e8 = light.w;
    float4 corners[4] = Constructarray4_float4_((float4(_e2 & 0xFF, _e2 >> 8 & 0xFF, _e2 >> 16 & 0xFF, _e2 >> 24) / 255.0), (float4(_e4 & 0xFF, _e4 >> 8 & 0xFF, _e4 >> 16 & 0xFF, _e4 >> 24) / 255.0), (float4(_e6 & 0xFF, _e6 >> 8 & 0xFF, _e6 >> 16 & 0xFF, _e6 >> 24) / 255.0), (float4(_e8 & 0xFF, _e8 >> 8 & 0xFF, _e8 >> 16 & 0xFF, _e8 >> 24) / 255.0));
    const float4 _e11 = bilinear(corners, corner_1);
    return (_e11 * MAX_LIGHT);
}

float occlusion(uint packed, float2 corner_2)
{
    float4 levels_1 = ((float4(packed & 0xFF, packed >> 8 & 0xFF, packed >> 16 & 0xFF, packed >> 24) / 255.0) * 255.0);
    float4 corners_1[4] = Constructarray4_float4_((levels_1.x).xxxx, (levels_1.y).xxxx, (levels_1.z).xxxx, (levels_1.w).xxxx);
    const float4 _e14 = bilinear(corners_1, corner_2);
    return _e14.x;
}

float3 brightness(float3 level)
{
    float3 scaled = pow((0.85).xxx, ((15.0).xxx - clamp(level, (0.0).xxx, (15.0).xxx)));
    return ((scaled - (0.08735426).xxx) / ((1.0 - 0.08735426)).xxx);
}

float3 dynamic_light(float3 position, float3 normal)
{
    float3 levels = (0.0).xxx;
    uint i_2 = 0u;

    float3 front = (position + (normal * 0.5));
    uint2 loop_bound = uint2(4294967295u, 4294967295u);
    bool loop_init = true;
    while(true) {
        if (all(loop_bound == uint2(0u, 0u))) { break; }
        loop_bound -= uint2(loop_bound.y == 0u, 1u);
        if (!loop_init) {
            uint _e48 = i_2;
            i_2 = (_e48 + 1u);
        }
        loop_init = false;
        uint _e10 = i_2;
        uint _e13 = lighting.dynamic_count;
        if ((_e10 < min(_e13, MAX_DYNAMIC_LIGHTS))) {
        } else {
            break;
        }
        {
            uint _e19 = i_2;
            DynamicLight light_3 = lighting.dynamic[_e19];
            float3 to_light = (light_3.position.xyz - position);
            float facing = saturate((1.0 + ((2.0 * dot(normal, to_light)) / max(length(to_light), 0.0001))));
            float3 reach = (light_3.levels.xyz - (distance(light_3.position.xyz, front)).xxx);
            float3 _e42 = levels;
            levels = max(_e42, (max(reach, (0.0).xxx) * facing));
        }
    }
    float3 _e51 = levels;
    return _e51;
}

float2 pixel_art_uv(float2 uv, float2 size, float2 texels_per_pixel)
{
    float2 texel_2 = (uv * size);
    float2 edge = floor((texel_2 + (0.5).xx));
    float2 offset = clamp(((texel_2 - edge) / texels_per_pixel), (-0.5).xx, (0.5).xx);
    return ((edge + offset) / size);
}

uint frame_layer(uint texture_, Animation animation, uint frame)
{
    return ((frame == 0u) ? texture_ : ((animation.rest + frame) - 1u));
}

uint naga_mod(uint lhs, uint rhs) {
    return lhs % (rhs == 0u ? 1u : rhs);
}

uint naga_f2u32(float value) {
    return uint(clamp(value, 0.0, 4294967000.0));
}

Frame animated_frame(uint texture_1, Animation animation_1, float time)
{
    Frame out_1 = (Frame)0;

    out_1.layer = texture_1;
    out_1.next = texture_1;
    out_1.fade = 0.0;
    if ((animation_1.frames > 1u)) {
        float steps = (time / animation_1.frame_time);
        uint frame_2 = naga_mod(naga_f2u32(floor(steps)), animation_1.frames);
        const uint _e18 = frame_layer(texture_1, animation_1, frame_2);
        out_1.layer = _e18;
        const uint _e24 = frame_layer(texture_1, animation_1, naga_mod((frame_2 + 1u), animation_1.frames));
        out_1.next = _e24;
        out_1.fade = ((animation_1.blend != 0u) ? frac(steps) : 0.0);
    }
    Frame _e32 = out_1;
    const Frame frame_3 = _e32;
    return frame_3;
}

float4 sample_frame(float2 uv_1, Frame frame_1, float2 ddx_, float2 ddy_)
{
    float4 texel_1 = (float4)0;

    float4 _e8 = blocks.SampleGrad(blocks_sampler, float3(uv_1, int(frame_1.layer)), ddx_, ddy_);
    texel_1 = _e8;
    if ((frame_1.fade > 0.0)) {
        float4 next = blocks.SampleGrad(blocks_sampler, float3(uv_1, int(frame_1.next)), ddx_, ddy_);
        float4 _e18 = texel_1;
        texel_1 = lerp(_e18, next, frame_1.fade);
    }
    float4 _e21 = texel_1;
    return _e21;
}

float cut_out(float alpha)
{
    float _e3 = fwidth(alpha);
    return (((alpha - 0.5) / max(_e3, 0.0001)) + 0.5);
}

float3 fogged(float3 color, float3 position_1)
{
    return color;
}

VertexOutput_vertex vertex(Vertex vertex_1)
{
    VertexOutput out_ = (VertexOutput)0;

    const float4x4 _e3 = get_world_from_local(vertex_1.instance_index);
    const float4 _e7 = mesh_position_local_to_world(_e3, float4(vertex_1.position, 1.0));
    const float4 _e10 = position_world_to_clip(_e7.xyz);
    out_.position = _e10;
    if (((vertex_1.face.x & BACK_FACE) != 0u)) {
        float _e21 = out_.position.z;
        out_.position.z = (_e21 * 0.99998);
    }
    out_.world_position = _e7.xyz;
    const float3 _e29 = mesh_normal_local_to_world(vertex_1.normal, vertex_1.instance_index);
    out_.world_normal = _e29;
    out_.uv = vertex_1.uv;
    out_.corner = vertex_1.corner;
    out_.light = vertex_1.light;
    out_.face = vertex_1.face;
    out_.flow = vertex_1.flow;
    VertexOutput _e40 = out_;
    const VertexOutput vertexoutput = _e40;
    const VertexOutput_vertex vertexoutput_1 = { vertexoutput.world_position, vertexoutput.world_normal, vertexoutput.uv, vertexoutput.corner, vertexoutput.light, vertexoutput.face, vertexoutput.flow, vertexoutput.position };
    return vertexoutput_1;
}

uint2 NagaDimensions2DArray(Texture2DArray<float4> tex)
{
    uint4 ret;
    tex.GetDimensions(0, ret.x, ret.y, ret.z, ret.w);
    return ret.xy;
}

Animation ConstructAnimation(uint arg0, uint arg1, float arg2, float arg3, uint arg4) {
    Animation ret = (Animation)0;
    ret.frames = arg0;
    ret.rest = arg1;
    ret.frame_time = arg2;
    ret.scroll = arg3;
    ret.blend = arg4;
    return ret;
}

float4 fragment(FragmentInput_fragment fragmentinput_fragment) : SV_Target0
{
    VertexOutput in_ = { fragmentinput_fragment.position_3, fragmentinput_fragment.world_position_1, fragmentinput_fragment.world_normal_1, fragmentinput_fragment.uv_3, fragmentinput_fragment.corner_4, fragmentinput_fragment.light_2, fragmentinput_fragment.face_1, fragmentinput_fragment.flow_1 };
    float4 texel = (float4)0;

    float2 size_1 = float2(NagaDimensions2DArray(blocks));
    float time_1 = globals_time;
    uint texture_2 = (in_.face.x & TEXTURE_BITS);
    Animation animation_2 = ConstructAnimation(asuint(animations.Load(texture_2*20+0)), asuint(animations.Load(texture_2*20+4)), asfloat(animations.Load(texture_2*20+8)), asfloat(animations.Load(texture_2*20+12)), asuint(animations.Load(texture_2*20+16)));
    float2 _e15 = fwidth((in_.uv * size_1));
    float2 texels_per_pixel_1 = clamp(_e15, (1e-5).xx, (1.0).xx);
    float2 ddx_1 = ddx(in_.uv);
    float2 ddy_1 = ddy(in_.uv);
    const Frame _e25 = animated_frame(texture_2, animation_2, time_1);
    if (((in_.face.x & FLOWING) != 0u)) {
        float phase = frac((time_1 / FLOW_CYCLE));
        float2 shift = ((in_.flow * animation_2.scroll) * FLOW_CYCLE);
        const float2 _e44 = pixel_art_uv((in_.uv - (shift * phase)), size_1, texels_per_pixel_1);
        float2 first = frac(_e44);
        const float2 _e52 = pixel_art_uv((in_.uv - (shift * frac((phase + 0.5)))), size_1, texels_per_pixel_1);
        float2 second = frac(_e52);
        const float4 _e54 = sample_frame(first, _e25, ddx_1, ddy_1);
        const float4 _e55 = sample_frame(second, _e25, ddx_1, ddy_1);
        texel = lerp(_e54, _e55, abs((1.0 - (2.0 * phase))));
    } else {
        const float2 _e63 = pixel_art_uv(in_.uv, size_1, texels_per_pixel_1);
        bool wrapped = ((in_.face.x & WRAPPED) != 0u);
        const float4 _e72 = sample_frame((wrapped ? frac(_e63) : _e63), _e25, ddx_1, ddy_1);
        texel = _e72;
    }
    const float4 _e75 = baked_light(in_.light, in_.corner);
    float3 normal_1 = normalize(in_.world_normal);
    const float3 _e80 = dynamic_light(in_.world_position, normal_1);
    float3 block = max(_e75.yzw, _e80);
    float4 _e84 = lighting.sky;
    const float3 _e88 = brightness((_e75.x).xxx);
    float3 sky = (_e84.xyz * _e88);
    const float3 _e90 = brightness(block);
    float3 light_4 = max(max(sky, _e90), (0.025).xxx);
    const float _e99 = occlusion(in_.face.y, in_.corner);
    float ao = lerp(OCCLUDED, 1.0, (_e99 / 3.0));
    float _e105 = texel.w;
    const float _e106 = cut_out(_e105);
    float4 _e107 = texel;
    const float3 _e112 = fogged(((_e107.xyz * light_4) * ao), in_.world_position);
    return float4(_e112, _e106);
}
