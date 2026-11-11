# Metal shader bindings

Hand-port of `crates/engine-renderer/shaders/*.hlsl` to MSL (2.4+, macOS 13+, Apple GPU family).
Nothing here has been run through the Metal compiler; it was written against the HLSL and the Rust
sources only. This file is the contract the Rust Metal backend is written against.

## Conventions

- **One `.metal` file = one `MTLLibrary`.** Headers (`common.h`, `material.h`, `pbr.h`,
  `scene_frame.h`) are textually included; `#include "x.h"` resolves relative to the file.
- **Constant buffers**: HLSL `register(bN)` -> `[[buffer(N)]]`, same N. Buffers 0..7 are
  constants/data. Only buffers 0-4 are used: 0 frame-like, 1 object, 2 material, 3 skin, 4 cluster mask.
- **Vertex streams (vertex pulling, no `[[stage_in]]`, no vertex descriptor)**: the stream for D3D12
  input slot S is `[[buffer(16 + S)]]`. Slot 0 -> `buffer(16)`, slot 1 -> `buffer(17)`. Per-vertex
  streams are indexed by `[[vertex_id]]`, per-instance streams by `[[instance_id]]`.
  - For indexed draws Metal's `vertex_id` is the fetched index **plus `baseVertex`**, same as D3D12
    `SV_VertexID`/`BaseVertexLocation`. `instance_id` includes `baseInstance`, same as
    `StartInstanceLocation`.
  - The stride of a stream is `sizeof` of its struct below (the Rust `size_of`), never the sum of the
    fields the shader reads: shadow_depth reads only some members of the mesh vertex but the buffer
    stride is still 52 / 76 / 96.
- **Textures**: HLSL `tN` -> `[[texture(N)]]`, same N.
- **Samplers**: none are bound from Rust. Each is a program-scope `constexpr sampler` reproducing the
  D3D12 *static sampler* (all of them were static samplers in the root signatures).
- **Matrices**: column-major in memory, both languages. CPU memcpy unchanged, no transpose.
- **Function constants** (mesh fragment functions only): see below. They MUST all be supplied.
- Only fragment functions `mesh_fs`, `mesh_instanced_fs`, `mesh_skinned_fs` use the function
  constants. Creating a vertex function (or `mesh_vs` etc.) with or without constants is fine - they
  do not read them.

### Function constants (declared once in `common.h`)

| Index | Name | Type | Meaning (was `-D` in HLSL) |
|---|---|---|---|
| 0 | `MATERIAL_PBR` | bool | metallic-roughness BRDF; off = one Lambert lobe |
| 1 | `MATERIAL_NORMAL_MAP` | bool | normal-map fetch + tangent frame; off = geometric normal |
| 2 | `ENABLE_SHADOWS` | bool | all shadow lookups, contact-shadow upsample, atlas preview, cascade debug tint |

All eight combinations are valid (`MESH_FEATURES` in build.rs). Supply with `MTLFunctionConstantValues`
(`MTLDataTypeBool`, indices 0, 1, 2) to `makeFunction(name:constantValues:)`.

### Struct sizes (all `static_assert`ed in the MSL)

| MSL struct | Rust mirror | Bytes |
|---|---|---|
| `FrameConstants` (scene_frame.h) | `scene::FrameConstants` | 4064 |
| `ObjectConstants` (common.h) | `scene::ObjectConstants` | 96 |
| `MaterialConstants` (material.h) | `material::MaterialConstants` | 48 |
| `SkinPalette` (common.h) | 128 x `[f32;16]` bone matrices | 8192 |
| `ShadowViewConstants` (shadow_depth.metal) | `shadow::renderer::ShadowViewConstants` | 80 |
| `ParticleFrame` (particle.metal) | `particle_pipeline::ParticleConstants` | 96 |
| `UiFrame` (ui.metal) | `ui_pipeline::UiFrameConstants` | 32 |
| `MeshVertex` (common.h) | `mesh::MeshVertex` | 52 |
| `SkinnedVertex` (common.h) | `mesh::SkinnedVertex` | 76 |
| `InstanceTransform` (common.h) | `mesh_pipeline::InstanceTransform` | 96 |
| `ParticleInstance` (particle.metal) | `engine_render_data::ParticleInstance` | 36 |
| `UiVertex` (ui.metal) | `ui_pipeline::UiVertex` | 48 |
| `TriangleVertex` / `InstancedVertex` | `pipeline::Vertex` | 28 |
| `InstanceData` (instanced.metal) | `instancing::InstanceData` | 28 |

