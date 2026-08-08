// Batched UI: every rectangle, bar, outline and glyph in one vertex buffer and one draw.
//
// The whole UI is a stream of textured, tinted quads. A glyph samples its cell in the font
// atlas; a solid rectangle samples one opaque texel in the same atlas. That is the entire
// reason a health bar and the text on top of it are the same draw call rather than two.
//
// Positions arrive in **pixels** with the origin at the top-left of the client area, which
// is how the layout code thinks, and are converted to clip space here. Doing it on the GPU
// keeps the CPU-side vertex build a straight copy of what the layout produced, and means a
// resize changes two floats in a constant rather than every vertex.

cbuffer UiFrame : register(b0)
{
    // 2 / width and -2 / height: the multiply that takes pixels to normalized device
    // coordinates. Precomputed on the CPU so the shader does no divide.
    float2 pixel_to_clip;
    float2 ui_padding;
};

Texture2D<float> atlas : register(t0);
SamplerState atlas_sampler : register(s0);

struct VertexInput
{
    float2 position : POSITION;   // pixels, top-left origin
    float2 uv       : TEXCOORD0;
    float4 color    : COLOR;
};

struct PixelInput
{
    float4 position : SV_POSITION;
    float2 uv       : TEXCOORD0;
    float4 color    : COLOR;
};

PixelInput VSMain(VertexInput input)
{
    PixelInput output;
    // x: 0..width  -> -1..1.  y: 0..height -> 1..-1, because screen y grows downward and
    // clip-space y grows upward.
    output.position = float4(
        input.position.x * pixel_to_clip.x - 1.0,
        1.0 + input.position.y * pixel_to_clip.y,
        0.0,
        1.0);
    output.uv = input.uv;
    output.color = input.color;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    // The atlas is single-channel coverage: 1 inside a glyph, 0 outside. It modulates alpha
    // only, so a glyph is the vertex colour cut to the letter shape rather than a greyscale
    // wash of it. A solid rectangle samples the opaque texel and comes out untouched.
    float coverage = atlas.Sample(atlas_sampler, input.uv);
    return float4(input.color.rgb, input.color.a * coverage);
}
