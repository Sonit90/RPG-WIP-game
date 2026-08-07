//! Descriptor heaps and their allocators.
//!
//! # Status: partly staged
//!
//! [`CpuDescriptorHeap`] is live — the swap chain's RTVs and the depth buffer's DSV come
//! from it, and that is every descriptor the renderer creates today.
//!
//! [`GpuDescriptorHeap`] is **not used by the production render path**, deliberately.
//! Nothing samples a texture yet, so the mesh root signature binds three root CBVs: a GPU
//! address written straight into the root arguments, with no descriptor to allocate and no
//! table to bind. That is the cheapest binding D3D12 offers, and while it remains available
//! a shader-visible heap would be pure overhead.
//!
//! It exists now rather than later because the shape it needs is decided by a question that
//! is easier to answer before there are callers than after: one heap or many, and a bump
//! region per frame slot or a general allocator. Both answers are below, both are tested,
//! and both are what the first textured material will need on day one.
//!
//! * **Expected to be used by:** the milestone that gives materials a texture, which turns
//!   the material root CBV into a descriptor table.
//! * **Intentionally staged.** Do not delete it as unused; do not wire it without a caller
//!   that needs a descriptor.
//!
//! Three lifetimes, three strategies. Using one allocator for all of them would mean
//! paying the most expensive strategy's cost for the cheapest case, and the per-draw case
//! is the one that runs thousands of times a second.
//!
//! | Heap | Visibility | Allocation | Reclaimed |
//! | --- | --- | --- | --- |
//! | RTV, DSV | CPU only | free list | when the view is released |
//! | CBV/SRV/UAV — persistent | shader-visible | free list | when the material is released |
//! | CBV/SRV/UAV — per frame | shader-visible | **bump** | whole region, at frame start |
//!
//! ## Why the per-frame region bumps
//!
//! A descriptor table for one draw lives exactly one frame. Tracking it individually — a
//! free list entry, a release call, a lifetime to get wrong — buys nothing, because every
//! one of them dies at the same moment. Bumping an offset costs an add and a compare, and
//! reclaiming a whole frame's worth costs one store.
//!
//! That is the answer to "descriptor allocation must not become a per-object CPU
//! bottleneck": for the per-object case it is not an allocator at all.
//!
//! ## Why the shader-visible heap is one heap
//!
//! Changing the bound descriptor heap is one of the more expensive state changes in D3D12
//! — on some drivers it flushes. One heap, split into regions, means the heap is bound
//! once per frame and never changed. It is also the shape a bindless renderer needs, so
//! that path stays open without being built now.
//!
//! ## Alignment with `frames_in_flight`
//!
//! The per-frame region is divided into one slice per frame slot. A slice may only be
//! reset once the GPU has finished the frame that used it, which is exactly what the frame
//! ring already establishes before `begin_frame` returns.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_TYPE,
    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_GPU_DESCRIPTOR_HANDLE, ID3D12DescriptorHeap,
};

use crate::device::RenderDevice;
use crate::error::RendererError;

/// An index into a descriptor heap, plus the handles that address it.
///
/// The index is what the owner returns to free the descriptor; the handles are what D3D12
/// consumes. `gpu` is `None` for a CPU-only heap, where no GPU-side address exists.
// `Eq` is not derived: the D3D12 handle types are plain address wrappers that implement
// `PartialEq` only. Comparing handles is a diagnostic convenience, not something the
// engine relies on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DescriptorHandle {
    /// Slot within the heap.
    pub index: u32,
    /// CPU-side address, for creating views.
    pub cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
    /// GPU-side address, for binding tables. Absent on CPU-only heaps.
    pub gpu: Option<D3D12_GPU_DESCRIPTOR_HANDLE>,
}

/// Usage of one allocator.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorStats {
    /// Slots the allocator can hand out.
    pub capacity: u32,
    /// Slots currently held.
    pub used: u32,
    /// Largest `used` ever reached.
    pub peak: u32,
    /// Allocations refused because the region was full.
    pub failures: u32,
}

impl DescriptorStats {
    /// Slots still available.
    #[must_use]
    pub fn available(&self) -> u32 {
        self.capacity.saturating_sub(self.used)
    }

    /// Fraction of the region in use, in `0.0..=1.0`.
    #[must_use]
    pub fn utilization(&self) -> f64 {
        if self.capacity == 0 {
            0.0
        } else {
            f64::from(self.used) / f64::from(self.capacity)
        }
    }
}

