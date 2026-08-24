// The frame's shader ABI, and the lighting every mesh pipeline shades with.
//
// Included by mesh.hlsl, mesh_instanced.hlsl and mesh_skinned.hlsl. Before shadows those
// three carried three copies of the same `cbuffer Frame`, the same material buffer and the
// same six lines of Lambert — and a test (tests/instanced_equivalence.rs) asserting that the
// copies produced identical images. Shadow sampling is two hundred lines rather than six, so
// the copies became one file: the three pipelines cannot disagree about lighting if there is
// only one lighting function.
//
// The CPU mirror of `cbuffer Frame` is `FrameConstants` in src/scene.rs, field for field. A
// change here is a change there, and `scene.rs` carries the offset assertions that fail when
// only one of the two is edited.

#ifndef SCENE_FRAME_HLSLI
#define SCENE_FRAME_HLSLI

// Must match settings.rs: MAX_CASCADES, MAX_LOCAL_LIGHTS, MAX_LOCAL_SHADOW_VIEWS.
#define MAX_CASCADES 4
#define MAX_LOCAL_LIGHTS 32
#define MAX_LOCAL_SHADOW_VIEWS 16

// Must match light_cluster.rs: CLUSTERS_X/Y/Z and CLUSTER_MASK_VECTORS.
//
// The grid is fixed rather than derived from the viewport: a cluster is a fraction of the
// screen, not a number of pixels, so the same grid serves 720p and 4K and nothing has to be
// reallocated when the window is resized.
#define CLUSTERS_X 16
#define CLUSTERS_Y 9
#define CLUSTERS_Z 24

cbuffer Frame : register(b0)
{
    column_major float4x4 view_projection;
    float3 camera_position;
    float frame_padding;
    // The direction the light travels, not the direction towards it. Stored the way it is
    // authored ("the sun points down and to the left") and negated once, here, rather than
    // at every call site that sets it.
    float3 light_direction;
    float light_intensity;
    float3 light_color;
    float ambient_intensity;
    float3 ambient_color;
    float frame_padding_2;

    // ---- directional shadows -------------------------------------------------------
    // One light view-projection per cascade, and where each cascade lives in the atlas.
    column_major float4x4 cascade_view_projection[MAX_CASCADES];
    float4 cascade_uv_scale_bias[MAX_CASCADES];
    // The far distance of each cascade, in metres along the view direction.
    float4 cascade_far;
    // How much world one texel of each cascade covers, in metres. What the normal-offset
    // bias is scaled by, so one "texel" of offset means the right distance in every cascade.
    float4 cascade_texel_world;
    // x: cascade count  y: blend fraction  z: PCF kernel code  w: debug mode
    float4 shadow_params;
    // x: normal offset in texels  y: its ceiling in metres  z: shadow strength (0 = off)
    // w: disc-kernel radius in texels
    float4 shadow_params2;
    // x,y: atlas size in texels   z,w: its reciprocal
    float4 shadow_atlas_size;
    // x: camera near  y: camera far  z,w: unused
    float4 camera_params;
    // x: contact ray length  y: steps  z: thickness  w: max distance (0 = off)
    float4 contact_params;
    column_major float4x4 inverse_view_projection;

    // ---- local lights --------------------------------------------------------------
    float4 light_position_range[MAX_LOCAL_LIGHTS];   // xyz position, w range
    float4 light_color_intensity[MAX_LOCAL_LIGHTS];  // rgb colour,  w intensity
    float4 light_direction_inner[MAX_LOCAL_LIGHTS];  // xyz spot axis, w inner cosine
    // x: outer cosine  y: first shadow view (-1 for none)  z: view count  w: 1 for a spot
    float4 light_shadow[MAX_LOCAL_LIGHTS];
    float4 local_light_count;

    column_major float4x4 local_view_projection[MAX_LOCAL_SHADOW_VIEWS];
    float4 local_uv_scale_bias[MAX_LOCAL_SHADOW_VIEWS];

    // ---- the light cluster grid ----------------------------------------------------
    // x,y,z: the grid dimensions  w: 1 when a grid was built for this frame
    float4 cluster_params;
    // x: depth-slice scale  y: its bias  z: camera near  w: camera far
    float4 cluster_depth;
    // x,y: the contact-shadow target's size in texels  z,w: its reciprocal. A zero width
    // means the pass did not run this frame.
    float4 contact_target;
    // x: the kernel near cascades use  y: the one far cascades use  z: the first cascade
    // that uses it. z >= MAX_CASCADES means every cascade uses x.
    float4 filter_lod;
};

