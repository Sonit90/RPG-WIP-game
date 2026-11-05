// The frame's shader ABI, and the lighting every mesh pipeline shades with.
// Port of scene_frame.hlsli.
//
// Included by mesh.metal, mesh_instanced.metal, mesh_skinned.metal and contact.metal.
//
// The CPU mirror of `FrameConstants` below is `FrameConstants` in src/scene.rs, field for
// field (4064 bytes).
//
// # Permutations
//
// MATERIAL_PBR / MATERIAL_NORMAL_MAP / ENABLE_SHADOWS are Metal function constants declared in
// common.h (the HLSL used -D defines). A feature that is off is *not compiled*: the compiler
// removes the dead branch when the function is specialized.
//
// # Resources
//
// HLSL declared the textures and the cluster mask at global scope. In MSL they are entry-point
// parameters, so the mesh fragment entry points gather them into a `SceneResources` at the top
// and pass it down. See BINDINGS.md for the indices.

#ifndef METAL_SCENE_FRAME_H
#define METAL_SCENE_FRAME_H

#include "common.h"
#include "material.h"
#include "pbr.h"

// Must match settings.rs: MAX_CASCADES, MAX_LOCAL_LIGHTS, MAX_LOCAL_SHADOW_VIEWS.
#define MAX_CASCADES 4
#define MAX_LOCAL_LIGHTS 32
#define MAX_LOCAL_SHADOW_VIEWS 16

// Must match light_cluster.rs: CLUSTERS_X/Y/Z and CLUSTER_MASK_VECTORS.
//
// The grid is fixed rather than derived from the viewport: a cluster is a fraction of the
// screen, not a number of pixels, so the same grid serves 720p and 4K.
#define CLUSTERS_X 16
#define CLUSTERS_Y 9
#define CLUSTERS_Z 24

// HLSL constant buffer Frame at b0. FrameConstants (src/scene.rs), 4064 bytes.
//
// HLSL packs a float3 followed by a float into one 16-byte row; that is `packed_float3` + an
// explicit `float` here. Every other member is a float4, a float4x4 or an array of those, all
// of which are 16-byte aligned in MSL, so no further padding is needed. Matrices are
// column-major in both languages and are memcpy'd untransposed.
struct FrameConstants
{
    float4x4 view_projection;               // offset    0
    packed_float3 camera_position;          // offset   64
    float frame_padding;                    // offset   76
    // The direction the light travels, not the direction towards it.
    packed_float3 light_direction;          // offset   80
    float light_intensity;                  // offset   92
    packed_float3 light_color;              // offset   96
    float ambient_intensity;                // offset  108
    packed_float3 ambient_color;            // offset  112
    float frame_padding_2;                  // offset  124

    // ---- directional shadows ------------------------------------------------------
    // One light view-projection per cascade, and where each cascade lives in the atlas.
    float4x4 cascade_view_projection[MAX_CASCADES];   // offset  128
    float4 cascade_uv_scale_bias[MAX_CASCADES];       // offset  384
    // The far distance of each cascade, in metres along the view direction.
    float4 cascade_far;                     // offset  448
    // How much world one texel of each cascade covers, in metres.
    float4 cascade_texel_world;             // offset  464
    // x: cascade count  y: blend fraction  z: PCF kernel code  w: debug mode
    float4 shadow_params;                   // offset  480
    // x: normal offset in texels  y: its ceiling in metres  z: shadow strength (0 = off)
    // w: disc-kernel radius in texels
    float4 shadow_params2;                  // offset  496
    // x,y: atlas size in texels   z,w: its reciprocal
    float4 shadow_atlas_size;               // offset  512
    // x: camera near  y: camera far  z,w: viewport width, height (see cluster_of)
    float4 camera_params;                   // offset  528
    // x: contact ray length  y: steps  z: thickness  w: max distance (0 = off)
    float4 contact_params;                  // offset  544
    float4x4 inverse_view_projection;       // offset  560

    // ---- local lights -------------------------------------------------------------
    float4 light_position_range[MAX_LOCAL_LIGHTS];    // xyz position, w range      offset  624
    float4 light_color_intensity[MAX_LOCAL_LIGHTS];   // rgb colour,  w intensity   offset 1136
    float4 light_direction_inner[MAX_LOCAL_LIGHTS];   // xyz spot axis, w inner cos offset 1648
    // x: outer cosine  y: first shadow view (-1 for none)  z: view count  w: 1 for a spot
    float4 light_shadow[MAX_LOCAL_LIGHTS];            // offset 2160
    float4 local_light_count;               // offset 2672

    float4x4 local_view_projection[MAX_LOCAL_SHADOW_VIEWS];  // offset 2688
    float4 local_uv_scale_bias[MAX_LOCAL_SHADOW_VIEWS];      // offset 3712

    // ---- the light cluster grid ---------------------------------------------------
    // x,y,z: the grid dimensions  w: 1 when a grid was built for this frame
    float4 cluster_params;                  // offset 3968
    // x: depth-slice scale  y: its bias  z: camera near  w: camera far
    float4 cluster_depth;                   // offset 3984
    // x,y: the contact-shadow target's size in texels  z,w: its reciprocal. A zero width
    // means the pass did not run this frame.
    float4 contact_target;                  // offset 4000
    // x: the kernel near cascades use  y: the one far cascades use  z: the first cascade
    // that uses it. z >= MAX_CASCADES means every cascade uses x.
    float4 filter_lod;                      // offset 4016

    // ---- the camera fade ----------------------------------------------------------
    // xyz: what the camera is looking at (the character's chest)  w: 1 when the fade is on
    float4 camera_fade;                     // offset 4032
    // x: near start  y: near end  z: tube radius at the target  w: minimum opacity
    float4 camera_fade_params;              // offset 4048
};
static_assert(sizeof(FrameConstants) == 4064, "FrameConstants must match the Rust FrameConstants (4064 bytes)");

