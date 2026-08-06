//! Command context tests against a real device.
//!
//! The unit tests in `command.rs` cover the bookkeeping — what counts as a redundant bind,
//! what the batching ratio means. These cover the part that needs a driver to be true:
//! that a batched barrier array is accepted, that a transition to the same state is
//! correctly dropped rather than submitted, and that the statistics match what was
//! recorded.
//!
//! No window and no swap chain: the questions here are about recording, not presentation.
//! Like every GPU test in this crate they take the process-wide lock and skip rather than
//! fail without a D3D12 adapter.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::command::CommandContext;
use engine_renderer::device::RenderDevice;

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, ID3D12CommandAllocator, ID3D12CommandList, ID3D12Resource,
};
use windows::core::Interface;

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

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

/// A device, an allocator and a context, which every test here needs.
struct Harness {
    device: RenderDevice,
    allocator: ID3D12CommandAllocator,
    context: CommandContext,
}

impl Harness {
    fn new(device: RenderDevice) -> Self {
        let allocator = device
            .create_command_allocator()
            .expect("allocator creation must succeed");
        let context =
            CommandContext::new(&device, &allocator).expect("context creation must succeed");
        Self {
            device,
            allocator,
            context,
        }
    }

    fn buffer(
        &self,
        state: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES,
    ) -> ID3D12Resource {
        self.device
            .create_buffer(256, D3D12_HEAP_TYPE_DEFAULT, state)
            .expect("buffer creation must succeed")
    }

    /// Submits the closed list and waits for the GPU to finish it.
    ///
    /// A barrier the driver rejects surfaces as a device-removed error here rather than as
    /// a silent no-op, which is the point of submitting at all.
    fn submit_and_wait(&mut self) {
        let submission = [Some(
            self.context
                .list()
                .cast::<ID3D12CommandList>()
                .expect("cast must succeed"),
        )];
        // SAFETY: FFI call submitting a closed list that outlives the call.
        unsafe { self.device.queue().ExecuteCommandLists(&submission) };

        let mut fence = engine_renderer::sync::GpuFence::new(&self.device)
            .expect("fence creation must succeed");
        fence
            .wait_for_idle(self.device.queue())
            .expect("idle wait must succeed");
    }
}

#[test]
fn barriers_batch_into_one_submission() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("barriers_batch_into_one_submission") else {
        return;
    };
    let mut harness = Harness::new(device);

    let buffers: Vec<ID3D12Resource> = (0..4)
        .map(|_| harness.buffer(D3D12_RESOURCE_STATE_COMMON))
        .collect();

    harness
        .context
        .begin(&harness.allocator)
        .expect("begin must succeed");

    for buffer in &buffers {
        // COPY_SOURCE, not UNORDERED_ACCESS: a buffer created without
        // `ALLOW_UNORDERED_ACCESS` cannot enter that state, and D3D12 rejects the whole
        // command list with E_INVALIDARG at Close if asked to. The copy states are valid
        // for any buffer.
        harness.context.transition(
            buffer,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
    }
    assert_eq!(
        harness.context.pending_barriers(),
        4,
        "transitions must queue rather than submit immediately"
    );
    assert_eq!(
        harness.context.stats().barrier_flushes,
        0,
        "nothing should have been submitted yet"
    );

    harness.context.flush_barriers();

    let stats = harness.context.stats();
    assert_eq!(harness.context.pending_barriers(), 0);
    assert_eq!(stats.barriers, 4);
    assert_eq!(
        stats.barrier_flushes, 1,
        "four transitions must cost one ResourceBarrier call"
    );
    assert!((stats.barriers_per_flush() - 4.0).abs() < 1e-12);

    harness.context.close().expect("close must succeed");
    harness.submit_and_wait();
}

