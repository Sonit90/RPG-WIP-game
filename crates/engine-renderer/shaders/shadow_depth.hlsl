// Depth-only rendering, for the shadow atlas and for the camera's depth prepass.
//
// The same geometry the camera pass draws, with everything that is not position removed:
// no normal, no lighting, no render target. A pipeline state built from this writes depth
// and nothing else, which is what §5 means by an optimized depth-only pipeline — on most
// hardware a pixel shader that outputs nothing lets the rasterizer run at double rate.
//
// Two vertex paths, matching the two the camera pass has for geometry that moves:
//
//     VSInstanced   rigid geometry, model matrix per instance   (the batched path)
//     VSSkinned     characters, bone palette per draw
//
// There is deliberately no per-object rigid path. The shadow pass always batches, even when
// the camera pass has been switched to per-object draws for an A/B measurement: which path
// the *camera* uses is a developer switch, and a shadow map is the same depth either way.
//
// The alpha-tested variant is a real pixel shader that discards, and the opaque variant has
// no pixel shader at all — the pipeline binds a null PS. Two pipeline states rather than one
// with a branch, because "no pixel shader" is a hardware fast path and a cutout leaf is a
// small fraction of any scene.

#include "material.hlsli"

// The view being rendered. Deliberately *not* the frame constants: a shadow pass runs once
// per cascade and once per local light, and re-uploading two kilobytes of lighting data for
// each of them to change one matrix would be most of the pass's CPU cost.
//
// Declared at b0, which is the same root parameter the camera pass binds its frame constants
// to — the root signature says "a constant buffer at b0" and each shader says what it holds.
cbuffer ShadowView : register(b0)
{
    column_major float4x4 light_view_projection;
    // x: 1 when this view is a cascade, 0 for a local light. Unused by the shader today;
    // present so the depth pass has somewhere to grow without changing the root signature.
    float4 shadow_view_params;
};

cbuffer Object : register(b1)
{
    column_major float4x4 model;
    float4 instance_tint;
    float instance_emissive;
    float instance_receives_shadow;
    float2 object_padding;
};

#define MAX_BONES 128

cbuffer Skin : register(b3)
{
    column_major float4x4 bones[MAX_BONES];
};

struct RigidVertex
{
    float3 position : POSITION;
    float3 normal   : NORMAL;
    float2 uv       : TEXCOORD0;

    float4 model0 : INSTANCE_MODEL0;
    float4 model1 : INSTANCE_MODEL1;
    float4 model2 : INSTANCE_MODEL2;
    float4 model3 : INSTANCE_MODEL3;
    float4 tint   : INSTANCE_TINT;
    float2 emissive_scale : INSTANCE_EMISSIVE;
};

struct SkinnedVertex
{
    float3 position : POSITION;
    float3 normal   : NORMAL;
    float2 uv       : TEXCOORD0;
    uint4  joints   : BLENDINDICES0;
    float4 weights  : BLENDWEIGHT0;
};

struct DepthPixel
{
    float4 position : SV_POSITION;
    // Carried for the alpha-tested pixel shader. The opaque pipeline binds no pixel shader
    // at all and the interpolator is dropped by the driver, so this costs nothing there.
    float2 uv : TEXCOORD0;
};

DepthPixel VSInstanced(RigidVertex input)
{
    // Vector-first, for the reason mesh_instanced.hlsl explains at length: the four
    // per-instance rows carry the columns of a column-major matrix, so what the shader
    // assembles is the transpose and mul(v, M) is the transform mul(M, v) would perform.
    float4x4 model_matrix = float4x4(input.model0, input.model1, input.model2, input.model3);
    float4 world = mul(float4(input.position, 1.0), model_matrix);

    DepthPixel output;
    output.position = mul(light_view_projection, world);
    output.uv = input.uv;
    return output;
}

DepthPixel VSSkinned(SkinnedVertex input)
{
    float4x4 skin =
          bones[input.joints.x] * input.weights.x
        + bones[input.joints.y] * input.weights.y
        + bones[input.joints.z] * input.weights.z
        + bones[input.joints.w] * input.weights.w;

    float4 world = mul(model, mul(skin, float4(input.position, 1.0)));

    DepthPixel output;
    output.position = mul(light_view_projection, world);
    output.uv = input.uv;
    return output;
}

// The alpha test, and nothing else.
//
// No return value: the pipeline has no render target bound, so there is nothing to write.
// What this shader does is decide whether the fragment reaches the depth buffer at all,
// which is what makes a leaf cast a leaf-shaped shadow instead of a rectangle.
void PSAlpha(DepthPixel input)
{
    float alpha = base_color.a * base_color_map.Sample(base_color_sampler, input.uv).a;
    if (alpha < alpha_cutoff)
    {
        discard;
    }
}
