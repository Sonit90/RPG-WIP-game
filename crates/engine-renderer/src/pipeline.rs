//! Root signature and pipeline state for position + colour geometry.
//!
//! One pipeline: position + colour vertices, no constant buffers, no textures, no depth.
//! Materials and passes arrive with Milestones 11 and 12.
//!
//! The shaders are DXIL blobs compiled by `build.rs` and embedded by [`crate::shader`];
//! creating the pipeline state costs no compilation.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD,
    D3D12_BLEND_ZERO, D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_COMPARISON_FUNC_LESS,
    D3D12_CULL_MODE_BACK, D3D12_DEPTH_STENCIL_DESC, D3D12_DEPTH_WRITE_MASK_ALL,
    D3D12_FILL_MODE_SOLID, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_LOGIC_OP_NOOP, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC,
    D3D12_RENDER_TARGET_BLEND_DESC, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12SerializeRootSignature,
    ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::depth::DEPTH_FORMAT;
use crate::error::RendererError;
use crate::shader::{TRIANGLE_PS, TRIANGLE_VS, bytecode};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// One vertex: clip-space position and a colour.
///
/// `repr(C)` is required — the input layout below describes this exact memory layout to
/// the GPU, so Rust must not be free to reorder the fields.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// Clip-space position. Phase 0 has no camera, so vertices are already in clip space.
    pub position: [f32; 3],
    /// Linear RGBA colour, interpolated across the triangle.
    pub color: [f32; 4],
}

/// The triangle drawn by Milestone 4, in clip space and counter-clockwise.
///
/// Winding matters: the rasterizer culls back faces, and a clockwise triangle would be
/// invisible rather than obviously wrong.
pub const TRIANGLE_VERTICES: [Vertex; 3] = [
    Vertex {
        position: [0.0, 0.6, 0.0],
        color: [1.0, 0.2, 0.2, 1.0],
    },
    Vertex {
        position: [-0.6, -0.5, 0.0],
        color: [0.2, 1.0, 0.3, 1.0],
    },
    Vertex {
        position: [0.6, -0.5, 0.0],
        color: [0.3, 0.4, 1.0, 1.0],
    },
];

/// Root signature and pipeline state for position + colour geometry.
///
/// Holds no geometry of its own: meshes are separate ([`crate::mesh::GpuMesh`]) so the
/// scene can be swapped without rebuilding shaders, which is what the stress test needs.
pub struct ScenePipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl ScenePipeline {
    /// Creates the root signature and pipeline state from the embedded DXIL.
    ///
    /// The shaders were compiled by `build.rs`; nothing is compiled here.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;
        let pipeline_state = create_pipeline_state(device, &root_signature)?;

        log::info!(
            "scene pipeline ready: position + colour, shader model {}",
            crate::shader::SHADER_MODEL
        );

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

/// Creates an empty root signature.
///
/// The shaders take no constant buffers, textures or samplers, so the signature only has
/// to permit an input-assembler layout.
pub(crate) fn create_root_signature(
    device: &ID3D12Device,
) -> Result<ID3D12RootSignature, RendererError> {
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        ..Default::default()
    };

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: FFI call. `desc` is fully initialized and outlives the call; both
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

/// Creates the graphics pipeline state for the triangle.
fn create_pipeline_state(
    device: &ID3D12Device,
    root_signature: &ID3D12RootSignature,
) -> Result<ID3D12PipelineState, RendererError> {
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
            SemanticName: s!("COLOR"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: size_of::<[f32; 3]>() as u32,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ];

    let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: input_elements.as_ptr(),
            NumElements: input_elements.len() as u32,
        },
        // SAFETY: copies the interface pointer without changing its refcount. The copy
        // lives only for this call, and the real reference is owned by the caller.
        pRootSignature: unsafe { std::mem::transmute_copy(root_signature) },
        VS: bytecode(TRIANGLE_VS),
        PS: bytecode(TRIANGLE_PS),
        RasterizerState: rasterizer_desc(),
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

