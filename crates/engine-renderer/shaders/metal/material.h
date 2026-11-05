// The material constant buffer, its four-texture set, and how a fragment reads them.
// Port of material.hlsli.
//
// Read by the mesh pipelines (which shade with it) and by the depth-only shadow pipeline
// (which reads only `alpha_cutoff` and `base_color`, and samples only the base colour map).

#ifndef METAL_MATERIAL_H
#define METAL_MATERIAL_H

#include "common.h"

// HLSL constant buffer Material at b2. MaterialConstants (src/material.rs), 48 bytes.
struct MaterialConstants
{
    float4 base_color;          // offset  0
    packed_float3 emissive;     // offset 16  (shares a 16-byte row with roughness)
    float roughness;            // offset 28
    float metallic;             // offset 32
    // Below this alpha the fragment is discarded. Zero means the material is opaque and no
    // test is performed.
    float alpha_cutoff;         // offset 36
    // glTF's normalTexture.scale: how hard the normal map pushes. 1 is the map as authored,
    // 0 flattens it to the geometric normal.
    float normal_scale;         // offset 40
    // glTF's occlusionTexture.strength, applied by glTF's own formula below.
    float occlusion_strength;   // offset 44
};
static_assert(sizeof(MaterialConstants) == 48, "MaterialConstants must match the Rust MaterialConstants (48 bytes)");

// The material texture set (HLSL t0..t3), read through one sampler.
//
//   texture(0)  base colour  sRGB view, so the sample arrives linear. Multiplied with base_color.
//   texture(1)  normal       tangent-space, linear. Consumed by apply_normal_map.
//   texture(2)  ORM          R occlusion, G roughness, B metallic. The glTF packing.
//   texture(3)  emissive     sRGB view. Multiplied with emissive.
//
// Every material binds all four. A material with no such map points at a shared neutral
// texture rather than leaving the slot unbound.
struct MaterialTextures
{
    texture2d<float> base_color_map;
    texture2d<float> normal_map;
    texture2d<float> orm_map;
    texture2d<float> emissive_map;
};

// HLSL SamplerState base_color_sampler at s0 - the D3D12 static sampler of
// mesh_pipeline.rs `static_sampler()`: D3D12_FILTER_ANISOTROPIC, WRAP on U/V/W, MaxAnisotropy 8,
// MipLODBias 0, MinLOD 0, MaxLOD FLT_MAX, comparison ALWAYS (none), border opaque black
// (unused with wrap). One sampler for all four maps (and for the shadow pass's alpha test).
constexpr sampler base_color_sampler(
    coord::normalized,
    address::repeat,
    mag_filter::linear,
    min_filter::linear,
    mip_filter::linear,
    max_anisotropy(8),
    lod_clamp(0.0f, MAXFLOAT));

// One fragment's material, sampled and combined with the constants.
struct MaterialSample
{
    float4 base_color;   // Linear RGBA: the sRGB view decoded the texture, the factor is linear.
    float3 normal_ts;    // Tangent-space normal, still packed in [0, 1].
    float occlusion;     // ORM.r - ambient occlusion, applied to the ambient term only.
    float roughness;     // ORM.g * the material's roughness factor.
    float metallic;      // ORM.b * the material's metallic factor.
    float3 emissive;     // Emissive factor times the emissive map, linear.
};

// Reads all four maps and folds in the constants.
//
// glTF's convention throughout: every texture *scales* its factor rather than replacing it.
// Three of the four fetches are unconditional; the normal map is behind MATERIAL_NORMAL_MAP.
inline MaterialSample sample_material(
    constant MaterialConstants& mc,
    thread const MaterialTextures& tex,
    float2 uv)
{
    MaterialSample result;
    result.base_color = mc.base_color * tex.base_color_map.sample(base_color_sampler, uv);
    if (MATERIAL_NORMAL_MAP)
    {
        result.normal_ts = tex.normal_map.sample(base_color_sampler, uv).rgb;
    }
    else
    {
        // The flat tangent-space normal, packed: what the shared fallback texel holds. No fetch,
        // and apply_normal_map is not called on this path at all.
        result.normal_ts = float3(0.5, 0.5, 1.0);
    }

    float3 orm = tex.orm_map.sample(base_color_sampler, uv).rgb;
    // glTF: `1 + strength * (sampled - 1)`. A strength of 1 is the map, 0 is no occlusion at
    // all, and the interpolation is towards *unoccluded* rather than towards black.
    result.occlusion = saturate(1.0 + mc.occlusion_strength * (saturate(orm.r) - 1.0));
    // Saturated, not just clamped by the BRDF later: a texture authored outside [0, 1] would
    // otherwise reach make_surface as a roughness of 4 and produce a surface with no highlight.
    result.roughness = saturate(mc.roughness * orm.g);
    result.metallic = saturate(mc.metallic * orm.b);

    result.emissive = float3(mc.emissive) * tex.emissive_map.sample(base_color_sampler, uv).rgb;
    return result;
}

// The material maps as the debug views display them, sampled and nothing more. Modes 5, 6, 7;
// mode 4 is the base colour, which shade_surface already has in hand.
inline float3 material_map_preview(thread const MaterialTextures& tex, uint mode, float2 uv)
{
    if (mode == 5u)
    {
        if (MATERIAL_NORMAL_MAP)
        {
            return tex.normal_map.sample(base_color_sampler, uv).rgb;
        }
        // A material with no normal map has nothing to preview. The flat normal is what the
        // fallback texel holds and what the shading uses.
        return float3(0.5, 0.5, 1.0);
    }
    if (mode == 6u)
    {
        return tex.orm_map.sample(base_color_sampler, uv).rgb;
    }
    return tex.emissive_map.sample(base_color_sampler, uv).rgb;
}

#endif // METAL_MATERIAL_H