// The material constants and the four-texture material set, shared with the depth-only
// shadow pipeline — which reads the alpha cutoff out of the same layout.
//
// The material set owns t0..t3, so every texture below starts at t4.
#include "material.hlsli"

// The BRDF the lighting below evaluates. Included here rather than by each pipeline so the
// three mesh shaders cannot end up with different lobes.
#include "pbr.hlsli"

// The one shadow atlas: every cascade, every spot light and every point-light face.
// Sampled through a *comparison* sampler, so the hardware performs the depth test and the
// bilinear filter together — which is what makes a 3x3 kernel nine taps rather than
// thirty-six.
Texture2D<float> shadow_atlas : register(t4);
SamplerComparisonState shadow_sampler : register(s1);

// One bitmask per cluster: bit i means local light i reaches it. See light_cluster.rs.
//
// A buffer rather than part of `cbuffer Frame`, which is where it lived first: 13.5 KB of
// masks in the constant buffer made every read of that buffer slower, cascade matrices
// included, and cost more than the lighting the grid exists to save.
ByteAddressBuffer cluster_light_mask : register(t6);

// What the contact-shadow pass wrote: red the shadow factor, green the view distance the ray
// started from. Half the camera's width and height by default - see shadow/contact.rs and
// `contact_shadow_at` below for why that is affordable and how the green channel makes it
// look like it is not.
Texture2D<float2> contact_shadows : register(t7);

// Last frame's... no: *this* frame's depth, from the prepass, for the contact-shadow march.
// Bound to a 1x1 texture when contact shadows are off, so the pipeline needs no permutation.
Texture2D<float> scene_depth : register(t5);
SamplerState depth_sampler : register(s2);

// ---------------------------------------------------------------------------------------
// Shadow sampling
//
// Everything below returns a *shadow factor*: 1 is fully lit, 0 is fully shadowed. It is
// deliberately not multiplied into a light colour here — §13 asks for the sampler to stay
// reusable, and the lighting function below is the only place the two are combined.
// ---------------------------------------------------------------------------------------

// Clamps an atlas coordinate to its own tile, half a texel in from the edge.
//
// A PCF kernel at the edge of a tile would otherwise read its neighbour: the atlas holds a
// cascade and four spot lights side by side, and one light's depth appearing inside
// another's shadow is a bug that looks like a light leak.
float2 clamp_to_tile(float2 uv, float4 scale_bias)
{
    float2 half_texel = 0.5 * shadow_atlas_size.zw;
    float2 low = scale_bias.zw + half_texel;
    float2 high = scale_bias.zw + scale_bias.xy - half_texel;
    return clamp(uv, low, high);
}

// One depth comparison, filtered by the hardware.
float shadow_tap(float2 atlas_uv, float compare, float4 scale_bias)
{
    return shadow_atlas.SampleCmpLevelZero(
        shadow_sampler, clamp_to_tile(atlas_uv, scale_bias), compare);
}

// Percentage-closer filtering over a square kernel.
//
// The kernel width comes from the quality preset: one tap (the comparison sampler's own 2x2
// is already a soft edge), nine, or twenty-five. Unrolled per branch rather than looped over
// a variable radius, because a dynamic loop around a texture fetch is markedly slower than
// three static ones and there are only three widths.
// Interleaved gradient noise: a per-pixel angle, cheap and without a texture.
//
// Deliberately a function of the *pixel* and of nothing else. The engine has no temporal
// accumulation and no TAA, so an angle that changed per frame would turn a static penumbra
// into boiling noise. Fixed per pixel, the same budget of taps reads as a fine dither that
// stands still while the camera moves - which is the behaviour a filter without a temporal
// pass has to have.
float interleaved_gradient_noise(float2 pixel)
{
    return frac(52.9829189 * frac(dot(pixel, float2(0.06711056, 0.00583715))));
}

