//! Upload ring and batched staging, against a real device.
//!
//! The allocation arithmetic is unit-tested in `upload.rs`. What needs a driver is that a
//! persistently mapped upload buffer really is writable, that the addresses handed out
//! correspond to the bytes written, and that a batched staging copy actually lands in
//! device-local memory — which is only observable by reading it back.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::command::CommandContext;
use engine_renderer::device::RenderDevice;
use engine_renderer::resource::{GpuBuffer, TrackedResource};
use engine_renderer::upload::{
    CONSTANT_BUFFER_ALIGNMENT, DEFAULT_ALIGNMENT, StagingUploader, UploadRing,
};

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_READBACK, D3D12_RANGE, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
    ID3D12CommandList,
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

/// Copies `source` into a readback buffer and returns its contents as `u32`s.
///
/// Reading back is the only way to prove data reached device-local memory: an upload that
/// silently wrote nowhere looks identical from the CPU side.
fn read_back_u32(device: &RenderDevice, source: &GpuBuffer, count: usize) -> Vec<u32> {
    let bytes = count * size_of::<u32>();
    let readback = device
        .create_buffer(
            bytes,
            D3D12_HEAP_TYPE_READBACK,
            windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .expect("readback buffer must be creatable");

    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(device, &allocator).expect("context must be creatable");
    context.begin(&allocator).expect("begin must succeed");
    context.transition_to(source, D3D12_RESOURCE_STATE_COPY_SOURCE);
    context.flush_barriers();

    // SAFETY: FFI call on the open list. Both resources outlive the submission, which this
    // function waits for before returning.
    unsafe {
        context
            .list()
            .CopyBufferRegion(&readback, 0, source.resource(), 0, bytes as u64);
    }
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

    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    let range = D3D12_RANGE {
        Begin: 0,
        End: bytes,
    };
    // SAFETY: FFI call mapping subresource 0 of a readback buffer this function owns. The
    // read range covers exactly the bytes copied above.
    unsafe { readback.Map(0, Some(&range), Some(&mut mapped)) }.expect("map must succeed");
    // SAFETY: `mapped` addresses at least `bytes` readable bytes, which the copy filled.
    let values = unsafe { std::slice::from_raw_parts(mapped.cast::<u32>(), count) }.to_vec();
    // SAFETY: unmapping the buffer mapped immediately above; a null written range states
    // the CPU wrote nothing.
    unsafe { readback.Unmap(0, None) };

    values
}

#[test]
fn ring_allocations_are_aligned_and_distinct() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("ring_allocations_are_aligned_and_distinct") else {
        return;
    };

    let mut ring = UploadRing::new(&device, 64 * 1024, 3).expect("ring must be creatable");
    assert_eq!(ring.slot_count(), 3);

    let first = ring
        .allocate(0, 4, DEFAULT_ALIGNMENT)
        .expect("the slot has room");
    let second = ring
        .allocate(0, 4, DEFAULT_ALIGNMENT)
        .expect("the slot has room");
    let constants = ring
        .allocate(0, 4, CONSTANT_BUFFER_ALIGNMENT)
        .expect("the slot has room");

    assert_eq!(first.offset(), 0);
    assert_eq!(second.offset(), DEFAULT_ALIGNMENT as u64);
    assert_eq!(
        constants.gpu_address() % CONSTANT_BUFFER_ALIGNMENT as u64,
        0,
        "a constant buffer view's address must be 256-byte aligned or D3D12 rejects it"
    );
    assert!(
        second.gpu_address() > first.gpu_address(),
        "allocations advance through the slot"
    );
}

#[test]
fn slots_are_independent_and_reset_individually() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("slots_are_independent_and_reset_individually") else {
        return;
    };

    let mut ring = UploadRing::new(&device, 1024, 3).expect("ring must be creatable");

    for slot in 0..3 {
        ring.allocate(slot, 512, DEFAULT_ALIGNMENT)
            .expect("the slot has room");
        assert_eq!(ring.stats(slot).used, 512);
    }

    // Two frames in flight must not share memory: one would overwrite data the GPU is
    // still reading for the other.
    let addresses: Vec<u64> = (0..3)
        .map(|slot| {
            ring.allocate(slot, 16, DEFAULT_ALIGNMENT)
                .expect("the slot has room")
                .gpu_address()
        })
        .collect();
    assert_ne!(addresses[0], addresses[1]);
    assert_ne!(addresses[1], addresses[2]);

    ring.begin_frame(1);
    assert_eq!(ring.stats(1).used, 0);
    assert_eq!(
        ring.stats(0).used,
        528,
        "resetting one slot must not touch another"
    );
    assert_eq!(ring.stats(1).peak, 528, "the peak survives the reset");
}

#[test]
fn a_full_slot_refuses_and_says_what_is_left() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_full_slot_refuses_and_says_what_is_left") else {
        return;
    };

    let mut ring = UploadRing::new(&device, 256, 1).expect("ring must be creatable");
    ring.allocate(0, 200, DEFAULT_ALIGNMENT)
        .expect("the slot has room");

    let error = ring
        .allocate(0, 128, DEFAULT_ALIGNMENT)
        .expect_err("128 bytes do not fit in the 56 that are left");
    let message = error.to_string();
    assert!(
        message.contains("128") && message.contains("256"),
        "a full ring is a sizing mistake and the message should be enough to fix it: {message}"
    );
    assert_eq!(ring.stats(0).failures, 1);
    assert_eq!(ring.stats(0).used, 200, "a refusal consumes nothing");
}

