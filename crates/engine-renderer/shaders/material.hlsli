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

// The material's base colour map, bound as a one-descriptor table at root parameter 3, and
// the sampler that reads it — a static sampler in the root signature, so there is no
// descriptor to allocate and nothing per-material to bind for it.
//
// Every material has a map. An untextured one points at a shared 1x1 white texel, which
// costs one texture fetch that returns 1 and saves a second pipeline state, a second shader
// and the branch that would choose between them.
Texture2D base_color_map : register(t0);
SamplerState base_color_sampler : register(s0);

#endif // MATERIAL_HLSLI