Where manual padding / packing was needed:

- `FrameConstants`: `camera_position`, `light_direction`, `light_color`, `ambient_color` are
  `packed_float3` followed by an explicit `float` (`frame_padding`, `light_intensity`,
  `ambient_intensity`, `frame_padding_2`) - HLSL packs float3+float in one 16-byte row; a bare MSL
  `float3` is 16 bytes and would break it. Everything else is `float4` / `float4x4` / arrays thereof
  (all 16-byte strided in MSL, same as the cbuffer), so no other padding. Offsets are listed in the
  struct comments and match `scene.rs`' `offset_of!` asserts (camera_position 64, light_direction 80,
  light_color 96, ambient_color 112, cascade_view_projection 128, cascade_uv_scale_bias 384,
  cascade_far 448, ..., light_shadow 2160, local_view_projection 2688, cluster_params 3968,
  camera_fade_params 4048, total 4064).
- `MaterialConstants.emissive`: `packed_float3` at 16, `roughness` at 28 (shares the row).
- `ParticleFrame`: `packed_float3` + `float` twice.
- All vertex / instance structs use `packed_*` members: MSL `float4`/`float3`/`ushort4` have 16/16/8-byte
  alignment and would pad `MeshVertex` (tangent at 32 is ok but the stride 52 is not a multiple of 16),
  `SkinnedVertex`, `ParticleInstance` (36) and the 28-byte triangle structs.
- `SkinnedVertex.joints`: the D3D12 element is `R16G16B16A16_UINT` at offset 52 (8 bytes, 4-byte
  aligned). Declared as `uint joints01` (52), `uint joints23` (56); joint0 = `joints01 & 0xFFFF`,
  joint1 = `joints01 >> 16`, joint2 = `joints23 & 0xFFFF`, joint3 = `joints23 >> 16` (little-endian).
- `MeshVertex.color` (`R8G8B8A8_UNORM` at 48) is a plain `uint`; no shader reads it (HLSL did not either).
  `unpack_unorm4x8_to_float` is the decoder if that changes.

How struct layouts were verified: each MSL struct was compared member by member against the Rust
`#[repr(C)]` struct, its `size_of`/`offset_of` assertions (`scene.rs` tests: FrameConstants 4064,
ObjectConstants 96; `material.rs` test: MaterialConstants 48; `mesh_pipeline.rs` tests: MeshVertex 52,
SkinnedVertex 76, InstanceTransform 96; `particle_pipeline.rs`: ParticleConstants 96, particle record 36;
`ui_pipeline.rs`: UiVertex 48 / `UiFrameConstants` 32) and the D3D12 `AlignedByteOffset`s in
`mesh_vertex_elements`, `skinned_vertex_elements`, `instanced_vertex_elements`, `particle_pipeline::input_elements`,
`ui_pipeline::input_elements`, `pipeline.rs`, `instancing.rs`. The MSL offsets were summed by hand
and the `static_assert(sizeof(..))` lines will catch drift at Metal compile time.

## Samplers (all `constexpr`, program scope)

