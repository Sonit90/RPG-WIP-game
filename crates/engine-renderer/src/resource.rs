//! GPU resources and their state.
//!
//! ## The problem this solves
//!
//! Until now a caller wrote `transition(&buffer, PRESENT, RENDER_TARGET)` — it stated both
//! the current state and the wanted one. Stating the current state is stating something
//! the caller has to *remember*, across frames and across code paths, and getting it wrong
//! produces a validation error at best and a read of an untransitioned resource at worst.
//!
//! A resource knows its own state. `transition_to(&texture, RENDER_TARGET)` cannot state
//! the wrong "before", because it does not state one.
//!
//! ## Tracking is per-resource and single-queue
//!
//! Not per-subresource: every texture here has one mip and one slice. Not cross-queue:
//! there is one queue. Both limits are real and both are documented rather than
//! engineered around, because the renderer has no case that needs either.
//!
//! ## Why the state is a `Cell`
//!
//! Recording borrows resources immutably — a texture is referenced by its view, by the
//! render list, and by the command context in the same frame — while a transition must
//! update the state. `Cell` makes that a single-threaded interior mutation, which is
//! exactly what it is: command recording happens on one thread, and `GpuTexture` is
//! deliberately not `Sync`.
//!
//! ## Destruction is deferred
//!
//! Releasing a resource the GPU is still reading is a use-after-free with a driver crash
//! at the far end. The safe options are to block until the GPU is idle — correct and
//! fully serializing — or to hold the resource until the fence value covering its last use
//! has passed. [`DeferredReleaseQueue`] does the second, so releasing a resource costs a
//! push rather than a pipeline drain.

use std::cell::Cell;