// Everything the lighting reads that is not in a scalar argument. Built at the top of each
// mesh fragment entry point from its own parameters.
struct SceneResources
{
    constant FrameConstants* frame;          // buffer(0)
    constant MaterialConstants* material;    // buffer(2)
    MaterialTextures textures;               // texture(0..3)
    // The one shadow atlas: every cascade, every spot light and every point-light face.
    // Sampled through the comparison sampler `shadow_sampler`, so the hardware performs the
    // depth test and the bilinear filter together.
    depth2d<float> shadow_atlas;             // texture(4)
    // (HLSL t5 `scene_depth` is NOT here: the mesh pixel shaders never read it - only the
    // contact-shadow march does, and that runs in contact.metal. mesh_fs therefore does not
    // declare texture(5).)
    // What the contact-shadow pass wrote: red the shadow factor, green the view distance the
    // ray started from.
    texture2d<float> contact_shadows;        // texture(7)
    // One bitmask per cluster: bit i means local light i reaches it. See light_cluster.rs.
    const device uint* cluster_light_mask;   // buffer(4)
};

// HLSL SamplerComparisonState shadow_sampler at s1 - mesh_pipeline.rs `shadow_sampler()`:
// D3D12_FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT, BORDER on U/V/W with an OPAQUE WHITE
// border, comparison LESS_EQUAL, MipLODBias 0, MaxAnisotropy 1, MinLOD 0, MaxLOD FLT_MAX.
constexpr sampler shadow_sampler(
    coord::normalized,
    address::clamp_to_border,
    border_color::opaque_white,
    mag_filter::linear,
    min_filter::linear,
    mip_filter::nearest,
    compare_func::less_equal,
    lod_clamp(0.0f, MAXFLOAT));

// HLSL SamplerState depth_sampler at s2 - mesh_pipeline.rs `depth_sampler()`:
// D3D12_FILTER_MIN_MAG_MIP_POINT, CLAMP on U/V/W, MipLODBias 0, MinLOD 0, MaxLOD FLT_MAX.
// Point-filtered and clamped: a depth buffer holds distances, and the average of two distances
// is a surface that is not there.
constexpr sampler depth_sampler(
    coord::normalized,
    address::clamp_to_edge,
    mag_filter::nearest,
    min_filter::nearest,
    mip_filter::nearest,
    lod_clamp(0.0f, MAXFLOAT));

// ---------------------------------------------------------------------------------------
// Shadow sampling
//
// Everything below returns a *shadow factor*: 1 is fully lit, 0 is fully shadowed. It is
// deliberately not multiplied into a light colour here.
// ---------------------------------------------------------------------------------------

// Clamps an atlas coordinate to its own tile, half a texel in from the edge.
//
// A PCF kernel at the edge of a tile would otherwise read its neighbour: the atlas holds a
// cascade and four spot lights side by side, and one light's depth appearing inside
// another's shadow is a bug that looks like a light leak.
inline float2 clamp_to_tile(constant FrameConstants& f, float2 uv, float4 scale_bias)
{
    float2 half_texel = 0.5 * f.shadow_atlas_size.zw;
    float2 low = scale_bias.zw + half_texel;
    float2 high = scale_bias.zw + scale_bias.xy - half_texel;
    return clamp(uv, low, high);
}

// One depth comparison, filtered by the hardware. (SampleCmpLevelZero)
inline float shadow_tap(thread const SceneResources& r, float2 atlas_uv, float compare, float4 scale_bias)
{
    return r.shadow_atlas.sample_compare(
        shadow_sampler, clamp_to_tile(*r.frame, atlas_uv, scale_bias), compare, level(0.0f));
}

// Interleaved gradient noise: a per-pixel angle, cheap and without a texture.
//
// Deliberately a function of the *pixel* and of nothing else. The engine has no temporal
// accumulation and no TAA, so an angle that changed per frame would turn a static penumbra
// into boiling noise.
inline float interleaved_gradient_noise(float2 pixel)
{
    return fract(52.9829189 * fract(dot(pixel, float2(0.06711056, 0.00583715))));
}

// The three discs, as constants.
//
// A golden-angle (Vogel) spiral: `r = sqrt((i + 0.5) / n)`, `theta = i * 2.39996323`. Baked
// rather than computed: rotating a constant point costs a multiply and an add, and the
// rotation itself is one sine and one cosine for the whole kernel.
constant float2 VOGEL_4[4] = {
    float2(+0.353553, +0.000000),
    float2(-0.451544, +0.413652),
    float2(+0.069116, -0.787542),
    float2(+0.569142, +0.742346)
};
constant float2 VOGEL_6[6] = {
    float2(+0.288675, +0.000000),
    float2(-0.368684, +0.337745),
    float2(+0.056433, -0.643026),
    float2(+0.464703, +0.606123),
    float2(-0.852787, -0.150846),
    float2(+0.807834, -0.513878)
};
constant float2 VOGEL_8[8] = {
    float2(+0.250000, +0.000000),
    float2(-0.319290, +0.292496),
    float2(+0.048872, -0.556877),
    float2(+0.402444, +0.524918),
    float2(-0.738535, -0.130636),
    float2(+0.699605, -0.445031),
    float2(-0.234004, +0.870484),
    float2(-0.446271, -0.859268)
};

