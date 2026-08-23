//! The D3D12 objects that draw [`crate::mesh::MeshVertex`] geometry: root signature and
//! pipeline states.
//!
//! Split out of `scene.rs` at the Milestone 15 checkpoint. What used to share that file —
//! `RenderWorld`, the handle types, `Lighting` — is pure data and moved to
//! `engine-render-data`; this is everything in the old file that genuinely needs a device.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_COMPARISON_FUNC_ALWAYS, D3D12_COMPARISON_FUNC_LESS_EQUAL,
    D3D12_DEPTH_STENCIL_DESC, D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    D3D12_DESCRIPTOR_RANGE_TYPE_SRV, D3D12_FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT,
    D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_FLOAT32_MAX,
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_ROOT_DESCRIPTOR,
    D3D12_ROOT_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0,
    D3D12_ROOT_PARAMETER_TYPE_CBV, D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
    D3D12_ROOT_PARAMETER_TYPE_SRV,
    D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    D3D12_SHADER_VISIBILITY_ALL, D3D12_SHADER_VISIBILITY_PIXEL, D3D12_SHADER_VISIBILITY_VERTEX,
    D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK, D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
    D3D12_STATIC_SAMPLER_DESC, D3D12_TEXTURE_ADDRESS_MODE_BORDER, D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
    D3D12_TEXTURE_ADDRESS_MODE_WRAP, D3D12SerializeRootSignature, ID3D12Device,
    ID3D12PipelineState, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R16G16B16A16_UINT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT,
    DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::depth::DEPTH_FORMAT;
use crate::error::RendererError;
use crate::mesh::{MeshVertex, SkinnedVertex};
use crate::pipeline::{blend_desc, depth_stencil_desc, rasterizer_desc, wireframe_rasterizer_desc};
use crate::scene::{ROOT_PARAMETER_REGISTERS, ROOT_SKIN_CBV};
use crate::shader::{
    MESH_INSTANCED_PS, MESH_INSTANCED_VS, MESH_PS, MESH_SKINNED_PS, MESH_SKINNED_VS, MESH_VS,
    bytecode,
};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// Root signature and pipeline state for [`MeshVertex`] geometry.
pub struct MeshPipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl MeshPipeline {
    /// Creates the solid-fill root signature and pipeline state from the embedded DXIL.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, rasterizer_desc())
    }

    /// Creates the same pipeline with wireframe fill.
    ///
    /// A separate pipeline state rather than a dynamic state: D3D12 bakes the fill mode
    /// into the PSO, so a wireframe toggle is a choice between two objects created at
    /// startup, not a per-frame state change.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn wireframe(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, wireframe_rasterizer_desc())
    }

    /// Creates the pipeline with an explicit rasterizer state.
    fn with_rasterizer(
        device: &ID3D12Device,
        rasterizer: D3D12_RASTERIZER_DESC,
    ) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;

        let input_elements = mesh_vertex_elements();

        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            // SAFETY: copies the interface pointer without changing its refcount; the copy
            // lives only for this call and the real reference is owned here.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(MESH_VS),
            PS: bytecode(MESH_PS),
            RasterizerState: rasterizer,
            BlendState: blend_desc(),
            DepthStencilState: depth_stencil_desc(),
            DSVFormat: DEPTH_FORMAT,
            SampleMask: u32::MAX,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        desc.RTVFormats[0] = BACK_BUFFER_FORMAT;
        debug_assert_eq!(
            size_of::<MeshVertex>(),
            32,
            "the input layout offsets above describe a 32-byte vertex"
        );
        debug_assert_ne!(
            desc.DepthStencilState,
            D3D12_DEPTH_STENCIL_DESC::default(),
            "the scene pipeline must test depth"
        );

        // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode
        // and input-element array outlive the call.
        let pipeline_state =
            unsafe { device.CreateGraphicsPipelineState(&desc) }.map_err(|error| {
                RendererError::api("ID3D12Device::CreateGraphicsPipelineState", error)
            })?;
        // `desc.pRootSignature` is a borrowed copy in a ManuallyDrop; leaving it alone is
        // what keeps it from releasing a reference it never took.

        Ok(Self {
            root_signature,
            pipeline_state,
        })
    }

    /// The root signature to bind before drawing.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state to bind before drawing.
    #[must_use]
    pub fn pipeline_state(&self) -> &ID3D12PipelineState {
        &self.pipeline_state
    }
}

