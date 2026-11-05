// Batched UI: every rectangle, bar, outline, glyph, icon and the map in one vertex buffer and
// one draw. Port of ui.hlsl.
//
// The whole UI is a stream of textured, tinted quads. A glyph samples its cell in the UI
// atlas; an icon samples its cell in the same atlas; a solid rectangle samples one opaque
// texel in it. The dungeon map's picture is the one thing that is not in the atlas, and a
// per-vertex flag picks it instead - still the same draw.
//
// Positions arrive in **pixels** with the origin at the top-left of the client area, and are
// converted to clip space here.
//
// Entry points: ui_vs (HLSL VSMain), ui_fs (HLSL PSMain).

#include <metal_stdlib>
using namespace metal;

// HLSL constant buffer UiFrame at b0. UiFrameConstants (src/ui_pipeline.rs), 32 bytes.
struct UiFrame
{
    // 2 / width and -2 / height: the multiply that takes pixels to normalized device
    // coordinates. Precomputed on the CPU so the shader does no divide.
    float2 pixel_to_clip;     // offset  0
    // Where the atlas stops being the font and starts being icons.
    float icon_band_v;        // offset  8
    float ui_padding;         // offset 12
    // The font atlas's size in texels, and how far its distance field reaches, in texels.
    float2 font_atlas_size;   // offset 16
    float font_range;         // offset 24
    float ui_padding2;        // offset 28
};
static_assert(sizeof(UiFrame) == 32, "UiFrame must match the Rust UiFrameConstants (32 bytes)");

// ui_pipeline.rs `UiVertex`, 48 bytes. D3D12 input slot 0, per-vertex.
struct UiVertex
{
    packed_float2 position;   // offset  0  R32G32_FLOAT        POSITION   pixels, top-left origin
    packed_float2 uv;         // offset  8  R32G32_FLOAT        TEXCOORD0
    packed_float4 color;      // offset 16  R32G32B32A32_FLOAT  COLOR
    // offset 32  R32G32B32A32_FLOAT  TEXCOORD1: circle centre (px), radius (px), picture
    // (0 atlas, 1 map; other values select glyph / panel / outline modes, see ui_fs).
    packed_float4 clip;
};
static_assert(sizeof(UiVertex) == 48, "UiVertex must match the Rust UiVertex (48 bytes)");

// Textures: atlas = texture(0), map_picture = texture(1), font_field = texture(2). Bound as
// entry-point parameters of ui_fs.

// The three D3D12 static samplers of ui_pipeline.rs (all CLAMP / MipLODBias 0 / MaxAnisotropy
// 1 / comparison ALWAYS unless noted):

// HLSL SamplerState glyph_sampler at s0. D3D12_FILTER_MIN_MAG_MIP_POINT, CLAMP,
// MinLOD 0, MaxLOD 0. Point, for the font: the glyphs are a 1-bit bitmap magnified to integer
// scales, and filtering one produces grey fringes around letters drawn to have none.
constexpr sampler glyph_sampler(
    coord::normalized,
    address::clamp_to_edge,
    mag_filter::nearest,
    min_filter::nearest,
    mip_filter::nearest,
    lod_clamp(0.0f, 0.0f));

// HLSL SamplerState icon_sampler at s1. D3D12_FILTER_MIN_MAG_MIP_LINEAR, CLAMP,
// MinLOD 0, MaxLOD 0. Linear, for the icons: an icon is rasterized at 88 texels and drawn at
// 44, and point sampling that minification aliases badly.
constexpr sampler icon_sampler(
    coord::normalized,
    address::clamp_to_edge,
    mag_filter::linear,
    min_filter::linear,
    mip_filter::linear,
    lod_clamp(0.0f, 0.0f));

// HLSL SamplerState map_sampler at s2. D3D12_FILTER_MIN_MAG_MIP_LINEAR, BORDER on U/V/W
// with a TRANSPARENT BLACK border, MinLOD 0, MaxLOD FLT_MAX. Linear and mipmapped,
// transparent past the edge: the map is minified on the minimap and magnified on the
// full-screen map, and off the level there is nothing to draw.
constexpr sampler map_sampler(
    coord::normalized,
    address::clamp_to_border,
    border_color::transparent_black,
    mag_filter::linear,
    min_filter::linear,
    mip_filter::linear,
    lod_clamp(0.0f, MAXFLOAT));