// One disc point turned by the pixel's own rotation.
//
// `turn` is `float2(cos, sin)` of the angle, computed once per pixel. The per-pixel rotation
// is what removes the spiral's own structure, which is otherwise as visible along a shadow
// edge as a grid's staircase.
inline float2 rotate_tap(float2 tap, float2 turn)
{
    return float2(tap.x * turn.x - tap.y * turn.y, tap.x * turn.y + tap.y * turn.x);
}

// Percentage-closer filtering.
//
// Two families. The grid kernels (codes 0-2) place their taps one texel apart; the disc
// kernels (codes 3-5) spend a smaller budget of taps on a rotated Vogel disc of
// `shadow_params2.w` texels. Unrolled per branch rather than looped over a variable radius,
// because a dynamic loop around a texture fetch is markedly slower than a static one.
// (`kernel` is a reserved word in MSL, hence `filter_kernel`.)
inline float shadow_pcf(
    thread const SceneResources& r,
    float2 atlas_uv,
    float compare,
    float4 scale_bias,
    uint filter_kernel,
    float2 pixel)
{
    constant FrameConstants& f = *r.frame;
    if (filter_kernel == 0)
    {
        return shadow_tap(r, atlas_uv, compare, scale_bias);
    }

    float2 texel = f.shadow_atlas_size.zw;
    float total = 0.0;

    if (filter_kernel >= 3)
    {
        float angle = interleaved_gradient_noise(pixel) * 6.2831853;
        // One sine and one cosine for the whole kernel, not one pair per tap.
        float2 turn = float2(cos(angle), sin(angle));
        float2 radius = texel * f.shadow_params2.w;
        if (filter_kernel == 3)
        {
#pragma unroll
            for (uint i4 = 0; i4 < 4; ++i4)
            {
                total += shadow_tap(
                    r, atlas_uv + rotate_tap(VOGEL_4[i4], turn) * radius, compare, scale_bias);
            }
            return total * 0.25;
        }
        if (filter_kernel == 4)
        {
#pragma unroll
            for (uint i6 = 0; i6 < 6; ++i6)
            {
                total += shadow_tap(
                    r, atlas_uv + rotate_tap(VOGEL_6[i6], turn) * radius, compare, scale_bias);
            }
            return total * (1.0 / 6.0);
        }
#pragma unroll
        for (uint i8 = 0; i8 < 8; ++i8)
        {
            total += shadow_tap(
                r, atlas_uv + rotate_tap(VOGEL_8[i8], turn) * radius, compare, scale_bias);
        }
        return total * 0.125;
    }

    if (filter_kernel == 1)
    {
#pragma unroll
        for (int y = -1; y <= 1; ++y)
        {
#pragma unroll
            for (int x = -1; x <= 1; ++x)
            {
                total += shadow_tap(
                    r, atlas_uv + float2(float(x), float(y)) * texel, compare, scale_bias);
            }
        }
        return total / 9.0;
    }

#pragma unroll
    for (int y2 = -2; y2 <= 2; ++y2)
    {
#pragma unroll
        for (int x2 = -2; x2 <= 2; ++x2)
        {
            total += shadow_tap(
                r, atlas_uv + float2(float(x2), float(y2)) * texel, compare, scale_bias);
        }
    }
    return total / 25.0;
}

// Projects a world position into one shadow view and filters it.
//
// Returns 1 - fully lit - for anything outside the view, which is what makes a cascade's
// edge a fade into the next cascade rather than a wall of darkness.
inline float shadow_in_view(
    thread const SceneResources& r,
    float3 world_position,
    float4x4 light_view_projection,
    float4 scale_bias,
    uint filter_kernel,
    float2 pixel)
{
    float4 light_clip = light_view_projection * float4(world_position, 1.0);
    if (light_clip.w <= 0.0)
    {
        return 1.0;
    }
    float3 ndc = light_clip.xyz / light_clip.w;
    if (any(abs(ndc.xy) > float2(1.0)) || ndc.z < 0.0 || ndc.z > 1.0)
    {
        return 1.0;
    }
    // Clip space is Y-up and a texture is Y-down, which is the sign flip below.
    float2 uv = ndc.xy * float2(0.5, -0.5) + 0.5;
    return shadow_pcf(r, uv * scale_bias.xy + scale_bias.zw, ndc.z, scale_bias, filter_kernel, pixel);
}

// Which cube face a direction from a point light falls on: +X, -X, +Y, -Y, +Z, -Z.
//
// The CPU builds the six views in this order (see shadow/local.rs), and a test there asserts
// the two agree. Getting it wrong lights five sixths of a room correctly.
inline uint cube_face_of(float3 direction)
{
    float3 magnitude = abs(direction);
    if (magnitude.x >= magnitude.y && magnitude.x >= magnitude.z)
    {
        return direction.x < 0.0 ? 1u : 0u;
    }
    if (magnitude.y >= magnitude.z)
    {
        return direction.y < 0.0 ? 3u : 2u;
    }
    return direction.z < 0.0 ? 5u : 4u;
}

// How far in front of the camera a point is, in metres.
inline float view_depth_of(constant FrameConstants& f, float3 world_position)
{
    float4 clip = f.view_projection * float4(world_position, 1.0);
    return clip.w;
}

// Which cascade covers `depth`, and how far it has crossed into the next one.
//
// The CPU mirror is `cascade_blend` in shadow/cascade.rs, and it is the same arithmetic:
// find the first cascade whose far plane is beyond the fragment, then measure how far into
// that cascade's blend region it lies.
inline void select_cascade(
    constant FrameConstants& f,
    float depth,
    thread uint& near_index,
    thread uint& far_index,
    thread float& blend)
{
    uint count = uint(f.shadow_params.x);
    near_index = count;
#pragma unroll
    for (uint index = 0; index < MAX_CASCADES; ++index)
    {
        if (index < count && near_index == count && depth < f.cascade_far[index])
        {
            near_index = index;
        }
    }
    far_index = near_index;
    blend = 0.0;
    if (near_index >= count)
    {
        return;
    }

    float far_edge = f.cascade_far[near_index];
    float near_edge = near_index == 0 ? f.camera_params.x : f.cascade_far[near_index - 1];
    float width = (far_edge - near_edge) * f.shadow_params.y;
    if (width > 0.0 && near_index + 1 < count && depth > far_edge - width)
    {
        far_index = near_index + 1;
        blend = saturate((depth - (far_edge - width)) / width);
    }
}