#[test]
fn closing_flushes_queued_barriers() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("closing_flushes_queued_barriers") else {
        return;
    };
    let mut harness = Harness::new(device);
    let buffer = harness.buffer(D3D12_RESOURCE_STATE_COMMON);

    harness
        .context
        .begin(&harness.allocator)
        .expect("begin must succeed");
    harness.context.transition(
        &buffer,
        D3D12_RESOURCE_STATE_COMMON,
        D3D12_RESOURCE_STATE_COPY_DEST,
    );
    assert_eq!(harness.context.pending_barriers(), 1);

    // A queued barrier that is never submitted has not happened. Closing must not leave
    // one behind.
    harness.context.close().expect("close must succeed");
    assert_eq!(harness.context.pending_barriers(), 0);
    assert_eq!(harness.context.stats().barriers, 1);

    harness.submit_and_wait();
}

#[test]
fn a_transition_to_the_same_state_is_dropped() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_transition_to_the_same_state_is_dropped") else {
        return;
    };
    let mut harness = Harness::new(device);
    let buffer = harness.buffer(D3D12_RESOURCE_STATE_COMMON);

    harness
        .context
        .begin(&harness.allocator)
        .expect("begin must succeed");

    // D3D12 rejects a barrier whose before and after states are equal. Submitting one
    // would be a validation error describing a state change that is not one.
    harness.context.transition(
        &buffer,
        D3D12_RESOURCE_STATE_COPY_DEST,
        D3D12_RESOURCE_STATE_COPY_DEST,
    );
    assert_eq!(harness.context.pending_barriers(), 0);

    harness.context.close().expect("close must succeed");
    assert_eq!(harness.context.stats().barriers, 0);
    assert_eq!(
        harness.context.stats().barrier_flushes,
        0,
        "an empty batch must not cost a call"
    );
}

#[test]
fn exceeding_the_batch_capacity_flushes_rather_than_growing() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("exceeding_the_batch_capacity_flushes_rather_than_growing")
    else {
        return;
    };
    let mut harness = Harness::new(device);

    // More transitions than the pending buffer holds. Growing it would allocate in the
    // middle of recording a frame, which is exactly what the fixed capacity avoids.
    let buffers: Vec<ID3D12Resource> = (0..20)
        .map(|_| harness.buffer(D3D12_RESOURCE_STATE_COMMON))
        .collect();

    harness
        .context
        .begin(&harness.allocator)
        .expect("begin must succeed");
    for buffer in &buffers {
        harness.context.transition(
            buffer,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
    }
    harness.context.close().expect("close must succeed");

    let stats = harness.context.stats();
    assert_eq!(stats.barriers, 20, "no transition may be lost");
    assert!(
        stats.barrier_flushes >= 2,
        "the batch must have been submitted more than once, got {stats:?}"
    );
    assert!(stats.barriers_per_flush() > 1.0, "batching still applied");

    harness.submit_and_wait();
}

#[test]
fn beginning_a_frame_clears_the_previous_statistics() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("beginning_a_frame_clears_the_previous_statistics") else {
        return;
    };
    let mut harness = Harness::new(device);
    let buffer = harness.buffer(D3D12_RESOURCE_STATE_COMMON);

    harness
        .context
        .begin(&harness.allocator)
        .expect("begin must succeed");
    harness.context.transition(
        &buffer,
        D3D12_RESOURCE_STATE_COMMON,
        D3D12_RESOURCE_STATE_COPY_DEST,
    );
    harness.context.close().expect("close must succeed");
    assert_eq!(harness.context.stats().barriers, 1);
    harness.submit_and_wait();

    // SAFETY: the GPU has finished the submission above, which is what Reset requires.
    unsafe { harness.allocator.Reset() }.expect("allocator reset must succeed");
    harness
        .context
        .begin(&harness.allocator)
        .expect("begin must succeed");

    assert_eq!(
        harness.context.stats(),
        Default::default(),
        "statistics describe one frame, not the process"
    );
    assert!(harness.context.is_recording());
    harness.context.close().expect("close must succeed");
    assert!(!harness.context.is_recording());
}
