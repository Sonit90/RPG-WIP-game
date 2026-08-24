// The metallic-roughness BRDF every mesh pipeline shades with.
//
// Its own file for the reason scene_frame.hlsli exists: three pipelines shade with it and a
// second copy would eventually disagree with the first. Nothing here reads a constant buffer
// or a texture - a `Surface` is built once per fragment from the material set, and every
// light evaluates against it - so this file has no ABI of its own to keep in step with Rust.
//
// # The energy convention, which is the one thing here that is not textbook
//
// A physically-normalized Lambert term is `albedo / PI`. This renderer's lights were authored
// against `albedo * intensity`, and the scenes, the shadow tests and the tuned intensities in
// engine-render-data all assume it. Dividing by PI here would darken every existing asset by
// 3.14 and turn a material milestone into a re-lighting exercise.
//
// So the diffuse lobe keeps its unnormalized form and the specular lobe is multiplied by PI
// to sit in the same units. The *ratio* between the two lobes - which is what a physically
// based model actually buys - is exactly the textbook one; only the shared scale differs, and
// that scale lives in `light_intensity`. Real photometric units are a lighting milestone, not
// this one.

#ifndef PBR_HLSLI
#define PBR_HLSLI

#define PBR_PI 3.14159265358979323846

// The lowest roughness a surface may claim.
//
// A perfect mirror is a delta function, and GGX's `D` divides by the square of a term that
// goes to zero with it: at roughness 0 a highlight becomes one pixel of near-infinity. 0.045
// is the usual floor and is visually indistinguishable from zero.
#define MIN_ROUGHNESS 0.045

// What one fragment's material and geometry mean to the lighting loop.
//
// Built once per fragment. The lights iterate over it, so everything that does not depend on
// the light direction - the normal, the view vector, the split of base colour into a diffuse
// albedo and a specular F0 - is computed here rather than thirty-two times.
struct Surface
{
    float3 normal;         // Shading normal, world space, unit. Normal-mapped when a map is bound.
    float3 view;           // Unit vector from the surface towards the camera.
    float3 diffuse_color;  // Base colour less the metallic part. A metal has no diffuse lobe.
    float3 f0;             // Specular reflectance at normal incidence.
    float roughness;       // Perceptual, clamped to [MIN_ROUGHNESS, 1].
    float alpha;           // roughness^2, the GGX parameter D and V actually use.
    float n_dot_v;         // Saturated, and never exactly zero.
    float occlusion;       // Baked ambient occlusion, from the ORM map's red channel.
};

// Splits a base colour into the two lobes the metallic workflow describes.
//
// A dielectric reflects about 4% of light specularly whatever colour it is and diffuses the
// rest tinted by its albedo. A metal has no diffuse transmission at all and tints its
// specular reflection with its base colour instead. A `metallic` between the two is not a
// physical material - it is the blend an authored texture uses across the edge of a rusted
// patch - so the two ends are lerped rather than branched between.
Surface make_surface(
    float3 normal,
    float3 view,
    float3 base_color,
    float roughness,
    float metallic,
    float occlusion)
{
    Surface surface;
    surface.normal = normal;
    surface.view = view;
    surface.diffuse_color = base_color * (1.0 - metallic);
    surface.f0 = lerp(float3(0.04, 0.04, 0.04), base_color, metallic);
    surface.roughness = clamp(roughness, MIN_ROUGHNESS, 1.0);
    surface.alpha = surface.roughness * surface.roughness;
    // Clamped away from zero rather than saturated to it: `n_dot_v` divides in the visibility
    // term, and a polygon seen edge-on is common enough to matter.
    surface.n_dot_v = max(dot(normal, view), 1e-4);
    surface.occlusion = occlusion;
    return surface;
}

// GGX / Trowbridge-Reitz normal distribution: the fraction of microfacets aimed at `h`.
//
// What makes a rough surface's highlight wide and dim and a smooth one's narrow and bright,
// and the only term in the BRDF that can produce a very large number - hence the roughness
// floor above and the guarded divide here.
float distribution_ggx(float n_dot_h, float alpha)
{
    float a2 = alpha * alpha;
    float d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PBR_PI * d * d, 1e-7);
}

// Smith height-correlated visibility: the geometry term with the BRDF's
// `4 * NdotL * NdotV` denominator already folded in.
//
// Written in the combined form deliberately. Computing G and dividing separately means
// dividing by a quantity that goes to zero at every grazing angle, and the visible result is
// a ring of fireflies around every silhouette.
float visibility_smith(float n_dot_v, float n_dot_l, float alpha)
{
    float a2 = alpha * alpha;
    float lambda_v = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    float lambda_l = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(lambda_v + lambda_l, 1e-5);
}

// Schlick's approximation of the Fresnel factor: how much more reflective a surface is at a
// grazing angle. The `pow(1 - x, 5)` is the whole approximation.
float3 fresnel_schlick(float3 f0, float v_dot_h)
{
    float f = pow(saturate(1.0 - v_dot_h), 5.0);
    return f0 + (1.0 - f0) * f;
}