// The three discs, as constants.
//
// A golden-angle (Vogel) spiral: `r = sqrt((i + 0.5) / n)`, `theta = i * 2.39996323`. Well
// spread for any tap count, which is why it is a spiral rather than a stored Poisson set —
// three widths from one rule instead of three tables somebody has to keep in agreement.
//
// **Baked rather than computed.** The obvious form evaluates a sine and a cosine per tap,
// which is four to eight transcendentals per pixel and measurably slower than the nine-tap
// grid it was supposed to beat: at Medium, a four-tap disc computing its own angles cost
// 0.044 ms more at 4K than the 3x3 it replaced. Rotating a constant point costs a multiply
// and an add, and the rotation itself is one sine and one cosine for the whole kernel.
static const float2 VOGEL_4[4] = {
    float2(+0.353553, +0.000000),
    float2(-0.451544, +0.413652),
    float2(+0.069116, -0.787542),
    float2(+0.569142, +0.742346)
};
static const float2 VOGEL_6[6] = {
    float2(+0.288675, +0.000000),
    float2(-0.368684, +0.337745),
    float2(+0.056433, -0.643026),
    float2(+0.464703, +0.606123),
    float2(-0.852787, -0.150846),
    float2(+0.807834, -0.513878)
};
static const float2 VOGEL_8[8] = {
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
// `turn` is `float2(cos, sin)` of the angle, computed once per pixel. (`point` is a reserved
// word in HLSL - it is an interpolation modifier - which is why the parameter is `tap`.) The per-pixel rotation
// is what removes the spiral's own structure, which is otherwise as visible along a shadow
// edge as a grid's staircase.
float2 rotate_tap(float2 tap, float2 turn)
{
    return float2(tap.x * turn.x - tap.y * turn.y, tap.x * turn.y + tap.y * turn.x);
}

// Percentage-closer filtering.
//
// Two families. The grid kernels (codes 0-2) place their taps one texel apart and are what
// the engine shipped with; the disc kernels (codes 3-5) spend a smaller budget of taps on a
// rotated Vogel disc of `shadow_params2.w` texels. Same width of penumbra, a quarter to a
// third of the fetches, and the error left over is noise rather than a staircase.
//
// Unrolled per branch rather than looped over a variable radius, because a dynamic loop
// around a texture fetch is markedly slower than a static one, and the branch itself is
// uniform across the whole frame.
float shadow_pcf(float2 atlas_uv, float compare, float4 scale_bias, uint kernel, float2 pixel)
{
    if (kernel == 0)
    {
        return shadow_tap(atlas_uv, compare, scale_bias);
    }

    float2 texel = shadow_atlas_size.zw;
    float total = 0.0;

    if (kernel >= 3)
    {
        float angle = interleaved_gradient_noise(pixel) * 6.2831853;
        // One sine and one cosine for the whole kernel, not one pair per tap.
        float2 turn = float2(cos(angle), sin(angle));
        float2 radius = texel * shadow_params2.w;
        // One unrolled loop per tap count, for the same reason the grid kernels have one
        // each: a dynamic loop around a texture fetch is markedly slower than a static one,
        // and it costs registers in a shader that is already the frame's largest.
        if (kernel == 3)
        {
            [unroll]
            for (uint i4 = 0; i4 < 4; ++i4)
            {
                total += shadow_tap(
                    atlas_uv + rotate_tap(VOGEL_4[i4], turn) * radius, compare, scale_bias);
            }
            return total * 0.25;
        }
        if (kernel == 4)
        {
            [unroll]
            for (uint i6 = 0; i6 < 6; ++i6)
            {
                total += shadow_tap(
                    atlas_uv + rotate_tap(VOGEL_6[i6], turn) * radius, compare, scale_bias);
            }
            return total * (1.0 / 6.0);
        }
        [unroll]
        for (uint i8 = 0; i8 < 8; ++i8)
        {
            total += shadow_tap(
                atlas_uv + rotate_tap(VOGEL_8[i8], turn) * radius, compare, scale_bias);
        }
        return total * 0.125;
    }

    if (kernel == 1)
    {
        [unroll]
        for (int y = -1; y <= 1; ++y)
        {
            [unroll]
            for (int x = -1; x <= 1; ++x)
            {
                total += shadow_tap(atlas_uv + float2(x, y) * texel, compare, scale_bias);
            }
        }
        return total / 9.0;
    }

    [unroll]
    for (int y2 = -2; y2 <= 2; ++y2)
    {
        [unroll]
        for (int x2 = -2; x2 <= 2; ++x2)
        {
            total += shadow_tap(atlas_uv + float2(x2, y2) * texel, compare, scale_bias);
        }
    }
    return total / 25.0;
}

// Projects a world position into one shadow view and filters it.
//
// Returns 1 — fully lit — for anything outside the view, which is what makes a cascade's
// edge a fade into the next cascade rather than a wall of darkness.
float shadow_in_view(
    float3 world_position,
    column_major float4x4 light_view_projection,
    float4 scale_bias,
    uint kernel,
    float2 pixel)
{
    float4 light_clip = mul(light_view_projection, float4(world_position, 1.0));
    if (light_clip.w <= 0.0)
    {
        return 1.0;
    }
    float3 ndc = light_clip.xyz / light_clip.w;
    if (any(abs(ndc.xy) > 1.0) || ndc.z < 0.0 || ndc.z > 1.0)
    {
        return 1.0;
    }
    // Clip space is Y-up and a texture is Y-down, which is the sign flip below and nothing
    // more mysterious than that.
    float2 uv = ndc.xy * float2(0.5, -0.5) + 0.5;
    return shadow_pcf(uv * scale_bias.xy + scale_bias.zw, ndc.z, scale_bias, kernel, pixel);
}

// Which cube face a direction from a point light falls on: +X, -X, +Y, -Y, +Z, -Z.
//
// The CPU builds the six views in this order (see shadow/local.rs), and a test there asserts
// the two agree. Getting it wrong lights five sixths of a room correctly.
uint cube_face_of(float3 direction)
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
float view_depth_of(float3 world_position)
{
    float4 clip = mul(view_projection, float4(world_position, 1.0));
    return clip.w;
}

// Which cascade covers `depth`, and how far it has crossed into the next one.
//
// The CPU mirror is `cascade_blend` in shadow/cascade.rs, and it is the same arithmetic:
// find the first cascade whose far plane is beyond the fragment, then measure how far into
// that cascade's blend region it lies.
void select_cascade(float depth, out uint near_index, out uint far_index, out float blend)
{
    uint count = (uint)shadow_params.x;
    near_index = count;
    [unroll]
    for (uint index = 0; index < MAX_CASCADES; ++index)
    {
        if (index < count && near_index == count && depth < cascade_far[index])
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

    float far_edge = cascade_far[near_index];
    float near_edge = near_index == 0 ? camera_params.x : cascade_far[near_index - 1];
    float width = (far_edge - near_edge) * shadow_params.y;
    if (width > 0.0 && near_index + 1 < count && depth > far_edge - width)
    {
        far_index = near_index + 1;
        blend = saturate((depth - (far_edge - width)) / width);
    }
}

// The screen-space contact shadow.
//
// A short march along the ray towards the light, comparing each step against the depth the
// prepass wrote. It resolves the centimetres a shadow map cannot: the gap under a boot, the
// seam where a crate meets the floor. Deliberately short — see ContactShadowSettings.
//
// The `thickness` test is what stops every silhouette in the frame from occluding
// everything behind it: the depth buffer records a surface, not a solid, so a sample that is
// *much* nearer than the ray is a different object the ray passes behind rather than an
// occluder it runs into.
float contact_shadow(float3 world_position, float3 normal, float3 to_light, float view_distance)
{
    float max_distance = contact_params.w;
    if (max_distance <= 0.0 || view_distance > max_distance)
    {
        return 1.0;
    }

    int steps = max((int)contact_params.y, 1);
    float step_length = contact_params.x / (float)steps;
    float near_plane = camera_params.x;
    float far_plane = camera_params.y;
    float projection_depth = far_plane / (near_plane - far_plane);

    // Fade the whole effect out over the last quarter of its range, so it does not switch
    // off along a visible circle on the floor.
    float fade = saturate((max_distance - view_distance) / (max_distance * 0.25));

    // Start the march half a step off the surface, along the normal.
    //
    // Without this the ray begins exactly on the surface it is shading, and on a large flat
    // one — a floor — the early samples project back onto that same floor. The depth
    // difference they find is then the floor's own slope in screen space, not an occluder,
    // and once the camera is low enough that the difference lands inside the thickness
    // window the whole floor reports occluded. Which parts of it do so depends on the
    // camera's angle, so the artefact swims about as the camera turns.
    float3 origin = world_position + normal * (step_length * 0.5);

    float occlusion = 0.0;
    [loop]
    for (int step = 1; step <= steps; ++step)
    {
        float3 sample_position = origin + to_light * (step_length * (float)step);
        float4 clip = mul(view_projection, float4(sample_position, 1.0));
        if (clip.w <= 0.0)
        {
            break;
        }
        float3 ndc = clip.xyz / clip.w;
        if (any(abs(ndc.xy) > 1.0))
        {
            break;
        }
        float2 uv = ndc.xy * float2(0.5, -0.5) + 0.5;
        float scene_ndc_depth = scene_depth.SampleLevel(depth_sampler, uv, 0).r;
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
        if (difference > 0.02 && difference < contact_params.z)
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
// contact shadow onto the wall behind it. The weight falls off with the disagreement, and a
// texel with no surface at all (the sky, which the pass writes as distance zero) is dropped
// outright.
//
// If every neighbour disagrees - a one-pixel-wide silhouette, where the reconstruction has
// nothing honest to say - the answer is "unoccluded" rather than a guess. Contact shadows
// only ever darken, so failing towards 1 fails towards the picture without them.
float contact_shadow_at(float2 pixel, float view_distance)
{
    if (contact_params.w <= 0.0 || view_distance > contact_params.w || contact_target.x < 1.0)
    {
        return 1.0;
    }

    float2 viewport = max(camera_params.zw, float2(1.0, 1.0));
    float2 coordinate = (pixel / viewport) * contact_target.xy - 0.5;
    float2 base = floor(coordinate);
    float2 fraction = coordinate - base;
    int2 limit = int2(contact_target.xy) - 1;

    float total = 0.0;
    float total_weight = 0.0;
    [unroll]
    for (int y = 0; y < 2; ++y)
    {
        [unroll]
        for (int x = 0; x < 2; ++x)
        {
            int2 texel = clamp(int2(base) + int2(x, y), int2(0, 0), limit);
            float2 tap = contact_shadows.Load(int3(texel, 0));
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
float directional_shadow(float3 world_position, float3 normal, float view_distance, float2 pixel)
{
    float strength = shadow_params2.z;
    uint count = (uint)shadow_params.x;
    if (strength <= 0.0 || count == 0)
    {
        return 1.0;
    }

    uint near_index;
    uint far_index;
    float blend;
    select_cascade(view_distance, near_index, far_index, blend);
    if (near_index >= count)
    {
        return 1.0;
    }

    // The filter's level of detail. A cascade past `filter_lod.z` steps down one kernel:
    // it covers roughly three times the distance for the same texels, so its penumbra is
    // already three times wider in world terms, and the far cascades are most of the screen
    // in an outdoor scene.
    uint kernel = near_index >= (uint)filter_lod.z ? (uint)filter_lod.y : (uint)filter_lod.x;
    float offset_scale = min(
        cascade_texel_world[near_index] * shadow_params2.x, shadow_params2.y);
    float3 offset_position = world_position + normal * offset_scale;

    float factor = shadow_in_view(
        offset_position,
        cascade_view_projection[near_index],
        cascade_uv_scale_bias[near_index],
        kernel,
        pixel);

    if (blend > 0.0)
    {
        float far_offset = min(
            cascade_texel_world[far_index] * shadow_params2.x, shadow_params2.y);
        uint far_kernel =
            far_index >= (uint)filter_lod.z ? (uint)filter_lod.y : (uint)filter_lod.x;
        float next = shadow_in_view(
            world_position + normal * far_offset,
            cascade_view_projection[far_index],
            cascade_uv_scale_bias[far_index],
            far_kernel,
            pixel);
        factor = lerp(factor, next, blend);
    }

    // Fade out over the last tenth of the shadow range, so the far edge of the last cascade
    // is a gradient rather than a line drawn across the world.
    float shadow_distance = cascade_far[count - 1];
    float fade = saturate((shadow_distance - view_distance) / max(shadow_distance * 0.1, 0.001));
    factor = lerp(1.0, factor, fade);

    return lerp(1.0, factor, strength);
}

// The shadow factor for one local light, or 1 if it has no shadow map.
float local_light_shadow(uint light, float3 world_position, float3 normal, float2 pixel)
{
    float first_view = light_shadow[light].y;
    if (first_view < 0.0)
    {
        return 1.0;
    }
    uint view = (uint)first_view;
    if (light_shadow[light].w < 0.5)
    {
        // A point light: pick the face the surface is on.
        view += cube_face_of(world_position - light_position_range[light].xyz);
    }
    if (view >= MAX_LOCAL_SHADOW_VIEWS)
    {
        return 1.0;
    }

    // A local map's texel covers more world the further it is from the light, so the normal
    // offset is scaled by distance rather than by a per-cascade constant.
    float distance_to_light = length(world_position - light_position_range[light].xyz);
    float offset = min(distance_to_light * 0.004 * shadow_params2.x, shadow_params2.y);
    return shadow_in_view(
        world_position + normal * offset,
        local_view_projection[view],
        local_uv_scale_bias[view],
        (uint)shadow_params.z,
        pixel);
}

// Which cluster a fragment falls in.
//
// Tiles come from the pixel position, the slice from the view depth on a logarithmic scale.
// The CPU mirror is `slice_of` and `tile_of` in light_cluster.rs, and it is the same
// arithmetic: a light whose cluster the shader computes differently from the way the CPU
// assigned it is a light that goes out in a band across the screen.
uint cluster_of(float2 pixel, float view_depth)
{
    float2 viewport = max(camera_params.zw, float2(1.0, 1.0));
    float2 tile = floor(pixel / viewport * float2(CLUSTERS_X, CLUSTERS_Y));
    tile = clamp(tile, float2(0.0, 0.0), float2(CLUSTERS_X - 1, CLUSTERS_Y - 1));

    float slice = floor(log(max(view_depth, 1e-4)) * cluster_depth.x + cluster_depth.y);
    slice = clamp(slice, 0.0, (float)(CLUSTERS_Z - 1));

    return (uint)tile.x + (uint)tile.y * CLUSTERS_X + (uint)slice * (CLUSTERS_X * CLUSTERS_Y);
}

// The lights this fragment has to evaluate, as a bitmask.
//
// From the cluster grid when the frame built one, and from the whole light list when it did
// not — the offscreen helpers and the older tests produce frames with lights and no camera
// to cluster against, and "every light" is the correct answer there.
uint lights_at(float2 pixel, float view_depth)
{
    uint count = min((uint)local_light_count.x, (uint)MAX_LOCAL_LIGHTS);
    // The common case in an outdoor scene, and the one worth an early exit: no local lights
    // at all means no grid lookup, so a frame lit only by the sun does not pay for the
    // structure that exists for the frames that are not.
    if (count == 0u)
    {
        return 0u;
    }
    uint live = count >= 32u ? 0xFFFFFFFFu : ((1u << count) - 1u);
    if (cluster_params.w < 0.5)
    {
        return live;
    }
    uint cluster = cluster_of(pixel, view_depth);
    return cluster_light_mask.Load(cluster * 4) & live;
}

// Everything the local lights add at a surface.
//
// The loop runs over the *cluster's* lights, not the frame's: `firstbitlow` walks the set
// bits of the mask, so a fragment with two lights near it costs two iterations whether the
// scene has two lights or thirty-two. Before the grid existed this was a loop over every
// light in the frame, and a torch in the far corner of the arena was evaluated and rejected
// on every pixel of the screen — see docs/shadows.md and light_cluster.rs.
float3 local_light_contribution(
    float3 world_position,
    Surface surface,
    float receives_shadow,
    float2 pixel,
    float view_distance)
{
    float3 normal = surface.normal;
    float3 total = float3(0.0, 0.0, 0.0);
    uint mask = lights_at(pixel, view_distance);
    [loop]
    while (mask != 0u)
    {
        uint light = firstbitlow(mask);
        mask &= mask - 1u;

        float3 to_light = light_position_range[light].xyz - world_position;
        float distance_to_light = length(to_light);
        float range = light_position_range[light].w;
        if (distance_to_light >= range)
        {
            continue;
        }
        float3 direction = to_light / max(distance_to_light, 1e-4);
        // The BRDF rather than a Lambert term, and evaluated before the shadow lookup for
        // the same reason the old cosine was: a light behind the surface contributes
        // nothing, and a fragment that contributes nothing must not pay for a shadow sample.
        float3 shaded = shade_light(surface, direction);
        if (all(shaded <= 0.0))
        {
            continue;
        }

        // Inverse-square, windowed so it reaches exactly zero at the light's range: an
        // unwindowed falloff never reaches zero, and a light that is never quite off cannot
        // be culled at any distance.
        float normalized = distance_to_light / range;
        float window = saturate(1.0 - normalized * normalized * normalized * normalized);
        float attenuation = window * window / (1.0 + distance_to_light * distance_to_light);

        if (light_shadow[light].w > 0.5)
        {
            // A spot light: the cone, with a soft rim between its inner and outer angles.
            float cosine = dot(-direction, normalize(light_direction_inner[light].xyz));
            float inner = light_direction_inner[light].w;
            float outer = light_shadow[light].x;
            float cone = saturate((cosine - outer) / max(inner - outer, 1e-4));
            attenuation *= cone * cone;
        }
        if (attenuation <= 0.0)
        {
            continue;
        }

        float shadow = 1.0;
        if (receives_shadow > 0.5)
        {
            shadow = local_light_shadow(light, world_position, normal, pixel);
        }

        total += shaded * light_color_intensity[light].rgb
            * (light_color_intensity[light].w * attenuation * shadow);
    }
    return total;
}

// A debug colour for the cascade a fragment landed in, or white past the last one.
float3 cascade_debug_tint(float view_distance)
{
    uint count = (uint)shadow_params.x;
    uint near_index;
    uint far_index;
    float blend;
    select_cascade(view_distance, near_index, far_index, blend);
    if (near_index >= count)
    {
        return float3(1.0, 1.0, 1.0);
    }
    float3 tints[MAX_CASCADES] = {
        float3(1.0, 0.4, 0.4),
        float3(0.4, 1.0, 0.4),
        float3(0.4, 0.55, 1.0),
        float3(1.0, 0.95, 0.4)
    };
    float3 tint = tints[near_index];
    if (blend > 0.0)
    {
        tint = lerp(tint, tints[far_index], blend);
    }
    return tint;
}

// The shadow atlas itself, drawn in the corner of the screen.
//
// Returns true when `pixel` is inside the preview and fills `color` with what to show there.
//
// # Why this is inside the mesh shader
//
// A debug view of a texture wants to be a fullscreen pass with its own pipeline. It is here
// instead because every pixel of the preview is covered by *some* fragment of the scene —
// the sky dome, if nothing else — and every one of those fragments takes this branch and
// writes the same colour, so the preview appears without a second pipeline, a second root
// signature, a second vertex buffer or a second draw. What it costs is one compare per
// fragment in a branch that is uniform across the frame and never taken while the debug mode
// is off. What it cannot do is draw over a hole in the scene, which is why the preview sits
// in the bottom-left corner where the ground always is.
//
// Empty atlas is white (the clear is the far plane) and a caster is darker the closer it is
// to the light. The red lines are the cascade tile borders, which is what makes "the third
// cascade is empty" and "a light's tile is bleeding into its neighbour" visible at a glance.
bool shadow_atlas_preview(float2 pixel, out float3 color)
{
    color = float3(0.0, 0.0, 0.0);
    float2 viewport = camera_params.zw;
    if (viewport.x < 1.0 || viewport.y < 1.0 || shadow_atlas_size.x < 1.0)
    {
        return false;
    }

    const float MARGIN = 16.0;
    float side = min(viewport.x, viewport.y) * 0.42;
    float2 size = float2(side * (shadow_atlas_size.x / shadow_atlas_size.y), side);
    float2 low = float2(MARGIN, viewport.y - MARGIN - size.y);
    float2 high = low + size;
    if (any(pixel < low) || any(pixel > high))
    {
        return false;
    }

    float2 uv = (pixel - low) / size;
    float depth = shadow_atlas.SampleLevel(depth_sampler, uv, 0);
    color = depth.xxx;

    // The cascade borders, two texels wide in screen space so they survive the downscale.
    float2 edge = 2.0 / size;
    uint count = (uint)shadow_params.x;
    for (uint index = 0u; index < count; ++index)
    {
        float4 rect = cascade_uv_scale_bias[index];
        float2 within = (uv - rect.zw) / max(rect.xy, 1e-6);
        if (any(within < 0.0) || any(within > 1.0))
        {
            continue;
        }
        float2 border = min(within, 1.0 - within);
        if (min(border.x, border.y) < max(edge.x, edge.y))
        {
            color = float3(0.9, 0.15, 0.15);
        }
    }
    return true;
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
// The BRDF is in pbr.hlsli: GGX, height-correlated Smith, Schlick, in the metallic-roughness
// workflow glTF describes. The sun and every local light evaluate the same `shade_light`
// against the same `Surface`, so a highlight on a character under a torch and one under the
// sun are the same model rather than two.
//
// Ambient is added *outside* the shadow factor, deliberately: the sun being blocked does not
// stop the sky from lighting a surface, and multiplying the ambient term by the shadow is
// what makes a shadowed area look like a hole in the world. Occlusion *is* applied to it,
// which is what the ORM map's red channel is for - a crevice is dark because the sky cannot
// reach it, which is a statement about ambient light and not about the sun.
float4 shade_surface(
    float4 screen_position,
    float3 world_position,
    float3 world_normal,
    float4 world_tangent,
    float2 uv,
    float4 tint,
    float emissive_scale,
    float receives_shadow)
{
    MaterialSample material = sample_material(uv);
    float4 albedo = material.base_color;
    // Alpha-tested materials: foliage, fences, grates. `alpha_cutoff` is zero for an opaque
    // material, so this is one comparison that is never taken for most of the scene. The
    // same test runs in the shadow pass, which is what makes a leaf's shadow leaf-shaped.
    if (alpha_cutoff > 0.0 && albedo.a < alpha_cutoff)
    {
        discard;
    }

    // The shading normal, out of the tangent frame the vertex carries. A material with no
    // normal map samples the flat fallback, and this is the interpolated normal again.
    float3 normal =
        apply_normal_map(world_normal, world_tangent, material.normal_ts, normal_scale);
    float3 to_light = normalize(-light_direction);
    // The geometric normal is kept for the two things that want the surface itself rather
    // than what the map made of it: the early-out below, and the shadow bias, which offsets
    // along the surface and would push itself into a crevice if it followed a normal map.
    float3 geometric_normal = normalize(world_normal);
    float lambert = saturate(dot(normal, to_light));
    float3 tinted = albedo.rgb * tint.rgb;
    float view_distance = view_depth_of(world_position);

    float3 view = camera_position - world_position;
    // A fragment exactly at the camera has no view direction. It cannot be visible either,
    // but the normalize would be a NaN that spreads through the whole pixel.
    view = dot(view, view) > 1e-12 ? normalize(view) : normal;
    Surface surface = make_surface(
        normal, view, tinted, material.roughness, material.metallic, material.occlusion);

    float shadow = 1.0;
    if (receives_shadow > 0.5 && lambert > 0.0)
    {
        shadow = directional_shadow(
            world_position, geometric_normal, view_distance, screen_position.xy);
        if (shadow > 0.0)
        {
            shadow = min(shadow, contact_shadow_at(screen_position.xy, view_distance));
        }
    }

    uint debug_mode = (uint)shadow_params.w;
    if (debug_mode == 3u)
    {
        float3 preview;
        if (shadow_atlas_preview(screen_position.xy, preview))
        {
            return float4(preview, 1.0);
        }
    }
    if (debug_mode == 2u)
    {
        // The shadow factor alone, on a neutral surface: what the cascades, the filter and
        // the biases are actually producing, with albedo and lighting out of the way.
        return float4(shadow.xxx, 1.0);
    }
    if (debug_mode >= 4u && debug_mode <= 7u)
    {
        // The material maps, raw: 4 base colour, 5 normal, 6 ORM, 7 emissive. What proves the
        // descriptor range really carries four distinct resources in the frozen order. No
        // shading is applied, deliberately - this is a resource-binding probe, and it stays
        // one now that the lighting model consumes all four.
        if (debug_mode == 4u)
        {
            return float4(albedo.rgb, 1.0);
        }
        return float4(material_map_preview(debug_mode, uv), 1.0);
    }
    if (debug_mode >= 8u)
    {
        // What the *shading* made of those maps, which is the other half of the question and
        // the one a raw texel view cannot answer: 8 the shading normal after the TBN
        // transform, 9 roughness, 10 metallic, 11 occlusion. A normal map whose handedness is
        // inverted looks perfectly correct at 5 and visibly wrong at 8.
        if (debug_mode == 8u)
        {
            return float4(normal * 0.5 + 0.5, 1.0);
        }
        if (debug_mode == 9u)
        {
            return float4(surface.roughness.xxx, 1.0);
        }
        if (debug_mode == 10u)
        {
            return float4(material.metallic.xxx, 1.0);
        }
        return float4(surface.occlusion.xxx, 1.0);
    }

    float3 direct = shade_light(surface, to_light) * light_color * (light_intensity * shadow);
    // Occlusion belongs to the ambient term and nothing else. Applying it to the sun as well
    // is the common shortcut, and what it produces is a surface that goes dark when a light
    // moves onto it: the map says how much *sky* reaches the point, and the sun is not sky.
    float3 ambient =
        surface.diffuse_color * ambient_color * (ambient_intensity * surface.occlusion);
    float3 local = local_light_contribution(
        world_position, surface, receives_shadow, screen_position.xy, view_distance);
    // The material's emissive factor times its emissive map, plus the per-object emissive
    // pulse - which tints with the object and is a gameplay effect (a dummy flashing white),
    // not a property of the surface.
    float3 glow = material.emissive + tinted * emissive_scale;

    float3 color = direct + ambient + local + glow;
    if (debug_mode == 1u)
    {
        color *= cascade_debug_tint(view_distance);
    }
    return float4(color, albedo.a * tint.a);
}

#endif // SCENE_FRAME_HLSLI
