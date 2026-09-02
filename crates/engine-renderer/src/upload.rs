//! Getting data to the GPU.
//!
//! Two paths, because the two kinds of upload have nothing in common except the word:
//!
//! | Path | Data | Lifetime | Cost |
//! | --- | --- | --- | --- |
//! | [`UploadRing`] | constants, instance transforms, dynamic vertices | one frame | a bump and a `memcpy` |
//! | [`StagingUploader`] | mesh and texture data | forever | one command list and one wait, batched |
//!
//! ## The ring
//!
//! One permanently mapped upload buffer per frame slot, allocated once. Writing per-frame
//! data is a bump of an offset and a copy into memory the GPU can already see — no
//! resource creation, no mapping, no barrier. The slot is reclaimed whole when the frame
//! ring proves the GPU has finished with it, exactly like the descriptor slices.
//!
//! Upload-heap memory is usually write-combined on a discrete GPU: writes are fast,
//! **reads are catastrophically slow**. Nothing here reads back, and
//! [`UploadAllocation::write`] only ever writes forward.
//!
//! ## Batched staging
//!
//! Static data has to end up on a `DEFAULT` heap, which the CPU cannot touch, so it goes
//! through a staging copy. Doing that one resource at a time means a staging buffer, a
//! command allocator, a command list and a full GPU wait *per mesh* — which is what the
//! Phase 0 mesh upload did. [`StagingUploader`] queues copies into one list and waits once,
//! so loading twenty meshes costs one synchronization rather than twenty.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATES,
    D3D12_SUBRESOURCE_FOOTPRINT, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    ID3D12CommandList,
};
use windows::core::Interface;

use crate::command::CommandContext;
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::{BufferSlice, GpuBuffer, GpuTexture, TrackedResource};
use crate::sync::GpuFence;

/// Alignment D3D12 requires for the row pitch of a texture upload.
///
/// `D3D12_TEXTURE_DATA_PITCH_ALIGNMENT`. A staging buffer whose rows are not padded to this
/// produces a sheared texture rather than an error, which is why the padding happens in one
/// place rather than at each call site.
pub const TEXTURE_ROW_PITCH_ALIGNMENT: usize = 256;

/// Alignment D3D12 requires for a constant-buffer view's start address.
pub const CONSTANT_BUFFER_ALIGNMENT: usize = 256;

/// Alignment D3D12 requires for the start of texture data in a staging buffer.
pub const TEXTURE_DATA_ALIGNMENT: usize = 512;

/// Default alignment for vertex, index and structured data.
///
/// Sixteen bytes covers every scalar and vector type the engine uses and keeps a
/// `float4`-shaped element from straddling two cache lines.
pub const DEFAULT_ALIGNMENT: usize = 16;

/// A reserved span of an upload buffer.
///
/// Carries both addresses because the two consumers differ: the CPU writes through the
/// pointer, and the GPU is given the virtual address in a view or a copy.
#[derive(Debug, Clone, Copy)]
pub struct UploadAllocation {
    /// Where the CPU writes.
    pointer: std::ptr::NonNull<u8>,
    /// Where the GPU reads.
    gpu_address: u64,
    /// Byte offset within the slot's buffer, for `CopyBufferRegion`.
    offset: u64,
    size: usize,
}

impl UploadAllocation {
    /// GPU virtual address of the span.
    #[must_use]
    pub fn gpu_address(&self) -> u64 {
        self.gpu_address
    }

    /// Byte offset within the slot's upload buffer.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Size of the span in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Copies `data` into the span.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if `data` does not fit.
    pub fn write<T: Copy>(&self, data: &[T]) -> Result<(), RendererError> {
        let bytes = std::mem::size_of_val(data);
        if bytes > self.size {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{bytes} bytes do not fit in a {} byte allocation",
                    self.size
                ),
            });
        }
        if bytes == 0 {
            return Ok(());
        }

        // SAFETY: `pointer` addresses `self.size` writable bytes inside a mapped upload
        // buffer that outlives this allocation, and `bytes` is not greater. Source and
        // destination cannot overlap: `data` is caller memory, the destination is
        // GPU-visible memory owned by the ring.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), self.pointer.as_ptr(), bytes);
        }
        Ok(())
    }
}

