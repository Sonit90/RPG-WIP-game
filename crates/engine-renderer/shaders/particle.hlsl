// Camera-facing particle quads, expanded on the GPU from one record per particle.
//
// The CPU uploads 36 bytes per particle — centre, size, colour, rotation — and the vertex
// shader builds the quad's four corners from the camera's right and up axes. Expanding on
// the CPU instead would write six vertices where the GPU needs one record, through the same
// upload ring, for geometry the vertex shader can produce from `SV_VertexID` for free.
//
// The draw is `DrawInstanced(4, particle_count)` over a triangle strip, so a whole blend
// mode is one draw call whatever the particle count is.
//
// There is no texture. The round, soft falloff every particle in this engine wants is two
// instructions of arithmetic on the corner coordinate, and a texture would cost a
// descriptor table, an atlas, a sampler and a load per pixel to reproduce it. When a
// particle needs a *shape* — a wisp, a crack, a rune — this is where an atlas arrives.

cbuffer ParticleFrame : register(b0)
{
    column_major float4x4 view_projection;
    // The camera's right and up axes in world space, already normalized. Handed in rather
    // than derived from the matrix: the CPU has the view basis to hand, and pulling it back
    // out of a composed view-projection in the shader is arithmetic per vertex to recover
    // something that was known per frame.
    float3 camera_right;
    float particle_padding;
    float3 camera_up;
    float particle_padding_2;
};

struct VertexInput
{
    // Per instance, input slot 0.
    float3 center   : POSITION;
    float size      : TEXCOORD0;
    float4 color    : COLOR;
    float rotation  : TEXCOORD1;
    uint vertex_id  : SV_VertexID;
};

struct PixelInput
{
    float4 position : SV_POSITION;
    // The corner in [-1, 1], which is what the falloff is computed from.
    float2 corner   : TEXCOORD0;
    float4 color    : COLOR;
};

PixelInput VSMain(VertexInput input)
{
    // Strip order: 0 = (-1, +1), 1 = (+1, +1), 2 = (-1, -1), 3 = (+1, -1).
    float2 corner = float2(
        (input.vertex_id & 1) ? 1.0 : -1.0,
        (input.vertex_id & 2) ? -1.0 : 1.0);

    float sine;
    float cosine;
    sincos(input.rotation, sine, cosine);
    float2 turned = float2(
        corner.x * cosine - corner.y * sine,
        corner.x * sine + corner.y * cosine);

    float3 world = input.center
        + camera_right * (turned.x * input.size)
        + camera_up * (turned.y * input.size);

    PixelInput output;
    output.position = mul(view_projection, float4(world, 1.0));
    output.corner = corner;
    output.color = input.color;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    // A soft round falloff, squared so the edge is gentle and the core is bright. Squared
    // rather than linear because a linear ramp reads as a flat disc with a hard rim, which
    // is exactly what a particle must not look like.
    float radial = saturate(1.0 - dot(input.corner, input.corner));
    float coverage = radial * radial * input.color.a;

    // Premultiplied alpha. One pixel shader serves both blend modes: additive is
    // `ONE, ONE` and alpha is `ONE, INV_SRC_ALPHA`, and neither needs the colour multiplied
    // by alpha again in the blender.
    return float4(input.color.rgb * coverage, coverage);
}
