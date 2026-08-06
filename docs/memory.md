# Memory

## Policy

There is no custom global allocator and no plan for one. `Box`, `Vec` and friends stay the
default for long-lived, individually-freed data. What the engine adds is bump allocation
for data with a *shared* lifetime, where the general-purpose allocator is doing work that
is not needed.

| Data | Allocator |
| --- | --- |
| Long-lived, individually freed (window, device, pipelines) | `Box` / `Vec` |
| Transient, discarded at frame end (render commands, temporary arrays) | `EngineContext::frame_arena` |
| Long-lived group with one shared lifetime | `Arena` |
| Fixed-size objects recycled at high rate | *not implemented* — see below |

## Why a bump allocator

Allocation is a compare and an add; reclamation is one store. No free list to walk, no
fragmentation, and no lock shared with the rest of the process. For a frame's scratch data
that is not a micro-optimization — it removes the general allocator from the frame path
entirely.

## Rules

1. **Destructors do not run.** Resetting an arena reclaims memory without dropping
   anything in it. The typed entry points therefore require `T: Copy`, which excludes
   every type with a destructor. A type owning a heap allocation, a file handle or a GPU
   resource cannot be arena-allocated, by construction rather than by convention.
2. **Nothing crosses a frame boundary.** `FrameAllocator::begin_frame` takes `&mut self`,
   and the arena hands out references derived from `&self`. The borrow checker rejects a
   reference that outlives the reset; this is not a rule anyone has to remember.
3. **A full arena returns `None`.** Running out of scratch memory is a capacity mistake,
   not a crash. `MemoryStats::failures` counts refusals, since a `None` that the caller
   handled leaves no other trace.
4. **One arena, one thread.** `Arena` is `Send` but deliberately not `Sync`: the offset is
   a `Cell`. A worker thread that needs scratch memory gets its own arena.

## Instrumentation

Every allocator reports `capacity`, `used`, `peak`, `allocations` and `failures`.

* `peak` survives `reset`, because a peak that resets every frame is not a peak. It is the
  number that sizes the arena — capacity should sit above the observed peak with headroom.
* `used` includes alignment padding, since that is what actually consumes the block.
* The frame allocator warns once when a frame crosses 75% of capacity, and only warns
  again after usage has dropped back below it. Warning at 100% would be useless: by then
  allocations are already failing. A per-frame warning would itself become the problem.

The counters are plain `Cell` increments on the allocation path — cheap enough to leave on
in release, which matters because a capacity problem that only appears under load is
exactly the one that needs reporting.

## Alignment

The block is 64-byte aligned (a cache line). Each allocation is aligned to its type's
requirement, computed against the *absolute* address rather than the offset, so alignments
above 64 are still correct.

This is tested explicitly for `u8` through `u128`, `f32`, `f64` and `#[repr(align(32))]` /
`#[repr(align(64))]` types, after a deliberate one-byte misalignment so nothing is
accidentally aligned. A bump allocator that returns a misaligned pointer produces a fault
or a silent slowdown far from the allocation site, which is exactly the class of bug that
does not get found by reading the arithmetic.

## Unsafe

`memory.rs` is the only module in `engine-core` with `unsafe`. Each block documents why it
is needed, which invariant makes it sound, who owns the memory and why the lifetime holds.
The invariants in one place:

* The block is allocated once in `with_capacity` and freed exactly once in `Drop`, with
  the same `Layout`.
* Every returned pointer lies within the block; the bounds check happens before the offset
  moves.
* Regions never overlap, because the offset only moves forward and never rewinds except in
  `reset`.
* `reset` requires `&mut self`, so no outstanding reference can exist when memory is
  reused. This is what makes `&self` allocation returning `&mut T` sound.

## Pool allocator

Not implemented. A pool needs a concrete use case — a fixed-size object recycled at high
rate — and Phase 1 has none. Entities, particles and GPU descriptor slots are all plausible
candidates later; implementing it now would be a guess at the block size, the recycling
policy and the ownership model.

## Known per-frame allocations

None in the engine core loop. `EngineContext` and the allocators are built once at startup;
recording metrics, applying input events and resetting the frame arena all operate on
pre-allocated storage.

Outside the core loop, and by design:

* Logging allocates while formatting. Banned from the per-frame path; frame metrics
  accumulate in counters and are logged on an interval.
* The debug overlay rebuilds its text four times a second, reusing the same `String`
  allocations after the first rebuild.
* `EngineError` allocates when constructed. A frame that fails has already lost.
