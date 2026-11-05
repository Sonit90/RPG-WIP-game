//! Material constants against a real device.
//!
//! The conversion from `MaterialDesc` to `MaterialConstants` is unit-tested without a GPU.
//! What needs a driver is that the padded 256-byte constant buffer really lands in
//! device-local memory with the struct at offset zero, and that its address satisfies the
//! alignment a root constant-buffer view requires — a misaligned CBV is rejected at bind
//! time, long after the mistake.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::command::CommandContext;
use engine_renderer::device::RenderDevice;
use engine_renderer::material::{Material, MaterialConstants, MaterialDesc};
use engine_renderer::resource::{GpuBuffer, TrackedResource};
use engine_renderer::upload::CONSTANT_BUFFER_ALIGNMENT;

use engine_core::math::Vec3;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_READBACK, D3D12_RANGE, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, ID3D12CommandList,
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

/// Copies the material's constants back and reinterprets them.
fn read_constants(device: &RenderDevice, buffer: &GpuBuffer) -> MaterialConstants {
    let bytes = size_of::<MaterialConstants>();
    let readback = device
        .create_buffer(
            bytes,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .expect("readback buffer must be creatable");

    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(device, &allocator).expect("context must be creatable");
    context.begin(&allocator).expect("begin must succeed");
    context.transition_to(buffer, D3D12_RESOURCE_STATE_COPY_SOURCE);
    context.flush_barriers();

    // SAFETY: FFI call on the open list. Both resources outlive the submission, which this
    // function waits for before returning.
    unsafe {
        context
            .list()
            .CopyBufferRegion(&readback, 0, buffer.resource(), 0, bytes as u64);
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
    // SAFETY: FFI call mapping subresource 0 of a readback buffer this function owns; the
    // read range covers exactly the bytes copied above.
    unsafe { readback.Map(0, Some(&range), Some(&mut mapped)) }.expect("map must succeed");
    // SAFETY: `mapped` addresses at least `size_of::<MaterialConstants>()` readable bytes,
    // which the copy filled, and the type is `repr(C)` plain data.
    let value = unsafe { *mapped.cast::<MaterialConstants>() };
    // SAFETY: unmapping the buffer mapped immediately above; a null written range states
    // the CPU wrote nothing.
    unsafe { readback.Unmap(0, None) };

    value
}

#[test]
fn a_materials_constants_reach_the_gpu_unchanged() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_materials_constants_reach_the_gpu_unchanged") else {
        return;
    };

    let desc = MaterialDesc::colored(0.25, 0.5, 0.75)
        .with_roughness(0.3)
        .with_metallic(0.9)
        .with_emissive(Vec3::new(0.0, 1.0, 0.5));
    let material = Material::new(&device, desc).expect("material upload must succeed");

    assert_eq!(read_constants(&device, material.buffer()), desc.into());
    assert_eq!(material.desc(), desc);
}

#[test]
fn the_constant_address_is_aligned_for_a_root_view() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("the_constant_address_is_aligned_for_a_root_view") else {
        return;
    };

    let material =
        Material::new(&device, MaterialDesc::default()).expect("material upload must succeed");
    let address = material.gpu_address();
    assert_ne!(address, 0);
    assert_eq!(
        address % CONSTANT_BUFFER_ALIGNMENT as u64,
        0,
        "a constant-buffer view is rejected unless its address is a 256-byte multiple"
    );
}

#[test]
fn a_batch_of_materials_uploads_correctly() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_batch_of_materials_uploads_correctly") else {
        return;
    };

    let descriptions = [
        MaterialDesc::colored(1.0, 0.0, 0.0),
        MaterialDesc::colored(0.0, 1.0, 0.0).with_roughness(0.1),
        MaterialDesc::colored(0.0, 0.0, 1.0).with_metallic(1.0),
    ];
    let materials = Material::new_many(&device, &descriptions).expect("batch upload must succeed");

    // Each material must have its own constants, not the last one queued: one staging
    // buffer reused across the batch would give all three the same colour.
    for (material, desc) in materials.iter().zip(descriptions) {
        assert_eq!(read_constants(&device, material.buffer()), desc.into());
    }
}

#[test]
fn an_invalid_material_is_rejected_before_it_reaches_the_gpu() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("an_invalid_material_is_rejected_before_it_reaches_the_gpu")
    else {
        return;
    };

    // Built by hand rather than through `with_roughness`, which clamps: the point is that
    // a value that bypassed the builder still cannot reach the GPU.
    let desc = MaterialDesc {
        roughness: f32::NAN,
        ..MaterialDesc::default()
    };
    assert!(Material::new(&device, desc).is_err());
}