// The screen-space contact shadow march (used by contact.metal only).
//
// A short march along the ray towards the light, comparing each step against the depth the
// prepass wrote. It resolves the centimetres a shadow map cannot: the gap under a boot, the
// seam where a crate meets the floor. Deliberately short - see ContactShadowSettings.
//
// The `thickness` test is what stops every silhouette in the frame from occluding
// everything behind it: the depth buffer records a surface, not a solid, so a sample that is
// *much* nearer than the ray is a different object the ray passes behind.
inline float contact_shadow(
    constant FrameConstants& f,
    depth2d<float> scene_depth,
    float3 world_position,
    float3 normal,
    float3 to_light,
    float view_distance)
{
    float max_distance = f.contact_params.w;
    if (max_distance <= 0.0 || view_distance > max_distance)
    {
        return 1.0;
    }

    int steps = max(int(f.contact_params.y), 1);
    float step_length = f.contact_params.x / float(steps);
    float near_plane = f.camera_params.x;
    float far_plane = f.camera_params.y;
    float projection_depth = far_plane / (near_plane - far_plane);

    // Fade the whole effect out over the last quarter of its range, so it does not switch
    // off along a visible circle on the floor.
    float fade = saturate((max_distance - view_distance) / (max_distance * 0.25));

    // Start the march half a step off the surface, along the normal.
    //
    // Without this the ray begins exactly on the surface it is shading, and on a large flat
    // one - a floor - the early samples project back onto that same floor. The depth
    // difference they find is then the floor's own slope in screen space, not an occluder,
    // and once the camera is low enough that the difference lands inside the thickness
    // window the whole floor reports occluded.
    float3 origin = world_position + normal * (step_length * 0.5);

    float occlusion = 0.0;
    for (int step_index = 1; step_index <= steps; ++step_index)
    {
        float3 sample_position = origin + to_light * (step_length * float(step_index));
        float4 clip = f.view_projection * float4(sample_position, 1.0);
        if (clip.w <= 0.0)
        {
            break;
        }
        float3 ndc = clip.xyz / clip.w;
        if (any(abs(ndc.xy) > float2(1.0)))
        {
            break;
        }
        float2 uv = ndc.xy * float2(0.5, -0.5) + 0.5;
        float scene_ndc_depth = scene_depth.sample(depth_sampler, uv, level(0.0f));
        // The prepass writes 1.0 where nothing was drawn; there is nothing to be occluded by.
        if (scene_ndc_depth >= 1.0)
        {
            continue;
        }

        // Both depths made linear, so `thickness` is a distance in metres rather than a
        // number that means something different at every range.
        float scene_distance = projection_depth * near_plane / (scene_ndc_depth + projection_depth);
        float ray_distance = clip.w;
        float difference = ray_distance - scene_distance;
        if (difference > 0.02 && difference < f.contact_params.z)
        {
            occlusion = 1.0;
            break;
        }
    }
    return saturate(1.0 - occlusion * fade);
}

// The contact-shadow term for a shaded pixel, reconstructed from the half-resolution pass.
//
// A **depth-weighted** upsample, not a bilinear one. The four neighbouring texels each carry
// the view distance their ray started from; a neighbour whose distance disagrees with this
// pixel's is a different surface, and blending its occlusion in is what would smear a foot's
// contact shadow onto the wall behind it. A texel with no surface at all (the sky, which the
// pass writes as distance zero) is dropped outright.
//
// If every neighbour disagrees the answer is "unoccluded" rather than a guess. Contact shadows
// only ever darken, so failing towards 1 fails towards the picture without them.
inline float contact_shadow_at(thread const SceneResources& r, float2 pixel, float view_distance)
{
    constant FrameConstants& f = *r.frame;
    if (f.contact_params.w <= 0.0 || view_distance > f.contact_params.w || f.contact_target.x < 1.0)
    {
        return 1.0;
    }

    float2 viewport = max(f.camera_params.zw, float2(1.0, 1.0));
    float2 coordinate = (pixel / viewport) * f.contact_target.xy - 0.5;
    float2 base = floor(coordinate);
    float2 fraction = coordinate - base;

    // The 2x2 block whose top-left texel is `base`, as two gathers - one per channel - rather
    // than four loads: the point at the block's shared corner gathers exactly those four
    // texels, and the clamping sampler clamps them at the target's edges. Gather order is
    // (0,1) (1,1) (1,0) (0,0) in x y z w (the same in D3D12 and Metal); the taps are then
    // visited in the loads' order, so the sums are the same to the last bit.
    float2 corner = (base + 1.0) / f.contact_target.xy;
    float4 shadow4 = r.contact_shadows.gather(depth_sampler, corner, int2(0), component::x);
    float4 distance4 = r.contact_shadows.gather(depth_sampler, corner, int2(0), component::y);
    float2 taps[4] = {
        float2(shadow4.w, distance4.w),
        float2(shadow4.z, distance4.z),
        float2(shadow4.x, distance4.x),
        float2(shadow4.y, distance4.y),
    };

    float total = 0.0;
    float total_weight = 0.0;
#pragma unroll
    for (int y = 0; y < 2; ++y)
    {
#pragma unroll
        for (int x = 0; x < 2; ++x)
        {
            float2 tap = taps[y * 2 + x];
            if (tap.y <= 0.0)
            {
                continue;
            }
            float bilinear = (x == 0 ? 1.0 - fraction.x : fraction.x)
                           * (y == 0 ? 1.0 - fraction.y : fraction.y);
            // Half a metre of disagreement halves the weight roughly eight times over. The
            // scale is in metres because both quantities are.
            float weight = bilinear / (1.0 + 16.0 * abs(tap.y - view_distance));
            total += tap.x * weight;
            total_weight += weight;
        }
    }
    if (total_weight <= 1e-5)
    {
        return 1.0;
    }
    return saturate(total / total_weight);
}

