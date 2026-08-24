// The material constant buffer, its four-texture set, and how a fragment reads them.
//
// Its own file because two very different pipelines read it: the mesh pipelines, which shade
// with it, and the depth-only shadow pipeline, which reads exactly one field out of it — the
// alpha cutoff — and samples exactly one texture. Declaring it twice would mean two struct
// layouts that have to agree with `MaterialConstants` in src/material.rs, and the one that
// is only used by the shadow pass is the one that would quietly drift.

#ifndef MATERIAL_HLSLI
#define MATERIAL_HLSLI

cbuffer Material : register(b2)
{
    float4 base_color;
    float3 emissive;
    float roughness;
    float metallic;
    // Below this alpha the fragment is discarded. Zero means the material is opaque and no
    // test is performed — which is the ordinary case, and costs one comparison.
    float alpha_cutoff;
    float2 material_padding;
};

// The material texture set: one contiguous four-descriptor table at root parameter 3, read
// through one static sampler. The registers are the ABI — src/material.rs's
// `MaterialTextureSlot` writes the descriptors in exactly this order, and
// tests/root_signature_mapping.rs asserts the two agree.
//
// Every material binds all four. A material with no such map points at a shared neutral
// texture rather than leaving the descriptor unbound: one texture fetch that returns the
// identity, against a shader permutation per absent map.
//
//   t0  base colour  sRGB view, so the sample arrives linear. Multiplied with `base_color`.
//   t1  normal       tangent-space, linear. Consumed by `apply_normal_map`.
//   t2  ORM          R occlusion, G roughness, B metallic. The glTF packing, and the order
//                    is not negotiable: a swapped G and B is a scene of plastic metal.
//   t3  emissive     sRGB view. Multiplied with `emissive`, glTF's own convention.
Texture2D base_color_map : register(t0);
Texture2D normal_map     : register(t1);
Texture2D orm_map        : register(t2);
Texture2D emissive_map   : register(t3);

// One sampler for all four, static in the root signature: there is nothing per-material to
// bind for it and no sampler heap at all. Kept named for the base colour map because that is
// what every existing call site says.
SamplerState base_color_sampler : register(s0);

// One fragment's material, sampled and combined with the constants.
//
// The four fetches happen unconditionally. There is no "has a normal map" flag to branch on
// and deliberately so: the alternative to a fallback texel is a shader permutation per absent
// map, and a fetch that returns the identity is cheaper than the pipeline switch would be.
struct MaterialSample
{
    float4 base_color;   // Linear RGBA: the sRGB view decoded the texture, the factor is linear.
    float3 normal_ts;    // Tangent-space normal, still packed in [0, 1].
    float occlusion;     // ORM.r — ambient occlusion, applied to the ambient term only.
    float roughness;     // ORM.g * the material's roughness factor.
    float metallic;      // ORM.b * the material's metallic factor.
    float3 emissive;     // Emissive factor times the emissive map, linear.
};

// Reads all four maps and folds in the constants.
//
// glTF's convention throughout: every texture *scales* its factor rather than replacing it,
// which is why each shared fallback is that channel's multiplicative identity and why a
// material with no maps at all shades exactly as its constants describe.
MaterialSample sample_material(float2 uv)
{
    MaterialSample result;
    result.base_color = base_color * base_color_map.Sample(base_color_sampler, uv);
    result.normal_ts = normal_map.Sample(base_color_sampler, uv).rgb;

    float3 orm = orm_map.Sample(base_color_sampler, uv).rgb;
    result.occlusion = saturate(orm.r);
    // Saturated, not just clamped by the BRDF later: a texture authored outside [0, 1] — a
    // float format, or a channel used for something else by mistake — would otherwise reach
    // `make_surface` as a roughness of 4 and produce a surface with no highlight at all
    // rather than a visibly wrong one.
    result.roughness = saturate(roughness * orm.g);
    result.metallic = saturate(metallic * orm.b);

    result.emissive = emissive * emissive_map.Sample(base_color_sampler, uv).rgb;
    return result;
}

// The material maps as the debug views display them, sampled and nothing more.
//
// Raw texels, with no shading, no colour-space conversion beyond the view's own and no
// factors applied: what proves the four descriptors of the range resolve to four distinct
// resources in the frozen order. Modes 5, 6 and 7; mode 4 is the base colour, which
// `shade_surface` already has in hand.
float3 material_map_preview(uint mode, float2 uv)
{
    if (mode == 5u)
    {
        return normal_map.Sample(base_color_sampler, uv).rgb;
    }
    if (mode == 6u)
    {
        return orm_map.Sample(base_color_sampler, uv).rgb;
    }
    return emissive_map.Sample(base_color_sampler, uv).rgb;
}

#endif // MATERIAL_HLSLI
