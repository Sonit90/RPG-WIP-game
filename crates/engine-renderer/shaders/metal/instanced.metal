// Instanced vertex + fragment shader for the Milestone 8 benchmark. Port of instanced.hlsl.
//
// The mesh is one unit triangle; every instance supplies its own offset, scale and
// colour through a second vertex stream. Both benchmark modes use this same shader, so
// the only difference between them is how many draw calls submit the instances.
//
// Entry points: instanced_vs (HLSL VSMain), instanced_fs (HLSL PSMain).

#include <metal_stdlib>
using namespace metal;

// pipeline.rs `Vertex`, 28 bytes. D3D12 input slot 0, per-vertex.
struct InstancedVertex
{
    packed_float3 position;   // offset  0  R32G32B32_FLOAT     POSITION
    packed_float4 color;      // offset 12  R32G32B32A32_FLOAT  COLOR
};
static_assert(sizeof(InstancedVertex) == 28, "InstancedVertex must match the Rust pipeline::Vertex (28 bytes)");

// instancing.rs `InstanceData`, 28 bytes. D3D12 input slot 1, per-instance (step rate 1).
struct InstanceData
{
    // xy is the clip-space offset, z the scale.
    packed_float3 transform;       // offset  0  R32G32B32_FLOAT     INSTANCE_TRANSFORM
    packed_float4 instance_color;  // offset 12  R32G32B32A32_FLOAT  INSTANCE_COLOR
};
static_assert(sizeof(InstanceData) == 28, "InstanceData must match the Rust instancing::InstanceData (28 bytes)");

struct InstancedV2F
{
    float4 position [[position]];
    float4 color;
};

vertex InstancedV2F instanced_vs(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device InstancedVertex* vertices [[buffer(16)]],
    const device InstanceData* instances [[buffer(17)]])
{
    InstancedV2F output;
    float3 position = float3(vertices[vertex_id].position);
    float3 transform = float3(instances[instance_id].transform);
    float2 placed = position.xy * transform.z + transform.xy;
    output.position = float4(placed, 0.0f, 1.0f);
    output.color = float4(vertices[vertex_id].color) * float4(instances[instance_id].instance_color);
    return output;
}

fragment float4 instanced_fs(InstancedV2F input [[stage_in]])
{
    return input.color;
}
