// Opaque mesh shading: transform by the camera, tint by the material.
//
// Matrices are declared column_major, which is both the HLSL default and the layout
// engine_core::math::Mat4 already has in memory — columns in consecutive registers. So a
// matrix is memcpy'd into the constant buffer with no transpose, and mul(M, v) is the same
// M * v the CPU-side math means.
//
// There is no lighting here. That is Milestone 14; this milestone is about geometry
// arriving in the right place, and a flat colour makes "the right place" unambiguous.

cbuffer Frame : register(b0)
{
    column_major float4x4 view_projection;
    float3 camera_position;
    float frame_padding;
    // The direction the light travels, not the direction towards it. Stored the way it is
    // authored ("the sun points down and to the left") and negated once, here, rather than
    // at every call site that sets it.
    float3 light_direction;
    float light_intensity;
    float3 light_color;
    float ambient_intensity;
    float3 ambient_color;
    float frame_padding_2;
};

cbuffer Object : register(b1)
{
    column_major float4x4 model;
};

cbuffer Material : register(b2)
{
    float4 base_color;
    float3 emissive;
    float roughness;
    float metallic;
    float3 material_padding;
};

struct VertexInput
{
    float3 position : POSITION;
    float3 normal   : NORMAL;
    float2 uv       : TEXCOORD0;
};

struct PixelInput
{
    float4 position     : SV_POSITION;
    float3 world_normal : NORMAL;
    float2 uv           : TEXCOORD0;
};

PixelInput VSMain(VertexInput input)
{
    PixelInput output;
    float4 world = mul(model, float4(input.position, 1.0));
    output.position = mul(view_projection, world);
    // Correct only for uniform scale. Non-uniform scale needs the inverse transpose, which
    // costs an extra matrix per object; nothing scales non-uniformly yet, and Milestone 14
    // is where a wrong normal would start to show.
    output.world_normal = mul((float3x3)model, input.normal);
    output.uv = input.uv;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    // Lambert diffuse plus a constant ambient term. Not a BRDF: roughness and metallic are
    // carried in the material and deliberately unused, because a specular lobe would be a
    // second thing to be wrong while establishing that normals and light direction are
    // right. Milestone 14's job is that a lit surface is brighter than an unlit one, for
    // the correct geometric reason.
    float3 normal = normalize(input.world_normal);
    float3 to_light = normalize(-light_direction);
    float lambert = saturate(dot(normal, to_light));

    float3 diffuse = base_color.rgb * light_color * (light_intensity * lambert);
    float3 ambient = base_color.rgb * ambient_color * ambient_intensity;
    return float4(diffuse + ambient + emissive, base_color.a);
}
