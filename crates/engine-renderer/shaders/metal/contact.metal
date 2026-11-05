// The screen-space contact shadow, as a pass of its own. Port of contact.hlsl.
//
// It used to be a function called from the mesh pixel shader: every shaded pixel marched
// twelve samples through the depth buffer, and at 4K that was 1.31 ms of a 2.50 ms frame.
// The march is unchanged; what changed is that it runs once per *output texel of this pass*
// rather than once per shaded pixel, and this pass is normally half the camera's width and
// half its height. A quarter of the rays.
//
// It is affordable because a contact shadow is a low-frequency term. What it *does* carry is a
// discontinuity at a depth edge, and that is what the second output channel is for: the
// lighting pass reconstructs from this target with a depth-weighted filter
// (`contact_shadow_at` in scene_frame.h) rather than a bilinear one.
//
// ```text
// depth prepass --> CopyResource --> scene_depth (full res)
//                                        |
//                                        v
//                        this pass, at 1/divisor on each axis
//                                        |
//                                        v
//                         contact target: R16G16_FLOAT
//                            r: shadow factor  g: view depth
//                                        |
//                                        v
//                        mesh pass, depth-weighted upsample
// ```
//
// The normal is reconstructed from the depth buffer rather than read from a G-buffer, because
// this renderer is forward and there is no G-buffer to read.
//
// Entry points: contact_vs (HLSL VSMain), contact_fs (HLSL PSMain). No vertex buffer.

#include "scene_frame.h"

struct ContactV2F
{
    float4 position [[position]];
    float2 uv;
};

// A fullscreen triangle from the vertex index alone: no vertex buffer, no input layout, no
// index buffer. Draw 3 vertices, vertexStart 0.
vertex ContactV2F contact_vs(uint vertex_id [[vertex_id]])
{
    ContactV2F output;
    output.uv = float2(float((vertex_id << 1) & 2u), float(vertex_id & 2u));
    output.position = float4(output.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

// A world position from this frame's depth buffer.
//
// Returns false where nothing was drawn - the prepass leaves 1.0 there - so the sky costs
// one comparison rather than a march.
inline bool world_position_at(
    constant FrameConstants& f,
    depth2d<float> scene_depth,
    float2 uv,
    thread float3& world_position,
    thread float& view_distance)
{
    world_position = float3(0.0, 0.0, 0.0);
    view_distance = 0.0;

    float depth = scene_depth.sample(depth_sampler, uv, level(0.0f));
    if (depth >= 1.0)
    {
        return false;
    }

    float2 ndc = uv * float2(2.0, -2.0) + float2(-1.0, 1.0);
    float4 clip = f.inverse_view_projection * float4(ndc, depth, 1.0);
    if (abs(clip.w) < 1e-6)
    {
        return false;
    }
    world_position = clip.xyz / clip.w;
    view_distance = view_depth_of(f, world_position);
    return true;
}

// Output: RG16Float colour attachment 0 (r: shadow factor, g: view distance).
fragment float2 contact_fs(
    ContactV2F input [[stage_in]],
    constant FrameConstants& frame [[buffer(0)]],
    depth2d<float> scene_depth [[texture(5)]])
{
    float3 world_position;
    float view_distance;
    if (!world_position_at(frame, scene_depth, input.uv, world_position, view_distance))
    {
        // Nothing here to shadow. A shadow factor of 1 and a depth of 0: the reconstruction
        // filter reads the depth channel to decide which neighbours belong to the same
        // surface, and a zero says "no surface", which is exactly right for the sky.
        return float2(1.0, 0.0);
    }

    // The normal from the depth buffer's own gradients. HLSL used the fine derivative intrinsics; MSL has
    // only dfdx/dfdy (per-quad derivatives), see BINDINGS.md.
    float3 dx = dfdx(world_position);
    float3 dy = dfdy(world_position);
    float3 normal = normalize(cross(dy, dx));
    // The reconstruction can come out facing away from the camera at a silhouette, where the
    // two derivatives straddle a depth discontinuity. Flip it rather than discard it: a
    // normal pointing into the surface would start the march below the floor.
    float3 to_camera = normalize(float3(frame.camera_position) - world_position);
    if (dot(normal, to_camera) < 0.0)
    {
        normal = -normal;
    }

    float3 to_light = normalize(-float3(frame.light_direction));
    // A surface the sun does not reach is already fully shadowed by the cascades; marching
    // for it would darken a face that is dark for a different reason.
    if (dot(normal, to_light) <= 0.0)
    {
        return float2(1.0, view_distance);
    }

    float shadow = contact_shadow(frame, scene_depth, world_position, normal, to_light, view_distance);
    return float2(shadow, view_distance);
}
