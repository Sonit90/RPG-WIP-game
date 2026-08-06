//! Bump allocation: an [`Arena`] and the per-frame allocator built on it.
//!
//! ## What this is for
//!
//! Transient data with a known common lifetime — render commands, visible-object lists,
//! temporary arrays built during a frame and thrown away at its end. For that shape, a
//! bump allocator is not a micro-optimization: allocation is a compare and an add, and
//! deallocation is one store to reset the offset. There is no free list to walk, no
//! fragmentation, and no allocator lock shared with the rest of the process.
//!
//! It is *not* a general-purpose allocator and there is no global allocator override.
//! Long-lived, individually-freed objects still use `Box`, `Vec` and friends.
//!
//! ## Destructors are not run
//!
//! Resetting an arena reclaims memory without dropping anything in it. Rather than leave
//! that as a footgun, the typed entry points require `T: Copy`, which excludes every type
//! with a destructor. Types that own a heap allocation, a file handle or a GPU resource
//! cannot be arena-allocated, and that is the intended restriction.
//!
//! ## Alignment
//!
//! The block itself is 64-byte aligned (a cache line), and every allocation is aligned to
//! its type's requirement. Alignment is tested explicitly for a spread of types rather
//! than assumed — a bump allocator that returns a misaligned pointer produces a fault or
//! a silent slowdown far from the allocation site.
//!
//! ## Threading
//!
//! [`Arena`] allocates through `&self` with interior mutability, so several live
//! allocations can coexist. It is therefore deliberately **not** `Sync`: one arena belongs
//! to one thread. A worker thread that needs scratch memory gets its own arena.

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::cell::Cell;
use std::fmt;
use std::ptr::NonNull;

/// Alignment of the underlying block: one cache line.
///
/// Every allocation of alignment 64 or less is then satisfiable from any reset arena, and
/// the first allocation of a frame starts on a cache-line boundary rather than straddling
/// one.
const BLOCK_ALIGN: usize = 64;

/// A snapshot of an allocator's usage.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStats {
    /// Bytes the allocator can hand out in total.
    pub capacity: usize,
    /// Bytes currently handed out, including alignment padding.
    pub used: usize,
    /// Largest `used` ever reached, across resets.
    ///
    /// The number that sizes the arena: capacity should sit above the observed peak with
    /// headroom, not above the current usage.
    pub peak: usize,
    /// Allocations served since the last reset.
    pub allocations: u32,
    /// Allocations refused because the arena was full, since construction.
    ///
    /// Non-zero means the capacity is too small — nothing else reports that, because a
    /// refused allocation returns `None` rather than failing loudly.
    pub failures: u32,
}

impl MemoryStats {
    /// Fraction of capacity currently used, in `0.0..=1.0`.
    #[must_use]
    pub fn utilization(&self) -> f64 {
        if self.capacity == 0 {
            0.0
        } else {
            self.used as f64 / self.capacity as f64
        }
    }

    /// Bytes still available before the arena is full.
    #[must_use]
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.used)
    }
}

impl fmt::Display for MemoryStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "used: {} | peak: {} | capacity: {} | allocations: {}",
            Bytes(self.used),
            Bytes(self.peak),
            Bytes(self.capacity),
            self.allocations
        )
    }
}

/// Byte count rendered in KiB/MiB, for reports.
struct Bytes(usize);

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const KIB: usize = 1024;
        const MIB: usize = KIB * 1024;
        match self.0 {
            bytes if bytes >= MIB => write!(f, "{:.2} MiB", bytes as f64 / MIB as f64),
            bytes if bytes >= KIB => write!(f, "{:.2} KiB", bytes as f64 / KIB as f64),
            bytes => write!(f, "{bytes} B"),
        }
    }
}

/// A linear (bump) allocator over one fixed block.
///
/// Allocation moves an offset forward; there is no individual deallocation. Everything is
/// reclaimed at once by [`Arena::reset`], which takes `&mut self` — so the borrow checker
/// guarantees no allocation from the arena is still alive when it happens.
pub struct Arena {
    /// Owning pointer to the block, allocated with `layout` and freed in `Drop`.
    block: NonNull<u8>,
    /// Layout the block was allocated with. Zero-capacity arenas have a zero-sized layout
    /// and a dangling block, which is never dereferenced.
    layout: Layout,
    offset: Cell<usize>,
    peak: Cell<usize>,
    allocations: Cell<u32>,
    failures: Cell<u32>,
}