use windows::Win32::Graphics::Direct3D12::{
    D3D12_CLEAR_VALUE, D3D12_CLEAR_VALUE_0, D3D12_DEPTH_STENCIL_VALUE, D3D12_HEAP_TYPE,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_GENERIC_READ,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use crate::device::RenderDevice;
use crate::error::RendererError;

/// A resource whose current state the engine tracks.
///
/// Implemented by every wrapper in this module. The command context takes this rather than
/// a raw `ID3D12Resource`, which is what makes a transition unable to name a wrong
/// "before" state.
pub trait TrackedResource {
    /// The underlying D3D12 resource.
    fn resource(&self) -> &ID3D12Resource;

    /// The state the engine believes the resource is in.
    fn state(&self) -> D3D12_RESOURCE_STATES;

    /// Records that the resource has entered `state`.
    ///
    /// Called by the command context when it queues a transition. Recording it at queue
    /// time rather than at flush time is correct because the barriers are submitted in
    /// order, before any command that depends on them.
    fn set_state(&self, state: D3D12_RESOURCE_STATES);
}

/// A GPU buffer: vertices, indices, constants or instance data.
pub struct GpuBuffer {
    resource: ID3D12Resource,
    size: usize,
    heap_type: D3D12_HEAP_TYPE,
    state: Cell<D3D12_RESOURCE_STATES>,
    gpu_address: u64,
}

impl GpuBuffer {
    /// Creates a buffer of `size` bytes in device-local memory.
    ///
    /// Starts in `COMMON`, from which buffers promote implicitly to whatever their first
    /// use requires — so a freshly created buffer needs no barrier before being copied
    /// into.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn new_default(device: &RenderDevice, size: usize) -> Result<Self, RendererError> {
        Self::new(
            device,
            size,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_COMMON,
        )
    }

    /// Creates a CPU-writable buffer of `size` bytes, for staging and per-frame data.
    ///
    /// Upload-heap resources are permanently in `GENERIC_READ` and cannot be transitioned;
    /// D3D12 rejects a barrier on one.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn new_upload(device: &RenderDevice, size: usize) -> Result<Self, RendererError> {
        Self::new(
            device,
            size,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )
    }

    /// Creates a buffer on an explicit heap in an explicit state.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn new(
        device: &RenderDevice,
        size: usize,
        heap_type: D3D12_HEAP_TYPE,
        initial_state: D3D12_RESOURCE_STATES,
    ) -> Result<Self, RendererError> {
        if size == 0 {
            return Err(RendererError::InvalidConfig {
                reason: "a buffer needs a non-zero size".to_owned(),
            });
        }
        let resource = device.create_buffer(size, heap_type, initial_state)?;
        // SAFETY: FFI call returning the resource's GPU address, valid while it lives.
        let gpu_address = unsafe { resource.GetGPUVirtualAddress() };

        Ok(Self {
            resource,
            size,
            heap_type,
            state: Cell::new(initial_state),
            gpu_address,
        })
    }

    /// Size in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// GPU virtual address, for views and root descriptors.
    #[must_use]
    pub fn gpu_address(&self) -> u64 {
        self.gpu_address
    }

    /// Whether this buffer lives on an upload heap and can be written by the CPU.
    #[must_use]
    pub fn is_upload(&self) -> bool {
        self.heap_type == D3D12_HEAP_TYPE_UPLOAD
    }

    /// Maps the buffer and leaves it mapped for its lifetime.
    ///
    /// Mapping an upload-heap resource is not free — it is a driver call — and a ring that
    /// mapped and unmapped per allocation would pay it thousands of times a frame. D3D12
    /// permits a resource to stay mapped indefinitely, and releasing it unmaps implicitly,
    /// so the pointer stays valid exactly as long as this `GpuBuffer` does.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if this is not an upload buffer, or
    /// [`RendererError::Api`] if mapping fails.
    pub fn map_persistent(&self) -> Result<std::ptr::NonNull<u8>, RendererError> {
        if !self.is_upload() {
            return Err(RendererError::InvalidConfig {
                reason: "only an upload-heap buffer can be mapped".to_owned(),
            });
        }

        let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
        // A read range of 0..0 states that the CPU will not read this buffer back, which
        // matters on discrete GPUs where the memory is write-combined.
        let read_range = windows::Win32::Graphics::Direct3D12::D3D12_RANGE { Begin: 0, End: 0 };
        // SAFETY: FFI call mapping subresource 0 of an upload-heap buffer this object owns.
        // No matching Unmap: the mapping is released when the resource is.
        unsafe { self.resource.Map(0, Some(&read_range), Some(&mut mapped)) }
            .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

        std::ptr::NonNull::new(mapped.cast::<u8>()).ok_or_else(|| RendererError::InvalidConfig {
            reason: "mapping an upload buffer produced a null pointer".to_owned(),
        })
    }

    /// Copies `data` into an upload-heap buffer.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if this is not an upload buffer or the
    /// data does not fit, or [`RendererError::Api`] if mapping fails.
    pub fn write<T: Copy>(&self, data: &[T]) -> Result<(), RendererError> {
        if !self.is_upload() {
            return Err(RendererError::InvalidConfig {
                reason: "only an upload-heap buffer can be written by the CPU".to_owned(),
            });
        }
        let bytes = std::mem::size_of_val(data);
        if bytes > self.size {
            return Err(RendererError::InvalidConfig {
                reason: format!("{bytes} bytes do not fit in a {} byte buffer", self.size),
            });
        }
        if bytes == 0 {
            return Ok(());
        }

        let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
        // A read range of 0..0 states that the CPU will not read this buffer back, which
        // matters on discrete GPUs where the memory is write-combined.
        let read_range = windows::Win32::Graphics::Direct3D12::D3D12_RANGE { Begin: 0, End: 0 };
        // SAFETY: FFI call mapping subresource 0 of an upload-heap buffer this object owns.
        unsafe { self.resource.Map(0, Some(&read_range), Some(&mut mapped)) }
            .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

        // SAFETY: `mapped` points to at least `self.size` writable bytes, `bytes` is not
        // greater, and the regions cannot overlap — `data` is caller memory and the
        // mapping is GPU-visible memory. The buffer is unmapped immediately after.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), mapped.cast::<u8>(), bytes);
            self.resource.Unmap(0, None);
        }
        Ok(())
    }
}

impl TrackedResource for GpuBuffer {
    fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    fn state(&self) -> D3D12_RESOURCE_STATES {
        self.state.get()
    }

    fn set_state(&self, state: D3D12_RESOURCE_STATES) {
        self.state.set(state);
    }
}

/// A 2D texture: a render target, a depth buffer, or a sampled image.
pub struct GpuTexture {
    resource: ID3D12Resource,
    width: u32,
    height: u32,
    /// Subresources in the chain, counting the full-size level. One for a render target, a
    /// depth buffer or an unmipped upload; `mipmap::level_count` for a sampled texture.
    ///
    /// Kept on the texture because the uploader has to know how many subresources to fill
    /// and the SRV has to say how many to read, and asking the resource description at each
    /// of those points would be two places that could disagree with the third.
    mip_levels: u32,
    format: DXGI_FORMAT,
    state: Cell<D3D12_RESOURCE_STATES>,
}

