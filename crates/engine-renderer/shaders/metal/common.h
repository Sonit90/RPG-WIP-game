// Shared by every Metal shader that draws meshes: the permutation function constants, the
// vertex-pulling structs, and the constant-buffer structs more than one entry point reads.
//
// Port of the shared parts of mesh.hlsl / mesh_instanced.hlsl / mesh_skinned.hlsl /
// shadow_depth.hlsl. See BINDINGS.md for the buffer / texture index of every entry point.
//
// Each .metal file is compiled into its OWN MTLLibrary and includes this header textually, so
// everything here is `inline` / `static` / program-scope constant.

#ifndef METAL_COMMON_H
#define METAL_COMMON_H

#include <metal_stdlib>
using namespace metal;

// ---- Pixel-shader permutations ---------------------------------------------------------
//
// The D3D12 build compiles PSMain once per combination of -DMATERIAL_PBR, -DMATERIAL_NORMAL_MAP
// and -DENABLE_SHADOWS (MESH_FEATURES in build.rs). Metal specializes the same fragment
// function with these function constants instead. The Rust side MUST supply all three (as
// bool) when it creates every mesh_fs / mesh_instanced_fs / mesh_skinned_fs function.
//
//   MATERIAL_PBR         the metallic-roughness BRDF (pbr.h). Off: one Lambert lobe.
//   MATERIAL_NORMAL_MAP  the normal fetch and the tangent frame. Off: the geometric normal.
//   ENABLE_SHADOWS       every shadow lookup. Off: `shadow = 1.0`, no cascades/PCF/contact.
constant bool MATERIAL_PBR [[function_constant(0)]];
constant bool MATERIAL_NORMAL_MAP [[function_constant(1)]];
constant bool ENABLE_SHADOWS [[function_constant(2)]];

// ---- Vertex pulling --------------------------------------------------------------------
//
// No [[stage_in]] and no vertex descriptor. The per-vertex stream (D3D12 input slot 0) is
// `const device VertexT* vertices [[buffer(16)]]` indexed by [[vertex_id]]; the per-instance
// stream (D3D12 input slot 1) is `const device InstanceT* instances [[buffer(17)]]` indexed by
// [[instance_id]]. All members use packed_* types so the MSL offsets equal the D3D12
// AlignedByteOffsets and the struct size equals the Rust size_of (no 16-byte vector alignment).

// MeshVertex (src/mesh.rs), 52 bytes. D3D12: mesh_vertex_elements() in src/mesh_pipeline.rs.
struct MeshVertex
{
    packed_float3 position;   // offset  0  R32G32B32_FLOAT     POSITION
    packed_float3 normal;     // offset 12  R32G32B32_FLOAT     NORMAL
    packed_float2 uv;         // offset 24  R32G32_FLOAT        TEXCOORD0
    packed_float4 tangent;    // offset 32  R32G32B32A32_FLOAT  TANGENT (xyz dir, w handedness)
    // offset 48  R8G8B8A8_UNORM  COLOR. Still unread by every shader; unpack with
    // unpack_unorm4x8_to_float(color) if it is ever used (byte 0 = R = low byte).
    uint color;
};
static_assert(sizeof(MeshVertex) == 52, "MeshVertex must match the Rust MeshVertex (52 bytes)");

// SkinnedVertex (src/mesh.rs), 76 bytes: the MeshVertex prefix, then joints and weights.
struct SkinnedVertex
{
    packed_float3 position;   // offset  0  R32G32B32_FLOAT
    packed_float3 normal;     // offset 12  R32G32B32_FLOAT
    packed_float2 uv;         // offset 24  R32G32_FLOAT
    packed_float4 tangent;    // offset 32  R32G32B32A32_FLOAT
    uint color;               // offset 48  R8G8B8A8_UNORM (unread)
    // offset 52  R16G16B16A16_UINT  BLENDINDICES0: four u16 joint indices. Declared as two
    // uints (not ushort4, whose 8-byte alignment would not fit at offset 52). Joint 0 is
    // the low half of joints01, joint 1 the high half; joint 2 the low half of joints23.
    uint joints01;            // offset 52
    uint joints23;            // offset 56
    packed_float4 weights;    // offset 60  R32G32B32A32_FLOAT  BLENDWEIGHT0
};
static_assert(sizeof(SkinnedVertex) == 76, "SkinnedVertex must match the Rust SkinnedVertex (76 bytes)");