/// Usage of one frame's upload slot.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UploadStats {
    /// Bytes the slot can hand out.
    pub capacity: usize,
    /// Bytes handed out this frame, including alignment padding.
    pub used: usize,
    /// Largest `used` ever reached, across resets.
    pub peak: usize,
    /// Allocations refused because the slot was full.
    pub failures: u32,
}

impl UploadStats {
    /// Bytes still available this frame.
    #[must_use]
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.used)
    }
}

/// One frame slot's upload buffer, permanently mapped.
struct UploadSlot {
    /// Kept alive because `pointer` maps it and `gpu_base` addresses it.
    buffer: GpuBuffer,
    pointer: std::ptr::NonNull<u8>,
    gpu_base: u64,
    cursor: usize,
    capacity: usize,
    peak: usize,
    failures: u32,
}

impl UploadSlot {
    fn new(device: &RenderDevice, capacity: usize) -> Result<Self, RendererError> {
        let buffer = GpuBuffer::new_upload(device, capacity)?;
        let pointer = buffer.map_persistent()?;
        Ok(Self {
            gpu_base: buffer.gpu_address(),
            buffer,
            pointer,
            cursor: 0,
            capacity,
            peak: 0,
            failures: 0,
        })
    }

    /// Reserves `size` bytes aligned to `align`.
    fn allocate(&mut self, size: usize, align: usize) -> Option<UploadAllocation> {
        debug_assert!(align.is_power_of_two(), "alignment must be a power of two");

        let start = (self.cursor + align - 1) & !(align - 1);
        let end = start.checked_add(size)?;
        if end > self.capacity {
            self.failures = self.failures.saturating_add(1);
            return None;
        }

        self.cursor = end;
        self.peak = self.peak.max(end);

        // SAFETY: `start` is below `capacity`, which is the mapped length, so the offset
        // stays inside the allocation the pointer came from.
        let pointer = unsafe { std::ptr::NonNull::new_unchecked(self.pointer.as_ptr().add(start)) };
        Some(UploadAllocation {
            pointer,
            gpu_address: self.gpu_base + start as u64,
            offset: start as u64,
            size,
        })
    }

    fn reset(&mut self) {
        self.cursor = 0;
    }

    fn stats(&self) -> UploadStats {
        UploadStats {
            capacity: self.capacity,
            used: self.cursor,
            peak: self.peak,
            failures: self.failures,
        }
    }
}

/// Per-frame upload memory: one mapped buffer per frame slot, bump-allocated.
pub struct UploadRing {
    slots: Vec<UploadSlot>,
}