/// Creates the root signature every mesh pipeline — rigid, instanced, skinned, and the
/// depth-only shadow pipelines — shares.
///
/// ```text
/// slot 0  CBV b0   frame constants        changes once per frame
/// slot 1  CBV b2   material constants     changes per batch
/// slot 2  CBV b1   object constants       changes per draw (unused when instancing)
/// slot 3  table t0 base colour map        changes per batch
/// slot 4  CBV b3   bone palette           changes per skinned character
/// slot 5  table t1 shadow atlas           changes once per frame
/// slot 6  table t2 scene depth            changes once per frame
/// slot 7  SRV   t3 clustered light grid   changes once per frame
/// static samplers s0 (base colour), s1 (shadow comparison), s2 (depth point)
/// ```
///
/// The two shadow tables are one descriptor each rather than a two-descriptor range,
/// deliberately: a range has to be contiguous in the heap, and the two SRVs are allocated
/// independently from the persistent region beside every material's map. Two root parameters
/// cost two DWORDs of the sixty-four available and remove the need for a range allocator
/// that would exist for exactly this pair.
///
/// Constant buffers are **root** CBVs: a GPU address written straight into the root
/// arguments, with no descriptor to allocate and no table to bind. That is the cheapest
/// binding D3D12 offers. The texture cannot be one — a root descriptor addresses a buffer,
/// not a texture — so it is a one-descriptor table into the persistent region of the shared
/// descriptor heap.
///
/// The sampler is **static**, declared in the signature rather than allocated: it never
/// changes, so there is nothing to bind per material and no sampler heap at all.
///
/// The CBV parameter order comes from [`ROOT_PARAMETER_REGISTERS`] rather than being written
/// out: root slots are ordered by update frequency and shader registers are ordered by
/// declaration, the two disagree, and having the mapping in one named place is what makes
/// it testable.
pub(crate) fn create_root_signature(
    device: &ID3D12Device,
) -> Result<ID3D12RootSignature, RendererError> {
    // Declared before `parameters` so it outlives the pointer the table parameter holds
    // into it. A range that went out of scope first would serialize garbage, and the failure
    // is a driver-side E_INVALIDARG that names nothing.
    let ranges = [D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    }];

    let mut parameters = Vec::with_capacity(8);
    parameters.extend_from_slice(&root_parameters());
    parameters.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: ranges.len() as u32,
                pDescriptorRanges: ranges.as_ptr(),
            },
        },
        // Pixel-only: the vertex shader never samples, and restricting visibility is free.
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    });
    parameters.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: SKIN_SHADER_REGISTER,
                RegisterSpace: 0,
            },
        },
        // Vertex-only: skinning happens before rasterization and nowhere else.
        ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
    });
    debug_assert_eq!(
        parameters.len() as u32,
        ROOT_SKIN_CBV + 1,
        "the ROOT_* constants index this array"
    );

    // The shadow atlas, at t1. Its own table because it changes once per frame while the
    // material's map changes per batch.
    let shadow_range = [D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 1,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    }];
    let depth_range = [D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 2,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    }];
    for range in [&shadow_range, &depth_range] {
        parameters.push(D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: range.len() as u32,
                    pDescriptorRanges: range.as_ptr(),
                },
            },
            // Pixel-only: shadows are sampled while shading, never while transforming.
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        });
    }
    debug_assert_eq!(
        parameters.len() as u32,
        crate::scene::ROOT_DEPTH_TABLE + 1,
        "the ROOT_* constants index this array"
    );

    // The clustered light grid, at t3. A **root** SRV rather than a table: it is a raw
    // buffer, and a raw buffer can be addressed by a GPU virtual address written straight
    // into the root arguments. No descriptor, no heap slot, and one bind per frame.
    //
    // It is deliberately not in the frame constant buffer, where it started: 13.5 KB of
    // masks took that buffer from 2.4 KB to 17.8 KB and made every read of it slower, which
    // cost more than the lighting the grid saves. See light_cluster.rs.
    parameters.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: CLUSTER_SHADER_REGISTER,
                RegisterSpace: 0,
            },
        },
        // Pixel-only: lights are gathered while shading.
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    });
    debug_assert_eq!(
        parameters.len() as u32,
        crate::scene::ROOT_CLUSTER_SRV + 1,
        "the ROOT_* constants index this array"
    );

    let samplers = [static_sampler(), shadow_sampler(), depth_sampler()];

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: FFI call. `desc` and the parameter array outlive the call, and both
    // out-parameters are valid.
    let result = unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        return Err(RendererError::api("D3D12SerializeRootSignature", error));
    }

    let Some(blob) = blob else {
        return Err(RendererError::InvalidConfig {
            reason: "root signature serialization produced no blob".to_owned(),
        });
    };

    // SAFETY: FFI calls reading the serialized blob's buffer, which stays alive for the
    // duration of the CreateRootSignature call below.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    // SAFETY: FFI call. `bytes` points into the live blob and node mask 0 targets the
    // single-adapter case.
    unsafe { device.CreateRootSignature(0, bytes) }
        .map_err(|error| RendererError::api("ID3D12Device::CreateRootSignature", error))
}

