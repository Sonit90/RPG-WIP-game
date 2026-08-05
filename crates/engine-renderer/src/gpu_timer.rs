//! GPU frame timing with D3D12 timestamp queries.
//!
//! # Why not wall-clock timing
//!
//! CPU timing around `ExecuteCommandLists` measures *submission*, not execution: the call
//! returns as soon as the driver has queued the work. Timing around a fence wait measures
//! how long the CPU was blocked, which mostly reflects how far ahead the CPU was allowed
//! to run. Neither is GPU time. Only timestamps written by the GPU itself, into a query
//! heap, measure how long the GPU spent on the frame.
//!
//! # Synchronization
//!
//! A timestamp pair is written on the GPU timeline, so it can only be read after the GPU
//! has passed it. Reading it immediately would mean blocking on a fence every frame —
//! which would serialize CPU and GPU and change the very thing being measured.
//!
//! Instead the results are read with a lag. Each frame slot owns its own pair of queries,
//! and a slot's timestamps are read at the start of its *next* use, which is
//! `FRAME_COUNT` frames later — a point where the renderer already waits for that slot's
//! fence for its own reasons. The CPU therefore never blocks to obtain GPU time, and the
//! reported GPU time is up to `FRAME_COUNT` frames stale. For a steady-state frame rate
//! that is irrelevant; when frame cost changes abruptly, the GPU number lags the CPU one
//! by a few frames, and the debug overlay must not be read as if they were simultaneous.
//!
//! # What the number covers
//!
//! The timestamps bracket the whole recorded command list, so the value is the GPU
//! execution span of the frame's own work. It excludes anything the GPU does for other
//! processes, and it excludes time between frames when the queue is idle.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_READBACK, D3D12_QUERY_HEAP_DESC,
    D3D12_QUERY_HEAP_TYPE_TIMESTAMP, D3D12_QUERY_TYPE_TIMESTAMP, D3D12_RANGE, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandQueue, ID3D12Device, ID3D12GraphicsCommandList,
    ID3D12QueryHeap, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};

use crate::error::RendererError;

/// Timestamps recorded per frame slot: one at the start of the list, one at the end.
const QUERIES_PER_FRAME: usize = 2;

/// Bytes per timestamp, which `ResolveQueryData` writes as `u64` ticks.
const BYTES_PER_QUERY: usize = size_of::<u64>();

/// Timestamp query heap, its readback buffer and the queue's tick rate.
pub struct GpuTimer {
    query_heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    /// Timestamp ticks per second on the queue these timestamps come from.
    frequency: u64,
    /// Slots whose timestamps have been resolved and are safe to read once their fence
    /// has been passed.
    resolved: Vec<bool>,
    frame_count: usize,
}

