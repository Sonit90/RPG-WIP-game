//! Root signature, pipeline state and the triangle's vertex buffer.
//!
//! Phase 0 / Milestone 4 only needs one pipeline: position + colour vertices, no
//! constant buffers, no textures, no depth. Anything more (materials, passes, a shader
//! system) is explicitly out of scope.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD,
    D3D12_BLEND_ZERO, D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_CULL_MODE_BACK, D3D12_DEPTH_STENCIL_DESC,
    D3D12_FILL_MODE_SOLID, D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_UPLOAD, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
    D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC, D3D12_LOGIC_OP_NOOP,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RANGE, D3D12_RASTERIZER_DESC,
    D3D12_RENDER_TARGET_BLEND_DESC, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SHADER_BYTECODE,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_VERTEX_BUFFER_VIEW, D3D12SerializeRootSignature,
    ID3D12Device, ID3D12PipelineState, ID3D12Resource, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::core::{PCSTR, s};

use crate::error::RendererError;
use crate::shader::compile_hlsl;
use crate::swapchain::BACK_BUFFER_FORMAT;

/// The HLSL source, embedded so a shipped binary needs no shader files beside it.
const TRIANGLE_HLSL: &str = include_str!("../shaders/triangle.hlsl");

/// Shader model used for both entry points.
///
/// 5_1 is available on every D3D12 device at feature level 11_0 and needs no DXIL
/// toolchain; shader model 6 would require `dxcompiler.dll` to be shipped.
const VS_TARGET: PCSTR = s!("vs_5_1");
const PS_TARGET: PCSTR = s!("ps_5_1");

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

/// Root signature, pipeline state and vertex buffer for the triangle.
///
/// Field order matters for the same reason it does in `Renderer`: the vertex buffer and
/// pipeline objects must be released before the device that created them, which is owned
/// by the caller and therefore outlives this struct.
pub struct TrianglePipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
    /// Kept alive because the vertex-buffer view points into it.
    vertex_buffer: ID3D12Resource,
    vertex_buffer_view: D3D12_VERTEX_BUFFER_VIEW,
    vertex_count: u32,
}

impl TrianglePipeline {
    /// Compiles the shaders and creates every object needed to draw `vertices`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::ShaderCompilation`] if the HLSL does not compile, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device, vertices: &[Vertex]) -> Result<Self, RendererError> {
        if vertices.is_empty() {
            return Err(RendererError::InvalidConfig {
                reason: "a pipeline needs at least one vertex".to_owned(),
            });
        }

        let root_signature = create_root_signature(device)?;

        let vertex_shader = compile_hlsl(TRIANGLE_HLSL, "triangle.hlsl", s!("VSMain"), VS_TARGET)?;
        let pixel_shader = compile_hlsl(TRIANGLE_HLSL, "triangle.hlsl", s!("PSMain"), PS_TARGET)?;

        let pipeline_state =
            create_pipeline_state(device, &root_signature, &vertex_shader, &pixel_shader)?;

        let (vertex_buffer, vertex_buffer_view) = create_vertex_buffer(device, vertices)?;

        log::info!(
            "triangle pipeline ready: {} vertices, shader model 5_1",
            vertices.len()
        );