/// The root parameters, in slot order, each a CBV at the register the mapping names.
///
/// Split out of [`create_root_signature`] so the regression test can inspect the array
/// without a device.
#[must_use]
pub(crate) fn root_parameters() -> [D3D12_ROOT_PARAMETER; ROOT_PARAMETER_REGISTERS.len()] {
    ROOT_PARAMETER_REGISTERS.map(|shader_register| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: shader_register,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    })
}

/// Shader register the bone palette binds to, matching `cbuffer Skin : register(b3)`.
const SKIN_SHADER_REGISTER: u32 = 3;

/// Shader register the light grid binds to, matching `ByteAddressBuffer ... : register(t3)`.
const CLUSTER_SHADER_REGISTER: u32 = 3;

/// The sampler every shadow lookup goes through, at `s1`.
///
/// A **comparison** sampler: the hardware performs the depth test *and* the bilinear filter
/// in one fetch, which is what makes a 3x3 PCF kernel nine taps instead of thirty-six. The
/// comparison is less-or-equal, so a receiver at exactly the depth the caster wrote counts as
/// lit — the opposite would make every surface shadow itself.
///
/// Border addressing with a white border: a coordinate that escapes the atlas reads "nothing
/// in the way" and the surface is lit. The sampling code clamps to the tile before it gets
/// there, so this is the second line of defence rather than the first.
fn shadow_sampler() -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: 1,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    }
}

/// The sampler the contact-shadow march reads scene depth through, at `s2`.
///
/// Point-filtered and clamped: a depth buffer holds distances, and the average of two
/// distances is a surface that is not there.
fn depth_sampler() -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: 2,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    }
}

/// The one sampler every material's base colour map is read through.
///
/// Trilinear with anisotropy would be the next step and needs a mip chain, which no texture
/// in this phase has. Wrapping rather than clamping because a character's UVs run across a
/// shared texture and a prism's cap lands exactly on a boundary; clamping would smear the
/// edge texel across a face.
fn static_sampler() -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    }
}

/// Root signature and pipeline state for skinned geometry.
///
/// The same root signature as the rigid pipelines — it is one signature with the palette slot
/// the others leave unbound — and a different input layout, a different vertex shader and the
/// same pixel shader arithmetic.
pub struct SkinnedMeshPipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl SkinnedMeshPipeline {
    /// Creates the root signature and pipeline state from the embedded DXIL.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, rasterizer_desc())
    }

    /// The same pipeline with wireframe fill.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn wireframe(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, wireframe_rasterizer_desc())
    }

    /// Creates the pipeline with an explicit rasterizer state.
    fn with_rasterizer(
        device: &ID3D12Device,
        rasterizer: D3D12_RASTERIZER_DESC,
    ) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;
        let input_elements = skinned_vertex_elements();

        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            // SAFETY: copies the interface pointer without changing its refcount; the copy
            // lives only for this call and the real reference is owned here.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(MESH_SKINNED_VS),
            PS: bytecode(MESH_SKINNED_PS),
            RasterizerState: rasterizer,
            BlendState: blend_desc(),
            DepthStencilState: depth_stencil_desc(),
            DSVFormat: DEPTH_FORMAT,
            SampleMask: u32::MAX,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        desc.RTVFormats[0] = BACK_BUFFER_FORMAT;

        // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode
        // and input-element array outlive the call.
        let pipeline_state =
            unsafe { device.CreateGraphicsPipelineState(&desc) }.map_err(|error| {
                RendererError::api("ID3D12Device::CreateGraphicsPipelineState", error)
            })?;

        Ok(Self {
            root_signature,
            pipeline_state,
        })
    }

    /// The root signature to bind before drawing.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state to bind before drawing.
    #[must_use]
    pub fn pipeline_state(&self) -> &ID3D12PipelineState {
        &self.pipeline_state
    }
}

