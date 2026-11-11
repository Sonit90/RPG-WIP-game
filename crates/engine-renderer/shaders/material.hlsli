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
    // glTF's normalTexture.scale: how hard the normal map pushes. 1 is the map as authored,
    // 0 flattens it to the geometric normal. In what used to be padding, so the buffer's
    // layout is unchanged - src/material.rs's MaterialConstants is the mirror.
    float normal_scale;
    // glTF's occlusionTexture.strength, applied by glTF's own formula below.
    float occlusion_strength;
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
// Three of the four fetches happen unconditionally; the normal map is behind
// `MATERIAL_NORMAL_MAP`. Until M5 all four were unconditional, on the argument that a fetch
// returning the identity texel is cheaper than a pipeline switch. Measurement said otherwise:
// the fetch and the tangent frame it feeds cost 0.071 ms at 1440p, most of it on materials
// that sample the flat fallback and get their own geometric normal back. A material without a
// map now compiles the fetch out entirely and `normal_ts` is the packed flat normal, so
// anything that still reads it sees +Z.
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
#if MATERIAL_NORMAL_MAP
    result.normal_ts = normal_map.Sample(base_color_sampler, uv).rgb;
#else
    // The flat tangent-space normal, packed: what the shared fallback texel holds. No fetch,
    // and `apply_normal_map` is not called on this path at all.
    result.normal_ts = float3(0.5, 0.5, 1.0);
#endif

    float3 orm = orm_map.Sample(base_color_sampler, uv).rgb;
    // glTF: `1 + strength * (sampled - 1)`. A strength of 1 is the map, 0 is no occlusion at
    // all, and the interpolation is towards *unoccluded* rather than towards black - which is
    // the difference between an artist dialling a map back and a scene going dark.
    result.occlusion = saturate(1.0 + occlusion_strength * (saturate(orm.r) - 1.0));
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
#if MATERIAL_NORMAL_MAP
        return normal_map.Sample(base_color_sampler, uv).rgb;
#else
        // A material with no normal map has nothing to preview, and compiling the fetch in for
        // a debug view would put it back in every pixel's shader. The flat normal is what the
        // fallback texel holds and what the shading uses.
        return float3(0.5, 0.5, 1.0);
#endif
    }
    if (mode == 6u)
    {
        return orm_map.Sample(base_color_sampler, uv).rgb;
    }
    return emissive_map.Sample(base_color_sampler, uv).rgb;
}

#endif // MATERIAL_HLSLI
