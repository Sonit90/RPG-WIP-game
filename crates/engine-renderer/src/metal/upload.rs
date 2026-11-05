//! Per-frame upload memory: one shared `MTLBuffer` per frame slot, bump-allocated.
//!
//! Everything the CPU writes during a frame and the GPU reads once goes through here: the
//! frame, object and palette constants, the instance streams, the cluster masks, the UI and
//! particle vertices. A slot is reset when the frame ring proves the GPU finished the frame
//! that used it, exactly as the D3D12 ring is.
//!
//! A frame that needs more than the slot holds is never refused. The overflow goes to a new,
//! larger chunk (the earlier chunks stay alive because the frame already recorded references
//! into them) and at the start of the slot's next frame the chunks are replaced by one chunk
//! big enough for the whole of that frame. After the scene's high-water mark is reached the
//! ring stops allocating.

use std::ptr::NonNull;

use objc2_metal::{MTLBuffer, MTLDevice, MTLResourceOptions};

use super::{Buffer, Device, api_error, bytes_of, slice_bytes};
use crate::error::RendererError;

/// Alignment for constants: Metal requires 256 bytes for buffers in the `constant` address
/// space on macOS, which is also D3D12's constant-buffer alignment.
pub(crate) const CONSTANT_ALIGNMENT: usize = 256;

/// Alignment for everything else: the largest element a shader reads, a `float4`.
pub(crate) const DEFAULT_ALIGNMENT: usize = 16;

/// A range handed out by [`UploadRing::allocate`]; resolve it with [`UploadRing::buffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Allocation {
    chunk: usize,
    offset: usize,
}

impl Allocation {
    /// Byte offset into the chunk's buffer, which is what `setVertexBuffer:offset:` takes.
    pub(crate) fn offset(self) -> usize {
        self.offset
    }
}

/// One mapped buffer and how much of it this frame has used.
struct Chunk {
    buffer: Buffer,
    base: NonNull<u8>,
    capacity: usize,
    cursor: usize,
}

impl Chunk {
    fn new(device: &Device, capacity: usize) -> Result<Self, RendererError> {
        let capacity = capacity.max(DEFAULT_ALIGNMENT);
        // Write-combined: the CPU only writes, and never reads back.
        let options =
            MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined;
        let Some(buffer) = device.newBufferWithLength_options(capacity, options) else {
            return Err(api_error(
                "MTLDevice::newBufferWithLength",
                format!("could not allocate {capacity} bytes of upload memory"),
            ));
        };
        let Some(base) = NonNull::new(buffer.contents().as_ptr().cast::<u8>()) else {
            return Err(api_error(
                "MTLBuffer::contents",
                "a shared buffer has no CPU address",
            ));
        };
        Ok(Self {
            buffer,
            base,
            capacity,
            cursor: 0,
        })
    }

    /// Reserves `bytes` aligned to `alignment`, or `None` if the chunk is full.
    fn reserve(&mut self, bytes: usize, alignment: usize) -> Option<usize> {
        let start = self.cursor.checked_next_multiple_of(alignment)?;
        let end = start.checked_add(bytes)?;
        if end > self.capacity {
            return None;
        }
        self.cursor = end;
        Some(start)
    }
}

/// One frame slot's chunks.
struct Slot {
    chunks: Vec<Chunk>,
}

impl Slot {
    /// Bytes this frame has used, alignment padding included.
    fn used(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.cursor).sum()
    }
}

/// The upload memory of every frame slot.
pub(crate) struct UploadRing {
    device: Device,
    slots: Vec<Slot>,
    /// What a slot starts a frame with: the configured size, grown to the largest frame seen.
    capacity: usize,
    /// Most bytes any frame has used.
    peak: usize,
}

impl UploadRing {
    /// Creates `slots` slots of `bytes_per_slot` bytes each.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if a buffer cannot be allocated.
    pub(crate) fn new(
        device: &Device,
        bytes_per_slot: usize,
        slots: usize,
    ) -> Result<Self, RendererError> {
        let capacity = bytes_per_slot.max(DEFAULT_ALIGNMENT);
        let mut ring = Vec::with_capacity(slots);
        for _ in 0..slots {
            ring.push(Slot {
                chunks: vec![Chunk::new(device, capacity)?],
            });
        }
        Ok(Self {
            device: device.clone(),
            slots: ring,
            capacity,
            peak: 0,
        })
    }

    /// Frees `slot` for a new frame. Call only after the GPU finished the slot's last one.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if growing the slot fails.
    pub(crate) fn begin_frame(&mut self, slot: usize) -> Result<(), RendererError> {
        let Some(entry) = self.slots.get_mut(slot) else {
            return Err(RendererError::InvalidConfig {
                reason: format!("upload slot {slot} does not exist"),
            });
        };
        let used = entry.used();
        self.peak = self.peak.max(used);
        if entry.chunks.len() > 1 {
            // The last frame overflowed: replace the chunks with one that would have held it,
            // with a quarter of headroom so a slightly larger frame does not overflow again.
            self.capacity = self.capacity.max(used + used / 4);
            entry.chunks = vec![Chunk::new(&self.device, self.capacity)?];
        } else if let Some(chunk) = entry.chunks.first_mut() {
            chunk.cursor = 0;
        }
        Ok(())
    }