/// The input elements a [`SkinnedVertex`] presents.
///
/// The first three are byte-for-byte the rigid layout, so a skinned mesh and a rigid one
/// share their position, normal and UV offsets and a change to one is a change to both.
pub(crate) fn skinned_vertex_elements() -> [D3D12_INPUT_ELEMENT_DESC; 5] {
    debug_assert_eq!(
        size_of::<SkinnedVertex>(),
        56,
        "the offsets below describe a 56-byte vertex"
    );
    let rigid = mesh_vertex_elements();
    [
        rigid[0],
        rigid[1],
        rigid[2],
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("BLENDINDICES"),
            SemanticIndex: 0,
            // Four 16-bit unsigned integers, read into the shader's `uint4`. A float format
            // here would silently convert and index the palette with a rounded value.
            Format: DXGI_FORMAT_R16G16B16A16_UINT,
            InputSlot: 0,
            AlignedByteOffset: 32,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("BLENDWEIGHT"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 40,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ]
}

/// The input elements the instanced pipelines consume: the rigid vertex, then the
/// per-instance stream.
///
/// Shared with the depth-only shadow pipeline, which draws the same batches through the same
/// stream — a shadow map of geometry laid out differently from the picture would be a shadow
/// of something else.
pub(crate) fn instanced_vertex_elements() -> Vec<D3D12_INPUT_ELEMENT_DESC> {
    let mut elements = Vec::with_capacity(9);
    elements.extend_from_slice(&mesh_vertex_elements());
    // Slot 1, stepped once per instance: four float4 rows of the model matrix.
    for (row, offset) in [0_u32, 16, 32, 48].into_iter().enumerate() {
        elements.push(D3D12_INPUT_ELEMENT_DESC {
            // One semantic, four indices. HLSL parses the trailing digits of
            // `INSTANCE_MODEL0`..`INSTANCE_MODEL3` as the semantic *index*, so the shader
            // declares one name four times over — and a layout that spelled out four
            // distinct names matches nothing and fails PSO creation with a bare
            // E_INVALIDARG that names neither the element nor the reason.
            SemanticName: INSTANCE_MODEL_SEMANTIC,
            SemanticIndex: row as u32,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 1,
            AlignedByteOffset: offset,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
            // One step per instance. A rate of zero here would make every instance read the
            // first matrix, which renders the whole batch stacked at one place.
            InstanceDataStepRate: 1,
        });
    }
    // Per-instance visual state, in the same stream after the matrix: a colour multiply and
    // an emissive add. Two more elements rather than a second vertex buffer, so a styled
    // batch binds exactly what an unstyled one does.
    elements.push(D3D12_INPUT_ELEMENT_DESC {
        SemanticName: INSTANCE_TINT_SEMANTIC,
        SemanticIndex: 0,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        InputSlot: 1,
        AlignedByteOffset: 64,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    });
    elements.push(D3D12_INPUT_ELEMENT_DESC {
        SemanticName: INSTANCE_EMISSIVE_SEMANTIC,
        SemanticIndex: 0,
        // Two floats, not one: the emissive add, and beside it the per-instance "does this
        // receive shadows" flag. The second component reads a word that was already padding
        // in the instance record, so the stream did not grow.
        Format: DXGI_FORMAT_R32G32_FLOAT,
        InputSlot: 1,
        AlignedByteOffset: 80,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    });
    elements
}

/// One instance's local-to-world matrix, as the four rows the shader's vertex elements
/// declare.
///
/// `repr(C)`: the input layout below describes this exact memory, so Rust must not be free
/// to reorder it. Rows rather than columns because a vertex element is at most a `float4` —
/// and because [`Mat4`](engine_core::math::Mat4) stores columns, so writing them out as
/// rows is the transpose the shader would otherwise have to do, performed once by a memcpy
/// that was happening anyway.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct InstanceTransform {
    /// The matrix, row-major, sixteen floats.
    pub model: [f32; 16],
    /// Multiplied with the material's base colour.
    ///
    /// Per instance rather than per material, which is the whole point: two dummies with
    /// different tints still share one batch and one draw. A material change would split
    /// them.
    pub tint: [f32; 4],
    /// Added to the material's emissive term.
    pub emissive: f32,
    /// 1 when the lighting pass may darken this instance where a shadow falls on it.
    ///
    /// The second component of the `INSTANCE_EMISSIVE` element, which used to be padding.
    /// Per instance rather than per material for the same reason the tint is: two copies of
    /// one prop can disagree about it without splitting the batch.
    pub receives_shadow: f32,
    /// Padding to a 16-byte boundary, so the element offsets the input layout declares are
    /// the ones the struct actually has.
    pub padding: [f32; 2],
}

