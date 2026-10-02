//! The GPU side of the shadow atlas: one depth texture, its views, and the scene-depth copy
//! the contact-shadow march reads.
//!
//! # Lifetime
//!
//! Created once and kept. Nothing here is allocated per frame — §16 asks for that
//! explicitly, and it is also the only way the barriers stay simple: the atlas is one
//! resource that moves between `DEPTH_WRITE` and `PIXEL_SHADER_RESOURCE` twice a frame and is
//! never recreated inside one.
//!
//! It *is* recreated when the settings change the atlas size, and only then. Following the
//! convention the swap chain and the depth buffer already set, the caller must have waited
//! for the GPU before that happens: a texture released while a frame in flight still reads
//! it is a use-after-free inside the driver.
//!
//! # Formats
//!
//! The atlas is created as `R32_TYPELESS` rather than `D32_FLOAT`, which is not a detail: a
//! depth resource can only be *sampled* if it was created typeless, with the depth-stencil
//! view naming `D32_FLOAT` and the shader-resource view naming `R32_FLOAT`. Creating it as
//! `D32_FLOAT` and then asking for an SRV fails at descriptor creation with a message that
//! does not mention the resource's format at all.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_DEPTH_STENCIL_VIEW_DESC, D3D12_DEPTH_STENCIL_VIEW_DESC_0, D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
    D3D12_DSV_DIMENSION_TEXTURE2D, D3D12_DSV_FLAG_NONE, D3D12_RESOURCE_STATE_DEPTH_WRITE,
    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_SHADER_RESOURCE_VIEW_DESC,
    D3D12_SHADER_RESOURCE_VIEW_DESC_0, D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_DSV,
    D3D12_TEX2D_SRV,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_TYPELESS,
};

use crate::descriptor::{CpuDescriptorHeap, DescriptorHandle, GpuDescriptorHeap};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::{GpuTexture, TrackedResource};

use super::atlas::ShadowAtlasLayout;
use super::settings::ShadowSettings;

/// The depth format every shadow view is rendered with.
///
/// `D32_FLOAT`, matching [`crate::depth::DEPTH_FORMAT`]: the shadow pass and the camera's
/// depth prepass write through the same pipeline states, and a pipeline state bakes in its
/// depth format. One format for both is what lets the prepass reuse the shadow pipelines.
pub const SHADOW_FORMAT: DXGI_FORMAT = DXGI_FORMAT_D32_FLOAT;

/// The typeless format the underlying resource is created with, so it can be sampled.
pub const SHADOW_RESOURCE_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R32_TYPELESS;

/// The format the shader reads depth through.
pub const SHADOW_SRV_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R32_FLOAT;

/// The depth an empty shadow texel holds.
///
/// 1.0 — the far plane — so anything that is never rendered into is "nothing in the way",
/// and a light with no casters lights the scene rather than blacking it out.
pub const SHADOW_CLEAR: f32 = 1.0;

/// The shadow atlas, the scene-depth copy, and every view onto them.
pub struct ShadowResources {
    atlas: GpuTexture,
    /// Owned so the heap outlives the handle that points into it.
    ///
    /// Held, not read: the view below addresses a slot in this heap, and dropping it would
    /// leave the shadow passes rendering through a descriptor that addresses freed memory.
    /// Ownership *is* its purpose.
    #[allow(dead_code, reason = "keeps the heap the atlas view lives in alive")]
    dsv_heap: CpuDescriptorHeap,
    atlas_dsv: DescriptorHandle,
    /// The atlas's SRV, in the renderer's one shader-visible heap.
    atlas_srv: DescriptorHandle,
    /// A copy of the camera's depth buffer, for the contact-shadow march.
    ///
    /// A copy rather than the depth buffer itself: sampling the live depth buffer means
    /// binding it read-only for the whole camera pass, which means a second depth-stencil
    /// view, a second set of pipeline states with depth writes disabled, and a rule that
    /// nothing in the pass may write depth. The copy costs one `CopyResource` of a
    /// full-screen depth buffer — and it is only made when contact shadows are on.
    scene_depth: GpuTexture,
    scene_depth_srv: DescriptorHandle,
    /// Whether `scene_depth_srv` views the camera's own depth buffer, sampled in place,
    /// rather than `scene_depth`, a copy of it. See [`ShadowResources::sample_camera_depth`].
    camera_depth: bool,
    layout: ShadowAtlasLayout,
}