// What one light of unit radiance adds at this surface, before its colour, intensity,
// attenuation and shadow are applied.
//
// Returns zero for a light behind the surface, which is also the early-out every caller
// wants: no light means no shadow lookup either.
float3 shade_light(Surface surface, float3 to_light)
{
    float n_dot_l = saturate(dot(surface.normal, to_light));
    if (n_dot_l <= 0.0)
    {
        return float3(0.0, 0.0, 0.0);
    }

    float3 half_vector = normalize(surface.view + to_light);
    float n_dot_h = saturate(dot(surface.normal, half_vector));
    float v_dot_h = saturate(dot(surface.view, half_vector));

    float d = distribution_ggx(n_dot_h, surface.alpha);
    float v = visibility_smith(surface.n_dot_v, n_dot_l, surface.alpha);
    float3 f = fresnel_schlick(surface.f0, v_dot_h);

    // PI on the specular lobe rather than 1/PI on the diffuse one. See the file header: the
    // lobes are in the ratio the model describes, in the units this renderer's lights use.
    float3 specular = d * v * f * PBR_PI;
    // Energy taken by the specular reflection is not also available to the diffuse lobe. At a
    // dielectric's F0 of 0.04 that is a 4% darkening face-on and a much larger one at grazing
    // angles, which is where it is visible: the rim of a sphere goes shiny rather than bright.
    float3 diffuse = surface.diffuse_color * (1.0 - f);

    return (diffuse + specular) * n_dot_l;
}

// Tangent-space normal mapping.
//
// `tangent.w` is the handedness the asset pipeline stored: `bitangent = cross(N, T) * w`.
// Storing the sign rather than the bitangent is what makes a mirrored UV island - the left
// half of a character sharing the right half's texture - shade the right way up instead of
// inside out, and dropping it is the classic bug whose symptom is one side of a face lit from
// the wrong direction.
//
// The interpolated tangent is re-orthogonalized against the interpolated normal, because
// interpolating two unit vectors across a triangle does not preserve the angle between them.
//
// A material with no normal map samples the shared flat-normal fallback, which unpacks to +Z,
// so this returns the geometric normal to within a rounding error. One fetch and no branch,
// which is the trade M2 made deliberately.
float3 apply_normal_map(float3 world_normal, float4 world_tangent, float3 sampled, float scale)
{
    float3 n = normalize(world_normal);
    float3 t = world_tangent.xyz;
    if (dot(t, t) < 1e-8)
    {
        // No usable tangent frame. Every mesh this renderer draws carries one, so this is a
        // guard rather than an expected path: shading with the geometric normal is
        // wrong-but-stable, where normalizing a zero vector is a NaN that spreads through the
        // whole lighting term.
        return n;
    }
    t = normalize(t - n * dot(n, t));
    float3 b = cross(n, t) * (world_tangent.w < 0.0 ? -1.0 : 1.0);

    // Decoded about 128, not about 127.5.
    //
    // The textbook `2x - 1` assumes a texel of exactly 0.5 means "no perturbation", and an
    // 8-bit texture cannot hold 0.5: a flat normal is written as 128, which is 0.50196, and
    // `2x - 1` turns it into a tilt of 0.0039 in x and y. That is a fifth of a degree, and it
    // is on *every* surface with a flat normal map and on every material with no normal map
    // at all, since the shared fallback texel is 128 too. Small, but it means "no normal map"
    // is not quite the identity - a face exactly edge-on to a light picks up a hair of
    // diffuse, which is visible as a seam where a mapped surface meets an unmapped one.
    //
    // Scaling by 255/128 puts the zero exactly on 128 instead. The cost is that -1 is
    // reachable and +1 is not (255 decodes to 0.992), which is a rounding difference in the
    // most extreme texel a normal map can hold and is renormalized away below.
    // Z is *reconstructed*, not read.
    //
    // A normal map compressed to BC5 has only two channels — X and Y, at one byte a texel —
    // and its blue channel decodes to zero. An uncompressed one still carries a blue channel,
    // and for a unit normal it holds exactly what this computes. Reconstructing in both cases
    // means one decode for both formats rather than a shader permutation per texture, and it
    // is the reason a small procedural normal map (left uncompressed) and a large imported one
    // (BC5) shade identically.
    //
    // The clamp under the square root is what stops a texel whose X and Y exceed the unit
    // circle — quantization, or a map that was never normalized — from producing a NaN.
    float2 tangent_xy = sampled.xy * (255.0 / 128.0) - 1.0;
    // glTF's normalTexture.scale, applied the way glTF specifies: the tangent-space X and Y
    // are scaled and Z is left alone, so the vector tilts further from or closer to the
    // surface normal and the renormalize below keeps it a unit direction. Scaling all three
    // would be a no-op after that renormalize, which is the mistake this comment exists to
    // prevent.
    // Z comes from the *unscaled* pair, which is glTF's formula:
    // `normalize((sampled * 2 - 1) * float3(scale, scale, 1))`. Scaling first and then
    // reconstructing would fold the scale into Z as well and dampen the effect rather than
    // applying it.
    float tangent_z = sqrt(saturate(1.0 - dot(tangent_xy, tangent_xy)));
    float3 tangent_normal = float3(tangent_xy * scale, tangent_z);
    // No zero-length guard is needed: Z is 1 wherever X and Y are zero, so the vector below
    // always has a direction, and a scale of 0 gives exactly the geometric normal.
    float3 mapped = tangent_normal.x * t + tangent_normal.y * b + tangent_normal.z * n;
    return normalize(mapped);
}

#endif // PBR_HLSLI
