// The screen-space contact shadow, as a pass of its own.
//
// It used to be a function called from the mesh pixel shader: every shaded pixel marched
// twelve samples through the depth buffer, and at 4K that was 1.31 ms of a 2.50 ms frame —
// the single largest item in it. The march is unchanged; what changed is that it runs once
// per *output texel of this pass* rather than once per shaded pixel, and this pass is
// normally half the camera's width and half its height. A quarter of the rays.
//
// It is affordable to do this because a contact shadow is a low-frequency term. It darkens
// the centimetres where a boot meets the floor; it does not carry an edge the eye can
// resolve at full resolution. What it *does* carry is a discontinuity at a depth edge, and
// that is what the second output channel is for: the lighting pass reconstructs from this
// target with a depth-weighted filter (`contact_shadow_at` in scene_frame.hlsli) rather than
// a bilinear one, so a foot in front of a wall does not smear its shadow onto the wall.
//
// ```text
// depth prepass ──► CopyResource ──► scene_depth (full res)
//                                        │
//                                        ▼
//                        this pass, at 1/divisor on each axis
//                                        │
//                                        ▼
//                         contact target: R16G16_FLOAT
//                            r: shadow factor  g: view depth
//                                        │
//                                        ▼
//                        mesh pass, depth-weighted upsample
// ```
//
// The normal is reconstructed from the depth buffer rather than read from a G-buffer,
// because this renderer is forward and there is no G-buffer to read. Screen-space
// derivatives of the reconstructed view position give a normal that is exact on a flat
// surface and wrong along a silhouette, which is acceptable here: the normal is used only to
// lift the ray's origin half a step off the surface, and a wrong lift along a silhouette
// costs a texel of a term that is being reconstructed with a depth-aware filter anyway.

#include "scene_frame.hlsli"

struct Varyings
{
    float4 position : SV_Position;
    float2 uv       : TEXCOORD0;
};

// A fullscreen triangle from the vertex index alone: no vertex buffer, no input layout, no
// index buffer. Three vertices covering the screen beat two triangles covering it, because
// the diagonal seam of a quad is a row of pixels shaded by two different triangles.
Varyings VSMain(uint vertex_id : SV_VertexID)
{
    Varyings output;
    output.uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    output.position = float4(output.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

// A world position from this frame's depth buffer.
//
// Returns false where nothing was drawn — the prepass leaves 1.0 there — so the sky costs
// one comparison rather than a march.
bool world_position_at(float2 uv, out float3 world_position, out float view_distance)
{
    world_position = float3(0.0, 0.0, 0.0);
    view_distance = 0.0;

    float depth = scene_depth.SampleLevel(depth_sampler, uv, 0).r;
    if (depth >= 1.0)
    {
        return false;
    }

    float2 ndc = uv * float2(2.0, -2.0) + float2(-1.0, 1.0);
    float4 clip = mul(inverse_view_projection, float4(ndc, depth, 1.0));
    if (abs(clip.w) < 1e-6)
    {
        return false;
    }
    world_position = clip.xyz / clip.w;
    view_distance = view_depth_of(world_position);
    return true;
}

float2 PSMain(Varyings input) : SV_Target
{
    float3 world_position;
    float view_distance;
    if (!world_position_at(input.uv, world_position, view_distance))
    {
        // Nothing here to shadow. A shadow factor of 1 and a depth of 0: the reconstruction
        // filter reads the depth channel to decide which neighbours belong to the same
        // surface, and a zero says "no surface", which is exactly right for the sky.
        return float2(1.0, 0.0);
    }

    // The normal from the depth buffer's own gradients. `ddx_fine`/`ddy_fine` rather than the
    // coarse forms: the coarse ones share one derivative across a 2x2 quad, which on a
    // half-resolution target is a four-pixel neighbourhood of the camera image and enough to
    // tilt the normal visibly on curved geometry.
    float3 dx = ddx_fine(world_position);
    float3 dy = ddy_fine(world_position);
    float3 normal = normalize(cross(dy, dx));
    // The reconstruction can come out facing away from the camera at a silhouette, where the
    // two derivatives straddle a depth discontinuity. Flip it rather than discard it: a
    // normal pointing into the surface would start the march below the floor.
    float3 to_camera = normalize(camera_position - world_position);
    if (dot(normal, to_camera) < 0.0)
    {
        normal = -normal;
    }

    float3 to_light = normalize(-light_direction);
    // A surface the sun does not reach is already fully shadowed by the cascades; marching
    // for it would darken a face that is dark for a different reason.
    if (dot(normal, to_light) <= 0.0)
    {
        return float2(1.0, view_distance);
    }

    float shadow = contact_shadow(world_position, normal, to_light, view_distance);
    return float2(shadow, view_distance);
}
