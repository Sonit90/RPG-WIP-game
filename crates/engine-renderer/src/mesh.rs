//! GPU-resident vertex buffers.
//!
//! Geometry lives on a **default** heap, filled once through an upload-heap staging
//! buffer. That matters for the stress tests: an upload-heap vertex buffer is CPU-visible
//! memory the GPU reads across PCIe on every draw, so at 100,000 triangles a benchmark
//! would be measuring bus bandwidth rather than the renderer. A default-heap buffer lives
//! in device-local memory.
//!
//! Uploading blocks until the copy completes. That is acceptable because it happens when
//! the scene changes, never per frame.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD, D3D12_RANGE, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
    D3D12_VERTEX_BUFFER_VIEW, ID3D12Resource,
};
use windows::core::Interface;

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::pipeline::Vertex;
use crate::renderer::transition_barrier;
use crate::sync::GpuFence;

/// A vertex buffer in device-local memory.
pub struct GpuMesh {
    /// Kept alive because the view points into it.
    buffer: ID3D12Resource,
    view: D3D12_VERTEX_BUFFER_VIEW,
    vertex_count: u32,
}

impl GpuMesh {
    /// Uploads `vertices` to a default-heap buffer and waits for the copy to finish.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for empty input, or
    /// [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload(device: &RenderDevice, vertices: &[Vertex]) -> Result<Self, RendererError> {
        if vertices.is_empty() {
            return Err(RendererError::InvalidConfig {
                reason: "a mesh needs at least one vertex".to_owned(),
            });
        }

        let byte_size = std::mem::size_of_val(vertices);
        let destination = device.create_buffer(
            byte_size,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let staging = device.create_buffer(
            byte_size,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?;

        write_staging_raw(&staging, vertices.as_ptr().cast::<u8>(), byte_size)?;
        copy_and_wait(device, &staging, &destination, byte_size)?;

        // SAFETY: FFI call returning the resource's GPU address, valid while it lives.
        let gpu_address = unsafe { destination.GetGPUVirtualAddress() };
        let view = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: gpu_address,
            SizeInBytes: byte_size as u32,
            StrideInBytes: size_of::<Vertex>() as u32,
        };

        log::debug!(
            "uploaded {} vertices ({} KiB) to a default-heap buffer",
            vertices.len(),
            byte_size / 1024
        );

        Ok(Self {
            buffer: destination,
            view,
            vertex_count: vertices.len() as u32,
        })
    }

    /// The vertex-buffer view to bind.
    #[must_use]
    pub fn view(&self) -> &D3D12_VERTEX_BUFFER_VIEW {
        &self.view
    }

    /// Number of vertices in the buffer.
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    /// Triangles in the buffer, assuming a triangle list.
    #[must_use]
    pub fn triangle_count(&self) -> u32 {
        self.vertex_count / 3
    }

    /// Uploads any tightly packed `repr(C)` data as a vertex stream.
    ///
    /// Used for per-instance streams, whose element type is not [`Vertex`].
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for empty input, or
    /// [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload_raw<T: Copy>(device: &RenderDevice, data: &[T]) -> Result<Self, RendererError> {
        if data.is_empty() {
            return Err(RendererError::InvalidConfig {
                reason: "a vertex stream needs at least one element".to_owned(),
            });
        }

        let byte_size = std::mem::size_of_val(data);
        let destination = device.create_buffer(
            byte_size,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let staging = device.create_buffer(
            byte_size,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?;

        write_staging_raw(&staging, data.as_ptr().cast::<u8>(), byte_size)?;
        copy_and_wait(device, &staging, &destination, byte_size)?;

        // SAFETY: FFI call returning the resource's GPU address, valid while it lives.
        let gpu_address = unsafe { destination.GetGPUVirtualAddress() };
        Ok(Self {
            buffer: destination,
            view: D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: gpu_address,
                SizeInBytes: byte_size as u32,
                StrideInBytes: size_of::<T>() as u32,
            },
            vertex_count: data.len() as u32,
        })
    }

    /// The underlying resource, for diagnostics.
    #[must_use]
    pub fn resource(&self) -> &ID3D12Resource {
        &self.buffer
    }
}

/// Copies `byte_size` bytes from `source` into an upload-heap buffer.
fn write_staging_raw(
    staging: &ID3D12Resource,
    source: *const u8,
    byte_size: usize,
) -> Result<(), RendererError> {
    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    // A read range of 0..0 states the CPU will not read this buffer back.
    let read_range = D3D12_RANGE { Begin: 0, End: 0 };
    // SAFETY: FFI call mapping subresource 0 of an upload-heap buffer.
    unsafe { staging.Map(0, Some(&read_range), Some(&mut mapped)) }
        .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

    // SAFETY: the mapping covers `byte_size` bytes, which is exactly the size the
    // resource was created with, and the two allocations are distinct.
    unsafe {
        std::ptr::copy_nonoverlapping(source, mapped.cast::<u8>(), byte_size);
        staging.Unmap(0, None);
    }
    Ok(())
}

/// Records and executes the staging copy, then blocks until the GPU finishes it.
fn copy_and_wait(
    device: &RenderDevice,
    staging: &ID3D12Resource,
    destination: &ID3D12Resource,
    byte_size: usize,
) -> Result<(), RendererError> {
    let allocator = device.create_command_allocator()?;
    let list = device.create_command_list(&allocator)?;
    // The device layer hands back a closed list; recording needs it open.
    // SAFETY: FFI call. The list was created and closed by the device layer, and the
    // allocator is live and used by nothing else, which is what Reset requires.
    unsafe { list.Reset(&allocator, None) }
        .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;

    // SAFETY: FFI calls recording into the open list. Both resources outlive the
    // submission because this function waits for it to complete before returning, and the
    // states match how the buffers were created.
    unsafe {
        list.CopyBufferRegion(destination, 0, staging, 0, byte_size as u64);
        list.ResourceBarrier(&[transition_barrier(
            destination,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
        )]);
    }

    // SAFETY: FFI call closing the list recorded above.
    unsafe { list.Close() }
        .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;

    let submission = [Some(list.cast().map_err(|error| {
        RendererError::api("ID3D12GraphicsCommandList::cast", error)
    })?)];
    // SAFETY: FFI call submitting the closed list, which outlives the call.
    unsafe { device.queue().ExecuteCommandLists(&submission) };

    // Blocking here is what makes the staging buffer safe to drop on return.
    let mut fence = GpuFence::new(device)?;
    fence.wait_for_idle(device.queue())
}

// `GpuMesh` needs a device, so its behaviour is covered by the GPU integration tests in
// `tests/stress_scene.rs` rather than by unit tests here.