impl GpuTimer {
    /// Creates a timer with `frame_count` independent slots.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the query heap, the readback buffer or the
    /// timestamp frequency cannot be obtained. A queue that does not support timestamps
    /// fails here rather than silently reporting zero.
    pub fn new(
        device: &ID3D12Device,
        queue: &ID3D12CommandQueue,
        frame_count: usize,
    ) -> Result<Self, RendererError> {
        let query_count = frame_count * QUERIES_PER_FRAME;

        let heap_desc = D3D12_QUERY_HEAP_DESC {
            Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
            Count: query_count as u32,
            NodeMask: 0,
        };
        let mut query_heap: Option<ID3D12QueryHeap> = None;
        // SAFETY: FFI call with an initialized descriptor that outlives the call and a
        // valid out-parameter.
        unsafe { device.CreateQueryHeap(&heap_desc, &mut query_heap) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateQueryHeap", error))?;
        let Some(query_heap) = query_heap else {
            return Err(RendererError::InvalidConfig {
                reason: "timestamp query heap creation produced no heap".to_owned(),
            });
        };

        let readback = create_readback_buffer(device, query_count * BYTES_PER_QUERY)?;

        // SAFETY: FFI call on a live queue. It fails only if the queue type cannot
        // support timestamps, which is reported rather than hidden.
        let frequency = unsafe { queue.GetTimestampFrequency() }.map_err(|error| {
            RendererError::api("ID3D12CommandQueue::GetTimestampFrequency", error)
        })?;

        log::debug!("GPU timestamp frequency: {frequency} ticks/s");

        Ok(Self {
            query_heap,
            readback,
            frequency,
            resolved: vec![false; frame_count],
            frame_count,
        })
    }

    /// Records the opening timestamp for `slot`.
    ///
    /// Must be called on an open command list, before the frame's work.
    pub fn begin_frame(&self, list: &ID3D12GraphicsCommandList, slot: usize) {
        // SAFETY: FFI call on an open list. The query index is inside the heap because
        // `slot` is bounded by `frame_count`, which sized the heap.
        unsafe {
            list.EndQuery(
                &self.query_heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                begin_query_index(slot) as u32,
            );
        }
    }

    /// Records the closing timestamp for `slot` and resolves both into the readback
    /// buffer.
    ///
    /// `ResolveQueryData` is a GPU-side copy, so the values become readable only once the
    /// GPU has passed this point — see the module documentation.
    pub fn end_frame(&mut self, list: &ID3D12GraphicsCommandList, slot: usize) {
        let first = begin_query_index(slot);
        // SAFETY: FFI calls on an open list. The query range and the destination offset
        // both lie inside objects this timer owns and sized for `frame_count` slots.
        unsafe {
            list.EndQuery(
                &self.query_heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                end_query_index(slot) as u32,
            );
            list.ResolveQueryData(
                &self.query_heap,
                D3D12_QUERY_TYPE_TIMESTAMP,
                first as u32,
                QUERIES_PER_FRAME as u32,
                &self.readback,
                (first * BYTES_PER_QUERY) as u64,
            );
        }
        self.resolved[slot] = true;
    }

    /// Reads the GPU time recorded for `slot`, in milliseconds.
    ///
    /// # Safety of the read
    ///
    /// The caller must have waited for the fence value signalled after that slot's
    /// submission; otherwise the readback buffer may still be being written. The renderer
    /// calls this immediately after its per-slot pacing wait.
    ///
    /// Returns `None` if the slot has never been resolved, or if the timestamps are not
    /// ordered — which happens if the GPU was reset between them.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if mapping the readback buffer fails.
    pub fn read_frame_ms(&self, slot: usize) -> Result<Option<f64>, RendererError> {
        if !self.resolved.get(slot).copied().unwrap_or(false) {
            return Ok(None);
        }

        let offset = begin_query_index(slot) * BYTES_PER_QUERY;
        let range = D3D12_RANGE {
            Begin: offset,
            End: offset + QUERIES_PER_FRAME * BYTES_PER_QUERY,
        };

        let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: FFI call mapping subresource 0 of a readback buffer, declaring exactly
        // the range this slot owns.
        unsafe { self.readback.Map(0, Some(&range), Some(&mut mapped)) }
            .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

        // SAFETY: the mapping covers the whole buffer, and `offset` plus two `u64`s stays
        // inside it because the buffer was sized for `frame_count * QUERIES_PER_FRAME`
        // timestamps. The values are read unaligned-safe via `read_unaligned`.
        let (begin, end) = unsafe {
            let base = mapped.cast::<u8>().add(offset).cast::<u64>();
            (base.read_unaligned(), base.add(1).read_unaligned())
        };

        // SAFETY: FFI call. A written range of 0..0 states the CPU modified nothing.
        unsafe {
            self.readback
                .Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        }

        Ok(ticks_to_ms(begin, end, self.frequency))
    }

    /// Timestamp ticks per second reported by the queue.
    #[must_use]
    pub fn frequency(&self) -> u64 {
        self.frequency
    }

    /// Number of independent frame slots.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }
}

/// Index of the opening timestamp query for `slot`.
#[must_use]
pub fn begin_query_index(slot: usize) -> usize {
    slot * QUERIES_PER_FRAME
}

/// Index of the closing timestamp query for `slot`.
#[must_use]
pub fn end_query_index(slot: usize) -> usize {
    begin_query_index(slot) + 1
}

/// Converts a timestamp pair into milliseconds.
///
/// Returns `None` when the pair cannot describe a duration: a zero frequency, or an end
/// timestamp that does not follow the beginning, which indicates the GPU was reset
/// between the two queries.
#[must_use]
pub fn ticks_to_ms(begin: u64, end: u64, frequency: u64) -> Option<f64> {
    if frequency == 0 || end < begin {
        return None;
    }
    let ticks = end - begin;
    Some(ticks as f64 * 1000.0 / frequency as f64)
}

/// Creates a readback buffer of `bytes` bytes for resolved query data.
fn create_readback_buffer(
    device: &ID3D12Device,
    bytes: usize,
) -> Result<ID3D12Resource, RendererError> {
    let heap_properties = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes as u64,
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
    // SAFETY: FFI call with initialized descriptors that outlive it. COPY_DEST is the
    // required initial state for a readback buffer that ResolveQueryData writes into.
    unsafe {
        device.CreateCommittedResource(
            &heap_properties,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut buffer,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

    buffer.ok_or_else(|| RendererError::InvalidConfig {
        reason: "timestamp readback buffer creation produced no resource".to_owned(),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn query_indices_do_not_overlap_between_slots() {
        let mut seen = Vec::new();
        for slot in 0..4 {
            seen.push(begin_query_index(slot));
            seen.push(end_query_index(slot));
        }
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "each slot needs its own queries");
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn converts_ticks_to_milliseconds() {
        // 10 MHz clock: 10_000 ticks is exactly 1 ms.
        assert_eq!(ticks_to_ms(0, 10_000, 10_000_000), Some(1.0));
        assert_eq!(ticks_to_ms(5_000, 15_000, 10_000_000), Some(1.0));
        let quarter = ticks_to_ms(0, 2_500, 10_000_000).expect("valid pair");
        assert!((quarter - 0.25).abs() < 1e-12);
    }

    #[test]
    fn zero_length_span_is_zero_not_none() {
        assert_eq!(ticks_to_ms(42, 42, 1_000_000), Some(0.0));
    }

    #[test]
    fn rejects_a_zero_frequency() {
        assert_eq!(ticks_to_ms(0, 1_000, 0), None);
    }

    #[test]
    fn rejects_timestamps_that_run_backwards() {
        assert_eq!(ticks_to_ms(1_000, 999, 10_000_000), None);
    }

    #[test]
    fn realistic_frame_times_land_in_the_expected_range() {
        // A 240 FPS budget is 4.167 ms; at a typical 100 MHz timestamp clock that is
        // 416_700 ticks.
        let ms = ticks_to_ms(0, 416_700, 100_000_000).expect("valid pair");
        assert!((ms - 4.167).abs() < 0.001, "{ms}");
    }
}