| MSL sampler | HLSL | D3D12 static sampler it reproduces | MSL description |
|---|---|---|---|
| `base_color_sampler` (material.h) | `s0` mesh/shadow-alpha | `mesh_pipeline::static_sampler`: `FILTER_ANISOTROPIC`, WRAP x3, MaxAniso 8, bias 0, MinLOD 0, MaxLOD FLT_MAX, ALWAYS | `address::repeat`, min/mag/mip `linear`, `max_anisotropy(8)`, `lod_clamp(0, MAXFLOAT)` |
| `shadow_sampler` (scene_frame.h) | `s1` mesh | `mesh_pipeline::shadow_sampler`: `COMPARISON_MIN_MAG_LINEAR_MIP_POINT`, BORDER x3, opaque white, `LESS_EQUAL`, MinLOD 0, MaxLOD FLT_MAX | `clamp_to_border`, `border_color::opaque_white`, min/mag `linear`, mip `nearest`, `compare_func::less_equal`, `lod_clamp(0, MAXFLOAT)` |
| `depth_sampler` (scene_frame.h) | `s2` mesh + contact | `mesh_pipeline::depth_sampler`: `MIN_MAG_MIP_POINT`, CLAMP x3, ALWAYS | `clamp_to_edge`, min/mag/mip `nearest`, `lod_clamp(0, MAXFLOAT)` |
| `glyph_sampler` (ui.metal) | `s0` ui | `ui_pipeline` `sampler(MIN_MAG_MIP_POINT)`: CLAMP x3, MinLOD 0, MaxLOD 0 | `clamp_to_edge`, `nearest` x3, `lod_clamp(0, 0)` |
| `icon_sampler` (ui.metal) | `s1` ui | `sampler(MIN_MAG_MIP_LINEAR)`: CLAMP x3, MinLOD 0, MaxLOD 0 | `clamp_to_edge`, `linear` x3, `lod_clamp(0, 0)` |
| `map_sampler` (ui.metal) | `s2` ui | `map_sampler`: `MIN_MAG_MIP_LINEAR`, BORDER x3, transparent black, MinLOD 0, MaxLOD f32::MAX | `clamp_to_border`, `border_color::transparent_black`, `linear` x3, `lod_clamp(0, MAXFLOAT)` |

All were true static samplers (no dynamic descriptor-heap samplers). `clamp_to_border` needs an Apple
GPU Mac (supported on Apple silicon).

## Entry points

For every fragment/vertex function: "Used" lists only resources the function actually references;
the Rust side only has to bind those (binding extras is harmless). No function writes depth
(`[[depth]]`) or any other builtin output. Discard (`discard_fragment()`) is used by
`mesh_fs`, `mesh_instanced_fs`, `mesh_skinned_fs` (dither fade, alpha cutoff) and `shadow_depth_alpha_fs`
(alpha cutoff) only.

### triangle.metal

| | `triangle_vs` (VSMain) | `triangle_fs` (PSMain) |
|---|---|---|
| buffer(16) | `TriangleVertex[]`, 28 B stride, per-vertex | - |
| Stage-in | `[[vertex_id]]` | `TriangleV2F` {position, color} |
| Output | position + color | `float4` colour(0) |
| Depth/discard | none | none |

`TriangleVertex`: `position` `packed_float3` @0 (R32G32B32_FLOAT), `color` `packed_float4` @12
(R32G32B32A32_FLOAT).

### instanced.metal

| | `instanced_vs` (VSMain) | `instanced_fs` (PSMain) |
|---|---|---|
| buffer(16) | `InstancedVertex[]` 28 B, per-vertex, `[[vertex_id]]` | - |
| buffer(17) | `InstanceData[]` 28 B, per-instance, `[[instance_id]]` | - |
| Depth/discard | none | none |

`InstancedVertex`: `position` `packed_float3` @0 R32G32B32_FLOAT, `color` `packed_float4` @12
R32G32B32A32_FLOAT. `InstanceData`: `transform` `packed_float3` @0 R32G32B32_FLOAT (xy offset, z scale),
`instance_color` `packed_float4` @12 R32G32B32A32_FLOAT. Instance step rate 1.

### mesh.metal

