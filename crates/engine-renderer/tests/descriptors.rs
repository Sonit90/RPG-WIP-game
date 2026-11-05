//! Descriptor heap tests against a real device.
//!
//! The allocator arithmetic is unit-tested in `descriptor.rs` without a GPU. What needs a
//! device is the part that cannot be reasoned about from the source: the descriptor stride
//! is hardware-specific, so handle addresses have to be checked against what the device
//! actually reports, and a shader-visible heap has to be accepted by the driver at the
//! requested size.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_renderer::RendererError;
use engine_renderer::descriptor::{CpuDescriptorHeap, GpuDescriptorHeap};
use engine_renderer::device::RenderDevice;

use windows::Win32::Graphics::Direct3D12::{
    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
};

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

#[test]
fn cpu_handles_step_by_the_devices_descriptor_size() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("cpu_handles_step_by_the_devices_descriptor_size") else {
        return;
    };

    for kind in [
        D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
    ] {
        let mut heap =
            CpuDescriptorHeap::new(&device, kind, 8).expect("heap creation must succeed");

        // The stride is hardware-specific — it differs between RTV and DSV, and between
        // vendors. Computing handles with an assumed size produces views that address the
        // wrong descriptor, which is invisible until something renders wrong.
        let increment = device.descriptor_size(kind);
        assert_eq!(heap.increment(), increment);
        assert!(increment > 0, "{kind:?} must report a real descriptor size");

        let first = heap.allocate().expect("the heap has room");
        let second = heap.allocate().expect("the heap has room");

        assert_eq!(first.index, 0);
        assert_eq!(second.index, 1);
        assert_eq!(
            second.cpu.ptr - first.cpu.ptr,
            increment as usize,
            "consecutive {kind:?} handles must be one descriptor apart"
        );
        assert!(
            first.gpu.is_none(),
            "a CPU-only heap has no GPU-side address"
        );
    }
}

#[test]
fn a_full_cpu_heap_reports_which_heap_it_was() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_full_cpu_heap_reports_which_heap_it_was") else {
        return;
    };

    let mut heap = CpuDescriptorHeap::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 2)
        .expect("heap creation must succeed");
    let first = heap.allocate().expect("the heap has room");
    let _second = heap.allocate().expect("the heap has room");

    let error = heap.allocate().expect_err("the heap is full");
    let message = error.to_string();
    assert!(
        message.contains("full") && message.contains('2'),
        "a full heap is a sizing mistake and the message should say which heap: {message}"
    );

    // Releasing makes room again.
    heap.free(first).expect("the handle came from this heap");
    assert!(heap.allocate().is_ok());
}

#[test]
fn gpu_handles_carry_both_addresses_and_step_together() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("gpu_handles_carry_both_addresses_and_step_together") else {
        return;
    };

    let mut heap = GpuDescriptorHeap::new(&device, 16, 8, 3).expect("heap creation must succeed");
    let increment = device.descriptor_size(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);

    let first = heap.allocate_persistent().expect("the region has room");
    let second = heap.allocate_persistent().expect("the region has room");

    let (first_gpu, second_gpu) = (
        first.gpu.expect("a shader-visible heap has GPU addresses"),
        second.gpu.expect("a shader-visible heap has GPU addresses"),
    );

    assert_eq!(second.cpu.ptr - first.cpu.ptr, increment as usize);
    assert_eq!(
        second_gpu.ptr - first_gpu.ptr,
        u64::from(increment),
        "the CPU and GPU views of the same heap must advance in step"
    );
}

#[test]
fn frame_slices_are_disjoint_from_each_other_and_from_the_persistent_region() {
    let _guard = gpu_lock();
    let Some(device) =
        device_or_skip("frame_slices_are_disjoint_from_each_other_and_from_the_persistent_region")
    else {
        return;
    };

    const PERSISTENT: u32 = 8;
    const PER_FRAME: u32 = 4;
    let mut heap = GpuDescriptorHeap::new(&device, PERSISTENT, PER_FRAME, 3)
        .expect("heap creation must succeed");

    let mut indices = Vec::new();
    for _ in 0..PERSISTENT {
        indices.push(
            heap.allocate_persistent()
                .expect("the region has room")
                .index,
        );
    }
    for slot in 0..3 {
        heap.begin_frame(slot);
        for _ in 0..PER_FRAME {
            indices.push(
                heap.allocate_frame(slot, 1)
                    .expect("the slice has room")
                    .index,
            );
        }
    }

    // Two regions handing out the same index would let one frame overwrite a descriptor
    // the GPU is still reading for another.
    let mut sorted = indices.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        indices.len(),
        "every descriptor index must be handed out at most once"
    );
    assert_eq!(sorted.len() as u32, PERSISTENT + PER_FRAME * 3);
}

#[test]
fn a_frame_slice_is_reclaimed_whole_and_reused() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_frame_slice_is_reclaimed_whole_and_reused") else {
        return;
    };

    let mut heap = GpuDescriptorHeap::new(&device, 4, 8, 2).expect("heap creation must succeed");

    heap.begin_frame(0);
    let first = heap.allocate_frame(0, 8).expect("the slice has room");
    assert_eq!(heap.frame_stats(0).used, 8);
    assert!(
        heap.allocate_frame(0, 1).is_err(),
        "the slice is full until the frame ends"
    );

    // The next use of this slot resets it — safe because the frame ring has proven the GPU
    // finished the frame that used it.
    heap.begin_frame(0);
    assert_eq!(heap.frame_stats(0).used, 0);
    let reused = heap.allocate_frame(0, 8).expect("the slice was reclaimed");
    assert_eq!(
        reused.index, first.index,
        "a reset slice hands out the same descriptors again"
    );
    assert_eq!(
        heap.frame_stats(0).peak,
        8,
        "the peak is what sizes the slice, so a reset must not lower it"
    );
}

#[test]
fn a_descriptor_table_is_contiguous() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_descriptor_table_is_contiguous") else {
        return;
    };

    let mut heap = GpuDescriptorHeap::new(&device, 0, 16, 1).expect("heap creation must succeed");
    let increment = device.descriptor_size(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);

    heap.begin_frame(0);
    let table = heap.allocate_frame(0, 4).expect("the slice has room");
    let next = heap.allocate_frame(0, 4).expect("the slice has room");

    // A descriptor table must be contiguous: D3D12 binds it by its first handle and a
    // count, so a gap would silently bind the wrong descriptors.
    assert_eq!(next.index, table.index + 4);
    assert_eq!(
        next.cpu.ptr - table.cpu.ptr,
        4 * increment as usize,
        "the second table must begin exactly four descriptors after the first"
    );
}

#[test]
fn a_heap_with_no_frame_slices_is_rejected() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_heap_with_no_frame_slices_is_rejected") else {
        return;
    };

    assert!(
        GpuDescriptorHeap::new(&device, 16, 8, 0).is_err(),
        "a heap with no frame slices could not serve a single frame"
    );
    assert!(
        GpuDescriptorHeap::new(&device, 0, 0, 1).is_ok(),
        "an empty heap is legal: a renderer that binds nothing still starts"
    );
}