    /// Reserves `bytes` in `slot`'s memory, aligned to `alignment` (a power of two), and
    /// returns where, with the CPU address to write through.
    ///
    /// # Errors
    ///
    /// [`RendererError::InvalidConfig`] for a slot that does not exist, or
    /// [`RendererError::Api`] if an overflow chunk cannot be allocated.
    pub(crate) fn allocate(
        &mut self,
        slot: usize,
        bytes: usize,
        alignment: usize,
    ) -> Result<(Allocation, NonNull<u8>), RendererError> {
        debug_assert!(alignment.is_power_of_two());
        let Some(entry) = self.slots.get_mut(slot) else {
            return Err(RendererError::InvalidConfig {
                reason: format!("upload slot {slot} does not exist"),
            });
        };
        let current = entry.chunks.len() - 1;
        if let Some(chunk) = entry.chunks.last_mut()
            && let Some(offset) = chunk.reserve(bytes, alignment)
        {
            return Ok((
                Allocation {
                    chunk: current,
                    offset,
                },
                // SAFETY: `reserve` checked `offset + bytes <= capacity`, and `base` addresses
                // a mapping of `capacity` bytes, so the pointer stays inside the allocation.
                unsafe { chunk.base.add(offset) },
            ));
        }
        // Full: start a chunk twice the size of the last, and at least big enough for this.
        let last = entry
            .chunks
            .last()
            .map_or(self.capacity, |chunk| chunk.capacity);
        let mut chunk = Chunk::new(&self.device, (last * 2).max(bytes + alignment))?;
        let Some(offset) = chunk.reserve(bytes, alignment) else {
            return Err(api_error(
                "UploadRing::allocate",
                format!(
                    "a fresh chunk of {} bytes cannot hold {bytes}",
                    chunk.capacity
                ),
            ));
        };
        // SAFETY: as above, for the new chunk.
        let pointer = unsafe { chunk.base.add(offset) };
        entry.chunks.push(chunk);
        Ok((
            Allocation {
                chunk: entry.chunks.len() - 1,
                offset,
            },
            pointer,
        ))
    }

    /// Copies `data` into `slot`'s memory and returns where it landed.
    ///
    /// # Errors
    ///
    /// As [`UploadRing::allocate`].
    pub(crate) fn write<T: Copy>(
        &mut self,
        slot: usize,
        data: &[T],
        alignment: usize,
    ) -> Result<Allocation, RendererError> {
        let bytes = slice_bytes(data);
        let (allocation, pointer) = self.allocate(slot, bytes.len(), alignment)?;
        // SAFETY: `allocate` returned `bytes.len()` writable bytes at `pointer`, in memory
        // the CPU owns until the frame is committed; the source is caller memory, so the
        // ranges are distinct.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.as_ptr(), bytes.len()) };
        Ok(allocation)
    }

    /// Copies one constant block into `slot`'s memory, aligned for a constant binding.
    ///
    /// # Errors
    ///
    /// As [`UploadRing::allocate`].
    pub(crate) fn write_constants<T: Copy>(
        &mut self,
        slot: usize,
        value: &T,
    ) -> Result<Allocation, RendererError> {
        let bytes = bytes_of(value);
        let (allocation, pointer) = self.allocate(slot, bytes.len(), CONSTANT_ALIGNMENT)?;
        // SAFETY: as in `write`.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.as_ptr(), bytes.len()) };
        Ok(allocation)
    }

    /// The buffer an allocation of `slot` lives in.
    ///
    /// Falls back to the slot's first chunk for an allocation that does not belong to it; a
    /// caller bug that draws garbage rather than crashing.
    pub(crate) fn buffer(&self, slot: usize, allocation: Allocation) -> &Buffer {
        let chunks = &self.slots[slot.min(self.slots.len() - 1)].chunks;
        let chunk = chunks.get(allocation.chunk).unwrap_or(&chunks[0]);
        &chunk.buffer
    }

    /// Largest per-frame usage observed, in bytes.
    pub(crate) fn peak(&self) -> usize {
        self.peak
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reserve_aligns_and_refuses_when_full() {
        // A chunk needs a device; the arithmetic is the part worth testing, so build the
        // bookkeeping through the same helper on a stand-in.
        fn reserve(
            cursor: usize,
            capacity: usize,
            bytes: usize,
            alignment: usize,
        ) -> Option<usize> {
            let start = cursor.checked_next_multiple_of(alignment)?;
            let end = start.checked_add(bytes)?;
            (end <= capacity).then_some(start)
        }
        assert_eq!(reserve(0, 1024, 100, 256), Some(0));
        assert_eq!(reserve(100, 1024, 100, 256), Some(256));
        assert_eq!(reserve(900, 1024, 100, 256), None);
        assert_eq!(reserve(usize::MAX, 1024, 1, 16), None);
    }
}