impl ShadowResources {
    /// Creates the atlas and its views for `settings`.
    ///
    /// `descriptors` is the renderer's one shader-visible heap; the SRVs are allocated from
    /// its persistent region and live as long as these resources do.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails, or
    /// [`RendererError::InvalidConfig`] if the descriptor heap is full.
    pub fn new(
        device: &RenderDevice,
        descriptors: &mut GpuDescriptorHeap,
        settings: &ShadowSettings,
        scene_size: (u32, u32),
    ) -> Result<Self, RendererError> {
        let layout = ShadowAtlasLayout::new(settings);
        let (width, height) = layout.size();
        // A one-texel atlas when shadows are off: the descriptor table is bound every frame
        // whether or not the shader reads it, and a table pointing at nothing is not a
        // validation error — it is a read of whatever the heap happens to contain.
        let atlas = Self::create_depth_texture(device, width.max(1), height.max(1))?;

        let mut dsv_heap = CpuDescriptorHeap::new(device, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1)?;
        let atlas_dsv = dsv_heap.allocate()?;
        create_depth_view(device, &atlas, atlas_dsv);

        let atlas_srv = descriptors.allocate_persistent()?;
        create_depth_srv(device, &atlas, atlas_srv);

        let (depth_width, depth_height) = Self::scene_depth_size(settings, scene_size);
        let scene_depth = Self::create_depth_copy(device, depth_width, depth_height)?;
        let scene_depth_srv = descriptors.allocate_persistent()?;
        create_depth_srv(device, &scene_depth, scene_depth_srv);

        Ok(Self {
            atlas,
            dsv_heap,
            atlas_dsv,
            atlas_srv,
            scene_depth,
            scene_depth_srv,
            camera_depth: false,
            layout,
        })
    }

    /// How big the scene-depth copy needs to be.
    ///
    /// One texel unless contact shadows are on: the resource has to exist so the descriptor
    /// is valid, and a full-screen depth-sized texture that nothing samples is eight
    /// megabytes of nothing.
    fn scene_depth_size(settings: &ShadowSettings, scene_size: (u32, u32)) -> (u32, u32) {
        if settings.enabled && settings.contact.enabled {
            (scene_size.0.max(1), scene_size.1.max(1))
        } else {
            (1, 1)
        }
    }

    fn create_depth_texture(
        device: &RenderDevice,
        width: u32,
        height: u32,
    ) -> Result<GpuTexture, RendererError> {
        GpuTexture::sampleable_depth(device, width, height, SHADOW_CLEAR)
    }

    /// Creates the texture the camera's depth is copied into.
    ///
    /// **Not** a depth-stencil resource: nothing ever renders into it. It is a copy
    /// destination and a shader resource, and creating it as a depth target would mean it
    /// spent its life in `DEPTH_WRITE` — a state a shader may not read from, which is a
    /// device hang rather than a validation message on some drivers. It still has to be
    /// `R32_TYPELESS`, because that is what makes it copy-compatible with the `D32_FLOAT`
    /// depth buffer *and* readable as `R32_FLOAT`.
    ///
    /// It starts in `PIXEL_SHADER_RESOURCE`, which is where the lighting pass expects it and
    /// where [`ShadowRenderer::copy_scene_depth`](super::renderer::ShadowRenderer::copy_scene_depth)
    /// puts it back.
    fn create_depth_copy(
        device: &RenderDevice,
        width: u32,
        height: u32,
    ) -> Result<GpuTexture, RendererError> {
        GpuTexture::new(
            device,
            width,
            height,
            SHADOW_RESOURCE_FORMAT,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )
    }

    /// Rebuilds the resources for new settings or a new scene size.
    ///
    /// Returns whether anything was actually recreated, so the caller can log a resolution
    /// change and skip the GPU wait when nothing needed one.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] for the same reasons [`ShadowResources::new`] does.
    pub fn resize(
        &mut self,
        device: &RenderDevice,
        settings: &ShadowSettings,
        scene_size: (u32, u32),
    ) -> Result<bool, RendererError> {
        let layout = ShadowAtlasLayout::new(settings);
        let (width, height) = layout.size();
        let (depth_width, depth_height) = if self.camera_depth {
            (1, 1)
        } else {
            Self::scene_depth_size(settings, scene_size)
        };

        let atlas_changed = layout.size() != self.layout.size();
        let depth_changed = (depth_width, depth_height) != self.scene_depth.size();
        self.layout = layout;
        if !atlas_changed && !depth_changed {
            return Ok(false);
        }

        if atlas_changed {
            self.atlas = Self::create_depth_texture(device, width.max(1), height.max(1))?;
            // The descriptors are reused rather than freed and reallocated: they are one
            // slot each and writing a new view over them is what the API is for.
            create_depth_view(device, &self.atlas, self.atlas_dsv);
            create_depth_srv(device, &self.atlas, self.atlas_srv);
        }
        if depth_changed {
            self.scene_depth = Self::create_depth_copy(device, depth_width, depth_height)?;
            // In camera-depth mode the view stays on the camera's buffer; the copy is a
            // placeholder nothing reads.
            if !self.camera_depth {
                create_depth_srv(device, &self.scene_depth, self.scene_depth_srv);
            }
        }
        Ok(true)
    }