impl UploadRing {
    /// Creates one buffer of `bytes_per_frame` for each of `frames_in_flight` slots.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero slot count or size, or
    /// [`RendererError::Api`] if a buffer cannot be created or mapped.
    pub fn new(
        device: &RenderDevice,
        bytes_per_frame: usize,
        frames_in_flight: usize,
    ) -> Result<Self, RendererError> {
        if frames_in_flight == 0 || bytes_per_frame == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "an upload ring needs at least one slot of non-zero size, \
                     got {frames_in_flight} x {bytes_per_frame}"
                ),
            });
        }

        let slots = (0..frames_in_flight)
            .map(|_| UploadSlot::new(device, bytes_per_frame))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { slots })
    }

    /// Number of frame slots.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Reclaims slot `slot`'s whole buffer.
    ///
    /// Safe only once the GPU has finished the frame that used it — which the frame ring
    /// establishes before `begin_frame` returns.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is out of range, which would be an engine bug.
    pub fn begin_frame(&mut self, slot: usize) {
        self.slots[slot].reset();
    }

    /// Reserves `size` bytes aligned to `align` in slot `slot`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the slot cannot fit the request,
    /// naming what was asked for and what is left — a full ring is a sizing mistake and
    /// the message should be enough to fix it.
    pub fn allocate(
        &mut self,
        slot: usize,
        size: usize,
        align: usize,
    ) -> Result<UploadAllocation, RendererError> {
        self.slots[slot]
            .allocate(size, align)
            .ok_or_else(|| RendererError::InvalidConfig {
                reason: format!(
                    "upload slot {slot} cannot fit {size} bytes ({} of {} used)",
                    self.slots[slot].cursor, self.slots[slot].capacity
                ),
            })
    }

    /// Reserves space for `data` and copies it in.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the slot cannot fit the data.
    pub fn write<T: Copy>(
        &mut self,
        slot: usize,
        data: &[T],
        align: usize,
    ) -> Result<UploadAllocation, RendererError> {
        let allocation = self.allocate(slot, std::mem::size_of_val(data), align)?;
        allocation.write(data)?;
        Ok(allocation)
    }

    /// Reserves space for one constant buffer's worth of `value` and copies it in.
    ///
    /// Aligned to [`CONSTANT_BUFFER_ALIGNMENT`], and the *size* is rounded up too: D3D12
    /// requires a constant buffer view's size to be a multiple of 256 bytes, not only its
    /// address.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the slot cannot fit the data.
    pub fn write_constants<T: Copy>(
        &mut self,
        slot: usize,
        value: &T,
    ) -> Result<UploadAllocation, RendererError> {
        let padded = size_of::<T>().next_multiple_of(CONSTANT_BUFFER_ALIGNMENT);
        let allocation = self.allocate(slot, padded, CONSTANT_BUFFER_ALIGNMENT)?;
        allocation.write(std::slice::from_ref(value))?;
        Ok(allocation)
    }

    /// The buffer backing slot `slot`, for `CopyBufferRegion` sources.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is out of range.
    #[must_use]
    pub fn buffer(&self, slot: usize) -> &GpuBuffer {
        &self.slots[slot].buffer
    }

    /// Usage of slot `slot`.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is out of range.
    #[must_use]
    pub fn stats(&self, slot: usize) -> UploadStats {
        self.slots[slot].stats()
    }

    /// Largest usage observed across every slot.
    ///
    /// The number that sizes the ring: capacity should sit above the observed peak with
    /// headroom.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.slots.iter().map(|slot| slot.peak).max().unwrap_or(0)
    }
}

/// How large one chunk of staging memory is, in bytes.
///
/// Four megabytes: a level's whole content is a few of these, and a single asset larger than
/// one gets a chunk of its own rather than being refused.
const STAGING_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Where in the arena a staged copy lives.
#[derive(Debug, Clone, Copy)]
struct StagingSpan {
    chunk: usize,
    offset: usize,
}

/// One permanently mapped upload buffer the queued copies are staged through.
struct StagingChunk {
    buffer: GpuBuffer,
    base: std::ptr::NonNull<u8>,
    used: usize,
}

/// Upload-heap memory for staging, suballocated rather than created per copy.
///
/// # Why
///
/// `CreateCommittedResource` is a driver allocation, and it was measured at roughly half a
/// millisecond each on the reference machine. A model is a staging buffer per mesh buffer,
/// per material and per mip level, so a level's worth of content was several hundred of them
/// — 431 ms of a 1.0 s launch, for about a megabyte of data. A chunk is created once and
/// reused for the life of the uploader; a copy is now a bump of an offset and a `memcpy`.
struct StagingArena {
    chunks: Vec<StagingChunk>,
}

