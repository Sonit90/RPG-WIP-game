// Opaque mesh shading: transform by the camera, shade with the sun, the shadow atlas and
// whatever local lights reach the surface. Port of mesh.hlsl.
//
// Matrices are column-major in both HLSL (declared column_major) and MSL, so a matrix is
// memcpy'd into the constant buffer with no transpose and the matrix-vector product is `M * v`.
//
// The frame constants, the material constants and the whole lighting function live in
// scene_frame.h, shared with the instanced and skinned pipelines. Only the two things that
// genuinely differ between them are here: how a vertex reaches world space, and where the
// per-object tint comes from.
//
// Entry points: mesh_vs (HLSL VSMain), mesh_fs (HLSL PSMain; needs the three function
// constants from common.h).

#include "scene_frame.h"

vertex MeshV2F mesh_vs(
    uint vertex_id [[vertex_id]],
    const device MeshVertex* vertices [[buffer(16)]],
    constant FrameConstants& frame [[buffer(0)]],
    constant ObjectConstants& object [[buffer(1)]])
{
    MeshVertex input = vertices[vertex_id];
    float4 tangent = float4(input.tangent);
    float4x4 model = object.model;

    MeshV2F output;
    float4 world = model * float4(float3(input.position), 1.0);
    output.position = frame.view_projection * world;
    // The world position is what every shadow lookup starts from: a cascade, a spot light
    // and the contact-shadow march all project it into a different space.
    output.world_position = world.xyz;
    // Correct only for uniform scale. Non-uniform scale needs the inverse transpose, which
    // costs an extra matrix per object; nothing scales non-uniformly yet.
    output.world_normal = upper_3x3(model) * float3(input.normal);
    // The same transform the normal gets, for the same reason and with the same uniform-scale
    // caveat. `apply_normal_map` re-orthogonalizes the pair after interpolation.
    output.world_tangent =
        float4(upper_3x3(model) * tangent.xyz, tangent.w);
    output.uv = float2(input.uv);
    return output;
}

fragment float4 mesh_fs(
    MeshV2F input [[stage_in]],
    constant FrameConstants& frame [[buffer(0)]],
    constant ObjectConstants& object [[buffer(1)]],
    constant MaterialConstants& material [[buffer(2)]],
    const device uint* cluster_light_mask [[buffer(4)]],
    texture2d<float> base_color_map [[texture(0)]],
    texture2d<float> normal_map [[texture(1)]],
    texture2d<float> orm_map [[texture(2)]],
    texture2d<float> emissive_map [[texture(3)]],
    depth2d<float> shadow_atlas [[texture(4)]],
    texture2d<float> contact_shadows [[texture(7)]])
{
    SceneResources resources = {
        &frame,
        &material,
        MaterialTextures{ base_color_map, normal_map, orm_map, emissive_map },
        shadow_atlas,
        contact_shadows,
        cluster_light_mask
    };
    return shade_surface(
        resources,
        input.position,
        input.world_position,
        input.world_normal,
        input.world_tangent,
        input.uv,
        object.instance_tint,
        object.instance_emissive,
        object.instance_receives_shadow,
        object.instance_fade,
        object.instance_see_through);
}
