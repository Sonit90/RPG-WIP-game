// Depth-only rendering, for the shadow atlas and for the camera's depth prepass.
// Port of shadow_depth.hlsl.
//
// The same geometry the camera pass draws, with everything that is not position removed:
// no normal, no lighting, no render target. Two vertex paths, matching the two the camera
// pass has for geometry that moves:
//
//     shadow_depth_instanced_vs   (HLSL VSInstanced) rigid geometry, model matrix per instance
//     shadow_depth_skinned_vs     (HLSL VSSkinned)   characters, bone palette per draw
//
// There is deliberately no per-object rigid path. The shadow pass always batches.
//
// The alpha-tested variant is a real fragment shader that discards
// (shadow_depth_alpha_fs, HLSL PSAlpha), and the opaque variant has no fragment shader at all
// - the Rust side creates that render pipeline with a null fragment function.

#include "common.h"
#include "material.h"

// HLSL constant buffer ShadowView at b0. ShadowViewConstants (src/shadow/renderer.rs), 80 bytes.
//
// The view being rendered. Deliberately *not* the frame constants: a shadow pass runs once
// per cascade and once per local light, and re-uploading two kilobytes of lighting data for
// each of them to change one matrix would be most of the pass's CPU cost.
struct ShadowViewConstants
{
    float4x4 light_view_projection;   // offset  0  column-major
    // x: 1 when this view is a cascade, 0 for a local light. Unused by the shader today.
    float4 shadow_view_params;        // offset 64
};
static_assert(sizeof(ShadowViewConstants) == 80, "ShadowViewConstants must match the Rust ShadowViewConstants (80 bytes)");

struct DepthV2F
{
    float4 position [[position]];
    // Carried for the alpha-tested fragment shader. The opaque pipeline binds no fragment
    // function and the interpolator is dropped, so this costs nothing there.
    float2 uv;
};

// HLSL RigidVertex: the MeshVertex stream (slot 0, only position and uv are read; the buffer
// stride is still sizeof(MeshVertex) == 52) plus the InstanceTransform stream (slot 1, only
// model0..3 are read; stride 96).
vertex DepthV2F shadow_depth_instanced_vs(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device MeshVertex* vertices [[buffer(16)]],
    const device InstanceTransform* instances [[buffer(17)]],
    constant ShadowViewConstants& shadow_view [[buffer(0)]])
{
    MeshVertex input = vertices[vertex_id];
    InstanceTransform instance = instances[instance_id];

    // See mesh_instanced.metal: the four per-instance float4s are the columns of the real
    // local-to-world matrix. HLSL built the row-filled transpose and multiplied vector-first;
    // MSL's column-filled float4x4 is the real matrix and `model * v` is the same transform.
    float4x4 model_matrix = float4x4(
        float4(instance.model0),
        float4(instance.model1),
        float4(instance.model2),
        float4(instance.model3));
    float4 world = model_matrix * float4(float3(input.position), 1.0);

    DepthV2F output;
    output.position = shadow_view.light_view_projection * world;
    output.uv = float2(input.uv);
    return output;
}

// HLSL SkinnedVertex: the SkinnedVertex stream (slot 0, only position, uv, joints and weights
// are read; stride sizeof(SkinnedVertex) == 76).
vertex DepthV2F shadow_depth_skinned_vs(
    uint vertex_id [[vertex_id]],
    const device SkinnedVertex* vertices [[buffer(16)]],
    constant ShadowViewConstants& shadow_view [[buffer(0)]],
    constant ObjectConstants& object [[buffer(1)]],
    constant SkinPalette& palette [[buffer(3)]])
{
    SkinnedVertex input = vertices[vertex_id];

    uint joint_x = input.joints01 & 0xFFFFu;
    uint joint_y = input.joints01 >> 16;
    uint joint_z = input.joints23 & 0xFFFFu;
    uint joint_w = input.joints23 >> 16;
    float4 weights = float4(input.weights);

    float4x4 skin =
          palette.bones[joint_x] * weights.x
        + palette.bones[joint_y] * weights.y
        + palette.bones[joint_z] * weights.z
        + palette.bones[joint_w] * weights.w;

    float4 world = object.model * (skin * float4(float3(input.position), 1.0));

    DepthV2F output;
    output.position = shadow_view.light_view_projection * world;
    output.uv = float2(input.uv);
    return output;
}

// The alpha test, and nothing else.
//
// No return value: the pipeline has no render target bound, so there is nothing to write.
// What this shader does is decide whether the fragment reaches the depth buffer at all,
// which is what makes a leaf cast a leaf-shaped shadow instead of a rectangle.
//
// Uses only the base-colour factor / alpha_cutoff from the Material buffer (buffer 2) and
// the base colour map (texture 0) through base_color_sampler (material.h).
fragment void shadow_depth_alpha_fs(
    DepthV2F input [[stage_in]],
    constant MaterialConstants& material [[buffer(2)]],
    texture2d<float> base_color_map [[texture(0)]])
{
    float alpha = material.base_color.a * base_color_map.sample(base_color_sampler, input.uv).a;
    if (alpha < material.alpha_cutoff)
    {
        discard_fragment();
    }
}
