//! Instanced rendering: per-instance data, its grid layout, and the pipeline that
//! consumes it.
//!
//! This exists for the Milestone 8 benchmark, which compares issuing N draw calls against
//! issuing one instanced draw. Both modes use the pipeline built here, the same unit
//! triangle and the same instance buffer, so the **only** variable between them is the
//! number of draw calls. Mode A selects instance `i` with `StartInstanceLocation`; mode B
//! passes an instance count. Nothing else changes.
//!
//! # Status: staged, not used by the production render path
//!
//! `Renderer::render_world` does not touch [`InstancedPipeline`]. It draws through
//! [`crate::mesh_pipeline::MeshPipeline`], one `DrawIndexedInstanced` per visible object
//! with an instance count of one — the deliberately unoptimized Milestone 15 baseline that
//! batching and instancing have to beat.
//!
//! What lives here is the *measurement* of what they would be worth, not the feature. It
//! draws a unit triangle with per-instance colours and transforms, which is enough to price
//! draw-call submission against instanced submission and nothing more; it does not
//! understand meshes, materials or a `RenderWorld`.
//!
//! * **Used today by:** `engine-bench`'s `performance_test`, via [`crate::bench`].
//! * **Expected to inform:** the batching and instancing milestones, which will need a real
//!   per-instance path against `MeshVertex` geometry rather than this one.
//! * **Intentionally staged.** Do not delete it as unused — the benchmark is a caller — and
//!   do not mistake it for the instancing feature itself.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::depth::DEPTH_FORMAT;
use crate::error::RendererError;
use crate::pipeline::{
    Vertex, blend_desc, create_root_signature, depth_stencil_desc, rasterizer_desc,
};
use crate::shader::{INSTANCED_PS, INSTANCED_VS, bytecode};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// Per-instance data: where to put the triangle and what colour to tint it.
///
/// `repr(C)` for the same reason as [`Vertex`] — the input layout describes this exact
/// memory layout to the GPU.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceData {
    /// Clip-space offset in `xy`, uniform scale in `z`.
    pub transform: [f32; 3],
    /// Colour multiplied with the mesh's vertex colour.
    pub color: [f32; 4],
}