impl InstanceTransform {
    /// Builds the per-instance data for a world transform and an unstyled surface.
    #[must_use]
    pub fn new(model: engine_core::math::Mat4) -> Self {
        Self::styled(model, engine_render_data::InstanceStyle::NEUTRAL)
    }

    /// Builds the per-instance data for a world transform and a visual state.
    ///
    /// Receives shadows, which is what ordinary geometry does; a caller with an opinion uses
    /// [`InstanceTransform::shadowed`].
    #[must_use]
    pub fn styled(
        model: engine_core::math::Mat4,
        style: engine_render_data::InstanceStyle,
    ) -> Self {
        Self::shadowed(model, style, true)
    }

    /// Builds the per-instance data for a world transform, a visual state and a shadow flag.
    #[must_use]
    pub fn shadowed(
        model: engine_core::math::Mat4,
        style: engine_render_data::InstanceStyle,
        receives_shadow: bool,
    ) -> Self {
        // `to_cols_array` is the column-major layout; the shader declares `row_major` and
        // assembles the four elements back into a matrix, so the two cancel and no
        // transpose instruction is emitted anywhere.
        Self {
            model: model.to_cols_array(),
            tint: style.tint,
            emissive: style.emissive,
            receives_shadow: f32::from(u8::from(receives_shadow)),
            padding: [0.0; 2],
        }
    }
}

/// Root signature and pipeline state for instanced [`MeshVertex`] geometry.
///
/// The same shading as [`MeshPipeline`], with the model matrix arriving per instance
/// instead of per draw. That is what lets a run of objects sharing a mesh and a material
/// collapse into one `DrawIndexedInstanced`, removing both the draw and the per-object
/// constant-buffer bind.
pub struct InstancedMeshPipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl InstancedMeshPipeline {
    /// Creates the root signature and pipeline state from the embedded DXIL.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, rasterizer_desc())
    }

    /// The same pipeline with wireframe fill.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn wireframe(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, wireframe_rasterizer_desc())
    }

    /// Creates the pipeline with an explicit rasterizer state.
    fn with_rasterizer(
        device: &ID3D12Device,
        rasterizer: D3D12_RASTERIZER_DESC,
    ) -> Result<Self, RendererError> {
        // The same root signature as the non-instanced pipeline: same frame and material
        // registers, same slot order. The object slot simply goes unused, which costs two
        // DWORDs of root space and buys both pipelines one signature, one material upload
        // path and one set of `ROOT_*` constants to keep correct.
        let root_signature = create_root_signature(device)?;

        let input_elements = instanced_vertex_elements();

        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            // SAFETY: copies the interface pointer without changing its refcount; the copy
            // lives only for this call and the real reference is owned here.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(MESH_INSTANCED_VS),
            PS: bytecode(MESH_INSTANCED_PS),
            RasterizerState: rasterizer,
            BlendState: blend_desc(),
            DepthStencilState: depth_stencil_desc(),
            DSVFormat: DEPTH_FORMAT,
            SampleMask: u32::MAX,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        desc.RTVFormats[0] = BACK_BUFFER_FORMAT;
        debug_assert_eq!(
            size_of::<InstanceTransform>(),
            96,
            "the instance element offsets above describe a 64-byte matrix followed by a \
             16-byte tint and a 4-byte emissive"
        );

        // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode
        // and input-element array outlive the call.
        let pipeline_state =
            unsafe { device.CreateGraphicsPipelineState(&desc) }.map_err(|error| {
                RendererError::api("ID3D12Device::CreateGraphicsPipelineState", error)
            })?;

        Ok(Self {
            root_signature,
            pipeline_state,
        })
    }

    /// The root signature to bind before drawing.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state to bind before drawing.
    #[must_use]
    pub fn pipeline_state(&self) -> &ID3D12PipelineState {
        &self.pipeline_state
    }
}