| | `mesh_vs` (VSMain) | `mesh_fs` (PSMain) |
|---|---|---|
| buffer(0) | `FrameConstants` 4064 B (only `view_projection` read) | `FrameConstants` 4064 B |
| buffer(1) | `ObjectConstants` 96 B | `ObjectConstants` 96 B (tint/emissive/shadow/fade fields) |
| buffer(2) | - | `MaterialConstants` 48 B |
| buffer(4) | - | `const device uint*` cluster light mask: `CLUSTERS_X*Y*Z = 16*9*24 = 3456` uints (13824 B). Index = cluster id; bit i = local light i. (HLSL `ByteAddressBuffer` t6.) |
| buffer(16) | `MeshVertex[]` 52 B per-vertex | - |
| texture(0) | - | `texture2d<float>` base colour (sRGB view) |
| texture(1) | - | `texture2d<float>` normal map (read only when `MATERIAL_NORMAL_MAP`) |
| texture(2) | - | `texture2d<float>` ORM |
| texture(3) | - | `texture2d<float>` emissive (sRGB view) |
| texture(4) | - | `depth2d<float>` shadow atlas (Depth32Float, sampled with `shadow_sampler` compare, and `depth_sampler` for the debug preview) |
| texture(5) | - | **not declared** (HLSL `scene_depth` is only read by the contact march, which lives in contact.metal) |
| texture(7) | - | `texture2d<float>` contact-shadow target (R16G16_FLOAT, `.rg`), gathered with `depth_sampler` |
| function constants | none | 0/1/2 (above) |
| Depth/discard | none | `discard_fragment()` (fade, alpha cutoff) |

Vertex `MeshVertex` (D3D12 `mesh_vertex_elements`, slot 0, per-vertex):

| Field | Offset | MSL | DXGI | Semantic |
|---|---|---|---|---|
| `position` | 0 | `packed_float3` | R32G32B32_FLOAT | POSITION |
| `normal` | 12 | `packed_float3` | R32G32B32_FLOAT | NORMAL |
| `uv` | 24 | `packed_float2` | R32G32_FLOAT | TEXCOORD0 |
| `tangent` | 32 | `packed_float4` | R32G32B32A32_FLOAT | TANGENT (xyz dir, w handedness) |
| `color` | 48 | `uint` | R8G8B8A8_UNORM | COLOR (unread) |

Vertex output `MeshV2F`: `position [[position]]`, `world_position`, `world_normal`, `world_tangent`
(float4), `uv`. The same struct is the fragment `[[stage_in]]`. Target: colour(0), straight `float4`
(blend state is a pipeline matter; unchanged from D3D12).

### mesh_instanced.metal

| | `mesh_instanced_vs` (VSMain) | `mesh_instanced_fs` (PSMain) |
|---|---|---|
| buffer(0) | `FrameConstants` (only `view_projection`) | `FrameConstants` |
| buffer(2) | - | `MaterialConstants` |
| buffer(4) | - | cluster mask (as mesh_fs) |
| buffer(16) | `MeshVertex[]` 52 B, per-vertex, `[[vertex_id]]` | - |
| buffer(17) | `InstanceTransform[]` 96 B, per-instance, `[[instance_id]]` | - |
| texture(0..4, 7) | - | as mesh_fs |
| function constants | none | 0/1/2 |
| Depth/discard | none | `discard_fragment()` |

No Object buffer (buffer 1 is NOT used). `InstanceTransform` (D3D12 `instanced_vertex_elements`, slot 1,
per-instance, step rate 1): `model0..model3` `packed_float4` @0/16/32/48 (R32G32B32A32_FLOAT,
INSTANCE_MODEL0..3; the four **columns** of the local-to-world matrix), `tint` `packed_float4` @64
(INSTANCE_TINT), `emissive_scale` `packed_float4` @80 (INSTANCE_EMISSIVE: x emissive add, y receives
shadow, z fade, w see-through). Vertex output `MeshInstancedV2F` = `MeshV2F` + `tint` (interpolated) +
`emissive_scale` (`[[flat]]`, HLSL `nointerpolation`).

