// Opaque mesh shading: transform by the camera, shade with the sun, the shadow atlas and
// whatever local lights reach the surface.
//
// Matrices are declared column_major, which is both the HLSL default and the layout
// engine_core::math::Mat4 already has in memory — columns in consecutive registers. So a
// matrix is memcpy'd into the constant buffer with no transpose, and mul(M, v) is the same
// M * v the CPU-side math means.
//
// The frame constants, the material constants and the whole lighting function live in
// scene_frame.hlsli, shared with the instanced and skinned pipelines. Only the two things
// that genuinely differ between them are here: how a vertex reaches world space, and where
// the per-object tint comes from.

#include "scene_frame.hlsli"

cbuffer Object : register(b1)
{
    column_major float4x4 model;
    // Per-object visual state, the constant-buffer mirror of the instanced pipeline's
    // per-instance stream. The two shaders must shade identically or an instanced frame and
    // a non-instanced frame are different images.
    float4 instance_tint;
    float instance_emissive;
    // 1 when the lighting pass may darken this object where a shadow falls on it. Carried in
    // what used to be padding, so the per-object constants did not grow a register for a
    // flag.
    float instance_receives_shadow;
    float2 object_padding;
};

struct VertexInput
{
    float3 position : POSITION;
    float3 normal   : NORMAL;
    float2 uv       : TEXCOORD0;
    // The tangent frame normal mapping needs: xyz the direction texture-space U runs, w the
    // handedness of the bitangent. Generated or imported by engine-assets; see pbr.hlsli.
    float4 tangent  : TANGENT;
    // In the layout and still unread. Vertex tinting is its own decision about how authored
    // colour combines with a material, and this milestone is about the material.
    float4 color    : COLOR;
};

struct PixelInput
{
    float4 position       : SV_POSITION;
    float3 world_position : TEXCOORD1;
    float3 world_normal   : NORMAL;
    // The tangent in world space, handedness carried through untouched in w: it is a property
    // of the UV layout, not of where the object stands.
    float4 world_tangent  : TANGENT;
    float2 uv             : TEXCOORD0;
};

PixelInput VSMain(VertexInput input)
{
    PixelInput output;
    float4 world = mul(model, float4(input.position, 1.0));
    output.position = mul(view_projection, world);
    // The world position is what every shadow lookup starts from: a cascade, a spot light
    // and the contact-shadow march all project it into a different space.
    output.world_position = world.xyz;
    // Correct only for uniform scale. Non-uniform scale needs the inverse transpose, which
    // costs an extra matrix per object; nothing scales non-uniformly yet.
    output.world_normal = mul((float3x3)model, input.normal);
    // The same transform the normal gets, for the same reason and with the same uniform-scale
    // caveat. `apply_normal_map` re-orthogonalizes the pair after interpolation.
    output.world_tangent =
        float4(mul((float3x3)model, input.tangent.xyz), input.tangent.w);
    output.uv = input.uv;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    return shade_surface(
        input.position,
        input.world_position,
        input.world_normal,
        input.world_tangent,
        input.uv,
        instance_tint,
        instance_emissive,
        instance_receives_shadow);
}