    /// The atlas texture, for transitions and copies.
    #[must_use]
    pub fn atlas(&self) -> &GpuTexture {
        &self.atlas
    }

    /// The depth-stencil view the shadow passes render through.
    #[must_use]
    pub fn atlas_dsv(&self) -> DescriptorHandle {
        self.atlas_dsv
    }

    /// The shader-visible handle the lighting pass samples the atlas through.
    #[must_use]
    pub fn atlas_srv(&self) -> DescriptorHandle {
        self.atlas_srv
    }

    /// Points the contact-shadow march at the camera's own depth buffer, sampled in place.
    ///
    /// Replaces the full-screen copy: the march runs between the depth prepass and the camera
    /// pass, when nothing writes depth, so the buffer can simply be read — moved to
    /// `PIXEL_SHADER_RESOURCE` for the march and back to `DEPTH_WRITE` after it — and the copy
    /// (a full-resolution read and write every frame, 0.1 ms at 4K) and the texture it filled
    /// (33 MB at 4K) both go. `depth` must be typeless, as [`GpuTexture::sampleable_depth`]
    /// creates it. Called again whenever the depth buffer is recreated.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the placeholder copy cannot be created.
    pub fn sample_camera_depth(
        &mut self,
        device: &RenderDevice,
        depth: &GpuTexture,
    ) -> Result<(), RendererError> {
        if !self.camera_depth || self.scene_depth.size() != (1, 1) {
            self.scene_depth = Self::create_depth_copy(device, 1, 1)?;
        }
        self.camera_depth = true;
        create_depth_srv(device, depth, self.scene_depth_srv);
        Ok(())
    }

    /// Whether the march samples the camera's depth buffer in place rather than a copy.
    #[must_use]
    pub fn samples_camera_depth(&self) -> bool {
        self.camera_depth
    }

    /// The scene-depth copy, for the transitions around the copy.
    #[must_use]
    pub fn scene_depth(&self) -> &GpuTexture {
        &self.scene_depth
    }

    /// The shader-visible handle the contact-shadow march reads depth through.
    #[must_use]
    pub fn scene_depth_srv(&self) -> DescriptorHandle {
        self.scene_depth_srv
    }

    /// Where each view lives inside the atlas.
    #[must_use]
    pub fn layout(&self) -> &ShadowAtlasLayout {
        &self.layout
    }

    /// The atlas size in texels.
    #[must_use]
    pub fn atlas_size(&self) -> (u32, u32) {
        self.atlas.size()
    }

    /// The state the atlas must be in to be rendered into.
    #[must_use]
    pub fn write_state(&self) -> windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES {
        D3D12_RESOURCE_STATE_DEPTH_WRITE
    }

    /// The state the atlas must be in to be sampled.
    #[must_use]
    pub fn read_state(&self) -> windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES {
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
    }
}

/// Creates a depth-stencil view naming the typed format for a typeless resource.
///
/// The `None` description the rest of the renderer passes cannot be used here: a typeless
/// resource has no format of its own to infer, and the call fails.
fn create_depth_view(device: &RenderDevice, texture: &GpuTexture, handle: DescriptorHandle) {
    let desc = D3D12_DEPTH_STENCIL_VIEW_DESC {
        Format: SHADOW_FORMAT,
        ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
        Flags: D3D12_DSV_FLAG_NONE,
        Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 },
        },
    };
    // SAFETY: FFI call. The texture outlives the view, and the handle addresses a slot in a
    // heap the caller owns.
    unsafe {
        device
            .device()
            .CreateDepthStencilView(texture.resource(), Some(&desc), handle.cpu);
    }
}

/// Creates the shader-resource view depth is sampled through.
fn create_depth_srv(device: &RenderDevice, texture: &GpuTexture, handle: DescriptorHandle) {
    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: SHADOW_SRV_FORMAT,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        // The default component mapping, spelled out: leaving it zero is an invalid encoding
        // the debug layer rejects.
        Shader4ComponentMapping: 0x1688,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                PlaneSlice: 0,
                ResourceMinLODClamp: 0.0,
            },
        },
    };
    // SAFETY: FFI call, as above.
    unsafe {
        device
            .device()
            .CreateShaderResourceView(Some(texture.resource()), Some(&desc), handle.cpu);
    }
}