// The directional light's shadow factor at a world position.
//
// ```text
// world position -> cascade -> light space -> PCF -> blend with the next cascade -> factor
// ```
//
// `normal` is used for the normal-offset bias: the sample position is pushed along the
// surface normal by a fraction of a shadow texel before it is projected, which fixes the
// acne that is left after the rasterizer's slope bias without the peter-panning a large
// constant bias would cause.
inline float directional_shadow(
    thread const SceneResources& r,
    float3 world_position,
    float3 normal,
    float view_distance,
    float2 pixel)
{
    constant FrameConstants& f = *r.frame;
    float strength = f.shadow_params2.z;
    uint count = uint(f.shadow_params.x);
    if (strength <= 0.0 || count == 0)
    {
        return 1.0;
    }

    uint near_index;
    uint far_index;
    float blend;
    select_cascade(f, view_distance, near_index, far_index, blend);
    if (near_index >= count)
    {
        return 1.0;
    }

    // The filter's level of detail. A cascade past `filter_lod.z` steps down one kernel:
    // it covers roughly three times the distance for the same texels, so its penumbra is
    // already three times wider in world terms.
    uint filter_kernel = near_index >= uint(f.filter_lod.z) ? uint(f.filter_lod.y) : uint(f.filter_lod.x);
    float offset_scale = min(
        f.cascade_texel_world[near_index] * f.shadow_params2.x, f.shadow_params2.y);
    float3 offset_position = world_position + normal * offset_scale;

    float factor = shadow_in_view(
        r,
        offset_position,
        f.cascade_view_projection[near_index],
        f.cascade_uv_scale_bias[near_index],
        filter_kernel,
        pixel);

    if (blend > 0.0)
    {
        float far_offset = min(
            f.cascade_texel_world[far_index] * f.shadow_params2.x, f.shadow_params2.y);
        uint far_kernel =
            far_index >= uint(f.filter_lod.z) ? uint(f.filter_lod.y) : uint(f.filter_lod.x);
        float next = shadow_in_view(
            r,
            world_position + normal * far_offset,
            f.cascade_view_projection[far_index],
            f.cascade_uv_scale_bias[far_index],
            far_kernel,
            pixel);
        factor = mix(factor, next, blend);
    }

    // Fade out over the last tenth of the shadow range, so the far edge of the last cascade
    // is a gradient rather than a line drawn across the world.
    float shadow_distance = f.cascade_far[count - 1];
    float fade = saturate((shadow_distance - view_distance) / max(shadow_distance * 0.1, 0.001));
    factor = mix(1.0, factor, fade);

    return mix(1.0, factor, strength);
}

// The shadow factor for one local light, or 1 if it has no shadow map.
inline float local_light_shadow(
    thread const SceneResources& r,
    uint light,
    float3 world_position,
    float3 normal,
    float2 pixel)
{
    constant FrameConstants& f = *r.frame;
    float first_view = f.light_shadow[light].y;
    if (first_view < 0.0)
    {
        return 1.0;
    }
    uint view = uint(first_view);
    if (f.light_shadow[light].w < 0.5)
    {
        // A point light: pick the face the surface is on.
        view += cube_face_of(world_position - f.light_position_range[light].xyz);
    }
    if (view >= MAX_LOCAL_SHADOW_VIEWS)
    {
        return 1.0;
    }

    // A local map's texel covers more world the further it is from the light, so the normal
    // offset is scaled by distance rather than by a per-cascade constant.
    float distance_to_light = length(world_position - f.light_position_range[light].xyz);
    float offset = min(distance_to_light * 0.004 * f.shadow_params2.x, f.shadow_params2.y);
    return shadow_in_view(
        r,
        world_position + normal * offset,
        f.local_view_projection[view],
        f.local_uv_scale_bias[view],
        uint(f.shadow_params.z),
        pixel);
}

// Which cluster a fragment falls in.
//
// Tiles come from the pixel position, the slice from the view depth on a logarithmic scale.
// The CPU mirror is `slice_of` and `tile_of` in light_cluster.rs, and it is the same
// arithmetic: a light whose cluster the shader computes differently from the way the CPU
// assigned it is a light that goes out in a band across the screen.
inline uint cluster_of(constant FrameConstants& f, float2 pixel, float view_depth)
{
    float2 viewport = max(f.camera_params.zw, float2(1.0, 1.0));
    float2 tile = floor(pixel / viewport * float2(float(CLUSTERS_X), float(CLUSTERS_Y)));
    tile = clamp(tile, float2(0.0, 0.0), float2(float(CLUSTERS_X - 1), float(CLUSTERS_Y - 1)));

    float slice = floor(log(max(view_depth, 1e-4)) * f.cluster_depth.x + f.cluster_depth.y);
    slice = clamp(slice, 0.0, float(CLUSTERS_Z - 1));

    return uint(tile.x) + uint(tile.y) * uint(CLUSTERS_X) + uint(slice) * uint(CLUSTERS_X * CLUSTERS_Y);
}

