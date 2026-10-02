//! What the GPU's fixed-function counters say about the camera's mesh pass.
//!
//! # Why
//!
//! The mesh pass is pixel-bound at 1440p and above, and about half of it is shadow sampling.
//! Whether that cost is spent once per screen pixel or several times — fragments that are
//! shaded and then hidden, or layers drawn over one another — decides which fix is worth
//! making. A timestamp cannot tell those apart. A `PIPELINE_STATISTICS` query can:
//! `PSInvocations` divided by the pixels on screen is what the shading actually paid for.
//!
//! # Sections
//!
//! The pass is counted in [`SECTIONS`] consecutive, non-overlapping queries — the rigid
//! opaque batches, the characters, and the blended phases — so a ratio above one can be
//! attributed rather than guessed at. D3D12 does not promise that two statistics queries may
//! be open at once, so the sections are sequential and the whole pass is their sum.
//!
//! # Cost and synchronization
//!
//! Off unless asked for (`--pipeline-stats`): statistics queries are cheap on current hardware
//! but not free on every driver, and nothing a timing comparison measures should carry them.
//! Read exactly like [`crate::gpu_timer::GpuTimer`] reads its timestamps — resolved at the end
//! of the frame, read at the slot's next fence wait — so the CPU never blocks for them and the
//! numbers lag the frame by the ring's depth.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_QUERY_DATA_PIPELINE_STATISTICS, D3D12_QUERY_HEAP_DESC,
    D3D12_QUERY_HEAP_TYPE_PIPELINE_STATISTICS, D3D12_QUERY_TYPE_PIPELINE_STATISTICS, D3D12_RANGE,
    ID3D12Device, ID3D12GraphicsCommandList, ID3D12QueryHeap, ID3D12Resource,
};

use crate::error::RendererError;
use crate::gpu_timer::create_readback_buffer;

/// Bytes one resolved statistics query occupies.
const BYTES_PER_QUERY: usize = size_of::<D3D12_QUERY_DATA_PIPELINE_STATISTICS>();

/// How many consecutive sections the mesh pass is counted in.
pub const SECTIONS: usize = 3;

/// The rigid opaque and alpha-tested batches, whole and dithered.
pub const OPAQUE_SECTION: usize = 0;
/// The skinned characters drawn whole.
pub const CHARACTER_SECTION: usize = 1;
/// The transparent phase and the see-through characters: everything blended.
pub const BLENDED_SECTION: usize = 2;

/// Names for a report, indexed by section.
pub const SECTION_NAMES: [&str; SECTIONS] = ["opaque", "characters", "blended"];

/// What one section of one frame's mesh pass made the pipeline do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScenePassStatistics {
    /// Vertex shader invocations.
    pub vertex_invocations: u64,
    /// Primitives that reached the rasterizer.
    pub rasterized_primitives: u64,
    /// Pixel shader invocations: the number this module exists for.
    pub pixel_invocations: u64,
}

impl ScenePassStatistics {
    /// Pixel shader invocations per pixel of a `width` x `height` target. `None` for an empty
    /// target.
    #[must_use]
    pub fn pixel_invocations_per_pixel(&self, width: u32, height: u32) -> Option<f64> {
        let pixels = u64::from(width) * u64::from(height);
        (pixels > 0).then(|| self.pixel_invocations as f64 / pixels as f64)
    }

    /// The two added together.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            vertex_invocations: self.vertex_invocations + other.vertex_invocations,
            rasterized_primitives: self.rasterized_primitives + other.rasterized_primitives,
            pixel_invocations: self.pixel_invocations + other.pixel_invocations,
        }
    }
}

/// One frame's sections.
pub type SceneStatistics = [ScenePassStatistics; SECTIONS];

/// [`SECTIONS`] statistics queries per frame slot, around the camera's mesh pass.
pub struct PipelineStatistics {
    query_heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    /// Slots whose queries have been resolved and may be read once their fence has passed.
    resolved: Vec<bool>,
    /// The section open on the list being recorded, if any, so an `end` without a `begin`
    /// (a frame that failed half way) is a no-op rather than a D3D12 error.
    open: Option<usize>,
}