impl GpuTexture {
    /// Wraps an existing resource whose state is known.
    ///
    /// Used for swap-chain back buffers, which DXGI creates and the engine only borrows.
    /// A back buffer is in `PRESENT` when the swap chain hands it over.
    #[must_use]
    pub fn adopt(
        resource: ID3D12Resource,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        state: D3D12_RESOURCE_STATES,
    ) -> Self {
        Self {
            resource,
            width,
            height,
            mip_levels: 1,
            format,
            state: Cell::new(state),
        }
    }

    /// Creates a render-target texture, starting in `RENDER_TARGET`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn render_target(
        device: &RenderDevice,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        clear_color: [f32; 4],
    ) -> Result<Self, RendererError> {
        let clear_value = D3D12_CLEAR_VALUE {
            Format: format,
            Anonymous: D3D12_CLEAR_VALUE_0 { Color: clear_color },
        };
        let resource = device.create_texture_2d(
            width,
            height,
            1,
            format,
            D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            Some(clear_value),
        )?;
        Ok(Self {
            resource,
            width,
            height,
            mip_levels: 1,
            format,
            state: Cell::new(D3D12_RESOURCE_STATE_RENDER_TARGET),
        })
    }

    /// Creates a depth-stencil texture, starting in `DEPTH_WRITE`.
    ///
    /// `clear_depth` becomes the resource's optimized clear value. Clearing with a
    /// different value later is slower on some hardware, so the depth the renderer clears
    /// to and the one stated here must agree.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn depth_buffer(
        device: &RenderDevice,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        clear_depth: f32,
    ) -> Result<Self, RendererError> {
        let clear_value = D3D12_CLEAR_VALUE {
            Format: format,
            Anonymous: D3D12_CLEAR_VALUE_0 {
                DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                    Depth: clear_depth,
                    Stencil: 0,
                },
            },
        };
        let resource = device.create_texture_2d(
            width,
            height,
            1,
            format,
            D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            Some(clear_value),
        )?;
        Ok(Self {
            resource,
            width,
            height,
            mip_levels: 1,
            format,
            state: Cell::new(D3D12_RESOURCE_STATE_DEPTH_WRITE),
        })
    }

    /// Creates a depth texture that can also be sampled.
    ///
    /// Two things separate this from [`GpuTexture::depth_buffer`], and both are required
    /// together: the resource is created **typeless**, and the depth-stencil view names the
    /// typed format instead of inheriting it. D3D12 refuses a shader-resource view of a
    /// `D32_FLOAT` resource — a depth format is not readable as colour — so a shadow map,
    /// which is written as depth and read as a value, has to be `R32_TYPELESS` with a
    /// `D32_FLOAT` DSV and an `R32_FLOAT` SRV over the same memory.
    ///
    /// The optimized clear value still names the typed format: it describes what a *clear*
    /// writes, and a clear goes through the depth-stencil view.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn sampleable_depth(
        device: &RenderDevice,
        width: u32,
        height: u32,
        clear_depth: f32,
    ) -> Result<Self, RendererError> {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R32_TYPELESS,
        };
        let clear_value = D3D12_CLEAR_VALUE {
            Format: DXGI_FORMAT_D32_FLOAT,
            Anonymous: D3D12_CLEAR_VALUE_0 {
                DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                    Depth: clear_depth,
                    Stencil: 0,
                },
            },
        };
        let resource = device.create_texture_2d(
            width,
            height,
            1,
            DXGI_FORMAT_R32_TYPELESS,
            D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            Some(clear_value),
        )?;
        Ok(Self {
            resource,
            width,
            height,
            mip_levels: 1,
            format: DXGI_FORMAT_R32_TYPELESS,
            state: Cell::new(D3D12_RESOURCE_STATE_DEPTH_WRITE),
        })
    }

    /// Creates a plain texture with no render-target or depth flags.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn new(
        device: &RenderDevice,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        initial_state: D3D12_RESOURCE_STATES,
    ) -> Result<Self, RendererError> {
        Self::with_mips(device, width, height, 1, format, initial_state)
    }

    /// Creates a plain texture with `mip_levels` subresources.
    ///
    /// The chain is *allocated* here and filled by
    /// [`crate::upload::StagingUploader::queue_texture_mips`]. A texture whose lower levels
    /// were never written samples garbage at distance rather than failing, so the two calls
    /// belong together and the uploader checks the count it is given against this one.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the device rejects the request.
    pub fn with_mips(
        device: &RenderDevice,
        width: u32,
        height: u32,
        mip_levels: u32,
        format: DXGI_FORMAT,
        initial_state: D3D12_RESOURCE_STATES,
    ) -> Result<Self, RendererError> {
        let mip_levels = mip_levels.max(1);
        let resource = device.create_texture_2d(
            width,
            height,
            mip_levels,
            format,
            D3D12_RESOURCE_FLAG_NONE,
            initial_state,
            None,
        )?;
        Ok(Self {
            resource,
            width,
            height,
            mip_levels,
            format,
            state: Cell::new(initial_state),
        })
    }

    /// Subresources in this texture's mip chain, counting the full-size level.
    #[must_use]
    pub fn mip_levels(&self) -> u32 {
        self.mip_levels
    }

    /// Size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Pixel format.
    #[must_use]
    pub fn format(&self) -> DXGI_FORMAT {
        self.format
    }

    /// Bytes one texel occupies, or `None` for a format nothing uploads through.
    ///
    /// Only the formats the engine actually stages data into are listed. A format reaching
    /// here that is not one of them means an upload is about to compute the wrong row pitch
    /// and copy garbage, so it returns `None` and the caller reports it rather than guessing
    /// a size from a format it does not understand.
    #[must_use]
    pub fn bytes_per_texel(&self) -> Option<usize> {
        self.layout()
            .filter(|layout| layout.block == 1)
            .map(|layout| layout.bytes)
    }

    /// How this texture's format is laid out in memory, or `None` for a format nothing
    /// uploads through.
    ///
    /// Block-compressed formats do not have a bytes-per-texel at all — BC1 is eight bytes for
    /// a 4x4 block whatever a single texel might mean — so the uploader asks this instead and
    /// computes row pitches in blocks. Keeping the two in one table is what stops a new
    /// format being added to one and forgotten in the other.
    #[must_use]
    pub fn layout(&self) -> Option<FormatLayout> {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC1_UNORM_SRGB, DXGI_FORMAT_BC5_UNORM,
            DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        };
        match self.format {
            DXGI_FORMAT_R8_UNORM => Some(FormatLayout { block: 1, bytes: 1 }),
            // sRGB is the same four bytes; the colour space is in how the *view* reads them,
            // not in how many bytes a row occupies.
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
                Some(FormatLayout { block: 1, bytes: 4 })
            }
            DXGI_FORMAT_BC1_UNORM | DXGI_FORMAT_BC1_UNORM_SRGB => {
                Some(FormatLayout { block: 4, bytes: 8 })
            }
            DXGI_FORMAT_BC5_UNORM => Some(FormatLayout {
                block: 4,
                bytes: 16,
            }),
            _ => None,
        }
    }
}

