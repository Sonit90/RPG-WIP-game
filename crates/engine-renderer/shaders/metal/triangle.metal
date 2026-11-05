// Minimal vertex + fragment shader pair for Phase 0 / Milestone 4. Port of triangle.hlsl.
//
// No constant buffers, no transforms, no lighting: vertices arrive in clip space and the
// colour is interpolated straight through.
//
// Entry points: triangle_vs (HLSL VSMain), triangle_fs (HLSL PSMain).

#include <metal_stdlib>
using namespace metal;

// pipeline.rs `Vertex`, 28 bytes. D3D12 input slot 0, per-vertex.
struct TriangleVertex
{
    packed_float3 position;   // offset  0  R32G32B32_FLOAT     POSITION
    packed_float4 color;      // offset 12  R32G32B32A32_FLOAT  COLOR
};
static_assert(sizeof(TriangleVertex) == 28, "TriangleVertex must match the Rust pipeline::Vertex (28 bytes)");

struct TriangleV2F
{
    float4 position [[position]];
    float4 color;
};

vertex TriangleV2F triangle_vs(
    uint vertex_id [[vertex_id]],
    const device TriangleVertex* vertices [[buffer(16)]])
{
    TriangleV2F output;
    output.position = float4(float3(vertices[vertex_id].position), 1.0f);
    output.color = float4(vertices[vertex_id].color);
    return output;
}

fragment float4 triangle_fs(TriangleV2F input [[stage_in]])
{
    return input.color;
}
