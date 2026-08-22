// Instanced mesh shading: the same lighting as mesh.hlsl, with the model matrix supplied
// per instance instead of per draw.
//
// mesh.hlsl reads `model` from a constant buffer the CPU rebinds before every draw. That
// costs one root-CBV bind and one draw call per object. Here the matrix arrives as four
// float4 rows in a second vertex stream, stepped once per instance, so a run of objects
// sharing a mesh and a material becomes one DrawIndexedInstanced.
//
// The frame and material constant buffers are unchanged and still live at b0 and b2 — the
// root signature is the same one, minus the per-object slot the instance stream replaces.
// The lighting is not merely "the same as mesh.hlsl": it is literally the same function, out
// of scene_frame.hlsli, which is what makes tests/instanced_equivalence.rs a check on the
// vertex path rather than on two copies of a pixel shader.

#include "scene_frame.hlsli"

struct VertexInput
{
    // Per-vertex, input slot 0 — the same MeshVertex layout mesh.hlsl consumes.
    float3 position : POSITION;
    float3 normal   : NORMAL;
    float2 uv       : TEXCOORD0;

    // Per-instance, input slot 1. Four rows of the local-to-world matrix.
    //
    // Rows rather than columns: a vertex element is at most a float4, so a matrix has to
    // arrive as four of them, and rows are what the CPU-side Mat4's columns transpose into
    // for free. `mul(v, M_rows)` is then the same transform `mul(M, v)` performs in
    // mesh.hlsl — the transpose is in the layout, not in an instruction.
    float4 model0 : INSTANCE_MODEL0;
    float4 model1 : INSTANCE_MODEL1;
    float4 model2 : INSTANCE_MODEL2;
    float4 model3 : INSTANCE_MODEL3;

    // Per-instance visual state. A colour multiply and an emissive add, which is what lets
    // one dummy flash white or fade to a corpse colour without leaving the batch its
    // material put it in. Neutral is tint (1,1,1,1) and emissive 0, and it costs the same
    // as not having the feature: the multiply and the add still happen.
    float4 tint     : INSTANCE_TINT;
    // x: the emissive add. y: 1 when this instance is darkened by shadows. The second
    // component reads a word that was padding in the instance record, so per-instance
    // shadow receipt costs no extra bytes in the stream.
    float2 emissive_scale : INSTANCE_EMISSIVE;
};

struct PixelInput
{
    float4 position       : SV_POSITION;
    float3 world_position : TEXCOORD1;
    float3 world_normal   : NORMAL;
    float2 uv             : TEXCOORD0;
    float4 tint           : COLOR0;
    float2 emissive_scale : COLOR1;
};

PixelInput VSMain(VertexInput input)
{
    PixelInput output;

    // HLSL's float4x4(a, b, c, d) constructor fills *rows*, and the four elements carry
    // the matrix's *columns* — engine_core::math::Mat4 is column-major and is memcpy'd in
    // untransposed. So `model` here is the transpose of the real local-to-world matrix.
    //
    // That is deliberate, and it is why the multiply is vector-first. mul(v, M) computes
    // v * M, which for M = Mtrue^T is exactly Mtrue * v — the transform mesh.hlsl performs
    // as mul(model, v). Writing mul(model, v) here instead transposes the transform, and
    // the symptom is a scene that renders with every object sheared and enormous rather
    // than anything that looks like a matrix bug.
    float4x4 model = float4x4(input.model0, input.model1, input.model2, input.model3);

    float4 world = mul(float4(input.position, 1.0), model);
    output.position = mul(view_projection, world);
    output.world_position = world.xyz;
    // Correct only for uniform scale, exactly as in mesh.hlsl. Vector-first for the same
    // reason, on the upper-left 3x3.
    output.world_normal = mul(input.normal, (float3x3)model);
    output.uv = input.uv;
    output.tint = input.tint;
    output.emissive_scale = input.emissive_scale;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    return shade_surface(
        input.position,
        input.world_position,
        input.world_normal,
        input.uv,
        input.tint,
        input.emissive_scale.x,
        input.emissive_scale.y);
}