/// How many bytes one block of a format occupies, and how many texels a block covers.
///
/// An uncompressed format is described as a block of one texel, so the row-pitch arithmetic
/// is the same expression for both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatLayout {
    /// Texels along each edge of a block. 1 for an uncompressed format, 4 for BC.
    pub block: u32,
    /// Bytes one block occupies.
    pub bytes: usize,
}

impl FormatLayout {
    /// Bytes one row of blocks occupies for an image `width` texels across.
    #[must_use]
    pub fn row_pitch(&self, width: u32) -> usize {
        width.div_ceil(self.block) as usize * self.bytes
    }

    /// Rows of blocks in an image `height` texels tall.
    #[must_use]
    pub fn rows(&self, height: u32) -> usize {
        height.div_ceil(self.block) as usize
    }

    /// Bytes a whole image of this size occupies.
    #[must_use]
    pub fn image_size(&self, width: u32, height: u32) -> usize {
        self.row_pitch(width) * self.rows(height)
    }
}

impl TrackedResource for GpuTexture {
    fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    fn state(&self) -> D3D12_RESOURCE_STATES {
        self.state.get()
    }

    fn set_state(&self, state: D3D12_RESOURCE_STATES) {
        self.state.set(state);
    }
}

/// Resources awaiting release, each with the fence value that must pass first.
///
/// The alternative — blocking until the GPU is idle before every release — is correct and
/// costs a full pipeline drain. This costs a push and a comparison per collected resource.
///
/// # Status: staged, not used by the production render path
///
/// The renderer owns one of these and calls [`DeferredReleaseQueue::collect`] every frame,
/// but **nothing calls [`DeferredReleaseQueue::release`] outside the tests**. Every
/// destruction the renderer performs today — replacing the stress scene, resizing the swap
/// chain — happens behind an explicit `wait_for_idle`, because both are rare and neither is
/// on a measured path.
///
/// That is a deliberate choice, not an oversight. Wiring this up needs a destruction API,
/// and the renderer has none: `create_mesh` and `create_material` push onto a `Vec` that is
/// never shortened, because nothing streams content in or out yet. Adding a `destroy_mesh`
/// purely to give this queue a caller would be inventing a workload to justify machinery.
///
/// * **Expected to be used by:** asset streaming, when a level can load and unload geometry
///   while frames are in flight. At that point a full pipeline drain per unload becomes a
///   visible hitch, which is exactly the cost this removes.
/// * **Intentionally staged.** The mechanism is correct and tested; what is missing is a
///   reason to use it.
///
/// Until then, the honest answer to "what stops a resource being freed while the GPU reads
/// it?" is: an explicit idle wait, everywhere.
#[derive(Default)]
pub struct DeferredReleaseQueue {
    /// Kept in submission order, so collection can stop at the first entry not yet passed.
    pending: Vec<(u64, ID3D12Resource)>,
    released: u64,
    peak_pending: usize,
}