impl StagingArena {
    /// An arena with no memory yet. The first allocation creates the first chunk.
    fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    /// Reserves `bytes` aligned to `alignment`, growing by a chunk when nothing fits.
    fn allocate(
        &mut self,
        device: &RenderDevice,
        bytes: usize,
        alignment: usize,
    ) -> Result<StagingSpan, RendererError> {
        let alignment = alignment.max(1);
        for (index, chunk) in self.chunks.iter_mut().enumerate() {
            let offset = chunk.used.next_multiple_of(alignment);
            if offset + bytes <= chunk.buffer.size() {
                chunk.used = offset + bytes;
                return Ok(StagingSpan {
                    chunk: index,
                    offset,
                });
            }
        }

        // Nothing fits: a new chunk, big enough for this request even if it is a large one.
        let size = STAGING_CHUNK_BYTES.max(bytes.next_multiple_of(alignment));
        let buffer = GpuBuffer::new_upload(device, size)?;
        let base = buffer.map_persistent()?;
        self.chunks.push(StagingChunk {
            buffer,
            base,
            used: bytes,
        });
        Ok(StagingSpan {
            chunk: self.chunks.len() - 1,
            offset: 0,
        })
    }

    /// Copies `data` into a span reserved for it.
    ///
    /// The caller reserved at least `data.len()` bytes at this span, which is the whole of
    /// the safety argument — the pointer is inside a mapped buffer this arena owns and the
    /// span cannot overlap another, because allocation only ever bumps forward.
    fn write(&self, span: StagingSpan, data: &[u8]) {
        let Some(chunk) = self.chunks.get(span.chunk) else {
            debug_assert!(false, "a span names a chunk the arena does not have");
            return;
        };
        debug_assert!(span.offset + data.len() <= chunk.buffer.size());
        // SAFETY: the destination is `data.len()` bytes inside a chunk of mapped
        // upload-heap memory reserved by `allocate` for exactly this write, and the source
        // is a slice. Upload memory is write-combined: this writes forward and never reads.
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                chunk.base.as_ptr().add(span.offset),
                data.len(),
            );
        }
    }

    /// The buffer a span lives in.
    fn buffer(&self, span: StagingSpan) -> Option<&GpuBuffer> {
        self.chunks.get(span.chunk).map(|chunk| &chunk.buffer)
    }

    /// Frees every reservation, keeping the memory. Called once the GPU has finished with it.
    fn reset(&mut self) {
        for chunk in &mut self.chunks {
            chunk.used = 0;
        }
    }
}

/// Batched one-off uploads of static data to device-local memory.
///
/// Queue several copies, then flush once. The alternative — a staging buffer, an allocator,
/// a list and a GPU wait per resource — costs one full synchronization per mesh, which is
/// what Phase 0's mesh upload did and what §14 asks to avoid.
pub struct StagingUploader {
    context: CommandContext,
    allocator: windows::Win32::Graphics::Direct3D12::ID3D12CommandAllocator,
    fence: GpuFence,
    /// Upload-heap memory the queued copies are staged through, reused after each flush.
    staging: StagingArena,
    queued: usize,
    /// Copies performed since creation, and flushes they cost.
    total_copies: u64,
    total_flushes: u64,
}

impl StagingUploader {
    /// Creates an uploader with its own allocator, command list and fence.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any object cannot be created.
    pub fn new(device: &RenderDevice) -> Result<Self, RendererError> {
        let allocator = device.create_command_allocator()?;
        let context = CommandContext::new(device, &allocator)?;
        Ok(Self {
            context,
            allocator,
            fence: GpuFence::new(device)?,
            staging: StagingArena::new(),
            queued: 0,
            total_copies: 0,
            total_flushes: 0,
        })
    }

