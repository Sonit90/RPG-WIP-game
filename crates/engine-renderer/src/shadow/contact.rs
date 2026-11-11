//! The contact-shadow pass: its target, its views and its pipeline state.
//!
//! The march itself is in `shaders/contact.hlsl`. What lives here is the one resource it
//! writes and the one pipeline that runs it.
//!
//! # Why it is a pass rather than a function
//!
//! It was a function, called from the mesh pixel shader, and every shaded pixel paid twelve
//! samples through the depth buffer for it. At 4K that measured 1.31 ms of a 2.50 ms frame:
//! the largest single item in the frame, larger than the cascades and the filtering together.
//!
//! A contact shadow is a low-frequency term — it darkens the centimetres where a boot meets
//! the floor — so it does not need a ray per shaded pixel. Run as its own pass at
//! `ContactShadowSettings::divisor` on each axis, half by default, it costs a quarter of the
//! rays and is reconstructed in the lighting pass with a depth-weighted filter.
//!
//! # The target's second channel
//!
//! `R16G16_FLOAT`: red is the shadow factor, green is the view distance the ray started
//! from. The green channel is not decoration. A bilinear upsample of the red channel alone
//! smears a foot's contact shadow onto the wall behind it, because bilinear does not know
//! that the two texels it is blending are different surfaces. The lighting pass reads green,
//! compares it against the depth of the pixel it is shading, and weights each of the four
//! neighbours by how well they agree — which is what makes a half-resolution term look like
//! a full-resolution one at exactly the places a half-resolution term would otherwise fail.
//!
//! Sixteen-bit float for both: the factor is 0..1 and the depth is metres, and neither wants
//! the bandwidth of a 32-bit target for a texture this pass is bandwidth-bound on.

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BLEND_DESC, D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_CULL_MODE_NONE, D3D12_DEPTH_STENCIL_DESC,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FILL_MODE_SOLID, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC,
    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
    D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV, ID3D12Device, ID3D12PipelineState,
    ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R16G16_FLOAT, DXGI_SAMPLE_DESC,
};

use crate::command::CommandContext;
use crate::descriptor::{CpuDescriptorHeap, DescriptorHandle, GpuDescriptorHeap};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::{GpuTexture, TrackedResource};
use crate::shader::{CONTACT_PS, CONTACT_VS, bytecode};

use super::settings::ContactShadowSettings;

/// Creates the shader-resource view the lighting pass reads the target through.
fn create_srv(device: &RenderDevice, texture: &GpuTexture, handle: DescriptorHandle) {
    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: CONTACT_FORMAT,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                PlaneSlice: 0,
                ResourceMinLODClamp: 0.0,
            },
        },
    };
    // SAFETY: FFI call. The texture outlives every frame that samples it, and the handle
    // addresses a slot in the renderer's own shader-visible heap.
    unsafe {
        device
            .device()
            .CreateShaderResourceView(texture.resource(), Some(&desc), handle.cpu);
    }
}

/// The format the pass renders into: red the shadow factor, green the view distance.
pub const CONTACT_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R16G16_FLOAT;

/// The target the contact-shadow pass writes, and the views onto it.
///
/// Kept for the renderer's lifetime and recreated only when the window or the settings change
/// its size — the same rule the shadow atlas follows, and for the same reason.
pub struct ContactShadowTarget {
    texture: GpuTexture,
    /// Owned so the heap outlives the view that points into it.
    #[allow(
        dead_code,
        reason = "keeps the heap the render-target view lives in alive"
    )]
    rtv_heap: CpuDescriptorHeap,
    rtv: DescriptorHandle,
    srv: DescriptorHandle,
    /// Whether the last resize produced a real target rather than the 1×1 placeholder.
    live: bool,
}

impl ContactShadowTarget {
    /// Creates the target for a camera of `scene_size` at `settings`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails, or
    /// [`RendererError::InvalidConfig`] if the descriptor heap is full.
    pub fn new(
        device: &RenderDevice,
        descriptors: &mut GpuDescriptorHeap,
        settings: &ContactShadowSettings,
        enabled: bool,
        scene_size: (u32, u32),
    ) -> Result<Self, RendererError> {
        let (width, height, live) = Self::size(settings, enabled, scene_size);
        let texture = Self::create(device, width, height)?;

        let mut rtv_heap = CpuDescriptorHeap::new(device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1)?;
        let rtv = rtv_heap.allocate()?;
        device.create_render_target_view(texture.resource(), rtv.cpu);

        let srv = descriptors.allocate_persistent()?;
        create_srv(device, &texture, srv);

        Ok(Self {
            texture,
            rtv_heap,
            rtv,
            srv,
            live,
        })
    }