impl Arena {
    /// Creates an arena with `capacity` bytes.
    ///
    /// The block is allocated once, here. A zero capacity is legal and yields an arena
    /// that refuses every non-empty allocation, which keeps "disabled" from being a
    /// special case at every call site.
    ///
    /// # Panics
    ///
    /// Panics if the allocation of `capacity` bytes fails, via the standard library's
    /// allocation-error hook. A failure to reserve the engine's scratch memory at startup
    /// is not something the caller can do anything about.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let layout = Layout::from_size_align(capacity, BLOCK_ALIGN)
            .unwrap_or_else(|_| handle_alloc_error(Layout::new::<u8>()));

        let block = if capacity == 0 {
            NonNull::dangling()
        } else {
            // SAFETY: `layout` has a non-zero size, which is `alloc`'s requirement. The
            // returned pointer is owned by this `Arena` and freed exactly once, in `Drop`,
            // with the same layout.
            let pointer = unsafe { alloc(layout) };
            match NonNull::new(pointer) {
                Some(pointer) => pointer,
                None => handle_alloc_error(layout),
            }
        };

        Self {
            block,
            layout,
            offset: Cell::new(0),
            peak: Cell::new(0),
            allocations: Cell::new(0),
            failures: Cell::new(0),
        }
    }

    /// Allocates `size` bytes aligned to `align`, uninitialized.
    ///
    /// Returns `None` if the arena does not have room; the caller decides whether that is
    /// fatal. `align` must be a power of two, as `Layout` requires.
    ///
    /// The bytes are **not** initialized, so the returned pointer must be written before
    /// it is read.
    pub fn alloc_bytes(&self, size: usize, align: usize) -> Option<NonNull<u8>> {
        debug_assert!(align.is_power_of_two(), "alignment must be a power of two");

        let base = self.block.as_ptr() as usize;
        let current = base.checked_add(self.offset.get())?;
        // Round the *absolute* address up, not the offset: the block is 64-byte aligned,
        // but an alignment above that still has to be satisfied against the real address.
        let aligned = current.checked_add(align - 1)? & !(align - 1);
        let padding = aligned - current;
        let end = self.offset.get().checked_add(padding)?.checked_add(size)?;

        if end > self.layout.size() {
            self.failures.set(self.failures.get().saturating_add(1));
            return None;
        }

        self.offset.set(end);
        if end > self.peak.get() {
            self.peak.set(end);
        }
        self.allocations
            .set(self.allocations.get().saturating_add(1));

        // SAFETY: `aligned` lies within the block: it is `base + offset + padding`, and
        // `offset + padding + size <= capacity` was checked above. Casting the address
        // back to a pointer is derived from `self.block`, so it keeps its provenance.
        NonNull::new(unsafe { self.block.as_ptr().add(aligned - base) })
    }

    /// Moves `value` into the arena and returns a reference to it.
    ///
    /// `T: Copy` because resetting the arena does not run destructors.
    ///
    /// The returned reference borrows the arena immutably, so many allocations can be
    /// alive at once; [`Arena::reset`] needs `&mut self` and therefore cannot run while
    /// any of them exist.
    #[allow(
        clippy::mut_from_ref,
        reason = "the defining property of a bump allocator: each call returns a distinct, \
                  never-reused region, so the &mut cannot alias anything. reset() takes \
                  &mut self, which is what stops the region being handed out again while \
                  this reference lives."
    )]
    pub fn alloc<T: Copy>(&self, value: T) -> Option<&mut T> {
        let pointer = self
            .alloc_bytes(size_of::<T>(), align_of::<T>())?
            .cast::<T>();
        // SAFETY: `alloc_bytes` returned a block of exactly `size_of::<T>()` bytes aligned
        // to `align_of::<T>()`, inside memory this arena owns. The write initializes it
        // before any read. The region is not handed out again until `reset`, which
        // requires `&mut self` and so cannot happen while this reference lives.
        unsafe {
            pointer.write(value);
            Some(&mut *pointer.as_ptr())
        }
    }

    /// Copies `values` into the arena and returns the copy.
    ///
    /// An empty slice is served without touching the arena: a zero-length allocation has
    /// no bytes to align and no address a caller may dereference.
    #[allow(
        clippy::mut_from_ref,
        reason = "see Arena::alloc — each call returns a distinct, never-reused region"
    )]
    pub fn alloc_slice<T: Copy>(&self, values: &[T]) -> Option<&mut [T]> {
        if values.is_empty() {
            return Some(&mut []);
        }
        let size = size_of::<T>().checked_mul(values.len())?;
        let pointer = self.alloc_bytes(size, align_of::<T>())?.cast::<T>();
        // SAFETY: the block is `values.len() * size_of::<T>()` bytes aligned for `T`, and
        // is owned by this arena. Source and destination cannot overlap: `values` lives
        // outside the region just allocated, which was never handed out before. The copy
        // initializes the whole slice before it is read.
        unsafe {
            std::ptr::copy_nonoverlapping(values.as_ptr(), pointer.as_ptr(), values.len());
            Some(std::slice::from_raw_parts_mut(
                pointer.as_ptr(),
                values.len(),
            ))
        }
    }

    /// Allocates room for `count` values of `T` without initializing them.
    ///
    /// Returns a slice of `MaybeUninit<T>`, which the caller must fill before reading.
    #[allow(
        clippy::mut_from_ref,
        reason = "see Arena::alloc — each call returns a distinct, never-reused region"
    )]
    pub fn alloc_uninit_slice<T: Copy>(
        &self,
        count: usize,
    ) -> Option<&mut [std::mem::MaybeUninit<T>]> {
        if count == 0 {
            return Some(&mut []);
        }
        let size = size_of::<T>().checked_mul(count)?;
        let pointer = self
            .alloc_bytes(size, align_of::<T>())?
            .cast::<std::mem::MaybeUninit<T>>();
        // SAFETY: the region is `count * size_of::<T>()` bytes aligned for `T` and owned
        // by this arena. `MaybeUninit<T>` has the same layout as `T` and imposes no
        // initialization requirement, so exposing it uninitialized is sound.
        unsafe { Some(std::slice::from_raw_parts_mut(pointer.as_ptr(), count)) }
    }

    /// Reclaims everything at once.
    ///
    /// `&mut self` is the safety mechanism, not a style choice: it proves no reference
    /// into the arena is still alive.
    ///
    /// The peak and failure counters survive, since they describe the arena's whole life.
    pub fn reset(&mut self) {
        self.offset.set(0);
        self.allocations.set(0);
    }

    /// Total bytes the arena can hand out.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.layout.size()
    }

    /// Bytes handed out since the last reset, including alignment padding.
    #[must_use]
    pub fn used(&self) -> usize {
        self.offset.get()
    }

    /// Largest `used` ever reached.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.peak.get()
    }

    /// Current usage snapshot.
    #[must_use]
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            capacity: self.capacity(),
            used: self.used(),
            peak: self.peak(),
            allocations: self.allocations.get(),
            failures: self.failures.get(),
        }
    }

    /// Clears the peak and failure counters.
    ///
    /// For measuring one phase in isolation; the engine does not call it per frame,
    /// because a peak that resets every frame is not a peak.
    pub fn reset_statistics(&mut self) {
        self.peak.set(self.offset.get());
        self.failures.set(0);
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        if self.layout.size() == 0 {
            return;
        }
        // SAFETY: `block` came from `alloc` with exactly `self.layout` in
        // `with_capacity`, has not been freed before (this runs once), and no reference
        // into it can outlive the arena, since every one of them borrows `&self`.
        unsafe { dealloc(self.block.as_ptr(), self.layout) };
    }
}