// The lights this fragment has to evaluate, as a bitmask.
//
// From the cluster grid when the frame built one, and from the whole light list when it did
// not - the offscreen helpers and the older tests produce frames with lights and no camera
// to cluster against, and "every light" is the correct answer there.
inline uint lights_at(thread const SceneResources& r, float2 pixel, float view_depth)
{
    constant FrameConstants& f = *r.frame;
    uint count = min(uint(f.local_light_count.x), uint(MAX_LOCAL_LIGHTS));
    // The common case in an outdoor scene, and the one worth an early exit: no local lights
    // at all means no grid lookup.
    if (count == 0u)
    {
        return 0u;
    }
    uint live = count >= 32u ? 0xFFFFFFFFu : ((1u << count) - 1u);
    if (f.cluster_params.w < 0.5)
    {
        return live;
    }
    uint cluster = cluster_of(f, pixel, view_depth);
    // ByteAddressBuffer.Load(cluster * 4) is word `cluster` of a uint array.
    return r.cluster_light_mask[cluster] & live;
}

// Everything the local lights add at a surface.
//
// The loop runs over the *cluster's* lights, not the frame's: `ctz` (HLSL firstbitlow) walks
// the set bits of the mask, so a fragment with two lights near it costs two iterations
// whether the scene has two lights or thirty-two. See docs/shadows.md and light_cluster.rs.
inline float3 local_light_contribution(
    thread const SceneResources& r,
    float3 world_position,
    Surface surface,
    float receives_shadow,
    float2 pixel,
    float view_distance)
{
    constant FrameConstants& f = *r.frame;
    float3 normal = surface.normal;
    float3 total = float3(0.0, 0.0, 0.0);
    uint mask = lights_at(r, pixel, view_distance);
    while (mask != 0u)
    {
        uint light = ctz(mask);
        mask &= mask - 1u;

        float3 to_light = f.light_position_range[light].xyz - world_position;
        float distance_to_light = length(to_light);
        float range = f.light_position_range[light].w;
        if (distance_to_light >= range)
        {
            continue;
        }
        float3 direction = to_light / max(distance_to_light, 1e-4);
        // The BRDF rather than a Lambert term, and evaluated before the shadow lookup: a
        // light behind the surface contributes nothing, and a fragment that contributes
        // nothing must not pay for a shadow sample.
        float3 shaded = shade_light(surface, direction);
        if (all(shaded <= float3(0.0)))
        {
            continue;
        }

        // Inverse-square, windowed so it reaches exactly zero at the light's range: an
        // unwindowed falloff never reaches zero, and a light that is never quite off cannot
        // be culled at any distance.
        float normalized_distance = distance_to_light / range;
        float window = saturate(1.0 - normalized_distance * normalized_distance * normalized_distance * normalized_distance);
        float attenuation = window * window / (1.0 + distance_to_light * distance_to_light);

        if (f.light_shadow[light].w > 0.5)
        {
            // A spot light: the cone, with a soft rim between its inner and outer angles.
            float cosine = dot(-direction, normalize(f.light_direction_inner[light].xyz));
            float inner = f.light_direction_inner[light].w;
            float outer = f.light_shadow[light].x;
            float cone = saturate((cosine - outer) / max(inner - outer, 1e-4));
            attenuation *= cone * cone;
        }
        if (attenuation <= 0.0)
        {
            continue;
        }

        float shadow = 1.0;
        if (ENABLE_SHADOWS)
        {
            if (receives_shadow > 0.5)
            {
                shadow = local_light_shadow(r, light, world_position, normal, pixel);
            }
        }

        total += shaded * f.light_color_intensity[light].rgb
            * (f.light_color_intensity[light].w * attenuation * shadow);
    }
    return total;
}

// A debug colour for each cascade.
constant float3 CASCADE_TINTS[MAX_CASCADES] = {
    float3(1.0, 0.4, 0.4),
    float3(0.4, 1.0, 0.4),
    float3(0.4, 0.55, 1.0),
    float3(1.0, 0.95, 0.4)
};

// A debug colour for the cascade a fragment landed in, or white past the last one.
inline float3 cascade_debug_tint(constant FrameConstants& f, float view_distance)
{
    uint count = uint(f.shadow_params.x);
    uint near_index;
    uint far_index;
    float blend;
    select_cascade(f, view_distance, near_index, far_index, blend);
    if (near_index >= count)
    {
        return float3(1.0, 1.0, 1.0);
    }
    float3 tint = CASCADE_TINTS[near_index];
    if (blend > 0.0)
    {
        tint = mix(tint, CASCADE_TINTS[far_index], blend);
    }
    return tint;
}