impl DeferredReleaseQueue {
    /// Creates an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedules `resource` for release once the GPU has passed `fence_value`.
    ///
    /// The caller must pass the value covering the resource's *last* use. Passing an
    /// earlier one releases the resource while the GPU may still be reading it, which is
    /// the exact bug this queue exists to prevent.
    pub fn release(&mut self, fence_value: u64, resource: ID3D12Resource) {
        self.pending.push((fence_value, resource));
        self.peak_pending = self.peak_pending.max(self.pending.len());
    }

    /// Drops every resource whose fence value the GPU has passed.
    ///
    /// Returns how many were released.
    pub fn collect(&mut self, completed_value: u64) -> usize {
        // Entries are pushed in increasing fence order, so everything releasable is a
        // prefix; the first entry that is not yet passed ends the scan.
        let releasable = self
            .pending
            .iter()
            .position(|(value, _)| *value > completed_value)
            .unwrap_or(self.pending.len());

        if releasable > 0 {
            self.pending.drain(..releasable);
            self.released += releasable as u64;
        }
        releasable
    }

    /// Drops everything, regardless of fence values.
    ///
    /// Only valid after the GPU has gone idle — at shutdown, or around a resize.
    pub fn clear(&mut self) {
        self.released += self.pending.len() as u64;
        self.pending.clear();
    }

    /// Resources waiting for their fence value.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Resources released since the queue was created.
    #[must_use]
    pub fn released(&self) -> u64 {
        self.released
    }

    /// Largest number of resources ever waiting at once.
    ///
    /// A number that keeps growing means resources are being released faster than frames
    /// complete, which is a leak in slow motion.
    #[must_use]
    pub fn peak_pending(&self) -> usize {
        self.peak_pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue's ordering logic, exercised without D3D12 resources.
    ///
    /// `DeferredReleaseQueue` holds COM pointers, which cannot be fabricated in a unit
    /// test; this mirrors its `collect` rule exactly so the rule itself is tested without
    /// a GPU. The GPU tests then confirm the real queue behaves the same.
    fn releasable_prefix(pending: &[u64], completed: u64) -> usize {
        pending
            .iter()
            .position(|value| *value > completed)
            .unwrap_or(pending.len())
    }

    #[test]
    fn nothing_is_released_before_its_fence_value_passes() {
        let pending = [10, 20, 30];
        assert_eq!(releasable_prefix(&pending, 0), 0);
        assert_eq!(releasable_prefix(&pending, 9), 0);
    }

    #[test]
    fn everything_up_to_the_completed_value_is_released() {
        let pending = [10, 20, 30];
        assert_eq!(releasable_prefix(&pending, 10), 1);
        assert_eq!(releasable_prefix(&pending, 25), 2);
        assert_eq!(releasable_prefix(&pending, 30), 3);
        assert_eq!(
            releasable_prefix(&pending, 1000),
            3,
            "a value beyond every entry releases them all"
        );
    }

    #[test]
    fn collection_stops_at_the_first_unpassed_entry() {
        // Entries share fence values when several resources die in one frame. The scan
        // must still stop at the first one the GPU has not reached.
        let pending = [10, 10, 10, 40, 40];
        assert_eq!(releasable_prefix(&pending, 10), 3);
        assert_eq!(releasable_prefix(&pending, 39), 3);
        assert_eq!(releasable_prefix(&pending, 40), 5);
    }

    #[test]
    fn an_empty_queue_releases_nothing() {
        assert_eq!(releasable_prefix(&[], 100), 0);
    }

    #[test]
    fn a_new_queue_is_empty() {
        let queue = DeferredReleaseQueue::new();
        assert_eq!(queue.pending(), 0);
        assert_eq!(queue.released(), 0);
        assert_eq!(queue.peak_pending(), 0);
    }
}
