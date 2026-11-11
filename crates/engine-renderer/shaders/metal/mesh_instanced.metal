// Instanced mesh shading: the same lighting as mesh.metal, with the model matrix supplied
// per instance instead of per draw. Port of mesh_instanced.hlsl.
//
// mesh.hlsl reads `model` from a constant buffer the CPU rebinds before every draw. Here the
// matrix arrives as four float4s in a second vertex stream (buffer 17, indexed by
// [[instance_id]]), so a run of objects sharing a mesh and a material becomes one instanced
// draw. The frame and material constant buffers are unchanged and still live at buffer 0 and
// buffer 2; there is no Object buffer (the instance stream replaces it).
//
// Entry points: mesh_instanced_vs (HLSL VSMain), mesh_instanced_fs (HLSL PSMain; needs the
// three function constants from common.h).

#include "scene_frame.h"

struct MeshInstancedV2F
{
    float4 position [[position]];
    float3 world_position;
    float3 world_normal;
    // The tangent in world space, handedness carried through untouched in w: it is a property
    // of the UV layout, not of where the instance stands.
    float4 world_tangent;
    float2 uv;
    float4 tint;
    // Constant across the instance, so not interpolated. (HLSL nointerpolation)
    float4 emissive_scale [[flat]];
};

vertex MeshInstancedV2F mesh_instanced_vs(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device MeshVertex* vertices [[buffer(16)]],
    const device InstanceTransform* instances [[buffer(17)]],
    constant FrameConstants& frame [[buffer(0)]])
{
    MeshVertex input = vertices[vertex_id];
    float4 tangent = float4(input.tangent);
    InstanceTransform instance = instances[instance_id];

    // The four per-instance float4s carry the *columns* of the column-major local-to-world
    // matrix (engine_core::math::Mat4 is memcpy'd in untransposed).
    //
    // HLSL's float4x4(a, b, c, d) fills *rows*, so there `model` was the transpose of the real
    // matrix and the vertex was multiplied vector-first: the vector-first product v * M equals Mtrue * v.
    // MSL's float4x4(a, b, c, d) fills *columns*, so here `model` IS the real matrix and the
    // ordinary `model * v` is the very same transform. (Both compute
    // v.x * model0 + v.y * model1 + v.z * model2 + v.w * model3.)
    float4x4 model = float4x4(
        float4(instance.model0),
        float4(instance.model1),
        float4(instance.model2),
        float4(instance.model3));

    MeshInstancedV2F output;
    float4 world = model * float4(float3(input.position), 1.0);
    output.position = frame.view_projection * world;
    output.world_position = world.xyz;
    // Correct only for uniform scale, exactly as in mesh.metal. HLSL's vector-first
    // the vector-first normal transform equals upper_3x3(model) * n here.
    output.world_normal = upper_3x3(model) * float3(input.normal);
    output.world_tangent = float4(upper_3x3(model) * tangent.xyz, tangent.w);
    output.uv = float2(input.uv);
    output.tint = float4(instance.tint);
    output.emissive_scale = float4(instance.emissive_scale);
    return output;
}

fragment float4 mesh_instanced_fs(
    MeshInstancedV2F input [[stage_in]],
    constant FrameConstants& frame [[buffer(0)]],
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
        input.tint,
        input.emissive_scale.x,
        input.emissive_scale.y,
        input.emissive_scale.z,
        input.emissive_scale.w);
}