        Ok(Self {
            root_signature,
            pipeline_state,
            vertex_buffer,
            vertex_buffer_view,
            vertex_count: vertices.len() as u32,
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

    /// The vertex-buffer view to bind to input slot 0.
    #[must_use]
    pub fn vertex_buffer_view(&self) -> &D3D12_VERTEX_BUFFER_VIEW {
        &self.vertex_buffer_view
    }

    /// Number of vertices to draw.
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    /// The buffer resource backing the view, kept for diagnostics.
    #[must_use]
    pub fn vertex_buffer(&self) -> &ID3D12Resource {
        &self.vertex_buffer
    }
}

/// Creates an empty root signature.
///
/// The shaders take no constant buffers, textures or samplers, so the signature only has
/// to permit an input-assembler layout.
fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, RendererError> {
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
    vertex_shader: &ID3DBlob,
    pixel_shader: &ID3DBlob,
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
        VS: shader_bytecode(vertex_shader),
        PS: shader_bytecode(pixel_shader),
        RasterizerState: rasterizer_desc(),
        BlendState: blend_desc(),
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
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

/// Wraps a compiled blob as a shader bytecode descriptor.
fn shader_bytecode(blob: &ID3DBlob) -> D3D12_SHADER_BYTECODE {
    // SAFETY: FFI calls on a live blob. The descriptor borrows the blob's buffer, and
    // every caller keeps the blob alive until the pipeline state is created.
    unsafe {
        D3D12_SHADER_BYTECODE {
            pShaderBytecode: blob.GetBufferPointer(),
            BytecodeLength: blob.GetBufferSize(),
        }
    }
}

/// Solid fill, back-face culling, no depth bias.
///
/// `FrontCounterClockwise` must be `true` here, and this is easy to get wrong: winding is
/// judged on the *render target*, where Y points down. [`TRIANGLE_VERTICES`] is
/// counter-clockwise in clip space (Y up) and stays counter-clockwise on screen, so
/// declaring clockwise as front-facing makes the triangle back-facing and it is culled —
/// silently, with no validation-layer message and a correctly recorded draw call.
fn rasterizer_desc() -> D3D12_RASTERIZER_DESC {
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: D3D12_CULL_MODE_BACK,
        FrontCounterClockwise: true.into(),
        DepthClipEnable: true.into(),
        ..Default::default()
    }
}

/// Opaque rendering: no blending, write every channel.
fn blend_desc() -> D3D12_BLEND_DESC {
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

/// Creates an upload-heap vertex buffer and copies `vertices` into it.
///
/// An upload heap is CPU-visible and GPU-readable, which is the simplest correct choice
/// for a handful of static vertices. It is *not* the right choice for real geometry: a
/// default-heap buffer filled once through an upload staging copy avoids reading vertex
/// data over PCIe every draw. That change belongs with the stress tests in Milestone 7,
/// where the difference is measurable.
fn create_vertex_buffer(
    device: &ID3D12Device,
    vertices: &[Vertex],
) -> Result<(ID3D12Resource, D3D12_VERTEX_BUFFER_VIEW), RendererError> {
    let byte_size = std::mem::size_of_val(vertices);

    let heap_properties = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        ..Default::default()
    };
    let resource_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: byte_size as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };

    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: FFI call. Both descriptors are fully initialized and outlive the call, and
    // `buffer` is a valid out-parameter. GENERIC_READ is the required initial state for
    // an upload-heap resource.
    unsafe {
        device.CreateCommittedResource(
            &heap_properties,
            D3D12_HEAP_FLAG_NONE,
            &resource_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut buffer,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

    let Some(buffer) = buffer else {
        return Err(RendererError::InvalidConfig {
            reason: "vertex buffer creation produced no resource".to_owned(),
        });
    };

    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    // A read range of 0..0 tells the driver the CPU will not read the buffer back.
    let read_range = D3D12_RANGE { Begin: 0, End: 0 };
    // SAFETY: FFI call mapping subresource 0 of a buffer created on an upload heap.
    unsafe { buffer.Map(0, Some(&read_range), Some(&mut mapped)) }
        .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

    // SAFETY: `mapped` points to at least `byte_size` writable bytes, as that is the size
    // the resource was created with, and the regions cannot overlap because `mapped` is a
    // fresh GPU-visible allocation.
    unsafe {
        std::ptr::copy_nonoverlapping(
            vertices.as_ptr().cast::<u8>(),
            mapped.cast::<u8>(),
            byte_size,
        );
        buffer.Unmap(0, None);
    }

    let view = D3D12_VERTEX_BUFFER_VIEW {
        // SAFETY: FFI call returning the resource's GPU address, valid while it lives.
        BufferLocation: unsafe { buffer.GetGPUVirtualAddress() },
        SizeInBytes: byte_size as u32,
        StrideInBytes: size_of::<Vertex>() as u32,
    };

    Ok((buffer, view))
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
