//! Resource creation, state tracking, destruction and recreation against a real device.
//!
//! The questions here cannot be answered without a driver: whether a barrier built from a
//! resource's tracked state is one D3D12 accepts, and whether a resource released while
//! the GPU may still be reading it survives. The second is the one that matters — getting
//! it wrong is a use-after-free whose symptom is a driver crash somewhere else entirely.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::command::CommandContext;
use engine_renderer::device::RenderDevice;
use engine_renderer::frame::FrameRing;
use engine_renderer::resource::{DeferredReleaseQueue, GpuBuffer, GpuTexture, TrackedResource};

use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_RENDER_TARGET, ID3D12CommandList,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM};
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

/// Records the context's commands and waits for the GPU to finish them.
///
/// Submitting matters: a barrier the driver rejects is invisible until something executes
/// it, and `Close` is where D3D12 reports it.
fn submit_and_wait(device: &RenderDevice, context: &mut CommandContext) {
    context.close().expect("close must succeed");
    let submission = [Some(
        context
            .list()
            .cast::<ID3D12CommandList>()
            .expect("cast must succeed"),
    )];
    // SAFETY: FFI call submitting a closed list that outlives the call.
    unsafe { device.queue().ExecuteCommandLists(&submission) };

    let mut fence = engine_renderer::sync::GpuFence::new(device).expect("fence must be creatable");
    fence
        .wait_for_idle(device.queue())
        .expect("idle wait must succeed");
}

#[test]
fn buffers_and_textures_are_created_in_the_state_they_report() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("buffers_and_textures_are_created_in_the_state_they_report")
    else {
        return;
    };

    let buffer = GpuBuffer::new_default(&device, 1024).expect("buffer creation must succeed");
    assert_eq!(buffer.size(), 1024);
    assert!(
        buffer.gpu_address() != 0,
        "a default-heap buffer has an address"
    );
    assert_eq!(buffer.state(), D3D12_RESOURCE_STATE_COMMON);
    assert!(!buffer.is_upload());

    let upload = GpuBuffer::new_upload(&device, 256).expect("buffer creation must succeed");
    assert!(upload.is_upload());

    let target = GpuTexture::render_target(
        &device,
        320,
        240,
        DXGI_FORMAT_R8G8B8A8_UNORM,
        [0.0, 0.0, 0.0, 1.0],
    )
    .expect("render target creation must succeed");
    assert_eq!(target.size(), (320, 240));
    assert_eq!(target.state(), D3D12_RESOURCE_STATE_RENDER_TARGET);

    let depth = GpuTexture::depth_buffer(&device, 320, 240, DXGI_FORMAT_D32_FLOAT, 1.0)
        .expect("depth buffer creation must succeed");
    assert_eq!(depth.state(), D3D12_RESOURCE_STATE_DEPTH_WRITE);
    assert_eq!(depth.format(), DXGI_FORMAT_D32_FLOAT);
}

#[test]
fn a_zero_sized_resource_is_rejected() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_zero_sized_resource_is_rejected") else {
        return;
    };

    assert!(GpuBuffer::new_default(&device, 0).is_err());
    assert!(
        GpuTexture::render_target(&device, 0, 240, DXGI_FORMAT_R8G8B8A8_UNORM, [0.0; 4]).is_err()
    );
}

#[test]
fn a_transition_updates_the_tracked_state_and_the_driver_accepts_it() {
    let _guard = gpu_lock();
    let Some(device) =
        device_or_skip("a_transition_updates_the_tracked_state_and_the_driver_accepts_it")
    else {
        return;
    };

    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(&device, &allocator).expect("context must be creatable");
    let buffer = GpuBuffer::new_default(&device, 512).expect("buffer must be creatable");

    context.begin(&allocator).expect("begin must succeed");

    context.transition_to(&buffer, D3D12_RESOURCE_STATE_COPY_DEST);
    assert_eq!(
        buffer.state(),
        D3D12_RESOURCE_STATE_COPY_DEST,
        "the resource must record the state it was transitioned into"
    );

    context.transition_to(&buffer, D3D12_RESOURCE_STATE_COPY_SOURCE);
    assert_eq!(buffer.state(), D3D12_RESOURCE_STATE_COPY_SOURCE);
    assert_eq!(context.pending_barriers(), 2);

    // Submitting is the assertion: a barrier whose "before" did not match the resource's
    // real state is rejected at Close with E_INVALIDARG.
    submit_and_wait(&device, &mut context);
    assert_eq!(context.stats().barriers, 2);
}

#[test]
fn transitioning_to_the_current_state_costs_nothing() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("transitioning_to_the_current_state_costs_nothing") else {
        return;
    };

    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(&device, &allocator).expect("context must be creatable");
    let texture = GpuTexture::render_target(
        &device,
        64,
        64,
        DXGI_FORMAT_R8G8B8A8_UNORM,
        [0.0, 0.0, 0.0, 1.0],
    )
    .expect("render target must be creatable");

    context.begin(&allocator).expect("begin must succeed");

    // The texture is already a render target. Issuing a barrier for that would be a
    // driver call describing a state change that is not one — this is the "do not blindly
    // issue barriers every frame" requirement, enforced by a comparison.
    context.transition_to(&texture, D3D12_RESOURCE_STATE_RENDER_TARGET);
    assert_eq!(context.pending_barriers(), 0);
    assert_eq!(context.stats().redundant_transitions, 1);

    submit_and_wait(&device, &mut context);
    assert_eq!(context.stats().barriers, 0);
}