### mesh_skinned.metal

| | `mesh_skinned_vs` (VSMain) | `mesh_skinned_fs` (PSMain) |
|---|---|---|
| buffer(0) | `FrameConstants` (only `view_projection`) | `FrameConstants` |
| buffer(1) | `ObjectConstants` (model) | `ObjectConstants` |
| buffer(2) | - | `MaterialConstants` |
| buffer(3) | `SkinPalette` 8192 B (128 x float4x4) | - |
| buffer(4) | - | cluster mask |
| buffer(16) | `SkinnedVertex[]` 76 B, per-vertex | - |
| texture(0..4, 7) | - | as mesh_fs |
| function constants | none | 0/1/2 |
| Depth/discard | none | `discard_fragment()` |

`SkinnedVertex` (D3D12 `skinned_vertex_elements`, slot 0): the five `MeshVertex` fields at the same offsets
(0/12/24/32/48), then `joints01` `uint` @52 and `joints23` `uint` @56 (together R16G16B16A16_UINT
BLENDINDICES0, 4 x u16), `weights` `packed_float4` @60 (R32G32B32A32_FLOAT BLENDWEIGHT0). Output struct
`MeshV2F` (as mesh.metal).

### shadow_depth.metal

All three entry points are for depth-only pipelines. Render pipeline for opaque casters: vertex function
only, **no fragment function**. Alpha-tested casters add `shadow_depth_alpha_fs`. (`shadow_depth_alpha_fs`
returns `void`; the pipeline has no colour attachment.)

| | `shadow_depth_instanced_vs` (VSInstanced) | `shadow_depth_skinned_vs` (VSSkinned) | `shadow_depth_alpha_fs` (PSAlpha) |
|---|---|---|---|
| buffer(0) | `ShadowViewConstants` 80 B (light view-projection @0, `shadow_view_params` float4 @64) | `ShadowViewConstants` 80 B | - |
| buffer(1) | - | `ObjectConstants` 96 B (`model`) | - |
| buffer(2) | - | - | `MaterialConstants` 48 B (`base_color.a`, `alpha_cutoff`) |
| buffer(3) | - | `SkinPalette` 8192 B | - |
| buffer(16) | `MeshVertex[]` 52 B, per-vertex (reads position, uv) | `SkinnedVertex[]` 76 B, per-vertex (reads position, uv, joints, weights) | - |
| buffer(17) | `InstanceTransform[]` 96 B, per-instance (reads model0..3) | - | - |
| texture(0) | - | - | `texture2d<float>` base colour (alpha read) |
| Output | `DepthV2F` {position, uv} | `DepthV2F` | none |
| Depth/discard | none | none | `discard_fragment()` when `alpha < alpha_cutoff` |

Same vertex layouts as above (the D3D12 shadow pipeline reuses `instanced_vertex_elements` /
`skinned_vertex_elements` verbatim, so buffer strides are 52+96 / 76).

### contact.metal

| | `contact_vs` (VSMain) | `contact_fs` (PSMain) |
|---|---|---|
| Vertex input | none: `[[vertex_id]]` only; draw 3 vertices, vertexStart 0, no buffers | `ContactV2F` {position, uv} |
| buffer(0) | - | `FrameConstants` 4064 B (view_projection, inverse_view_projection, camera_position, light_direction, camera_params, contact_params) |
| texture(5) | - | `depth2d<float>` scene depth (Depth32Float; sampled with `depth_sampler`, level 0) |
| Output | `position [[position]]`, `uv` | `float2` -> colour(0), an RG16Float target: r = shadow factor, g = view distance |
| Depth/discard | none | none |

### particle.metal