    // SAFETY: FFI call with a fully initialized descriptor whose referenced blobs and
    // input-element array outlive the call.
    let state = unsafe { device.CreateGraphicsPipelineState(&desc) }
        .map_err(|error| RendererError::api("ID3D12Device::CreateGraphicsPipelineState", error));

    // `desc.pRootSignature` is a non-owning copy of the interface pointer. `ManuallyDrop`
    // is what keeps it from releasing a reference it never took, so it must simply be
    // left alone when `desc` goes out of scope — unwrapping it here would drop the value
    // and underflow the root signature's refcount.
    state
}

/// Depth testing on, depth writing on, `LESS`.
///
/// A fragment is drawn when it is nearer than what is already in the depth buffer. Stencil
/// is off: nothing writes it, and enabling it would cost a read-modify-write per fragment
/// for a buffer nothing reads.
pub(crate) fn depth_stencil_desc() -> D3D12_DEPTH_STENCIL_DESC {
    D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: true.into(),
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
        DepthFunc: D3D12_COMPARISON_FUNC_LESS,
        StencilEnable: false.into(),
        ..Default::default()
    }
}

/// Solid fill, back-face culling, no depth bias.
///
/// `FrontCounterClockwise` must be `true` here, and this is easy to get wrong: winding is
/// judged on the *render target*, where Y points down. [`TRIANGLE_VERTICES`] is
/// counter-clockwise in clip space (Y up) and stays counter-clockwise on screen, so
/// declaring clockwise as front-facing makes the triangle back-facing and it is culled —
/// silently, with no validation-layer message and a correctly recorded draw call.
pub(crate) fn rasterizer_desc() -> D3D12_RASTERIZER_DESC {
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: D3D12_CULL_MODE_BACK,
        FrontCounterClockwise: true.into(),
        DepthClipEnable: true.into(),
        ..Default::default()
    }
}

/// Opaque rendering: no blending, write every channel.
pub(crate) fn blend_desc() -> D3D12_BLEND_DESC {
    let target = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: false.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    D3D12_BLEND_DESC {
        RenderTarget: [target; 8],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout_matches_the_input_elements() {
        assert_eq!(
            size_of::<Vertex>(),
            28,
            "3 floats + 4 floats, tightly packed"
        );
        assert_eq!(align_of::<Vertex>(), 4);
        assert_eq!(size_of::<[f32; 3]>(), 12, "COLOR starts at byte offset 12");
    }

    /// The triangle must not be culled, which requires the winding and the rasterizer's
    /// `FrontCounterClockwise` setting to agree. Getting this wrong renders nothing while
    /// every other signal — draw call recorded, validation layer silent — looks healthy,
    /// which is exactly what happened during Milestone 4.
    #[test]
    fn winding_matches_the_rasterizer_front_face_setting() {
        let [a, b, c] = TRIANGLE_VERTICES;
        // Signed area in clip space (Y up); positive means counter-clockwise.
        let clip_area = (b.position[0] - a.position[0]) * (c.position[1] - a.position[1])
            - (c.position[0] - a.position[0]) * (b.position[1] - a.position[1]);
        assert!(
            clip_area > 0.0,
            "vertices must be counter-clockwise in clip space, signed area {clip_area}"
        );

        // The render target has Y down, and the flip preserves this triangle's
        // counter-clockwise order on screen, so the front face must be declared
        // counter-clockwise or back-face culling discards it.
        assert!(
            rasterizer_desc().FrontCounterClockwise.as_bool(),
            "counter-clockwise vertices need FrontCounterClockwise = true"
        );
        assert_eq!(rasterizer_desc().CullMode, D3D12_CULL_MODE_BACK);
    }

    #[test]
    fn triangle_vertices_are_inside_clip_space() {
        for vertex in TRIANGLE_VERTICES {
            for axis in 0..2 {
                assert!(
                    (-1.0..=1.0).contains(&vertex.position[axis]),
                    "vertex {vertex:?} is outside the clip volume"
                );
            }
            assert!(vertex.color.iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }
}
