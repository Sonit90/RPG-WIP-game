// Batched UI: every rectangle, bar, outline, glyph, icon and the map in one vertex buffer and
// one draw.
//
// The whole UI is a stream of textured, tinted quads. A glyph samples its cell in the UI
// atlas; an icon samples its cell in the same atlas; a solid rectangle samples one opaque
// texel in it. That is the entire reason a health bar, the text on top of it and the ability
// icon beside it are one draw call rather than three. The dungeon map's picture is the one
// thing that is not in the atlas, and a per-vertex flag picks it instead — still the same
// draw.
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
    // The font atlas's size in texels, and how far its distance field reaches, in texels.
    float2 font_atlas_size;
    float font_range;
    float ui_padding2;
};

Texture2D<float4> atlas : register(t0);

// The level's baked map. Bound to the atlas when no level map exists, so a stray map vertex
// samples something valid rather than whatever the heap holds.
Texture2D<float4> map_picture : register(t1);

// The interface fonts, as a distance field: white, with 0.5 alpha on the outline.
Texture2D<float4> font_field : register(t2);

// Point, for the font: the glyphs are a 1-bit bitmap magnified to integer scales, and
// filtering one produces grey fringes around letters drawn to have none.
SamplerState glyph_sampler : register(s0);

// Linear, for the icons: an icon is rasterized at 88 texels and drawn at 44, and point
// sampling that minification aliases badly.
SamplerState icon_sampler : register(s1);

// Linear and mipmapped, transparent past the edge: the map is minified on the minimap and
// magnified on the full-screen map, and off the level there is nothing to draw.
SamplerState map_sampler : register(s2);

struct VertexInput
{
    float2 position : POSITION;   // pixels, top-left origin
    float2 uv       : TEXCOORD0;
    float4 color    : COLOR;
    float4 clip     : TEXCOORD1;  // circle centre (px), radius (px), picture (0 atlas, 1 map)
};

struct PixelInput
{
    float4 position : SV_POSITION;
    float2 uv       : TEXCOORD0;
    float4 color    : COLOR;
    // Constant across a quad by construction, so not interpolated at all.
    nointerpolation float4 clip : TEXCOORD1;
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
    output.clip = input.clip;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    // Derivatives first, in uniform control flow: the map is sampled through them below,
    // inside a branch where implicit derivatives would be undefined.
    float2 uv_dx = ddx(input.uv);
    float2 uv_dy = ddy(input.uv);

    float4 texel;
    if (input.clip.w < -0.5)
    {
        // A glyph with its edge moved outward by (-1 - picture) pixels: a thickened letter, or
        // the dark edge drawn under one. The same distance as the plain glyph below, shifted
        // before it becomes coverage. A wide edge ramps over up to two pixels, so an outline reads
        // as a soft dark rim rather than a hard second letter.
        float dilation = -input.clip.w - 1.0;
        float field = font_field.SampleLevel(icon_sampler, input.uv, 0.0).a;
        float texels_per_pixel = max(
            0.5 * (length(uv_dx * font_atlas_size) + length(uv_dy * font_atlas_size)),
            1e-4);
        float distance_pixels = (field - 0.5) * 2.0 * font_range / texels_per_pixel;
        // Must match dilation_ramp in ui_pipeline.rs, which bounds how far a glyph may grow.
        float ramp = clamp(dilation * 0.35, 1.0, 2.0);
        texel = float4(1.0, 1.0, 1.0, saturate((distance_pixels + dilation) / ramp + 0.5));
    }
    else if (input.clip.w > 2.5)
    {
        // A rounded panel: the signed distance to a rounded rectangle, in pixels. The texture
        // coordinate is this pixel's offset from the panel's centre; the clip holds its
        // half-size and radius, and the picture code carries a feather or a stroke width.
        float2 half_size = input.clip.xy;
        float radius = input.clip.z;
        float2 q = abs(input.uv) - (half_size - radius);
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
        float coverage;
        if (input.clip.w > 999.5)
        {
            float stroke = input.clip.w - 1000.0;
            coverage = saturate(0.5 - d) * saturate(0.5 + d + stroke);
        }
        else
        {
            float feather = input.clip.w - 3.0;
            coverage = feather > 0.0 ? saturate(0.5 - d / feather) : saturate(0.5 - d);
        }
        // The panel ignores the circle clip below: its clip slot is its own shape.
        return float4(input.color.rgb, input.color.a * coverage);
    }
    else if (input.clip.w > 1.5)
    {
        // A glyph: the distance from the outline, in atlas texels, turned into screen pixels
        // by how many texels one pixel covers, then into coverage with a one-pixel ramp. One
        // atlas, crisp at every size and every interface scale.
        float field = font_field.SampleLevel(icon_sampler, input.uv, 0.0).a;
        float texels_per_pixel = max(
            0.5 * (length(uv_dx * font_atlas_size) + length(uv_dy * font_atlas_size)),
            1e-4);
        float distance_pixels = (field - 0.5) * 2.0 * font_range / texels_per_pixel;
        texel = float4(1.0, 1.0, 1.0, saturate(distance_pixels + 0.5));
    }
    else if (input.clip.w > 0.5)
    {
        texel = map_picture.SampleGrad(map_sampler, input.uv, uv_dx, uv_dy);
    }
    else
    {
        // Which sampler to read through is a property of *where* in the atlas this fragment
        // is reading, so the coordinate decides it and no per-vertex flag is needed. The font
        // band is the top of the texture and the icon band is everything below it.
        //
        // SampleLevel rather than Sample: choosing a sampler is non-uniform control flow, in
        // which the implicit derivatives Sample wants are undefined. The atlas has one mip, so
        // asking for level 0 outright is both correct and what Sample would have found anyway.
        texel = input.uv.y >= icon_band_v
            ? atlas.SampleLevel(icon_sampler, input.uv, 0.0)
            : atlas.SampleLevel(glyph_sampler, input.uv, 0.0);
    }

    // Straight multiply, for both bands at once. The font band stores white with the glyph's
    // coverage in alpha, so a glyph comes out as the vertex colour cut to the letter shape
    // and a solid rectangle samples the opaque texel and comes out untouched — exactly what
    // the old single-channel atlas did. The icon band stores real colour, so a white tint
    // draws the art as authored and a darker one dims it.
    float4 color = input.color * texel;

    // The circle clip: a disc's own edge, or the minimap's. One pixel of coverage ramp, so
    // the rim is anti-aliased rather than stair-stepped. SV_POSITION is the pixel centre in
    // the same top-left pixel space the vertices were written in.
    if (input.clip.z > 0.0)
    {
        float distance_to_centre = distance(input.position.xy, input.clip.xy);
        color.a *= saturate(input.clip.z - distance_to_centre + 0.5);
    }
    return color;
}