    /// The dimensions the target needs, and whether it is a real one.
    ///
    /// One texel when contact shadows are off: the descriptor table is bound every frame
    /// whether or not the shader reads it, and a table pointing at nothing is not a
    /// validation error — it is a read of whatever the heap happens to contain.
    fn size(
        settings: &ContactShadowSettings,
        enabled: bool,
        scene_size: (u32, u32),
    ) -> (u32, u32, bool) {
        if enabled && settings.enabled {
            let (width, height) = settings.target_size(scene_size.0.max(1), scene_size.1.max(1));
            (width, height, true)
        } else {
            (1, 1, false)
        }
    }

    fn create(device: &RenderDevice, width: u32, height: u32) -> Result<GpuTexture, RendererError> {
        // Cleared to "fully lit, no surface". Nothing ever clears it — the pass covers every
        // texel — but the optimized clear value has to be *something*, and a value that
        // matches what an untouched texel should mean is the one that cannot mislead.
        GpuTexture::render_target(device, width, height, CONTACT_FORMAT, [1.0, 0.0, 0.0, 0.0])
    }

    /// Rebuilds the target if the size it needs has changed.
    ///
    /// Returns whether anything was recreated, so the caller can skip a GPU wait it does not
    /// need.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] for the same reasons [`ContactShadowTarget::new`] does.
    pub fn resize(
        &mut self,
        device: &RenderDevice,
        settings: &ContactShadowSettings,
        enabled: bool,
        scene_size: (u32, u32),
    ) -> Result<bool, RendererError> {
        let (width, height, live) = Self::size(settings, enabled, scene_size);
        if (width, height) == self.texture.size() {
            self.live = live;
            return Ok(false);
        }
        self.texture = Self::create(device, width, height)?;
        device.create_render_target_view(self.texture.resource(), self.rtv.cpu);
        create_srv(device, &self.texture, self.srv);
        self.live = live;
        Ok(true)
    }

    /// The target texture, for transitions.
    #[must_use]
    pub fn texture(&self) -> &GpuTexture {
        &self.texture
    }

    /// Its dimensions in texels.
    #[must_use]
    pub fn size_in_texels(&self) -> (u32, u32) {
        self.texture.size()
    }

    /// Whether this is a real target rather than the 1×1 placeholder.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The shader-visible handle the lighting pass binds.
    #[must_use]
    pub fn table(
        &self,
    ) -> Option<windows::Win32::Graphics::Direct3D12::D3D12_GPU_DESCRIPTOR_HANDLE> {
        self.srv.gpu
    }

    /// Records the pass: one fullscreen triangle into the target.
    ///
    /// The caller has already bound the root signature, the frame constants and the scene
    /// depth; this binds the target and issues the draw, and leaves the texture back in
    /// `PIXEL_SHADER_RESOURCE` where the lighting pass expects it.
    pub fn record(&self, commands: &mut CommandContext, pipeline: &ContactShadowPipeline) {
        if !self.live {
            return;
        }
        let (width, height) = self.texture.size();
        commands.transition_to(&self.texture, D3D12_RESOURCE_STATE_RENDER_TARGET);
        commands.set_render_target(self.rtv.cpu);
        commands.set_viewport(width, height);
        commands.set_pipeline(pipeline.root_signature(), pipeline.pipeline_state());
        commands.set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        // Three vertices and no buffers: the vertex shader builds the triangle from
        // SV_VertexID. A triangle rather than two, because a quad's diagonal is a row of
        // pixels shaded twice.
        commands.draw(3, 1);
        commands.transition_to(&self.texture, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
    }
}

/// The pipeline state the contact-shadow pass runs.
///
/// It shares the **mesh root signature**, deliberately: the pass reads the frame constants
/// and the scene depth, both of which that signature already declares at the slots the
/// renderer already binds them to. A signature of its own would be a second set of slot
/// constants for the same three arguments.
pub struct ContactShadowPipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl ContactShadowPipeline {
    /// Creates the pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        let root_signature = crate::mesh_pipeline::create_root_signature(device)?;

        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            // SAFETY: copies the interface pointer without changing its refcount; the copy
            // lives only for this call and the real reference is owned here.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(CONTACT_VS),
            PS: bytecode(CONTACT_PS),
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                // A fullscreen triangle has one winding and no back face worth culling.
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: false.into(),
                ..Default::default()
            },
            BlendState: D3D12_BLEND_DESC {
                RenderTarget: [D3D12_RENDER_TARGET_BLEND_DESC {
                    RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
                    ..Default::default()
                }; 8],
                ..Default::default()
            },
            // No depth at all: the pass reads depth as a texture and writes colour, so
            // binding a depth-stencil view would be a second use of a resource it is
            // already sampling.
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
        desc.RTVFormats[0] = CONTACT_FORMAT;

        // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode
        // outlives the call.
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
