// Skinned mesh shading: the same lighting as mesh.metal, with the vertex moved by a bone
// palette before anything else happens. Port of mesh_skinned.hlsl.
//
// The vertex is first blended through up to four bone matrices - which already contain
// `global * inverse_bind`, so they map a bind-pose vertex straight into the character's local
// space - and only then by the model matrix that places the character in the world.
//
//     skinned = sum(weight[i] * bones[joint[i]] * position)
//     world   = model * skinned
//
// The frame, object and material constant buffers are unchanged and still live at buffers
// 0, 1 and 2. The palette is buffer 3.
//
// Entry points: mesh_skinned_vs (HLSL VSMain), mesh_skinned_fs (HLSL PSMain; needs the three
// function constants from common.h).

#include "scene_frame.h"

vertex MeshV2F mesh_skinned_vs(
    uint vertex_id [[vertex_id]],
    const device SkinnedVertex* vertices [[buffer(16)]],
    constant FrameConstants& frame [[buffer(0)]],
    constant ObjectConstants& object [[buffer(1)]],
    constant SkinPalette& palette [[buffer(3)]])
{
    SkinnedVertex input = vertices[vertex_id];
    float4 tangent = float4(input.tangent);
    float4x4 model = object.model;

    // Four u16 joint indices, unpacked from two little-endian uints (R16G16B16A16_UINT).
    uint joint_x = input.joints01 & 0xFFFFu;
    uint joint_y = input.joints01 >> 16;
    uint joint_z = input.joints23 & 0xFFFFu;
    uint joint_w = input.joints23 >> 16;
    float4 weights = float4(input.weights);

    // The weighted sum of the four bone matrices. Summing the *matrices* and transforming
    // once is the same result as transforming four times and summing, for one quarter of the
    // work - and it is the form that lets the normal reuse the blend for free.
    float4x4 skin =
          palette.bones[joint_x] * weights.x
        + palette.bones[joint_y] * weights.y
        + palette.bones[joint_z] * weights.z
        + palette.bones[joint_w] * weights.w;

    float4 posed = skin * float4(float3(input.position), 1.0);
    float4 world = model * posed;

    MeshV2F output;
    output.position = frame.view_projection * world;
    output.world_position = world.xyz;
    // The same approximation mesh.metal makes - correct for uniform scale, which is what a
    // character has. A bone with non-uniform scale would need the inverse transpose of the
    // blend, which is a 3x3 inverse per vertex.
    output.world_normal = upper_3x3(model) * (upper_3x3(skin) * float3(input.normal));
    // The tangent is skinned by the same blended matrix as the normal. Skinning the normal
    // and not the tangent is the bug that makes a character's normal mapping look correct in
    // the bind pose and rotate against the light the moment an arm moves.
    output.world_tangent = float4(
        upper_3x3(model) * (upper_3x3(skin) * tangent.xyz), tangent.w);
    output.uv = float2(input.uv);
    return output;
}

fragment float4 mesh_skinned_fs(
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