#[test]
fn written_data_reaches_the_gpu_through_the_ring() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("written_data_reaches_the_gpu_through_the_ring") else {
        return;
    };

    let mut ring = UploadRing::new(&device, 4096, 1).expect("ring must be creatable");
    let values: Vec<u32> = (0..64).collect();
    let allocation = ring
        .write(0, &values, DEFAULT_ALIGNMENT)
        .expect("the slot has room");

    // Copy out of the ring's own buffer into device-local memory, then read it back. This
    // is the whole path: mapped write, GPU-visible address, copy, readback.
    let destination =
        GpuBuffer::new_default(&device, values.len() * size_of::<u32>()).expect("buffer");
    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(&device, &allocator).expect("context must be creatable");
    context.begin(&allocator).expect("begin must succeed");
    context.transition_to(&destination, D3D12_RESOURCE_STATE_COPY_DEST);
    context.flush_barriers();
    // SAFETY: FFI call on the open list; both resources outlive the wait below.
    unsafe {
        context.list().CopyBufferRegion(
            destination.resource(),
            0,
            ring.buffer(0).resource(),
            allocation.offset(),
            allocation.size() as u64,
        );
    }
    context.close().expect("close must succeed");
    let submission = [Some(
        context
            .list()
            .cast::<ID3D12CommandList>()
            .expect("cast must succeed"),
    )];
    // SAFETY: FFI call submitting a closed list that outlives the call.
    unsafe { device.queue().ExecuteCommandLists(&submission) };
    let mut fence = engine_renderer::sync::GpuFence::new(&device).expect("fence");
    fence
        .wait_for_idle(device.queue())
        .expect("idle wait must succeed");

    assert_eq!(read_back_u32(&device, &destination, values.len()), values);
}

#[test]
fn constant_writes_are_padded_to_the_required_size() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("constant_writes_are_padded_to_the_required_size") else {
        return;
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Small {
        value: f32,
    }

    let mut ring = UploadRing::new(&device, 4096, 1).expect("ring must be creatable");
    let first = ring
        .write_constants(0, &Small { value: 1.0 })
        .expect("the slot has room");
    let second = ring
        .write_constants(0, &Small { value: 2.0 })
        .expect("the slot has room");

    // D3D12 requires a constant buffer view's *size* to be a multiple of 256, not only its
    // address, so a 4-byte struct still occupies a whole 256 bytes.
    assert_eq!(first.size(), CONSTANT_BUFFER_ALIGNMENT);
    assert_eq!(
        second.gpu_address() - first.gpu_address(),
        CONSTANT_BUFFER_ALIGNMENT as u64
    );
}

#[test]
fn batched_uploads_cost_one_synchronization() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("batched_uploads_cost_one_synchronization") else {
        return;
    };

    let mut uploader = StagingUploader::new(&device).expect("uploader must be creatable");
    let buffers: Vec<GpuBuffer> = (0..8)
        .map(|_| GpuBuffer::new_default(&device, 64).expect("buffer must be creatable"))
        .collect();

    for (index, buffer) in buffers.iter().enumerate() {
        let data = [index as u32; 16];
        uploader
            .queue_buffer(
                &device,
                buffer,
                &data,
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            )
            .expect("queueing must succeed");
    }
    assert_eq!(uploader.queued(), 8);

    let copies = uploader.flush(&device).expect("flush must succeed");
    assert_eq!(copies, 8);
    assert_eq!(
        uploader.total_flushes(),
        1,
        "eight uploads must cost one GPU synchronization, not eight"
    );

    // Every buffer must hold its own data — a batch that copied the wrong staging buffer
    // into the wrong destination would still flush cleanly.
    for (index, buffer) in buffers.iter().enumerate() {
        let values = read_back_u32(&device, buffer, 16);
        assert!(
            values.iter().all(|value| *value == index as u32),
            "buffer {index} holds {values:?}"
        );
    }
}

#[test]
fn an_empty_flush_does_nothing() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("an_empty_flush_does_nothing") else {
        return;
    };

    let mut uploader = StagingUploader::new(&device).expect("uploader must be creatable");
    assert_eq!(uploader.flush(&device).expect("flush must succeed"), 0);
    assert_eq!(
        uploader.total_flushes(),
        0,
        "nothing queued must not cost a synchronization"
    );
}

#[test]
fn an_upload_larger_than_its_destination_is_rejected() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("an_upload_larger_than_its_destination_is_rejected") else {
        return;
    };

    let mut uploader = StagingUploader::new(&device).expect("uploader must be creatable");
    let buffer = GpuBuffer::new_default(&device, 32).expect("buffer must be creatable");

    assert!(
        uploader
            .queue_buffer(
                &device,
                &buffer,
                &[0_u32; 16],
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER
            )
            .is_err(),
        "64 bytes must not be copied into a 32 byte destination"
    );
    assert_eq!(uploader.queued(), 0);
}

#[test]
fn an_uploader_can_be_reused_across_batches() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("an_uploader_can_be_reused_across_batches") else {
        return;
    };

    let mut uploader = StagingUploader::new(&device).expect("uploader must be creatable");

    for round in 0..3_u32 {
        let buffer = GpuBuffer::new_default(&device, 64).expect("buffer must be creatable");
        uploader
            .queue_buffer(
                &device,
                &buffer,
                &[round; 16],
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            )
            .expect("queueing must succeed");
        uploader.flush(&device).expect("flush must succeed");

        assert_eq!(read_back_u32(&device, &buffer, 16), vec![round; 16]);
    }

    assert_eq!(uploader.total_copies(), 3);
    assert_eq!(uploader.total_flushes(), 3);
}
