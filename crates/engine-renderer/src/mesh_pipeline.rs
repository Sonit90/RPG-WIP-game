//! The D3D12 objects that draw [`crate::mesh::MeshVertex`] geometry: root signature and
//! pipeline states.
//!
//! Split out of `scene.rs` at the Milestone 15 checkpoint. What used to share that file —
//! `RenderWorld`, the handle types, `Lighting` — is pure data and moved to
//! `engine-render-data`; this is everything in the old file that genuinely needs a device.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_DEPTH_STENCIL_DESC, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_ROOT_DESCRIPTOR,
    D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_CBV,
    D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    D3D12_SHADER_VISIBILITY_ALL, D3D12SerializeRootSignature, ID3D12Device, ID3D12PipelineState,
    ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::depth::DEPTH_FORMAT;
use crate::error::RendererError;
use crate::mesh::MeshVertex;
use crate::pipeline::{blend_desc, depth_stencil_desc, rasterizer_desc, wireframe_rasterizer_desc};
use crate::scene::ROOT_PARAMETER_REGISTERS;
use crate::shader::{MESH_PS, MESH_VS, bytecode};
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

        let input_elements = [
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
        ];

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

/// Creates the three-constant-buffer root signature the mesh shader expects.
///
/// Root CBVs rather than descriptor tables: each is a GPU address written straight into the
/// root arguments, with no descriptor to allocate and no table to bind. That is the cheapest
/// binding D3D12 offers, and it is available precisely because nothing here samples a
/// texture yet.
///
/// The parameter order comes from [`ROOT_PARAMETER_REGISTERS`] rather than being written
/// out: root slots are ordered by update frequency and shader registers are ordered by
/// declaration, the two disagree, and having the mapping in one named place is what makes
/// it testable.
fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, RendererError> {
    let parameters = root_parameters();

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        ..Default::default()
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
