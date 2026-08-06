//! Multi-frame synchronization tests.
//!
//! These drive the frame ring directly against a real device: no window, no swap chain,
//! no presentation. That is deliberate — the questions here are "does the CPU wait for the
//! right fence value" and "is a slot's allocator safe to reset", and a window would add
//! a compositor and a presentation queue to a measurement that is about neither.
//!
//! Like every GPU test in this crate they take the process-wide [`gpu_lock`] and skip
//! rather than fail on a machine with no D3D12 adapter. See `docs/testing.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::device::RenderDevice;
use engine_renderer::frame::{FrameRing, MAX_FRAMES_IN_FLIGHT};

use std::time::Duration;

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COMMON, ID3D12CommandList,
    ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::core::Interface;

/// Serializes GPU work across test threads.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates a device, or returns `None` on a machine without a D3D12 adapter.
fn device_or_skip(test: &str) -> Option<RenderDevice> {
    match RenderDevice::new(false, false) {
        Ok(device) => Some(device),
        Err(RendererError::NoSuitableAdapter { examined }) => {
            println!("SKIP: {test} — no D3D12 adapter ({examined} examined)");
            None
        }
        Err(error) => panic!("device creation failed: {error}"),
    }
}

/// A pair of large buffers used to give the GPU measurable work.
///
/// Copying between them takes real GPU time without needing a pipeline, a render target or
/// a shader. That matters: an *empty* command list completes before the CPU can get back
/// around the loop, so a ring paced against empty frames never waits and the
/// synchronization behaviour under test never happens.
struct GpuLoad {
    source: ID3D12Resource,
    destination: ID3D12Resource,
    bytes: u64,
}

impl GpuLoad {
    /// 64 MiB per copy: large enough to outlast a CPU loop iteration on the target GPU,
    /// small enough not to strain a modest one.
    const BYTES: usize = 64 * 1024 * 1024;

    fn new(device: &RenderDevice) -> Self {
        // COMMON, because buffers promote to COPY_SOURCE / COPY_DEST implicitly on first
        // use, so no explicit barrier is needed for a copy.
        let source = device
            .create_buffer(
                Self::BYTES,
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_COMMON,
            )
            .expect("buffer creation must succeed");
        let destination = device
            .create_buffer(
                Self::BYTES,
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_COMMON,
            )
            .expect("buffer creation must succeed");
        Self {
            source,
            destination,
            bytes: Self::BYTES as u64,
        }
    }

    /// Records `copies` full-buffer copies into `list`.
    fn record(&self, list: &ID3D12GraphicsCommandList, copies: u32) {
        for _ in 0..copies {
            // SAFETY: FFI call on an open list. Both resources outlive the submission —
            // the test waits for idle before dropping them — and buffers promote from
            // COMMON to the copy states implicitly.
            unsafe {
                list.CopyBufferRegion(&self.destination, 0, &self.source, 0, self.bytes);
            }
        }
    }
}

/// Records and submits one frame from `slot`'s allocator, with `copies` copies of work.
///
/// `copies == 0` submits an empty list.
fn submit_frame(
    device: &RenderDevice,
    ring: &FrameRing,
    slot: usize,
    load: Option<&GpuLoad>,
    copies: u32,
) {
    let list = device
        .create_command_list(ring.allocator(slot))
        .expect("command list creation must succeed");
    // The device layer hands back a closed list; recording needs it open.
    // SAFETY: the list was just created and closed, and the allocator belongs to a slot
    // the ring has proven is free, which is what Reset requires.
    unsafe { list.Reset(ring.allocator(slot), None) }.expect("reset must succeed");

    if let Some(load) = load {
        load.record(&list, copies);
    }

    // SAFETY: closing the list recorded immediately above.
    unsafe { list.Close() }.expect("close must succeed");

    let submission = [Some(
        list.cast::<ID3D12CommandList>().expect("cast must succeed"),
    )];
    // SAFETY: FFI call submitting a closed list that outlives the call.
    unsafe { device.queue().ExecuteCommandLists(&submission) };
}

/// Runs `frames` frames through the ring, optionally loading the GPU on each.
fn run_frames(
    device: &RenderDevice,
    ring: &mut FrameRing,
    frames: u64,
    load: Option<&GpuLoad>,
    copies: u32,
) -> Vec<usize> {
    let mut slots = Vec::with_capacity(frames as usize);
    for _ in 0..frames {
        let slot = ring.begin_frame().expect("begin_frame must succeed");
        slots.push(slot);
        submit_frame(device, ring, slot, load, copies);
        ring.end_frame(device.queue(), slot)
            .expect("end_frame must succeed");
    }
    slots
}

#[test]
fn slots_cycle_and_allocators_are_reused() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("slots_cycle_and_allocators_are_reused") else {
        return;
    };

    let mut ring = FrameRing::new(&device, 3).expect("a 3-slot ring must be creatable");
    let slots = run_frames(&device, &mut ring, 9, None, 0);

    assert_eq!(slots, [0, 1, 2, 0, 1, 2, 0, 1, 2]);
    assert_eq!(ring.frame_index(), 9);
    assert_eq!(ring.stats().total_frames, 9);

    // Nine frames through three allocators means each was reset twice after its first
    // use. Reaching here without a device-removed error is the assertion: resetting an
    // allocator whose work is still executing is undefined behaviour in D3D12.
    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
}

