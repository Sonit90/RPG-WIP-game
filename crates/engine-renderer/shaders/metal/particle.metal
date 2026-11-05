// Camera-facing particle quads, expanded on the GPU from one record per particle.
// Port of particle.hlsl.
//
// The CPU uploads 36 bytes per particle - centre, size, colour, rotation - and the vertex
// shader builds the quad's four corners from the camera's right and up axes.
//
// The draw is `DrawInstanced(4, particle_count)` over a triangle strip (Metal: drawPrimitives
// triangleStrip, vertexCount 4, instanceCount particle_count), so a whole blend mode is one
// draw call whatever the particle count is.
//
// There is no texture. The round, soft falloff every particle wants is two instructions of
// arithmetic on the corner coordinate.
//
// Entry points: particle_vs (HLSL VSMain), particle_fs (HLSL PSMain).
//
// Vertex pulling: in D3D12 the particle record is input slot 0 *per instance* (step rate 1) and
// there is no per-vertex stream, so the corner comes from [[vertex_id]] (0..3) and the record
// from [[instance_id]]. By the convention "buffer(16 + D3D12 input slot)" the record stream
// is buffer(16) here - NOT buffer(17), which is reserved for slot-1 per-instance data in the
// other shaders.

#include <metal_stdlib>
using namespace metal;

// HLSL constant buffer ParticleFrame at b0. ParticleConstants (src/particle_pipeline.rs), 96 bytes.
struct ParticleFrame
{
    float4x4 view_projection;       // offset  0  column-major
    // The camera's right and up axes in world space, already normalized.
    packed_float3 camera_right;      // offset 64
    float particle_padding;          // offset 76
    packed_float3 camera_up;         // offset 80
    float particle_padding_2;        // offset 92
};
static_assert(sizeof(ParticleFrame) == 96, "ParticleFrame must match the Rust ParticleConstants (96 bytes)");

// engine_render_data::ParticleInstance, 36 bytes. D3D12 input slot 0, per-instance.
struct ParticleInstance
{
    packed_float3 center;     // offset  0  R32G32B32_FLOAT     POSITION
    float size;               // offset 12  R32_FLOAT           TEXCOORD0
    packed_float4 color;      // offset 16  R32G32B32A32_FLOAT  COLOR
    float rotation;           // offset 32  R32_FLOAT           TEXCOORD1
};
static_assert(sizeof(ParticleInstance) == 36, "ParticleInstance must match the Rust ParticleInstance (36 bytes)");

struct ParticleV2F
{
    float4 position [[position]];
    // The corner in [-1, 1], which is what the falloff is computed from.
    float2 corner;
    float4 color;
};

vertex ParticleV2F particle_vs(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device ParticleInstance* particles [[buffer(16)]],
    constant ParticleFrame& frame [[buffer(0)]])
{
    ParticleInstance input = particles[instance_id];

    // Strip order: 0 = (-1, +1), 1 = (+1, +1), 2 = (-1, -1), 3 = (+1, -1).
    float2 corner = float2(
        (vertex_id & 1u) != 0u ? 1.0 : -1.0,
        (vertex_id & 2u) != 0u ? -1.0 : 1.0);

    float sine = sin(input.rotation);
    float cosine = cos(input.rotation);
    float2 turned = float2(
        corner.x * cosine - corner.y * sine,
        corner.x * sine + corner.y * cosine);

    float3 world = float3(input.center)
        + float3(frame.camera_right) * (turned.x * input.size)
        + float3(frame.camera_up) * (turned.y * input.size);

    ParticleV2F output;
    output.position = frame.view_projection * float4(world, 1.0);
    output.corner = corner;
    output.color = float4(input.color);
    return output;
}

fragment float4 particle_fs(ParticleV2F input [[stage_in]])
{
    // A soft round falloff, squared so the edge is gentle and the core is bright. Squared
    // rather than linear because a linear ramp reads as a flat disc with a hard rim, which
    // is exactly what a particle must not look like.
    float radial = saturate(1.0 - dot(input.corner, input.corner));
    float coverage = radial * radial * input.color.a;

    // Premultiplied alpha. One fragment shader serves both blend modes: additive is
    // `ONE, ONE` and alpha is `ONE, ONE_MINUS_SRC_ALPHA`, and neither needs the colour
    // multiplied by alpha again in the blender.
    return float4(input.color.rgb * coverage, coverage);
}
