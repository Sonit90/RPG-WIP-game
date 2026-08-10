// Batched UI: every rectangle, bar, outline, glyph and icon in one vertex buffer and one draw.
//
// The whole UI is a stream of textured, tinted quads. A glyph samples its cell in the UI
// atlas; an icon samples its cell in the same atlas; a solid rectangle samples one opaque
// texel in it. That is the entire reason a health bar, the text on top of it and the ability
// icon beside it are one draw call rather than three.
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
    // Where the atlas stops being the font and starts being icons. The CPU owns the atlas
    // layout, so it hands the boundary over rather than this file restating it.
    float icon_band_v;
    float ui_padding;
};

Texture2D<float4> atlas : register(t0);

// Point, for the font: the glyphs are a 1-bit bitmap magnified to integer scales, and
// filtering one produces grey fringes around letters drawn to have none.
SamplerState glyph_sampler : register(s0);

// Linear, for the icons: an icon is rasterized at 88 texels and drawn at 44, and point
// sampling that minification aliases badly.
SamplerState icon_sampler : register(s1);

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
    // Which sampler to read through is a property of *where* in the atlas this fragment is
    // reading, so the coordinate decides it and no per-vertex flag is needed. The font band
    // is the top of the texture and the icon band is everything below it.
    //
    // SampleLevel rather than Sample: choosing a sampler is non-uniform control flow, in
    // which the implicit derivatives Sample wants are undefined. The atlas has one mip, so
    // asking for level 0 outright is both correct and what Sample would have found anyway.
    float4 texel = input.uv.y >= icon_band_v
        ? atlas.SampleLevel(icon_sampler, input.uv, 0.0)
        : atlas.SampleLevel(glyph_sampler, input.uv, 0.0);

    // Straight multiply, for both bands at once. The font band stores white with the glyph's
    // coverage in alpha, so a glyph comes out as the vertex colour cut to the letter shape
    // and a solid rectangle samples the opaque texel and comes out untouched — exactly what
    // the old single-channel atlas did. The icon band stores real colour, so a white tint
    // draws the art as authored and a darker one dims it.
    return input.color * texel;
}