    /// Queues a copy of `data` into `destination`, leaving it in `final_state`.
    ///
    /// Nothing reaches the GPU until [`StagingUploader::flush`].
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the data does not fit the destination,
    /// or [`RendererError::Api`] if a staging buffer cannot be created.
    pub fn queue_buffer<T: Copy>(
        &mut self,
        device: &RenderDevice,
        destination: &GpuBuffer,
        data: &[T],
        final_state: D3D12_RESOURCE_STATES,
    ) -> Result<(), RendererError> {
        let bytes = std::mem::size_of_val(data);
        if bytes == 0 {
            return Ok(());
        }
        if bytes > destination.size() {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{bytes} bytes do not fit in a {} byte destination",
                    destination.size()
                ),
            });
        }

        self.begin_if_idle()?;

        let span = self.staging.allocate(device, bytes, DEFAULT_ALIGNMENT)?;
        let probe = crate::upload_timing::Probe::start();
        // SAFETY: `data` is a slice of `T: Copy`, so its bytes are initialized and readable
        // for `size_of_val` bytes; the view is read and dropped inside this call.
        let bytes_of = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), bytes) };
        self.staging.write(span, bytes_of);
        probe.copy();

        let Some(staging) = self.staging.buffer(span) else {
            return Err(RendererError::InvalidConfig {
                reason: "the staging arena lost the span it just reserved".to_owned(),
            });
        };

        self.context
            .transition_to(destination, D3D12_RESOURCE_STATE_COPY_DEST);
        self.context.flush_barriers();

        // SAFETY: FFI call on the open list. Both resources outlive the submission — the
        // arena's chunks live as long as this uploader and the flush waits for the GPU
        // before anything is reused, and the destination is owned by the caller.
        unsafe {
            self.context.list().CopyBufferRegion(
                destination.resource(),
                0,
                staging.resource(),
                span.offset as u64,
                bytes as u64,
            );
        }

        self.context.transition_to(destination, final_state);

        self.queued += 1;
        Ok(())
    }

    /// Queues a copy of `data` into part of a buffer, leaving the whole resource in
    /// `final_state`.
    ///
    /// The state is the *resource's*, not the range's: D3D12 tracks a buffer's state whole,
    /// so every slice of an arena chunk shares one, and the caller passes a state that suits
    /// all of them — the renderer's geometry arena is read as vertices, indices and constants
    /// at once.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the data does not fit the slice, or
    /// [`RendererError::Api`] if a D3D12 call fails.
    pub fn queue_slice<T: Copy>(
        &mut self,
        device: &RenderDevice,
        destination: &BufferSlice,
        data: &[T],
        final_state: D3D12_RESOURCE_STATES,
    ) -> Result<(), RendererError> {
        let bytes = std::mem::size_of_val(data);
        if bytes == 0 {
            return Ok(());
        }
        if bytes > destination.size() {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{bytes} bytes do not fit in a {} byte slice",
                    destination.size()
                ),
            });
        }

        self.begin_if_idle()?;

        let span = self.staging.allocate(device, bytes, DEFAULT_ALIGNMENT)?;
        // SAFETY: `data` is a slice of `T: Copy`, so its bytes are initialized and readable
        // for `size_of_val` bytes; the view is read and dropped inside this call.
        let bytes_of = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), bytes) };
        self.staging.write(span, bytes_of);

        let Some(staging) = self.staging.buffer(span) else {
            return Err(RendererError::InvalidConfig {
                reason: "the staging arena lost the span it just reserved".to_owned(),
            });
        };

        self.context
            .transition_to(destination.buffer(), D3D12_RESOURCE_STATE_COPY_DEST);
        self.context.flush_barriers();

        // SAFETY: FFI call on the open list. The destination chunk is kept alive by the
        // slice the caller holds, and the staging arena's chunks live as long as this
        // uploader and are not reused until the flush has waited for the GPU.
        unsafe {
            self.context.list().CopyBufferRegion(
                destination.buffer().resource(),
                destination.offset() as u64,
                staging.resource(),
                span.offset as u64,
                bytes as u64,
            );
        }

        self.context
            .transition_to(destination.buffer(), final_state);

        self.queued += 1;
        Ok(())
    }

    /// Queues a copy of 8-bit texel `data` into a 2D texture, leaving it in `final_state`.
    ///
    /// Only single-byte formats are accepted, which is what the font atlas is. A wider
    /// format would need the caller's bytes-per-texel, and there is no second caller yet;
    /// guessing one from the `DXGI_FORMAT` would be a table to keep in step with nothing.
    ///
    /// The source rows are copied into a staging buffer whose row pitch is padded up to
    /// D3D12's 256-byte requirement. The font atlas is already 256 texels wide, so in
    /// practice this is a straight `memcpy`; the padding path exists so a future atlas of
    /// another width cannot silently produce a sheared texture.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the data does not describe the texture,
    /// or [`RendererError::Api`] if a staging buffer cannot be created.
    pub fn queue_texture(
        &mut self,
        device: &RenderDevice,
        destination: &GpuTexture,
        data: &[u8],
        final_state: D3D12_RESOURCE_STATES,
    ) -> Result<(), RendererError> {
        self.queue_texture_mips(device, destination, &[data], final_state)
    }

    /// Queues a copy of a whole mip chain into a texture.
    ///
    /// `levels` is one buffer per subresource, largest first, and must be exactly as long as
    /// the texture's mip count: a chain with a level missing leaves that subresource
    /// uninitialized, and an uninitialized mip is sampled as whatever the heap held before —
    /// which shows up as a distant surface flickering with someone else's texture rather
    /// than as an error anywhere.
    ///
    /// Each level is staged into its own upload buffer, because each has its own row pitch
    /// and D3D12 requires each row to start on a 256-byte boundary. One buffer holding all
    /// of them with the right offsets is the optimisation this deliberately does not make:
    /// uploads happen when content loads, and the code that would compute those offsets is
    /// the code that gets a mip's footprint wrong.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the level count or any level's byte count
    /// does not match the destination, or [`RendererError::Api`] if a D3D12 call fails.
    pub fn queue_texture_mips(
        &mut self,
        device: &RenderDevice,
        destination: &GpuTexture,
        levels: &[&[u8]],
        final_state: D3D12_RESOURCE_STATES,
    ) -> Result<(), RendererError> {
        let (base_width, base_height) = destination.size();
        if levels.len() != destination.mip_levels() as usize {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{} mip level(s) supplied for a {base_width}x{base_height} texture with {}",
                    levels.len(),
                    destination.mip_levels()
                ),
            });
        }

        // The destination is transitioned once, not once per level: every copy below writes
        // a different subresource of the same resource, and D3D12 transitions all
        // subresources together unless a subresource index is named.
        self.begin_if_idle()?;
        self.context
            .transition_to(destination, D3D12_RESOURCE_STATE_COPY_DEST);
        self.context.flush_barriers();

        for (level, data) in levels.iter().enumerate() {
            self.queue_level(device, destination, level as u32, data)?;
        }

        self.context.transition_to(destination, final_state);
        self.queued += 1;
        Ok(())
    }

    /// Opens the upload list if this is the first copy since the last flush.
    fn begin_if_idle(&mut self) -> Result<(), RendererError> {
        if self.queued != 0 {
            return Ok(());
        }
        // SAFETY: the previous flush waited for the GPU, so no submission from this
        // allocator is still executing — which is Reset's precondition.
        unsafe { self.allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;
        self.context.begin(&self.allocator)
    }

    /// Stages and copies one mip level. The caller has already transitioned the texture.
    fn queue_level(
        &mut self,
        device: &RenderDevice,
        destination: &GpuTexture,
        level: u32,
        data: &[u8],
    ) -> Result<(), RendererError> {
        let (base_width, base_height) = destination.size();
        let (width, height) = crate::mipmap::level_size(base_width, base_height, level);
        let Some(layout) = destination.layout() else {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "no layout is known for format {:?}, so its row pitch cannot be computed",
                    destination.format()
                ),
            });
        };
        let expected = layout.image_size(width, height);
        if data.len() != expected {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{} bytes supplied for mip {level} of a {base_width}x{base_height} texture, \
                     which is {width}x{height} in {:?} and needs {expected}",
                    data.len(),
                    destination.format()
                ),
            });
        }

        // In *blocks*, not texels: for an uncompressed format a block is one texel and this
        // is the arithmetic it always was, and for BC1 or BC5 it is the only arithmetic that
        // is right. A row pitch computed in texels would be four or eight times too small and
        // would upload a texture that looks like diagonal noise.
        let source_pitch = layout.row_pitch(width);
        let padded_pitch = source_pitch.next_multiple_of(TEXTURE_ROW_PITCH_ALIGNMENT);
        let rows = layout.rows(height);
        let staging_size = padded_pitch * rows;

        // 512-byte aligned: D3D12 requires the start of a placed texture footprint to be,
        // and a span that is not produces a copy the debug layer rejects.
        let span = self
            .staging
            .allocate(device, staging_size, TEXTURE_DATA_ALIGNMENT)?;
        if padded_pitch == source_pitch {
            self.staging.write(span, data);
        } else {
            let mut padded = vec![0u8; staging_size];
            for row in 0..rows {
                let from = row * source_pitch;
                let to = row * padded_pitch;
                padded[to..to + source_pitch].copy_from_slice(&data[from..from + source_pitch]);
            }
            self.staging.write(span, &padded);
        }
        let Some(staging) = self.staging.buffer(span) else {
            return Err(RendererError::InvalidConfig {
                reason: "the staging arena lost the span it just reserved".to_owned(),
            });
        };

        let source = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: reinterprets the COM smart pointer as the raw interface pointer the
            // copy location expects, without changing the reference count. The arena's
            // chunks live as long as this uploader and are not reused until the flush has
            // waited for the GPU, so the memory outlives the submission.
            pResource: unsafe { std::mem::transmute_copy(staging.resource()) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: span.offset as u64,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: destination.format(),
                        // Rounded up to whole blocks: D3D12 requires a compressed
                        // footprint's dimensions to be multiples of the block size, so the
                        // 2x2 and 1x1 tail of a mip chain is described as a 4x4 footprint
                        // holding one block. For an uncompressed format this rounds to
                        // itself.
                        Width: width.next_multiple_of(layout.block),
                        Height: height.next_multiple_of(layout.block),
                        Depth: 1,
                        RowPitch: padded_pitch as u32,
                    },
                },
            },
        };
        let target = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above. The destination texture is owned by the caller, which holds
            // it for the renderer's lifetime.
            pResource: unsafe { std::mem::transmute_copy(destination.resource()) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                // A 2D texture with one array slice numbers its subresources by mip level,
                // so this index *is* the level.
                SubresourceIndex: level,
            },
        };

        // SAFETY: FFI call on the open list. Both copy locations borrow resources that
        // outlive the submission — the staging buffer is held in `self.staging` until the
        // flush completes, and the destination is owned by the caller.
        unsafe {
            self.context
                .list()
                .CopyTextureRegion(&target, 0, 0, 0, &source, None);
        }

        Ok(())
    }

    /// Submits every queued copy and waits for the GPU to finish them.
    ///
    /// Blocking is acceptable here and only here: uploads happen when content is loaded,
    /// never per frame. Returns the number of copies performed.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if submission or the wait fails.
    pub fn flush(&mut self, device: &RenderDevice) -> Result<usize, RendererError> {
        if self.queued == 0 {
            return Ok(0);
        }

        self.context.close()?;
        let submission = [Some(
            self.context
                .list()
                .cast::<ID3D12CommandList>()
                .map_err(|error| {
                    RendererError::api("ID3D12GraphicsCommandList::cast<ID3D12CommandList>", error)
                })?,
        )];
        // SAFETY: FFI call submitting a closed list that outlives the call.
        unsafe { device.queue().ExecuteCommandLists(&submission) };

        self.fence.wait_for_idle(device.queue())?;

        // The wait proved every copy finished, so the staging memory is safe to hand out
        // again. The chunks themselves are kept: the whole point of the arena is that the
        // next batch does not pay to create them.
        self.staging.reset();
        let copies = self.queued;
        self.queued = 0;
        self.total_copies += copies as u64;
        self.total_flushes += 1;
        Ok(copies)
    }

    /// Copies queued but not yet submitted.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued
    }

    /// Copies performed since creation.
    #[must_use]
    pub fn total_copies(&self) -> u64 {
        self.total_copies
    }

    /// Flushes performed since creation.
    ///
    /// Each flush is one full GPU synchronization. Copies divided by flushes is how much
    /// the batching actually saved.
    #[must_use]
    pub fn total_flushes(&self) -> u64 {
        self.total_flushes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot's allocation arithmetic, without a GPU.
    ///
    /// Mirrors `UploadSlot::allocate` exactly; the GPU tests then confirm the real slot
    /// behaves the same against a mapped buffer.
    fn bump(cursor: &mut usize, capacity: usize, size: usize, align: usize) -> Option<usize> {
        let start = (*cursor + align - 1) & !(align - 1);
        let end = start.checked_add(size)?;
        if end > capacity {
            return None;
        }
        *cursor = end;
        Some(start)
    }

    #[test]
    fn allocations_respect_their_alignment() {
        let mut cursor = 0;
        assert_eq!(bump(&mut cursor, 4096, 1, 16), Some(0));
        assert_eq!(
            bump(&mut cursor, 4096, 4, 16),
            Some(16),
            "the second allocation must skip to the next 16-byte boundary"
        );
        assert_eq!(
            bump(&mut cursor, 4096, 4, CONSTANT_BUFFER_ALIGNMENT),
            Some(256),
            "a constant buffer must start on a 256-byte boundary"
        );
        assert_eq!(
            bump(&mut cursor, 4096, 4, TEXTURE_DATA_ALIGNMENT),
            Some(512)
        );
    }

    #[test]
    fn a_request_that_does_not_fit_is_refused_without_moving_the_cursor() {
        let mut cursor = 100;
        assert_eq!(bump(&mut cursor, 128, 64, 16), None);
        assert_eq!(cursor, 100, "a refusal must not consume the slot");
        assert_eq!(bump(&mut cursor, 128, 16, 16), Some(112));
    }

    #[test]
    fn an_allocation_at_exactly_the_capacity_succeeds() {
        let mut cursor = 0;
        assert_eq!(bump(&mut cursor, 256, 256, 16), Some(0));
        assert_eq!(cursor, 256);
        assert_eq!(bump(&mut cursor, 256, 1, 1), None, "the slot is now full");
    }

    #[test]
    fn constant_buffer_sizes_are_rounded_up_to_the_required_multiple() {
        // D3D12 requires a constant buffer view's *size* to be a multiple of 256, not just
        // its address, so a 4-byte struct still occupies 256 bytes.
        assert_eq!(4_usize.next_multiple_of(CONSTANT_BUFFER_ALIGNMENT), 256);
        assert_eq!(256_usize.next_multiple_of(CONSTANT_BUFFER_ALIGNMENT), 256);
        assert_eq!(260_usize.next_multiple_of(CONSTANT_BUFFER_ALIGNMENT), 512);
    }

    #[test]
    fn statistics_report_what_is_left() {
        let stats = UploadStats {
            capacity: 1024,
            used: 256,
            peak: 512,
            failures: 0,
        };
        assert_eq!(stats.available(), 768);

        let full = UploadStats {
            capacity: 64,
            used: 64,
            ..UploadStats::default()
        };
        assert_eq!(full.available(), 0);
    }

    #[test]
    fn the_alignment_constants_match_the_d3d12_requirements() {
        const { assert!(CONSTANT_BUFFER_ALIGNMENT == 256) };
        const { assert!(TEXTURE_DATA_ALIGNMENT == 512) };
        const { assert!(DEFAULT_ALIGNMENT.is_power_of_two()) };
    }
}