impl fmt::Debug for Arena {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arena")
            .field("capacity", &self.capacity())
            .field("used", &self.used())
            .field("peak", &self.peak())
            .finish()
    }
}

// SAFETY: `Arena` owns its block outright and hands out references derived from `&self`,
// so moving the whole arena to another thread moves everything it can reach. It is
// deliberately not `Sync`: the `Cell` offset is not synchronized, so shared `&Arena`
// access from two threads must remain impossible.
unsafe impl Send for Arena {}

/// Default frame-allocator capacity: 1 MiB.
///
/// A frame's transient data at this stage is a few kilobytes; a megabyte gives room for
/// render command buffers without being worth tuning yet. The peak in the statistics is
/// what should drive any change to this number.
pub const DEFAULT_FRAME_CAPACITY: usize = 1024 * 1024;

/// An arena whose lifetime is one frame.
///
/// ```text
/// frame start -> begin_frame() -> allocate ... -> frame end -> begin_frame() resets
/// ```
///
/// Individual deallocation does not exist. The engine calls [`FrameAllocator::begin_frame`]
/// at the top of each frame; everything allocated during the previous frame is gone from
/// that moment, which is exactly why nothing may hold an arena reference across frames —
/// and why nothing can: the reset takes `&mut self`.
#[derive(Debug)]
pub struct FrameAllocator {
    arena: Arena,
    /// Bytes used by the frame that just ended, for reporting.
    last_frame_used: usize,
    /// Usage above which a warning is logged once per crossing.
    budget: usize,
    /// Whether the budget warning has already been logged for the current excursion.
    budget_warned: bool,
}

