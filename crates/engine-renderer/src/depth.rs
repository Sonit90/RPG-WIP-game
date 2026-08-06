//! The depth buffer.
//!
//! One depth-stencil texture the size of the back buffer, cleared at the start of every
//! frame and tested against while drawing. Without it, geometry is drawn in submission
//! order and whatever was recorded last wins — which happens to look correct for a single
//! triangle and stops looking correct the moment there are two objects.

use crate::descriptor::{CpuDescriptorHeap, DescriptorHandle};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::{GpuTexture, TrackedResource};

use windows::Win32::Graphics::Direct3D12::D3D12_DESCRIPTOR_HEAP_TYPE_DSV;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_D32_FLOAT;

/// The depth format.
///
/// `D32_FLOAT` rather than `D24_UNORM_S8_UINT`: there is no stencil work in Phase 2, and a
/// 32-bit float spends its precision where a float naturally does — near the eye, which is
/// also where depth fighting is most visible. It is the same 4 bytes per pixel either way.
pub const DEPTH_FORMAT: DXGI_FORMAT = DXGI_FORMAT_D32_FLOAT;

/// The value the depth buffer is cleared to.
///
/// 1.0 is the far plane, and the comparison is `LESS`, so a fragment is drawn when it is
/// nearer than what is already there. Reverse-Z (clear to 0, compare `GREATER`) buys real
/// precision at long range; it is not adopted now because it touches this constant, the
/// projection matrix and one PSO field together, and the risk table already records that as
/// a cheap later change.
pub const DEPTH_CLEAR: f32 = 1.0;

/// A depth-stencil target and its view.
pub struct DepthBuffer {
    texture: GpuTexture,
    /// Owned so the heap outlives the handle that points into it.
    heap: CpuDescriptorHeap,
    view: DescriptorHandle,
    width: u32,
    height: u32,
}

impl DepthBuffer {
    /// Creates a depth buffer of the given size.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero dimension, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &RenderDevice, width: u32, height: u32) -> Result<Self, RendererError> {
        // One descriptor: there is one depth buffer. A heap sized for a hypothetical future
        // would be capacity nothing allocates from.
        let mut heap = CpuDescriptorHeap::new(device, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1)?;
        let texture = GpuTexture::depth_buffer(device, width, height, DEPTH_FORMAT, DEPTH_CLEAR)?;
        let view = heap.allocate()?;
        device.create_depth_stencil_view(texture.resource(), view.cpu);

        Ok(Self {
            texture,
            heap,
            view,
            width,
            height,
        })
    }

    /// Replaces the buffer with one of a new size.
    ///
    /// The caller must have waited for the GPU to finish with the old texture — the same
    /// requirement as resizing the swap chain, and for the same reason.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero dimension, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn resize(
        &mut self,
        device: &RenderDevice,
        width: u32,
        height: u32,
    ) -> Result<(), RendererError> {
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }

        // The descriptor is reused rather than freed and reallocated: it is one slot in a
        // heap of one, and writing a new view over it is exactly what the API is for.
        self.texture = GpuTexture::depth_buffer(device, width, height, DEPTH_FORMAT, DEPTH_CLEAR)?;
        device.create_depth_stencil_view(self.texture.resource(), self.view.cpu);
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// The texture, for transitions.
    #[must_use]
    pub fn texture(&self) -> &GpuTexture {
        &self.texture
    }

    /// The depth-stencil view to bind.
    #[must_use]
    pub fn view(&self) -> DescriptorHandle {
        self.view
    }

    /// The size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The descriptor heap the view lives in.
    ///
    /// Exposed so a caller can see the heap is one descriptor deep rather than guessing.
    #[must_use]
    pub fn heap(&self) -> &CpuDescriptorHeap {
        &self.heap
    }
}
