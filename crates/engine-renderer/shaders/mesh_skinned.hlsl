// Skinned mesh shading: the same lighting as mesh.hlsl, with the vertex moved by a bone
// palette before anything else happens.
//
// mesh.hlsl transforms a vertex by one model matrix. Here the vertex is first blended
// through up to four bone matrices — which already contain `global * inverse_bind`, so they
// map a bind-pose vertex straight into the character's local space — and only then by the
// model matrix that places the character in the world.
//
//     skinned = sum(weight[i] * bones[joint[i]] * position)
//     world   = model * skinned
//
// The frame, object and material constant buffers are unchanged and still live at b0, b1 and
// b2. The palette is b3, which is a root parameter the other pipelines simply do not use.
//
// One draw per skinned submesh, not instanced: every instance would need its own palette, and
// a per-instance array is a structured buffer and a second indirection. There are a handful
// of characters and hundreds of dummies, and the dummies are the ones that need instancing.

#include "scene_frame.hlsli"

cbuffer Object : register(b1)
{
    column_major float4x4 model;
    float4 instance_tint;
    float instance_emissive;
    float instance_receives_shadow;
    float2 object_padding;
};

// The bone palette. A fixed-size array, because a constant buffer has to have a size the
// shader knows — which is why the CPU side refuses a palette longer than this rather than
// letting a draw read past the end of the allocation into another object's constants.
//
// 128 matrices is 8 KB, written into the frame's upload ring once per character and bound
// once per submesh.
#define MAX_BONES 128

cbuffer Skin : register(b3)
{
    column_major float4x4 bones[MAX_BONES];
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
    uint4  joints   : BLENDINDICES0;
    float4 weights  : BLENDWEIGHT0;
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
    // The weighted sum of the four bone matrices. Summing the *matrices* and transforming
    // once is the same result as transforming four times and summing, for one quarter of the
    // work — and it is the form that lets the normal reuse the blend for free.
    //
    // Unrolled rather than looped: four is a constant, and the loop's induction variable
    // would index a constant-buffer array dynamically, which is markedly slower on some
    // hardware than four indexed reads the compiler can schedule.
    float4x4 skin =
          bones[input.joints.x] * input.weights.x
        + bones[input.joints.y] * input.weights.y
        + bones[input.joints.z] * input.weights.z
        + bones[input.joints.w] * input.weights.w;

    float4 posed = mul(skin, float4(input.position, 1.0));
    float4 world = mul(model, posed);

    PixelInput output;
    output.position = mul(view_projection, world);
    output.world_position = world.xyz;
    // The same approximation mesh.hlsl makes — correct for uniform scale, which is what a
    // character has. A bone with non-uniform scale would need the inverse transpose of the
    // blend, which is a 3x3 inverse per vertex.
    output.world_normal = mul((float3x3)model, mul((float3x3)skin, input.normal));
    // The tangent is skinned by the same blended matrix as the normal. Skinning the normal
    // and not the tangent is the bug that makes a character's normal mapping look correct in
    // the bind pose and rotate against the light the moment an arm moves.
    output.world_tangent = float4(
        mul((float3x3)model, mul((float3x3)skin, input.tangent.xyz)), input.tangent.w);
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