impl Default for FrameAllocator {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_FRAME_CAPACITY)
    }
}

impl FrameAllocator {
    /// Creates a frame allocator with `capacity` bytes and a budget of 75% of it.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            arena: Arena::with_capacity(capacity),
            last_frame_used: 0,
            // Warning at capacity would be useless: by then allocations are already
            // failing. Three quarters leaves room to react.
            budget: capacity / 4 * 3,
            budget_warned: false,
        }
    }

    /// Ends the previous frame and starts a new one, reclaiming everything.
    ///
    /// Logs once when a frame's usage crosses the budget, and once more only after usage
    /// has come back below it — a per-frame warning would itself become the problem.
    pub fn begin_frame(&mut self) {
        self.last_frame_used = self.arena.used();

        if self.last_frame_used > self.budget {
            if !self.budget_warned {
                log::warn!(
                    target: "Memory",
                    "frame allocator exceeded its budget: {}",
                    self.arena.stats()
                );
                self.budget_warned = true;
            }
        } else {
            self.budget_warned = false;
        }

        self.arena.reset();
    }

    /// The arena to allocate from during this frame.
    #[must_use]
    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// Bytes used by the frame that just ended.
    #[must_use]
    pub fn last_frame_used(&self) -> usize {
        self.last_frame_used
    }

    /// Usage above which a warning is logged.
    #[must_use]
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Sets the warning threshold.
    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }

    /// Current usage snapshot.
    #[must_use]
    pub fn stats(&self) -> MemoryStats {
        self.arena.stats()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn address_of<T: ?Sized>(value: &T) -> usize {
        std::ptr::from_ref(value).cast::<u8>() as usize
    }

    #[test]
    fn a_new_arena_is_empty_and_has_the_requested_capacity() {
        let arena = Arena::with_capacity(4096);
        let stats = arena.stats();
        assert_eq!(stats.capacity, 4096);
        assert_eq!(stats.used, 0);
        assert_eq!(stats.peak, 0);
        assert_eq!(stats.allocations, 0);
        assert_eq!(stats.available(), 4096);
        assert_eq!(stats.utilization(), 0.0);
    }

    #[test]
    fn a_value_survives_the_round_trip() {
        let arena = Arena::with_capacity(1024);
        let value = arena.alloc(42_u64).expect("the arena has room");
        assert_eq!(*value, 42);
        *value = 7;
        assert_eq!(*value, 7);
    }

    #[test]
    fn allocations_are_aligned_for_their_type() {
        // Alignments from 1 to 64, including an over-aligned type, checked against real
        // addresses rather than assumed from the arithmetic.
        #[repr(align(32))]
        #[derive(Clone, Copy)]
        #[allow(dead_code, reason = "only the type's alignment is under test")]
        struct Over32(u8);

        #[repr(align(64))]
        #[derive(Clone, Copy)]
        #[allow(dead_code, reason = "only the type's alignment is under test")]
        struct Over64(u8);

        let arena = Arena::with_capacity(4096);

        // A single misaligning byte first, so nothing is accidentally aligned.
        let _ = arena.alloc(1_u8).expect("the arena has room");

        macro_rules! check {
            ($type:ty, $value:expr) => {{
                let allocated = arena.alloc::<$type>($value).expect("the arena has room");
                assert_eq!(
                    address_of(allocated) % align_of::<$type>(),
                    0,
                    "{} is misaligned",
                    stringify!($type)
                );
            }};
        }

        check!(u8, 1);
        check!(u16, 1);
        check!(u32, 1);
        check!(u64, 1);
        check!(u128, 1);
        check!(f32, 1.0);
        check!(f64, 1.0);
        check!(Over32, Over32(1));
        check!(Over64, Over64(1));
    }

    #[test]
    fn allocations_do_not_overlap() {
        let arena = Arena::with_capacity(1024);
        let first = arena.alloc(1_u64).expect("the arena has room");
        let second = arena.alloc(2_u64).expect("the arena has room");
        let third = arena.alloc(3_u64).expect("the arena has room");

        // Both references are alive at once, which is the point of allocating through
        // `&self`; writing through one must not disturb the others.
        *first = 10;
        *second = 20;
        assert_eq!((*first, *second, *third), (10, 20, 3));

        let addresses = [address_of(first), address_of(second), address_of(third)];
        assert!(addresses[0] < addresses[1] && addresses[1] < addresses[2]);
        assert!(addresses[1] - addresses[0] >= size_of::<u64>());
    }

    #[test]
    fn a_slice_is_copied_intact() {
        let arena = Arena::with_capacity(1024);
        let source = [1_u32, 2, 3, 4, 5];
        let copy = arena.alloc_slice(&source).expect("the arena has room");
        assert_eq!(copy, source);

        copy[0] = 99;
        assert_eq!(source[0], 1, "the copy is independent of the source");
        assert_eq!(address_of(&copy[0]) % align_of::<u32>(), 0);
    }

    #[test]
    fn an_uninitialized_slice_can_be_filled() {
        let arena = Arena::with_capacity(1024);
        let slice = arena
            .alloc_uninit_slice::<u32>(4)
            .expect("the arena has room");
        for (index, slot) in slice.iter_mut().enumerate() {
            slot.write(index as u32);
        }
        // SAFETY: every element was written immediately above.
        let initialized: &[u32] = unsafe { &*(std::ptr::from_ref(slice) as *const [u32]) };
        assert_eq!(initialized, [0, 1, 2, 3]);
    }

    #[test]
    fn zero_sized_allocations_are_served_without_consuming_capacity() {
        let arena = Arena::with_capacity(64);

        let empty: &mut [u32] = arena.alloc_slice(&[]).expect("an empty slice always fits");
        assert!(empty.is_empty());
        let empty_uninit = arena
            .alloc_uninit_slice::<u64>(0)
            .expect("a zero-length request always fits");
        assert!(empty_uninit.is_empty());
        assert_eq!(arena.used(), 0, "no bytes were consumed");

        // A zero-sized *type* still goes through the allocator and stays aligned.
        let unit = arena.alloc(()).expect("a zero-sized type always fits");
        assert_eq!(*unit, ());
    }

    #[test]
    fn an_overflowing_allocation_is_refused_rather_than_wrapping() {
        let arena = Arena::with_capacity(128);
        assert!(arena.alloc_slice(&[0_u8; 129]).is_none());
        assert_eq!(arena.used(), 0, "a refused allocation consumes nothing");
        assert_eq!(arena.stats().failures, 1);

        // The arena is still usable afterwards.
        assert!(arena.alloc(1_u32).is_some());
    }

    #[test]
    fn allocating_exactly_the_capacity_succeeds_and_the_next_byte_fails() {
        let arena = Arena::with_capacity(64);
        let full = arena.alloc_slice(&[7_u8; 64]).expect("64 bytes of 64 fit");
        assert_eq!(full.len(), 64);
        assert_eq!(arena.used(), 64);
        assert_eq!(arena.stats().available(), 0);
        assert!((arena.stats().utilization() - 1.0).abs() < 1e-12);

        assert!(arena.alloc(1_u8).is_none());
    }

    #[test]
    fn a_zero_capacity_arena_refuses_everything_without_crashing() {
        let arena = Arena::with_capacity(0);
        assert_eq!(arena.capacity(), 0);
        assert!(arena.alloc(1_u8).is_none());
        assert!(arena.alloc_slice(&[1_u8, 2]).is_none());
        assert!(
            arena.alloc_slice::<u8>(&[]).is_some(),
            "an empty slice needs no memory"
        );
    }

    #[test]
    fn reset_reclaims_everything_and_reuses_the_same_memory() {
        let mut arena = Arena::with_capacity(256);
        let first_address = {
            let value = arena.alloc(1_u64).expect("the arena has room");
            address_of(value)
        };
        assert_eq!(arena.used(), size_of::<u64>());

        arena.reset();
        assert_eq!(arena.used(), 0);
        assert_eq!(arena.stats().allocations, 0);

        let second_address = {
            let value = arena.alloc(2_u64).expect("the arena has room");
            address_of(value)
        };
        assert_eq!(
            first_address, second_address,
            "a reset arena hands out the same memory again"
        );
    }

    #[test]
    fn the_peak_survives_a_reset() {
        let mut arena = Arena::with_capacity(1024);
        let _ = arena.alloc_slice(&[0_u8; 300]).expect("the arena has room");
        assert_eq!(arena.peak(), 300);

        arena.reset();
        assert_eq!(arena.used(), 0);
        assert_eq!(
            arena.peak(),
            300,
            "the peak describes the arena's whole life"
        );

        let _ = arena.alloc_slice(&[0_u8; 100]).expect("the arena has room");
        assert_eq!(arena.peak(), 300, "a smaller frame does not lower the peak");

        let _ = arena.alloc_slice(&[0_u8; 500]).expect("the arena has room");
        assert_eq!(arena.peak(), 600, "a larger frame raises it");
    }

    #[test]
    fn statistics_count_allocations_and_padding() {
        let arena = Arena::with_capacity(1024);
        let _ = arena.alloc(1_u8).expect("the arena has room");
        // Forces seven bytes of padding before an eight-byte alignment.
        let _ = arena.alloc(1_u64).expect("the arena has room");

        let stats = arena.stats();
        assert_eq!(stats.allocations, 2);
        assert_eq!(stats.used, 16, "1 byte + 7 padding + 8 bytes");
        assert!(stats.utilization() > 0.0);
    }

    #[test]
    fn resetting_statistics_clears_the_peak_and_failures() {
        let mut arena = Arena::with_capacity(64);
        let _ = arena.alloc_slice(&[0_u8; 32]).expect("the arena has room");
        assert!(arena.alloc_slice(&[0_u8; 64]).is_none());
        assert_eq!(arena.stats().failures, 1);

        arena.reset_statistics();
        assert_eq!(arena.stats().failures, 0);
        assert_eq!(arena.stats().peak, arena.used());
    }

    #[test]
    fn stats_render_in_human_units() {
        let text = MemoryStats {
            capacity: 1024 * 1024,
            used: 128 * 1024,
            peak: 256 * 1024,
            allocations: 12,
            failures: 0,
        }
        .to_string();
        assert!(text.contains("128.00 KiB"), "{text}");
        assert!(text.contains("256.00 KiB"), "{text}");
        assert!(text.contains("1.00 MiB"), "{text}");
    }

    #[test]
    fn a_frame_allocator_reclaims_everything_each_frame() {
        let mut frame = FrameAllocator::with_capacity(1024);

        for expected_frame in 1..=3_u32 {
            let value = frame
                .arena()
                .alloc(expected_frame)
                .expect("the arena has room");
            assert_eq!(*value, expected_frame);
            assert_eq!(frame.stats().used, size_of::<u32>());

            frame.begin_frame();
            assert_eq!(frame.stats().used, 0);
            assert_eq!(frame.last_frame_used(), size_of::<u32>());
        }

        assert_eq!(frame.stats().peak, size_of::<u32>());
    }

    #[test]
    fn a_frame_allocator_has_a_budget_below_its_capacity() {
        let frame = FrameAllocator::with_capacity(1000);
        assert_eq!(frame.budget(), 750);
        assert!(frame.budget() < frame.stats().capacity);
    }

    #[test]
    fn exceeding_the_budget_does_not_fail_allocations() {
        // The budget is a reporting threshold, not a limit: crossing it must not break a
        // frame that still fits in the arena.
        let mut frame = FrameAllocator::with_capacity(1024);
        frame.set_budget(16);

        let big = frame.arena().alloc_slice(&[0_u8; 512]);
        assert!(big.is_some());
        frame.begin_frame();
        assert_eq!(frame.last_frame_used(), 512);
        assert_eq!(frame.stats().failures, 0);
    }

    #[test]
    fn an_arena_can_move_between_threads() {
        let arena = Arena::with_capacity(128);
        let used = std::thread::spawn(move || {
            let value = arena.alloc(5_u32).expect("the arena has room");
            assert_eq!(*value, 5);
            arena.used()
        })
        .join()
        .expect("the thread must not panic");
        assert_eq!(used, size_of::<u32>());
    }
}