/// Index allocator with individual release.
///
/// A bump cursor for never-used slots plus a stack of returned ones, so both allocation
/// and release are O(1) with no scanning. Reusing the most recently freed slot first is
/// deliberate: it is the one most likely to still be in cache.
///
/// Pure arithmetic — no D3D12 — so every rule here is tested without a GPU.
#[derive(Debug)]
pub struct FreeListAllocator {
    capacity: u32,
    /// Next never-used slot.
    next: u32,
    /// Slots returned by `free`, most recent last.
    returned: Vec<u32>,
    used: u32,
    peak: u32,
    failures: u32,
}

impl FreeListAllocator {
    /// Creates an allocator over `capacity` slots.
    #[must_use]
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            next: 0,
            returned: Vec::new(),
            used: 0,
            peak: 0,
            failures: 0,
        }
    }

    /// Takes a slot, or `None` if the region is full.
    pub fn allocate(&mut self) -> Option<u32> {
        let index = if let Some(index) = self.returned.pop() {
            index
        } else if self.next < self.capacity {
            let index = self.next;
            self.next += 1;
            index
        } else {
            self.failures = self.failures.saturating_add(1);
            return None;
        };

        self.used += 1;
        self.peak = self.peak.max(self.used);
        Some(index)
    }

    /// Returns a slot to the allocator.
    ///
    /// Freeing a slot that was never allocated, or freeing one twice, would hand the same
    /// descriptor to two owners. Both are rejected rather than trusted.
    pub fn free(&mut self, index: u32) -> Result<(), RendererError> {
        if index >= self.next {
            return Err(RendererError::InvalidConfig {
                reason: format!("descriptor {index} was never allocated"),
            });
        }
        if self.returned.contains(&index) {
            return Err(RendererError::InvalidConfig {
                reason: format!("descriptor {index} was already freed"),
            });
        }
        self.returned.push(index);
        self.used -= 1;
        Ok(())
    }

    /// Usage snapshot.
    #[must_use]
    pub fn stats(&self) -> DescriptorStats {
        DescriptorStats {
            capacity: self.capacity,
            used: self.used,
            peak: self.peak,
            failures: self.failures,
        }
    }
}

/// Linear allocator over a contiguous range of slots.
///
/// Hands out ranges, because a descriptor table must be contiguous. Reset returns the
/// whole range at once.
#[derive(Debug, Clone, Copy)]
pub struct BumpAllocator {
    /// First slot of the region within the heap.
    base: u32,
    capacity: u32,
    cursor: u32,
    peak: u32,
    failures: u32,
}

impl BumpAllocator {
    /// Creates an allocator over `capacity` slots starting at `base`.
    #[must_use]
    pub fn new(base: u32, capacity: u32) -> Self {
        Self {
            base,
            capacity,
            cursor: 0,
            peak: 0,
            failures: 0,
        }
    }

    /// Takes `count` contiguous slots and returns the heap index of the first.
    ///
    /// Returns `None` if the region cannot fit them. A zero-length request succeeds
    /// without consuming anything, so a caller with nothing to bind needs no special case.
    pub fn allocate(&mut self, count: u32) -> Option<u32> {
        if count == 0 {
            return Some(self.base + self.cursor);
        }
        let end = self.cursor.checked_add(count)?;
        if end > self.capacity {
            self.failures = self.failures.saturating_add(1);
            return None;
        }
        let index = self.base + self.cursor;
        self.cursor = end;
        self.peak = self.peak.max(self.cursor);
        Some(index)
    }

    /// Reclaims the whole region.
    ///
    /// One store. The caller must have established that the GPU is finished with
    /// everything allocated from it — for a per-frame region that is the frame ring's
    /// pacing wait.
    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// Usage snapshot. `peak` survives resets, because a peak that resets every frame is
    /// not a peak — it is what sizes the region.
    #[must_use]
    pub fn stats(&self) -> DescriptorStats {
        DescriptorStats {
            capacity: self.capacity,
            used: self.cursor,
            peak: self.peak,
            failures: self.failures,
        }
    }
}

/// A CPU-only descriptor heap: render-target and depth-stencil views.
///
/// These are never bound as a table, so the heap is not shader-visible. Views are created
/// once per resource and released with it, which is what the free list is for.
pub struct CpuDescriptorHeap {
    heap: ID3D12DescriptorHeap,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
    increment: u32,
    base: D3D12_CPU_DESCRIPTOR_HANDLE,
    allocator: FreeListAllocator,
}

