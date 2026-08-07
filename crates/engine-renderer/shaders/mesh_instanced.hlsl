// Instanced mesh shading: the same lighting as mesh.hlsl, with the model matrix supplied
// per instance instead of per draw.
//
// mesh.hlsl reads `model` from a constant buffer the CPU rebinds before every draw. That
// costs one root-CBV bind and one draw call per object. Here the matrix arrives as four
// float4 rows in a second vertex stream, stepped once per instance, so a run of objects
// sharing a mesh and a material becomes one DrawIndexedInstanced.
//
// The frame and material constant buffers are unchanged and still live at b0 and b2 — the
// root signature is the same one, minus the per-object slot the instance stream replaces.
// Keeping the registers identical is what lets both pipelines share a root signature and
// the material upload path.

cbuffer Frame : register(b0)
{
    column_major float4x4 view_projection;
    float3 camera_position;
    float frame_padding;
    // The direction the light travels, not the direction towards it.
    float3 light_direction;
    float light_intensity;
    float3 light_color;
    float ambient_intensity;
    float3 ambient_color;
    float frame_padding_2;
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
    // Per-vertex, input slot 0 — the same MeshVertex layout mesh.hlsl consumes.
    float3 position : POSITION;
    float3 normal   : NORMAL;
    float2 uv       : TEXCOORD0;

    // Per-instance, input slot 1. Four rows of the local-to-world matrix.
    //
    // Rows rather than columns: a vertex element is at most a float4, so a matrix has to
    // arrive as four of them, and rows are what the CPU-side Mat4's columns transpose into
    // for free. `mul(v, M_rows)` is then the same transform `mul(M, v)` performs in
    // mesh.hlsl — the transpose is in the layout, not in an instruction.
    float4 model0 : INSTANCE_MODEL0;
    float4 model1 : INSTANCE_MODEL1;
    float4 model2 : INSTANCE_MODEL2;
    float4 model3 : INSTANCE_MODEL3;
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

    // HLSL's float4x4(a, b, c, d) constructor fills *rows*, and the four elements carry
    // the matrix's *columns* — engine_core::math::Mat4 is column-major and is memcpy'd in
    // untransposed. So `model` here is the transpose of the real local-to-world matrix.
    //
    // That is deliberate, and it is why the multiply is vector-first. mul(v, M) computes
    // v * M, which for M = Mtrue^T is exactly Mtrue * v — the transform mesh.hlsl performs
    // as mul(model, v). Writing mul(model, v) here instead transposes the transform, and
    // the symptom is a scene that renders with every object sheared and enormous rather
    // than anything that looks like a matrix bug.
    float4x4 model = float4x4(input.model0, input.model1, input.model2, input.model3);

    float4 world = mul(float4(input.position, 1.0), model);
    output.position = mul(view_projection, world);
    // Correct only for uniform scale, exactly as in mesh.hlsl. Non-uniform scale needs the
    // inverse transpose, which nothing yet uses.
    // Vector-first for the same reason, on the upper-left 3x3.
    output.world_normal = mul(input.normal, (float3x3)model);
    output.uv = input.uv;
    return output;
}

float4 PSMain(PixelInput input) : SV_TARGET
{
    // Byte-for-byte the same lighting as mesh.hlsl. If the two ever disagree, an instanced
    // frame and a non-instanced frame would be different images, and the equivalence test
    // in tests/instanced_equivalence.rs is what catches that.
    float3 normal = normalize(input.world_normal);
    float3 to_light = normalize(-light_direction);
    float lambert = saturate(dot(normal, to_light));

    float3 diffuse = base_color.rgb * light_color * (light_intensity * lambert);
    float3 ambient = base_color.rgb * ambient_color * ambient_intensity;
    return float4(diffuse + ambient + emissive, base_color.a);
}