struct UiV2F
{
    float4 position [[position]];
    float2 uv;
    float4 color;
    // Constant across a quad by construction, so not interpolated at all.
    float4 clip [[flat]];
};

vertex UiV2F ui_vs(
    uint vertex_id [[vertex_id]],
    const device UiVertex* vertices [[buffer(16)]],
    constant UiFrame& ui [[buffer(0)]])
{
    UiVertex input = vertices[vertex_id];
    float2 pixel = float2(input.position);

    UiV2F output;
    // x: 0..width  -> -1..1.  y: 0..height -> 1..-1, because screen y grows downward and
    // clip-space y grows upward.
    output.position = float4(
        pixel.x * ui.pixel_to_clip.x - 1.0,
        1.0 + pixel.y * ui.pixel_to_clip.y,
        0.0,
        1.0);
    output.uv = float2(input.uv);
    output.color = float4(input.color);
    output.clip = float4(input.clip);
    return output;
}

fragment float4 ui_fs(
    UiV2F input [[stage_in]],
    constant UiFrame& ui [[buffer(0)]],
    texture2d<float> atlas [[texture(0)]],
    texture2d<float> map_picture [[texture(1)]],
    texture2d<float> font_field [[texture(2)]])
{
    // Derivatives first, in uniform control flow: the map is sampled through them below,
    // inside a branch where implicit derivatives would be undefined.
    float2 uv_dx = dfdx(input.uv);
    float2 uv_dy = dfdy(input.uv);

    float4 texel;
    if (input.clip.w < -0.5)
    {
        // A glyph with its edge moved outward by (-1 - picture) pixels: a thickened letter, or
        // the dark edge drawn under one. The same distance as the plain glyph below, shifted
        // before it becomes coverage. A wide edge ramps over up to two pixels, so an outline
        // reads as a soft dark rim rather than a hard second letter.
        float dilation = -input.clip.w - 1.0;
        float field = font_field.sample(icon_sampler, input.uv, level(0.0f)).a;
        float texels_per_pixel = max(
            0.5 * (length(uv_dx * ui.font_atlas_size) + length(uv_dy * ui.font_atlas_size)),
            1e-4);
        float distance_pixels = (field - 0.5) * 2.0 * ui.font_range / texels_per_pixel;
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
        float d = length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
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
        float field = font_field.sample(icon_sampler, input.uv, level(0.0f)).a;
        float texels_per_pixel = max(
            0.5 * (length(uv_dx * ui.font_atlas_size) + length(uv_dy * ui.font_atlas_size)),
            1e-4);
        float distance_pixels = (field - 0.5) * 2.0 * ui.font_range / texels_per_pixel;
        texel = float4(1.0, 1.0, 1.0, saturate(distance_pixels + 0.5));
    }
    else if (input.clip.w > 0.5)
    {
        texel = map_picture.sample(map_sampler, input.uv, gradient2d(uv_dx, uv_dy));
    }
    else
    {
        // Which sampler to read through is a property of *where* in the atlas this fragment
        // is reading, so the coordinate decides it and no per-vertex flag is needed. The font
        // band is the top of the texture and the icon band is everything below it.
        //
        // Explicit level 0 rather than an implicit-derivative sample: choosing a sampler is
        // non-uniform control flow. The atlas has one mip, so level 0 is both correct and what
        // an implicit sample would have found anyway.
        if (input.uv.y >= ui.icon_band_v)
        {
            texel = atlas.sample(icon_sampler, input.uv, level(0.0f));
        }
        else
        {
            texel = atlas.sample(glyph_sampler, input.uv, level(0.0f));
        }
    }

    // Straight multiply, for both bands at once. The font band stores white with the glyph's
    // coverage in alpha, so a glyph comes out as the vertex colour cut to the letter shape
    // and a solid rectangle samples the opaque texel and comes out untouched. The icon band
    // stores real colour, so a white tint draws the art as authored and a darker one dims it.
    float4 color = input.color * texel;

    // The circle clip: a disc's own edge, or the minimap's. One pixel of coverage ramp, so
    // the rim is anti-aliased rather than stair-stepped. [[position]] is the pixel centre in
    // the same top-left pixel space the vertices were written in.
    if (input.clip.z > 0.0)
    {
        float distance_to_centre = distance(input.position.xy, input.clip.xy);
        color.a *= saturate(input.clip.z - distance_to_centre + 0.5);
    }
    return color;
}