impl CpuDescriptorHeap {
    /// Creates a non-shader-visible heap of `capacity` descriptors.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the heap cannot be created.
    pub fn new(
        device: &RenderDevice,
        kind: D3D12_DESCRIPTOR_HEAP_TYPE,
        capacity: u32,
    ) -> Result<Self, RendererError> {
        let heap = device.create_descriptor_heap(kind, capacity, false)?;
        // SAFETY: FFI call on the heap created immediately above. The returned handle is
        // an address into that heap and stays valid while it lives.
        let base = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };

        Ok(Self {
            heap,
            kind,
            increment: device.descriptor_size(kind),
            base,
            allocator: FreeListAllocator::new(capacity),
        })
    }

    /// Takes a descriptor slot.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the heap is full, naming the heap type
    /// and its capacity — a full descriptor heap is a sizing mistake, and the message
    /// should say which one.
    pub fn allocate(&mut self) -> Result<DescriptorHandle, RendererError> {
        let index = self
            .allocator
            .allocate()
            .ok_or_else(|| RendererError::InvalidConfig {
                reason: format!(
                    "descriptor heap {:?} is full ({} descriptors)",
                    self.kind,
                    self.allocator.stats().capacity
                ),
            })?;
        Ok(self.handle(index))
    }

    /// Returns a slot to the heap.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the slot was never allocated or has
    /// already been freed.
    pub fn free(&mut self, handle: DescriptorHandle) -> Result<(), RendererError> {
        self.allocator.free(handle.index)
    }

    /// The handle addressing slot `index`.
    #[must_use]
    pub fn handle(&self, index: u32) -> DescriptorHandle {
        DescriptorHandle {
            index,
            cpu: D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.base.ptr + index as usize * self.increment as usize,
            },
            gpu: None,
        }
    }

    /// Usage snapshot.
    #[must_use]
    pub fn stats(&self) -> DescriptorStats {
        self.allocator.stats()
    }

    /// The underlying heap.
    #[must_use]
    pub fn heap(&self) -> &ID3D12DescriptorHeap {
        &self.heap
    }

    /// Size of one descriptor on this device.
    #[must_use]
    pub fn increment(&self) -> u32 {
        self.increment
    }
}

/// The shader-visible CBV/SRV/UAV heap, split into a persistent region and one per-frame
/// region per frame slot.
///
/// ```text
/// [ persistent | frame 0 | frame 1 | frame 2 ]
///   free list    bump      bump      bump
/// ```
///
/// Bound once per frame and never changed, because changing the bound descriptor heap is
/// expensive on some drivers.
pub struct GpuDescriptorHeap {
    heap: ID3D12DescriptorHeap,
    increment: u32,
    cpu_base: D3D12_CPU_DESCRIPTOR_HANDLE,
    gpu_base: D3D12_GPU_DESCRIPTOR_HANDLE,
    persistent: FreeListAllocator,
    /// One bump allocator per frame slot, each over its own slice.
    frames: Vec<BumpAllocator>,
}