/// Semantic name of the per-instance matrix rows, matching `mesh_instanced.hlsl`.
///
/// One name, used with indices 0 to 3. The shader spells them `INSTANCE_MODEL0` through
/// `INSTANCE_MODEL3`, and HLSL reads those trailing digits as the semantic index rather
/// than as part of the name.
const INSTANCE_MODEL_SEMANTIC: windows::core::PCSTR = s!("INSTANCE_MODEL");

/// Semantic for the per-instance colour multiply.
const INSTANCE_TINT_SEMANTIC: windows::core::PCSTR = s!("INSTANCE_TINT");

/// Semantic for the per-instance emissive add.
const INSTANCE_EMISSIVE_SEMANTIC: windows::core::PCSTR = s!("INSTANCE_EMISSIVE");

/// The per-vertex input elements both mesh pipelines consume.
///
/// Shared rather than written twice: the instanced pipeline appends per-instance elements
/// to exactly this layout, and two copies that had to agree would eventually not.
fn mesh_vertex_elements() -> [D3D12_INPUT_ELEMENT_DESC; 3] {
    debug_assert_eq!(
        size_of::<MeshVertex>(),
        32,
        "the offsets below describe a 32-byte vertex"
    );
    [
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("NORMAL"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 12,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("TEXCOORD"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 24,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::{Mat4, Vec3};

    #[test]
    fn an_instance_is_the_size_the_input_layout_describes() {
        // The matrix rows sit at 0, 16, 32 and 48, the tint at 64 and the emissive at 80.
        // A different size or offset means the elements read across each other and every
        // object renders sheared or mistinted.
        assert_eq!(size_of::<InstanceTransform>(), 96);
        assert_eq!(align_of::<InstanceTransform>(), 4);
        assert_eq!(std::mem::offset_of!(InstanceTransform, model), 0);
        assert_eq!(std::mem::offset_of!(InstanceTransform, tint), 64);
        assert_eq!(std::mem::offset_of!(InstanceTransform, emissive), 80);
    }

    #[test]
    fn an_unstyled_instance_carries_the_neutral_style() {
        // `new` is the "no visual state" constructor and must not leave the tint at zero,
        // which would render every object black.
        let instance = InstanceTransform::new(Mat4::IDENTITY);
        assert_eq!(instance.tint, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(instance.emissive, 0.0);
    }

    #[test]
    fn a_styled_instance_carries_the_style_it_was_given() {
        let style = engine_render_data::InstanceStyle::tinted(0.5, 0.4, 0.3).with_emissive(0.8);
        let instance = InstanceTransform::styled(Mat4::IDENTITY, style);
        assert_eq!(instance.tint, [0.5, 0.4, 0.3, 1.0]);
        assert_eq!(instance.emissive, 0.8);
    }

    #[test]
    fn the_instance_stream_is_the_matrix_untransposed() {
        // The convention the shader depends on, pinned here because getting it wrong is
        // invisible on the CPU and catastrophic on the GPU.
        //
        // `Mat4` is column-major and is memcpy'd in as-is, so element `i` of the stream is
        // *column* i. HLSL's float4x4(a, b, c, d) fills rows, so the shader reconstructs the
        // transpose — which is why it multiplies vector-first. If this ever became a
        // transpose here instead, the two would cancel wrongly and the scene would explode.
        let matrix = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let instance = InstanceTransform::new(matrix);
        assert_eq!(instance.model, matrix.to_cols_array());
        // The translation lives in the fourth column, which is elements 12..15 — i.e. the
        // fourth float4 the input layout reads, at byte offset 48.
        assert_eq!(&instance.model[12..15], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn the_instance_semantic_carries_no_trailing_digit() {
        // `INSTANCE_MODEL0`..`INSTANCE_MODEL3` in HLSL is one semantic named
        // `INSTANCE_MODEL` with indices 0 to 3 — the digits are the index, not the name.
        // A layout that spelled out four distinct names matched nothing and failed pipeline
        // creation with a bare E_INVALIDARG that named neither the element nor the reason.
        // SAFETY: the constant is built from a static null-terminated literal by `s!`.
        let name = unsafe { INSTANCE_MODEL_SEMANTIC.to_string() };
        let Ok(name) = name else {
            panic!("the semantic name must be valid UTF-8");
        };
        assert_eq!(name, "INSTANCE_MODEL");
        assert!(
            !name.ends_with(|c: char| c.is_ascii_digit()),
            "a trailing digit would be parsed as the semantic index"
        );
    }
}