// The shadow atlas itself, drawn in the corner of the screen.
//
// Returns true when `pixel` is inside the preview and fills `color` with what to show there.
//
// Why this is inside the mesh shader: every pixel of the preview is covered by *some* fragment
// of the scene - the sky dome, if nothing else - and every one of those fragments takes this
// branch and writes the same colour, so the preview appears without a second pipeline or draw.
//
// Empty atlas is white (the clear is the far plane) and a caster is darker the closer it is
// to the light. The red lines are the cascade tile borders.
inline bool shadow_atlas_preview(thread const SceneResources& r, float2 pixel, thread float3& color)
{
    constant FrameConstants& f = *r.frame;
    color = float3(0.0, 0.0, 0.0);
    float2 viewport = f.camera_params.zw;
    if (viewport.x < 1.0 || viewport.y < 1.0 || f.shadow_atlas_size.x < 1.0)
    {
        return false;
    }

    const float MARGIN = 16.0;
    float side = min(viewport.x, viewport.y) * 0.42;
    float2 size = float2(side * (f.shadow_atlas_size.x / f.shadow_atlas_size.y), side);
    float2 low = float2(MARGIN, viewport.y - MARGIN - size.y);
    float2 high = low + size;
    if (any(pixel < low) || any(pixel > high))
    {
        return false;
    }

    float2 uv = (pixel - low) / size;
    float depth = r.shadow_atlas.sample(depth_sampler, uv, level(0.0f));
    color = float3(depth);

    // The cascade borders, two texels wide in screen space so they survive the downscale.
    float2 edge = 2.0 / size;
    uint count = uint(f.shadow_params.x);
    for (uint index = 0u; index < count; ++index)
    {
        float4 rect = f.cascade_uv_scale_bias[index];
        float2 within = (uv - rect.zw) / max(rect.xy, float2(1e-6));
        if (any(within < float2(0.0)) || any(within > float2(1.0)))
        {
            continue;
        }
        float2 border = min(within, float2(1.0) - within);
        if (min(border.x, border.y) < max(edge.x, edge.y))
        {
            color = float3(0.9, 0.15, 0.15);
        }
    }
    return true;
}

// The ordered-dither threshold for a pixel: a 4x4 Bayer matrix, sixteen levels.
//
// What a fade is made of. A surface drawn at opacity `a` keeps the pixels whose threshold is
// below `a` and discards the rest, so it stays in the opaque pass - it writes depth, casts its
// whole shadow and needs no sorting.
constant float BAYER_4X4[16] = {
    0.0, 8.0, 2.0, 10.0,
    12.0, 4.0, 14.0, 6.0,
    3.0, 11.0, 1.0, 9.0,
    15.0, 7.0, 13.0, 5.0,
};

inline float dither_threshold(float2 pixel)
{
    uint2 cell = uint2(pixel) & uint2(3u);
    return (BAYER_4X4[cell.y * 4u + cell.x] + 0.5) / 16.0;
}

// How much of a see-through surface at `world_position` the camera leaves drawn.
//
//   eye o ===================> target
//         near zone            tube: in front of the target, within the radius
//
// The near zone fades whatever is about to touch the eye, so nothing is cut open by the near
// plane. The tube fades whatever stands between the eye and the character, narrowing towards
// the eye and easing off just in front of the character so the ground it stands on is not
// caught. Solid geometry never reaches here.
inline float camera_fade_opacity(constant FrameConstants& f, float3 world_position)
{
    float3 camera_pos = float3(f.camera_position);
    float3 to_surface = world_position - camera_pos;
    float eye_distance = length(to_surface);
    float near_start = f.camera_fade_params.x;
    float near_end = f.camera_fade_params.y;
    float near_fade = saturate((eye_distance - near_start) / max(near_end - near_start, 1e-4));
    near_fade = near_fade * near_fade * (3.0 - 2.0 * near_fade);

    float3 axis = f.camera_fade.xyz - camera_pos;
    float length_to_target = length(axis);
    if (length_to_target < 1e-3)
    {
        return near_fade;
    }
    float3 direction = axis / length_to_target;
    float along = dot(to_surface, direction);
    float radius_at_target = f.camera_fade_params.z;
    // Narrow at the eye, full width at the target.
    float radius = radius_at_target * mix(0.35, 1.0, saturate(along / length_to_target));
    float across = length(to_surface - direction * along);
    // Within half a radius of the target's own depth the tube lets go: a surface level with
    // the character is beside it, not in front of it.
    float margin = radius_at_target * 0.5;
    float in_front = saturate((length_to_target - margin - along) / max(margin, 1e-4));
    float inside = 1.0 - smoothstep(radius * 0.6, radius, across);
    float occluding = along > 0.0 ? in_front * inside : 0.0;
    float tube = mix(1.0, f.camera_fade_params.w, occluding);
    return min(near_fade, tube);
}

// Discards this pixel if the surface's fade says it is one of the ones not drawn.
//
// `fade` is the entity's own (1 - opacity): what fades the player's character when the camera
// is close to it. `see_through` lets the frame's camera fade apply on top.
inline void apply_fade(
    constant FrameConstants& f,
    float4 screen_position,
    float3 world_position,
    float fade,
    float see_through)
{
    float opacity = 1.0 - fade;
    if (see_through > 0.5 && f.camera_fade.w > 0.5)
    {
        opacity *= camera_fade_opacity(f, world_position);
    }
    // The highest threshold is 15.5/16, so a whole surface never reaches the comparison's
    // losing side; the early test only spares it the table lookup.
    if (opacity < 0.999 && opacity < dither_threshold(screen_position.xy))
    {
        discard_fragment();
    }
}