/// The mesh every instance draws: a unit triangle centred on the origin.
///
/// Counter-clockwise, matching the rasterizer's front-face setting. Scaled and moved per
/// instance by the vertex shader.
pub const UNIT_TRIANGLE: [Vertex; 3] = [
    Vertex {
        position: [0.0, 1.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    },
    Vertex {
        position: [-1.0, -1.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    },
    Vertex {
        position: [1.0, -1.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    },
];

/// Fraction of its grid cell an instance occupies.
const CELL_FILL: f32 = 0.8;

/// Lays out `count` instances in a grid filling the clip volume.
///
/// Deterministic and allocation-reusing, like the stress grid, and for the same reason:
/// the two benchmark modes must draw exactly the same picture, and repeated runs must be
/// comparable.
pub fn build_instance_grid(count: u32, instances: &mut Vec<InstanceData>) {
    instances.clear();
    if count == 0 {
        return;
    }
    instances.reserve(count as usize);

    let (columns, rows) = crate::stress::grid_dimensions(count);
    let cell_width = 2.0 / columns as f32;
    let cell_height = 2.0 / rows as f32;
    // The unit triangle spans -1..1, so a scale of half the cell fills the cell.
    let scale = 0.5 * cell_width.min(cell_height) * CELL_FILL;

    for index in 0..count {
        let column = index % columns;
        let row = index / columns;
        let x = -1.0 + (column as f32 + 0.5) * cell_width;
        let y = 1.0 - (row as f32 + 0.5) * cell_height;

        let t = if count <= 1 {
            0.0
        } else {
            index as f32 / (count - 1) as f32
        };
        instances.push(InstanceData {
            transform: [x, y, scale],
            color: [
                1.0 - t * 0.85,
                0.15 + (1.0 - (t - 0.5).abs() * 2.0).clamp(0.0, 1.0) * 0.85,
                0.15 + t * 0.85,
                1.0,
            ],
        });
    }
}

/// Root signature and pipeline state for instanced position + colour geometry.
pub struct InstancedPipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl InstancedPipeline {
    /// Builds the pipeline state from the embedded instanced DXIL.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
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
                SemanticName: s!("COLOR"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: size_of::<[f32; 3]>() as u32,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            // Slot 1 advances once per instance rather than once per vertex.
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("INSTANCE_TRANSFORM"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32_FLOAT,
                InputSlot: 1,
                AlignedByteOffset: 0,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("INSTANCE_COLOR"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 1,
                AlignedByteOffset: size_of::<[f32; 3]>() as u32,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            },
        ];

        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            // SAFETY: copies the interface pointer without changing its refcount; the
            // copy lives only for this call and the real reference is owned here.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(INSTANCED_VS),
            PS: bytecode(INSTANCED_PS),
            RasterizerState: rasterizer_desc(),
            BlendState: blend_desc(),
            // Must match the scene pipeline: the two are swapped between benchmark modes
            // against the same render target and depth buffer, and a PSO whose DSV format
            // disagrees with the bound view is a driver error at draw time.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(count: u32) -> Vec<InstanceData> {
        let mut instances = Vec::new();
        build_instance_grid(count, &mut instances);
        instances
    }

    #[test]
    fn one_entry_per_instance() {
        for count in [1, 2, 100, 10_000] {
            assert_eq!(grid(count).len(), count as usize, "count {count}");
        }
        assert!(grid(0).is_empty());
    }

    #[test]
    fn instances_stay_inside_the_clip_volume() {
        for count in [1, 100, 10_000] {
            for instance in grid(count) {
                let [x, y, scale] = instance.transform;
                assert!(scale > 0.0, "scale must be positive: {instance:?}");
                assert!(
                    (-1.0..=1.0).contains(&(x + scale)) && (-1.0..=1.0).contains(&(x - scale)),
                    "instance leaves the clip volume horizontally: {instance:?}"
                );
                assert!(
                    (-1.0..=1.0).contains(&(y + scale)) && (-1.0..=1.0).contains(&(y - scale)),
                    "instance leaves the clip volume vertically: {instance:?}"
                );
            }
        }
    }

    #[test]
    fn scale_shrinks_as_the_instance_count_grows() {
        let one = grid(1)[0].transform[2];
        let many = grid(10_000)[0].transform[2];
        assert!(
            many < one,
            "10,000 instances must be smaller: {many} vs {one}"
        );
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(grid(500), grid(500));
    }

    #[test]
    fn rebuilding_reuses_the_allocation() {
        let mut instances = Vec::new();
        build_instance_grid(1_000, &mut instances);
        let capacity = instances.capacity();
        build_instance_grid(1_000, &mut instances);
        assert_eq!(instances.capacity(), capacity);
    }

    #[test]
    fn colours_stay_in_range() {
        for instance in grid(1_000) {
            for channel in instance.color {
                assert!((0.0..=1.0).contains(&channel), "{instance:?}");
            }
        }
    }

    #[test]
    fn the_unit_triangle_is_counter_clockwise() {
        let [a, b, c] = UNIT_TRIANGLE;
        let area = (b.position[0] - a.position[0]) * (c.position[1] - a.position[1])
            - (c.position[0] - a.position[0]) * (b.position[1] - a.position[1]);
        assert!(
            area > 0.0,
            "the instanced mesh would be culled, area {area}"
        );
    }

    #[test]
    fn instance_data_is_tightly_packed() {
        assert_eq!(size_of::<InstanceData>(), 28);
        assert_eq!(
            size_of::<[f32; 3]>(),
            12,
            "INSTANCE_COLOR starts at offset 12"
        );
    }
}
