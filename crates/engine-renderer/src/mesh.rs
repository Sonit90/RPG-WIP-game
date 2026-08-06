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
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD, D3D12_RANGE, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_VERTEX_BUFFER_VIEW, ID3D12CommandAllocator,
    ID3D12CommandQueue, ID3D12Device, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::core::Interface;

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
    pub fn upload(
        device: &ID3D12Device,
        queue: &ID3D12CommandQueue,
        vertices: &[Vertex],
    ) -> Result<Self, RendererError> {
        if vertices.is_empty() {
            return Err(RendererError::InvalidConfig {
                reason: "a mesh needs at least one vertex".to_owned(),
            });
        }

        let byte_size = std::mem::size_of_val(vertices);
        let destination = create_buffer(
            device,
            byte_size,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let staging = create_buffer(
            device,
            byte_size,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?;

        write_staging(&staging, vertices, byte_size)?;
        copy_and_wait(device, queue, &staging, &destination, byte_size)?;

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

    /// The underlying resource, for diagnostics.
    #[must_use]
    pub fn resource(&self) -> &ID3D12Resource {
        &self.buffer
    }
}

/// Creates a buffer of `byte_size` bytes on the given heap.
fn create_buffer(
    device: &ID3D12Device,
    byte_size: usize,
    heap_type: windows::Win32::Graphics::Direct3D12::D3D12_HEAP_TYPE,
    initial_state: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource, RendererError> {
    let heap_properties = D3D12_HEAP_PROPERTIES {
        Type: heap_type,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: byte_size as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };

    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: FFI call with fully initialized descriptors that outlive it, and a valid
    // out-parameter.
    unsafe {
        device.CreateCommittedResource(
            &heap_properties,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            initial_state,
            None,
            &mut buffer,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

    buffer.ok_or_else(|| RendererError::InvalidConfig {
        reason: "buffer creation produced no resource".to_owned(),
    })
}

/// Copies `vertices` into an upload-heap buffer.
fn write_staging(
    staging: &ID3D12Resource,
    vertices: &[Vertex],
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
        std::ptr::copy_nonoverlapping(
            vertices.as_ptr().cast::<u8>(),
            mapped.cast::<u8>(),
            byte_size,
        );
        staging.Unmap(0, None);
    }
    Ok(())
}

/// Records and executes the staging copy, then blocks until the GPU finishes it.
fn copy_and_wait(
    device: &ID3D12Device,
    queue: &ID3D12CommandQueue,
    staging: &ID3D12Resource,
    destination: &ID3D12Resource,
    byte_size: usize,
) -> Result<(), RendererError> {
    // SAFETY: FFI call on a live device with a valid command list type.
    let allocator: ID3D12CommandAllocator =
        unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandAllocator", error))?;
    // SAFETY: FFI call. The allocator is live and no initial pipeline state is needed for
    // a list that only copies.
    let list: ID3D12GraphicsCommandList =
        unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandList", error))?;

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
    unsafe { queue.ExecuteCommandLists(&submission) };

    // Blocking here is what makes the staging buffer safe to drop on return.
    let mut fence = GpuFence::new(device)?;
    fence.wait_for_idle(queue)
}

// `GpuMesh` needs a device, so its behaviour is covered by the GPU integration tests in
// `tests/stress_scene.rs` rather than by unit tests here.
