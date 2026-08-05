// Minimal vertex + pixel shader pair for Phase 0 / Milestone 4.
//
// No constant buffers, no transforms, no lighting: vertices arrive in clip space and the
// colour is interpolated straight through. A real shader system arrives well after
// Phase 0, so this file stays deliberately small.

struct VSInput
{
    float3 position : POSITION;
    float4 color    : COLOR;
};

struct PSInput
{
    float4 position : SV_POSITION;
    float4 color    : COLOR;
};

PSInput VSMain(VSInput input)
{
    PSInput output;
    output.position = float4(input.position, 1.0f);
    output.color = input.color;
    return output;
}

float4 PSMain(PSInput input) : SV_TARGET
{
    return input.color;
}