impl GpuDescriptorHeap {
    /// Creates the heap with `persistent_capacity` long-lived slots and
    /// `per_frame_capacity` slots for each of `frames_in_flight` slices.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if `frames_in_flight` is zero, or
    /// [`RendererError::Api`] if the heap cannot be created.
    pub fn new(
        device: &RenderDevice,
        persistent_capacity: u32,
        per_frame_capacity: u32,
        frames_in_flight: usize,
    ) -> Result<Self, RendererError> {
        if frames_in_flight == 0 {
            return Err(RendererError::InvalidConfig {
                reason: "a descriptor heap needs at least one frame slice".to_owned(),
            });
        }

        let total = persistent_capacity + per_frame_capacity * frames_in_flight as u32;
        let heap =
            device.create_descriptor_heap(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, total, true)?;

        // SAFETY: FFI calls on the heap created immediately above. Both handles are
        // addresses into it and stay valid while it lives.
        let (cpu_base, gpu_base) = unsafe {
            (
                heap.GetCPUDescriptorHandleForHeapStart(),
                heap.GetGPUDescriptorHandleForHeapStart(),
            )
        };

        let frames = (0..frames_in_flight)
            .map(|slot| {
                BumpAllocator::new(
                    persistent_capacity + per_frame_capacity * slot as u32,
                    per_frame_capacity,
                )
            })
            .collect();

        Ok(Self {
            heap,
            increment: device.descriptor_size(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            cpu_base,
            gpu_base,
            persistent: FreeListAllocator::new(persistent_capacity),
            frames,
        })
    }

    /// Takes a long-lived slot, for a material's views.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the persistent region is full.
    pub fn allocate_persistent(&mut self) -> Result<DescriptorHandle, RendererError> {
        let index = self
            .persistent
            .allocate()
            .ok_or_else(|| RendererError::InvalidConfig {
                reason: format!(
                    "the persistent descriptor region is full ({} descriptors)",
                    self.persistent.stats().capacity
                ),
            })?;
        Ok(self.handle(index))
    }

    /// Returns a long-lived slot.
    ///
    /// The caller must have established that no frame still in flight references it —
    /// releasing a descriptor the GPU is about to read is exactly the bug the frame ring's
    /// fence exists to prevent.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the slot was never allocated or has
    /// already been freed.
    pub fn free_persistent(&mut self, handle: DescriptorHandle) -> Result<(), RendererError> {
        self.persistent.free(handle.index)
    }

    /// Reclaims frame slot `slot`'s whole region.
    ///
    /// Called from the renderer's frame start, after the ring has proven the GPU finished
    /// the frame that last used this slot.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is out of range, which would be an engine bug rather than a
    /// runtime condition.
    pub fn begin_frame(&mut self, slot: usize) {
        self.frames[slot].reset();
    }

    /// Takes `count` contiguous slots from frame slot `slot`'s region.
    ///
    /// The returned handle addresses the first descriptor of the table.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the slice cannot fit `count` more
    /// descriptors this frame.
    pub fn allocate_frame(
        &mut self,
        slot: usize,
        count: u32,
    ) -> Result<DescriptorHandle, RendererError> {
        let index =
            self.frames[slot]
                .allocate(count)
                .ok_or_else(|| RendererError::InvalidConfig {
                    reason: format!(
                        "frame descriptor slice {slot} cannot fit {count} more descriptors \
                     ({} of {} used)",
                        self.frames[slot].stats().used,
                        self.frames[slot].stats().capacity
                    ),
                })?;
        Ok(self.handle(index))
    }

    /// The handle addressing slot `index`.
    #[must_use]
    pub fn handle(&self, index: u32) -> DescriptorHandle {
        let offset = index as usize * self.increment as usize;
        DescriptorHandle {
            index,
            cpu: D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.cpu_base.ptr + offset,
            },
            gpu: Some(D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: self.gpu_base.ptr + offset as u64,
            }),
        }
    }

    /// Usage of the persistent region.
    #[must_use]
    pub fn persistent_stats(&self) -> DescriptorStats {
        self.persistent.stats()
    }

    /// Usage of frame slot `slot`'s region.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is out of range.
    #[must_use]
    pub fn frame_stats(&self, slot: usize) -> DescriptorStats {
        self.frames[slot].stats()
    }

    /// The underlying heap, for binding.
    #[must_use]
    pub fn heap(&self) -> &ID3D12DescriptorHeap {
        &self.heap
    }

    /// Size of one descriptor on this device.
    #[must_use]
    pub fn increment(&self) -> u32 {
        self.increment
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_free_list_hands_out_distinct_slots() {
        let mut allocator = FreeListAllocator::new(4);
        let slots: Vec<u32> = (0..4)
            .map(|_| allocator.allocate().expect("the region has room"))
            .collect();

        assert_eq!(slots, [0, 1, 2, 3]);
        assert_eq!(allocator.stats().used, 4);
        assert_eq!(allocator.stats().available(), 0);
        assert!(allocator.allocate().is_none(), "a full region must refuse");
        assert_eq!(allocator.stats().failures, 1);
    }

    #[test]
    fn a_freed_slot_is_reused_before_a_fresh_one() {
        let mut allocator = FreeListAllocator::new(8);
        for _ in 0..3 {
            let _ = allocator.allocate().expect("the region has room");
        }

        allocator.free(1).expect("slot 1 was allocated");
        assert_eq!(
            allocator.allocate(),
            Some(1),
            "the most recently freed slot is the one most likely still in cache"
        );
        assert_eq!(allocator.allocate(), Some(3), "then a fresh slot");
    }

    #[test]
    fn freeing_an_unallocated_or_doubly_freed_slot_is_rejected() {
        let mut allocator = FreeListAllocator::new(4);
        let slot = allocator.allocate().expect("the region has room");

        assert!(
            allocator.free(3).is_err(),
            "slot 3 was never handed out; freeing it would let two owners share a descriptor"
        );
        allocator.free(slot).expect("slot 0 was allocated");
        assert!(
            allocator.free(slot).is_err(),
            "a double free would hand the same slot to two owners"
        );
    }

    #[test]
    fn the_free_list_peak_survives_release() {
        let mut allocator = FreeListAllocator::new(8);
        let slots: Vec<u32> = (0..5)
            .map(|_| allocator.allocate().expect("the region has room"))
            .collect();
        assert_eq!(allocator.stats().peak, 5);

        for slot in slots {
            allocator.free(slot).expect("each slot was allocated");
        }
        assert_eq!(allocator.stats().used, 0);
        assert_eq!(
            allocator.stats().peak,
            5,
            "the peak is what sizes the region, so releasing must not lower it"
        );
    }

    #[test]
    fn a_bump_allocator_hands_out_contiguous_ranges() {
        let mut allocator = BumpAllocator::new(100, 16);

        assert_eq!(allocator.allocate(4), Some(100));
        assert_eq!(allocator.allocate(4), Some(104), "ranges are contiguous");
        assert_eq!(allocator.allocate(8), Some(108));
        assert_eq!(allocator.stats().used, 16);
        assert!(allocator.allocate(1).is_none(), "the region is full");
    }

    #[test]
    fn a_bump_reset_reclaims_everything_at_once() {
        let mut allocator = BumpAllocator::new(0, 8);
        assert_eq!(allocator.allocate(8), Some(0));
        assert!(allocator.allocate(1).is_none());

        allocator.reset();
        assert_eq!(allocator.stats().used, 0);
        assert_eq!(
            allocator.allocate(8),
            Some(0),
            "a reset region hands out the same slots again"
        );
        assert_eq!(
            allocator.stats().peak,
            8,
            "the peak describes the region's whole life"
        );
    }

    #[test]
    fn a_zero_length_bump_request_consumes_nothing() {
        let mut allocator = BumpAllocator::new(10, 4);
        assert_eq!(
            allocator.allocate(0),
            Some(10),
            "a caller with nothing to bind needs no special case"
        );
        assert_eq!(allocator.stats().used, 0);
        assert_eq!(allocator.allocate(4), Some(10));
    }

    #[test]
    fn a_bump_request_larger_than_the_region_is_refused_without_wrapping() {
        let mut allocator = BumpAllocator::new(0, 8);
        assert!(allocator.allocate(9).is_none());
        assert!(allocator.allocate(u32::MAX).is_none(), "no overflow");
        assert_eq!(allocator.stats().used, 0, "a refusal consumes nothing");
        assert_eq!(allocator.stats().failures, 2);
    }

    #[test]
    fn frame_slices_do_not_overlap() {
        // The layout GpuDescriptorHeap builds: a persistent region, then one slice per
        // frame slot. Two slots handing out the same index would let one frame overwrite
        // a descriptor the GPU is still reading from another.
        const PERSISTENT: u32 = 32;
        const PER_FRAME: u32 = 16;

        let mut slices: Vec<BumpAllocator> = (0..3)
            .map(|slot| BumpAllocator::new(PERSISTENT + PER_FRAME * slot, PER_FRAME))
            .collect();

        let mut seen = Vec::new();
        for slice in &mut slices {
            for _ in 0..PER_FRAME {
                let index = slice.allocate(1).expect("the slice has room");
                assert!(
                    index >= PERSISTENT,
                    "a frame slice must not touch the persistent region"
                );
                assert!(!seen.contains(&index), "index {index} was handed out twice");
                seen.push(index);
            }
        }
        assert_eq!(seen.len() as u32, PER_FRAME * 3);
    }

    #[test]
    fn statistics_describe_the_region() {
        let stats = DescriptorStats {
            capacity: 100,
            used: 25,
            peak: 60,
            failures: 0,
        };
        assert_eq!(stats.available(), 75);
        assert!((stats.utilization() - 0.25).abs() < 1e-12);

        let empty = DescriptorStats::default();
        assert_eq!(empty.utilization(), 0.0, "an empty region is not full");
    }
}
