// The material constant buffer and its base colour map.
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
//   t0  base colour  sRGB, multiplied with `base_color`
//   t1  normal       tangent-space, linear. Bound and not yet read — Milestone 8 consumes it.
//   t2  ORM          R occlusion, G roughness, B metallic. The glTF packing.
//   t3  emissive     sRGB, eventually multiplied with `emissive`.
Texture2D base_color_map : register(t0);
Texture2D normal_map     : register(t1);
Texture2D orm_map        : register(t2);
Texture2D emissive_map   : register(t3);

// One sampler for all four, static in the root signature: there is nothing per-material to
// bind for it and no sampler heap at all. Kept named for the base colour map because that is
// what every existing call site says.
SamplerState base_color_sampler : register(s0);

// The material maps as the debug modes below display them, sampled and nothing more.
//
// This function is why the four textures survive DXIL dead-code elimination at a milestone
// where the lighting model reads only the first: the root signature declares a four-SRV
// range, and a shader that referenced one of them would make the other three untestable.
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