Particles have **no per-vertex stream**: the D3D12 layout is a single input slot 0 with
`InputSlotClass = PER_INSTANCE_DATA`, step rate 1. By the `buffer(16 + slot)` rule the record stream is
**buffer(16)** (not 17), indexed by `[[instance_id]]`; the quad corner comes from `[[vertex_id]]`
(0..3). Draw: triangle strip, 4 vertices, `particle_count` instances, vertexStart 0.

| | `particle_vs` (VSMain) | `particle_fs` (PSMain) |
|---|---|---|
| buffer(0) | `ParticleFrame` 96 B (view_projection @0, camera_right @64, camera_up @80) | - |
| buffer(16) | `ParticleInstance[]` 36 B, per-instance, `[[instance_id]]` | - |
| Output | `ParticleV2F` {position, corner, color} | `float4` colour(0), premultiplied alpha |
| Depth/discard | none | none |

`ParticleInstance`: `center` `packed_float3` @0 (R32G32B32_FLOAT POSITION), `size` `float` @12 (R32_FLOAT
TEXCOORD0), `color` `packed_float4` @16 (R32G32B32A32_FLOAT COLOR), `rotation` `float` @32 (R32_FLOAT TEXCOORD1).

### ui.metal

| | `ui_vs` (VSMain) | `ui_fs` (PSMain) |
|---|---|---|
| buffer(0) | `UiFrame` 32 B | `UiFrame` 32 B (`icon_band_v`, `font_atlas_size`, `font_range`) |
| buffer(16) | `UiVertex[]` 48 B per-vertex | - |
| texture(0) | - | `texture2d<float>` UI atlas |
| texture(1) | - | `texture2d<float>` map picture (mipmapped) |
| texture(2) | - | `texture2d<float>` font distance field (alpha used) |
| Output | `UiV2F` {position, uv, color, clip `[[flat]]`} | `float4` colour(0) |
| Depth/discard | none | none |

`UiVertex` (D3D12 `ui_pipeline::input_elements`, slot 0): `position` `packed_float2` @0 R32G32_FLOAT,
`uv` `packed_float2` @8 R32G32_FLOAT (TEXCOORD0), `color` `packed_float4` @16 R32G32B32A32_FLOAT,
`clip` `packed_float4` @32 R32G32B32A32_FLOAT (TEXCOORD1).

## Semantic differences / things the Rust side should know

1. **Fragment `[[position]]`** = HLSL `SV_Position` in framebuffer pixels, origin top-left, pixel centres at
   .5. Clip-space z is [0,1], y up, texture origin top-left in both APIs, so no projection changes.
2. **`ddx_fine`/`ddy_fine` (contact_fs) -> `dfdx`/`dfdy`.** Metal has no fine/coarse split. On Apple GPUs
   derivatives are per 2x2 quad. The HLSL code chose fine for quality at half resolution; the contact
   normal may be slightly different on curved surfaces. Visual-parity item to check.
3. **Derivatives after an early return**: `contact_fs` computes `dfdx` after a divergent early `return` for
   sky pixels (same in HLSL). Undefined by the spec in both APIs; kept 1:1.
4. **`discard`**: HLSL ends the invocation; `discard_fragment()` lets the lane keep running (output is
   suppressed). No shader writes memory, so this is unobservable.
5. **`SampleCmpLevelZero`** -> `sample_compare(..., level(0))`. Both use `LESS_EQUAL` (reference <= stored).
6. **Gather** order `contact_shadows.gather(...)` is (0,1)(1,1)(1,0)(0,0) in x,y,z,w in both APIs.
7. HLSL `[unroll]`/`[loop]` -> `#pragma unroll` on the constant-trip loops; `[loop]` dropped.
8. HLSL `bool` in constant buffers is never used; the `receives_shadow` etc. flags are floats (unchanged).
9. `SceneResources` (scene_frame.h) is a function-local aggregate containing pointers and
   texture/`depth2d` members, built at the top of each mesh fragment function and passed by `const thread&`.
   If the Metal compiler rejects textures as members of a local struct, replace its textures by explicit
   function parameters - the call graph is otherwise unchanged.
