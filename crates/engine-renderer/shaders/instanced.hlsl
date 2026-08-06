// Instanced vertex + pixel shader for the Milestone 8 benchmark.
//
// The mesh is one unit triangle; every instance supplies its own offset, scale and
// colour through a second vertex stream. Both benchmark modes use this same shader, so
// the only difference between them is how many draw calls submit the instances.

struct VSInput
{
    // Per-vertex, input slot 0.
    float3 position : POSITION;
    float4 color    : COLOR;

    // Per-instance, input slot 1. xy is the clip-space offset, z the scale.
    float3 transform     : INSTANCE_TRANSFORM;
    float4 instanceColor : INSTANCE_COLOR;
};

struct PSInput
{
    float4 position : SV_POSITION;
    float4 color    : COLOR;
};

PSInput VSMain(VSInput input)
{
    PSInput output;
    float2 placed = input.position.xy * input.transform.z + input.transform.xy;
    output.position = float4(placed, 0.0f, 1.0f);
    output.color = input.color * input.instanceColor;
    return output;
}

float4 PSMain(PSInput input) : SV_TARGET
{
    return input.color;
}