impl PipelineStatistics {
    /// Creates the heap and its readback buffer for `frame_count` slots.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the query heap or the readback buffer cannot be
    /// created.
    pub fn new(device: &ID3D12Device, frame_count: usize) -> Result<Self, RendererError> {
        let count = frame_count * SECTIONS;
        let heap_desc = D3D12_QUERY_HEAP_DESC {
            Type: D3D12_QUERY_HEAP_TYPE_PIPELINE_STATISTICS,
            Count: count as u32,
            NodeMask: 0,
        };
        let mut query_heap: Option<ID3D12QueryHeap> = None;
        // SAFETY: FFI call with an initialized descriptor that outlives the call and a valid
        // out-parameter.
        unsafe { device.CreateQueryHeap(&heap_desc, &mut query_heap) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateQueryHeap", error))?;
        let Some(query_heap) = query_heap else {
            return Err(RendererError::InvalidConfig {
                reason: "pipeline statistics query heap creation produced no heap".to_owned(),
            });
        };
        let readback = create_readback_buffer(device, count * BYTES_PER_QUERY)?;
        Ok(Self {
            query_heap,
            readback,
            resolved: vec![false; frame_count],
            open: None,
        })
    }

    /// Ends whatever section is open and opens `section`. Everything recorded until the next
    /// `begin` or [`PipelineStatistics::end`] is counted in it.
    pub fn begin(&mut self, list: &ID3D12GraphicsCommandList, slot: usize, section: usize) {
        if slot >= self.resolved.len() || section >= SECTIONS {
            return;
        }
        self.close_open(list, slot);
        // SAFETY: FFI call on an open list; the index is inside the heap because `slot` and
        // `section` are bounded by the counts that sized it.
        unsafe {
            list.BeginQuery(
                &self.query_heap,
                D3D12_QUERY_TYPE_PIPELINE_STATISTICS,
                (slot * SECTIONS + section) as u32,
            );
        }
        self.open = Some(section);
    }

    /// Closes the open section and resolves all of `slot`'s sections into the readback
    /// buffer. A section never opened this frame resolves as zero, which is what it counted.
    pub fn end(&mut self, list: &ID3D12GraphicsCommandList, slot: usize) {
        if slot >= self.resolved.len() {
            return;
        }
        self.close_open(list, slot);
        // SAFETY: FFI call on an open list. The range lies inside the heap and the
        // destination inside the readback buffer, both sized for every slot.
        unsafe {
            list.ResolveQueryData(
                &self.query_heap,
                D3D12_QUERY_TYPE_PIPELINE_STATISTICS,
                (slot * SECTIONS) as u32,
                SECTIONS as u32,
                &self.readback,
                (slot * SECTIONS * BYTES_PER_QUERY) as u64,
            );
        }
        self.resolved[slot] = true;
    }

    fn close_open(&mut self, list: &ID3D12GraphicsCommandList, slot: usize) {
        if let Some(section) = self.open.take() {
            // SAFETY: FFI call on an open list, ending the query this list began.
            unsafe {
                list.EndQuery(
                    &self.query_heap,
                    D3D12_QUERY_TYPE_PIPELINE_STATISTICS,
                    (slot * SECTIONS + section) as u32,
                );
            }
        }
    }

    /// Reads `slot`'s last resolved sections.
    ///
    /// The caller must have waited for that slot's fence, exactly as for
    /// [`crate::gpu_timer::GpuTimer::read_frame_ms`]; the answer describes the frame one lap
    /// ago.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if mapping the readback buffer fails.
    pub fn read(&self, slot: usize) -> Result<Option<SceneStatistics>, RendererError> {
        if !self.resolved.get(slot).copied().unwrap_or(false) {
            return Ok(None);
        }
        let offset = slot * SECTIONS * BYTES_PER_QUERY;
        let range = D3D12_RANGE {
            Begin: offset,
            End: offset + SECTIONS * BYTES_PER_QUERY,
        };
        let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: FFI call mapping subresource 0 of a readback buffer, declaring exactly the
        // range this slot's queries occupy.
        unsafe { self.readback.Map(0, Some(&range), Some(&mut mapped)) }
            .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;
        let mut sections = [ScenePassStatistics::default(); SECTIONS];
        for (section, out) in sections.iter_mut().enumerate() {
            // SAFETY: the mapping covers the buffer, and every record read stays inside this
            // slot's range, which the buffer was sized for. Read unaligned-safe.
            let data = unsafe {
                mapped
                    .cast::<u8>()
                    .add(offset + section * BYTES_PER_QUERY)
                    .cast::<D3D12_QUERY_DATA_PIPELINE_STATISTICS>()
                    .read_unaligned()
            };
            *out = ScenePassStatistics {
                vertex_invocations: data.VSInvocations,
                rasterized_primitives: data.CPrimitives,
                pixel_invocations: data.PSInvocations,
            };
        }
        // SAFETY: FFI call. A written range of 0..0 states the CPU modified nothing.
        unsafe {
            self.readback
                .Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        }
        Ok(Some(sections))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overdraw_is_invocations_over_pixels() {
        let stats = ScenePassStatistics {
            pixel_invocations: 3_000,
            ..ScenePassStatistics::default()
        };
        assert_eq!(stats.pixel_invocations_per_pixel(100, 20), Some(1.5));
        assert_eq!(stats.pixel_invocations_per_pixel(0, 20), None);
        assert_eq!(stats.plus(stats).pixel_invocations, 6_000);
    }
}