// InstanceTransform (src/mesh_pipeline.rs), 96 bytes, D3D12 input slot 1, per-instance.
// model0..model3 are the four COLUMNS of the local-to-world matrix (Mat4 is column-major and
// is memcpy'd untransposed), see mesh_instanced.metal for the construction.
struct InstanceTransform
{
    packed_float4 model0;           // offset  0  INSTANCE_MODEL0
    packed_float4 model1;           // offset 16  INSTANCE_MODEL1
    packed_float4 model2;           // offset 32  INSTANCE_MODEL2
    packed_float4 model3;           // offset 48  INSTANCE_MODEL3
    packed_float4 tint;             // offset 64  INSTANCE_TINT
    // offset 80  INSTANCE_EMISSIVE. x: the emissive add. y: 1 when this instance is darkened by
    // shadows. z: how much of it the dither removes (1 - opacity). w: 1 when the camera fade
    // may fade it.
    packed_float4 emissive_scale;
};
static_assert(sizeof(InstanceTransform) == 96, "InstanceTransform must match the Rust InstanceTransform (96 bytes)");

// ---- Constant buffers ------------------------------------------------------------------

// HLSL constant buffer Object at b1. ObjectConstants (src/scene.rs), 96 bytes.
//
// Per-object visual state, the constant-buffer mirror of the instanced pipeline's per-instance
// stream. mesh.hlsl and mesh_skinned.hlsl declare it identically; shadow_depth.hlsl too.
struct ObjectConstants
{
    float4x4 model;                 // offset  0  local-to-world, column-major
    float4 instance_tint;           // offset 64
    float instance_emissive;        // offset 80
    // 1 when the lighting pass may darken this object where a shadow falls on it.
    float instance_receives_shadow; // offset 84
    // How much of the object the dither removes (1 - opacity), and 1 when the frame's camera
    // fade may fade it. Both zero for a whole, solid object.
    float instance_fade;            // offset 88
    float instance_see_through;     // offset 92
};
static_assert(sizeof(ObjectConstants) == 96, "ObjectConstants must match the Rust ObjectConstants (96 bytes)");

// The bone palette. HLSL constant buffer Skin at b3. 128 column-major matrices = 8192 bytes.
#define MAX_BONES 128
struct SkinPalette
{
    float4x4 bones[MAX_BONES];
};
static_assert(sizeof(SkinPalette) == 8192, "SkinPalette must be 128 * 64 bytes");

// The upper-left 3x3 of a 4x4: the HLSL `(float3x3)M` cast. MSL float4x4 is column-major with
// the same logical layout as the HLSL column_major matrix, so the first three columns' xyz.
inline float3x3 upper_3x3(float4x4 m)
{
    return float3x3(m[0].xyz, m[1].xyz, m[2].xyz);
}

// The vertex-shader output of mesh.hlsl and mesh_skinned.hlsl (HLSL PixelInput). The same
// struct is the fragment shader's [[stage_in]]; [[position]] reads back as the fragment's
// pixel position, i.e. the HLSL system-value position.
struct MeshV2F
{
    float4 position [[position]];
    float3 world_position;
    float3 world_normal;
    // The tangent in world space, handedness carried through untouched in w: it is a property
    // of the UV layout, not of where the object stands.
    float4 world_tangent;
    float2 uv;
};

#endif // METAL_COMMON_H