#[test]
fn trivial_work_never_makes_the_cpu_wait() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("trivial_work_never_makes_the_cpu_wait") else {
        return;
    };

    // Even with a single slot, empty command lists complete before the CPU can get back
    // around the loop, so the pacing wait never blocks. This is worth asserting because
    // it is the opposite of the intuition that one slot must serialize: what serializes
    // is one slot *plus GPU work that outlasts a CPU iteration*.
    let mut ring = FrameRing::new(&device, 1).expect("a 1-slot ring must be creatable");
    let slots = run_frames(&device, &mut ring, 20, None, 0);

    assert!(slots.iter().all(|slot| *slot == 0));
    assert_eq!(ring.stats().total_frames, 20);
    // Not zero stalls: the GPU is shared with the desktop compositor, and an occasional
    // frame does lose the race. What the test pins down is that trivial work does not
    // *systematically* serialize — contrast the loaded case below, which stalls on every
    // frame. A threshold rather than an exact count is the difference between a test that
    // measures the renderer and one that measures whatever else the machine was doing.
    let stalled = ring.stats().stalled_frames;
    assert!(
        stalled <= 2,
        "empty frames should almost never make the CPU wait, but {stalled} of 20 did"
    );

    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
}

#[test]
fn one_slot_stalls_on_every_frame_under_real_gpu_load() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("one_slot_stalls_on_every_frame_under_real_gpu_load") else {
        return;
    };

    let load = GpuLoad::new(&device);
    let mut ring = FrameRing::new(&device, 1).expect("a 1-slot ring must be creatable");
    run_frames(&device, &mut ring, 6, Some(&load), 4);

    // One slot means one allocator: frame N+1 cannot be recorded until frame N has
    // finished on the GPU. With work that takes longer than a CPU loop iteration, that is
    // a stall on every frame after the first.
    assert!(
        ring.stats().stalled_frames >= 5,
        "expected a stall on each frame after the first, got {:?}",
        ring.stats()
    );
    assert!(ring.stats().total_stall > Duration::ZERO);
    assert!(ring.stats().max_stall >= ring.stats().last_stall);

    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
}

#[test]
fn three_slots_absorb_the_first_frames_without_waiting() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("three_slots_absorb_the_first_frames_without_waiting") else {
        return;
    };

    let load = GpuLoad::new(&device);
    let mut ring = FrameRing::new(&device, 3).expect("a 3-slot ring must be creatable");

    // Exactly one lap: every slot is used for the first time, so none of them can have
    // outstanding work to wait for, however slow the GPU is. This is precisely what the
    // extra slots buy — the CPU runs ahead by the slot count before the fence matters.
    run_frames(&device, &mut ring, 3, Some(&load), 4);
    assert_eq!(
        ring.stats().stalled_frames,
        0,
        "the first lap cannot wait: no slot has been used before"
    );

    // The next lap must wait, because now each slot has work in flight from its previous
    // use. That is the pacing the ring exists to provide.
    run_frames(&device, &mut ring, 3, Some(&load), 4);
    assert!(
        ring.stats().stalled_frames > 0,
        "the second lap must pace against the first, got {:?}",
        ring.stats()
    );

    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
}

#[test]
fn fence_values_advance_and_complete() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("fence_values_advance_and_complete") else {
        return;
    };

    let mut ring = FrameRing::new(&device, 3).expect("ring must be creatable");

    let mut previous = 0;
    for _ in 0..6 {
        let slot = ring.begin_frame().expect("begin_frame must succeed");
        submit_frame(&device, &ring, slot, None, 0);
        let value = ring
            .end_frame(device.queue(), slot)
            .expect("end_frame must succeed");
        assert!(
            value > previous,
            "fence values must be monotonic: {value} followed {previous}"
        );
        previous = value;
    }

    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
    assert!(
        ring.completed_value() >= previous,
        "after an idle wait the GPU must have passed every signalled value"
    );
}

#[test]
fn releasing_slots_lets_the_next_frames_run_without_waiting() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("releasing_slots_lets_the_next_frames_run_without_waiting")
    else {
        return;
    };

    let mut ring = FrameRing::new(&device, 3).expect("ring must be creatable");
    run_frames(&device, &mut ring, 3, None, 0);

    // This is the swap-chain-resize path: wait for idle, then declare every slot free.
    // The old fence values refer to submissions that touched destroyed back buffers.
    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
    ring.release_all_slots();
    ring.reset_stats();

    run_frames(&device, &mut ring, 3, None, 0);
    assert_eq!(
        ring.stats().stalled_frames,
        0,
        "slots released after an idle wait must not make the next frames block"
    );

    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
}

#[test]
fn an_out_of_range_frame_count_is_rejected() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("an_out_of_range_frame_count_is_rejected") else {
        return;
    };

    assert!(
        FrameRing::new(&device, 0).is_err(),
        "zero frames in flight has no meaning"
    );
    assert!(
        FrameRing::new(&device, MAX_FRAMES_IN_FLIGHT + 1).is_err(),
        "more frames than back buffers cannot help and is a wrong expectation"
    );
    for count in 1..=MAX_FRAMES_IN_FLIGHT {
        assert!(FrameRing::new(&device, count).is_ok(), "{count} slots");
    }
}