#[test]
fn a_resource_survives_until_the_gpu_has_passed_its_fence_value() {
    let _guard = gpu_lock();
    let Some(device) =
        device_or_skip("a_resource_survives_until_the_gpu_has_passed_its_fence_value")
    else {
        return;
    };

    let mut queue = DeferredReleaseQueue::new();
    let mut ring = FrameRing::new(&device, 3).expect("ring must be creatable");

    // A resource used by a frame that has been submitted but not completed.
    let buffer = GpuBuffer::new_default(&device, 4096).expect("buffer must be creatable");
    let resource = buffer.resource().clone();

    let slot = ring.begin_frame().expect("begin_frame must succeed");
    let list = device
        .create_command_list(ring.allocator(slot))
        .expect("list must be creatable");
    // SAFETY: the allocator belongs to a slot the ring proved is free.
    unsafe { list.Reset(ring.allocator(slot), None) }.expect("reset must succeed");
    // SAFETY: closing the list recorded above.
    unsafe { list.Close() }.expect("close must succeed");
    let submission = [Some(
        list.cast::<ID3D12CommandList>().expect("cast must succeed"),
    )];
    // SAFETY: FFI call submitting a closed list that outlives the call.
    unsafe { device.queue().ExecuteCommandLists(&submission) };
    let fence_value = ring
        .end_frame(device.queue(), slot)
        .expect("end_frame must succeed");

    drop(buffer);
    queue.release(fence_value, resource);
    assert_eq!(queue.pending(), 1);

    // Collecting against a value the GPU has not reached must release nothing: the
    // resource may still be referenced by work in flight.
    assert_eq!(queue.collect(fence_value.saturating_sub(1)), 0);
    assert_eq!(queue.pending(), 1);

    ring.wait_for_idle(device.queue())
        .expect("idle wait must succeed");
    assert_eq!(queue.collect(ring.completed_value()), 1);
    assert_eq!(queue.pending(), 0);
    assert_eq!(queue.released(), 1);
}

#[test]
fn resources_are_released_in_submission_order() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("resources_are_released_in_submission_order") else {
        return;
    };

    let mut queue = DeferredReleaseQueue::new();
    for value in [10_u64, 10, 20, 30] {
        let buffer = GpuBuffer::new_default(&device, 256).expect("buffer must be creatable");
        queue.release(value, buffer.resource().clone());
    }
    assert_eq!(queue.pending(), 4);
    assert_eq!(queue.peak_pending(), 4);

    assert_eq!(
        queue.collect(10),
        2,
        "both entries at value 10 release together"
    );
    assert_eq!(queue.collect(15), 0, "value 20 has not been reached");
    assert_eq!(queue.collect(30), 2);
    assert_eq!(queue.pending(), 0);
    assert_eq!(
        queue.peak_pending(),
        4,
        "the peak is what reveals a slow leak, so collection must not lower it"
    );
}

#[test]
fn a_resource_can_be_created_used_destroyed_and_recreated() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_resource_can_be_created_used_destroyed_and_recreated")
    else {
        return;
    };

    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(&device, &allocator).expect("context must be creatable");

    // Three full cycles, each creating a texture, transitioning it, using the GPU, and
    // dropping it after the GPU has finished.
    for cycle in 0..3 {
        let texture = GpuTexture::render_target(
            &device,
            128,
            128,
            DXGI_FORMAT_R8G8B8A8_UNORM,
            [0.0, 0.0, 0.0, 1.0],
        )
        .unwrap_or_else(|error| panic!("cycle {cycle}: creation failed: {error}"));

        // SAFETY: the GPU is idle from the previous cycle's wait, which is Reset's
        // precondition.
        unsafe { allocator.Reset() }.expect("allocator reset must succeed");
        context.begin(&allocator).expect("begin must succeed");
        context.transition_to(&texture, D3D12_RESOURCE_STATE_COPY_SOURCE);
        submit_and_wait(&device, &mut context);

        assert_eq!(texture.state(), D3D12_RESOURCE_STATE_COPY_SOURCE);
        drop(texture);
    }
}

#[test]
fn an_upload_buffer_accepts_data_and_rejects_what_does_not_fit() {
    let _guard = gpu_lock();
    let Some(device) =
        device_or_skip("an_upload_buffer_accepts_data_and_rejects_what_does_not_fit")
    else {
        return;
    };

    let buffer = GpuBuffer::new_upload(&device, 64).expect("buffer must be creatable");
    buffer.write(&[1_u32; 16]).expect("64 bytes fit exactly");
    buffer.write::<u32>(&[]).expect("an empty write is a no-op");

    assert!(
        buffer.write(&[0_u32; 17]).is_err(),
        "68 bytes must not be written into a 64 byte buffer"
    );

    let device_local = GpuBuffer::new_default(&device, 64).expect("buffer must be creatable");
    assert!(
        device_local.write(&[1_u32; 4]).is_err(),
        "a default-heap buffer is not CPU-writable"
    );
}