// The one lighting function every mesh pipeline calls.
//
// ```text
// base colour x factor x tint --> diffuse albedo + F0      (the metallic split)
// normal map  x TBN           --> shading normal
// ORM                         --> occlusion, roughness, metallic
//        |
//   cascade -> light space -> PCF -> cascade blend -> shadow factor
//        |                                              |
//        |                                       contact shadow
//        v                                              v
//   sun x BRDF x shadow  +  ambient x occlusion  +  local lights x BRDF  +  emissive
// ```
//
// The BRDF is in pbr.h: GGX, height-correlated Smith, Schlick, in the metallic-roughness
// workflow glTF describes. The sun and every local light evaluate the same `shade_light`
// against the same `Surface`.
//
// Ambient is added *outside* the shadow factor, deliberately: the sun being blocked does not
// stop the sky from lighting a surface. Occlusion *is* applied to it, which is what the ORM
// map's red channel is for.
//
// `discard_fragment()` is used where HLSL had `discard`. HLSL's discard ends the invocation;
// MSL's marks the fragment discarded and lets it run on, which only costs the (unobservable)
// remaining work and keeps the quad's derivatives defined for the later texture samples.
inline float4 shade_surface(
    thread const SceneResources& r,
    float4 screen_position,
    float3 world_position,
    float3 world_normal,
    float4 world_tangent,
    float2 uv,
    float4 tint,
    float emissive_scale,
    float receives_shadow,
    float fade,
    float see_through)
{
    constant FrameConstants& f = *r.frame;
    constant MaterialConstants& mc = *r.material;

    // Before anything is sampled: a pixel the fade removes costs nothing else.
    apply_fade(f, screen_position, world_position, fade, see_through);
    MaterialSample mat = sample_material(mc, r.textures, uv);
    float4 albedo = mat.base_color;
    // Alpha-tested materials: foliage, fences, grates. `alpha_cutoff` is zero for an opaque
    // material, so this is one comparison that is never taken for most of the scene. The
    // same test runs in the shadow pass, which is what makes a leaf's shadow leaf-shaped.
    if (mc.alpha_cutoff > 0.0 && albedo.a < mc.alpha_cutoff)
    {
        discard_fragment();
    }

    // The geometric normal is kept for the two things that want the surface itself rather
    // than what the map made of it: the early-out below, and the shadow bias, which offsets
    // along the surface and would push itself into a crevice if it followed a normal map.
    float3 geometric_normal = normalize(world_normal);
    float3 normal;
    if (MATERIAL_NORMAL_MAP)
    {
        // The shading normal, out of the tangent frame the vertex carries.
        normal = apply_normal_map(world_normal, world_tangent, mat.normal_ts, mc.normal_scale);
    }
    else
    {
        // No map on this material: the tangent frame, the fetch and the tangent-to-world
        // transform are all compiled out and the shading normal is the interpolated one.
        normal = geometric_normal;
    }
    float3 to_light = normalize(-float3(f.light_direction));
    float lambert = saturate(dot(normal, to_light));
    float3 tinted = albedo.rgb * tint.rgb;
    float view_distance = view_depth_of(f, world_position);

    float3 view = float3(f.camera_position) - world_position;
    // A fragment exactly at the camera has no view direction. It cannot be visible either,
    // but the normalize would be a NaN that spreads through the whole pixel.
    view = dot(view, view) > 1e-12 ? normalize(view) : normal;
    Surface surface = make_surface(
        normal, view, tinted, mat.roughness, mat.metallic, mat.occlusion);

    float shadow = 1.0;
    if (ENABLE_SHADOWS)
    {
        if (receives_shadow > 0.5 && lambert > 0.0)
        {
            shadow = directional_shadow(
                r, world_position, geometric_normal, view_distance, screen_position.xy);
            if (shadow > 0.0)
            {
                shadow = min(shadow, contact_shadow_at(r, screen_position.xy, view_distance));
            }
        }
    }

    uint debug_mode = uint(f.shadow_params.w);
    if (ENABLE_SHADOWS)
    {
        if (debug_mode == 3u)
        {
            float3 preview;
            if (shadow_atlas_preview(r, screen_position.xy, preview))
            {
                return float4(preview, 1.0);
            }
        }
    }
    if (debug_mode == 2u)
    {
        // The shadow factor alone, on a neutral surface: what the cascades, the filter and
        // the biases are actually producing, with albedo and lighting out of the way.
        return float4(shadow, shadow, shadow, 1.0);
    }
    if (debug_mode >= 4u && debug_mode <= 7u)
    {
        // The material maps, raw: 4 base colour, 5 normal, 6 ORM, 7 emissive. A
        // resource-binding probe; no shading is applied, deliberately.
        if (debug_mode == 4u)
        {
            return float4(albedo.rgb, 1.0);
        }
        return float4(material_map_preview(r.textures, debug_mode, uv), 1.0);
    }
    if (debug_mode >= 8u)
    {
        // What the *shading* made of those maps: 8 the shading normal after the TBN
        // transform, 9 roughness, 10 metallic, 11 occlusion. A normal map whose handedness is
        // inverted looks perfectly correct at 5 and visibly wrong at 8.
        if (debug_mode == 8u)
        {
            return float4(normal * 0.5 + 0.5, 1.0);
        }
        if (debug_mode == 9u)
        {
            return float4(float3(surface.roughness), 1.0);
        }
        if (debug_mode == 10u)
        {
            return float4(float3(mat.metallic), 1.0);
        }
        return float4(float3(surface.occlusion), 1.0);
    }

    float3 direct = shade_light(surface, to_light) * float3(f.light_color) * (f.light_intensity * shadow);
    // Occlusion belongs to the ambient term and nothing else. Applying it to the sun as well
    // is the common shortcut, and what it produces is a surface that goes dark when a light
    // moves onto it: the map says how much *sky* reaches the point, and the sun is not sky.
    float3 ambient =
        surface.diffuse_color * float3(f.ambient_color) * (f.ambient_intensity * surface.occlusion);
    float3 local = local_light_contribution(
        r, world_position, surface, receives_shadow, screen_position.xy, view_distance);
    // The material's emissive factor times its emissive map, plus the per-object emissive
    // pulse - which tints with the object and is a gameplay effect (a dummy flashing white),
    // not a property of the surface.
    float3 glow = mat.emissive + tinted * emissive_scale;

    float3 color = direct + ambient + local + glow;
    if (ENABLE_SHADOWS)
    {
        if (debug_mode == 1u)
        {
            color *= cascade_debug_tint(f, view_distance);
        }
    }
    return float4(color, albedo.a * tint.a);
}

#endif // METAL_SCENE_FRAME_H
