//! GPU-resident vertex buffers.
//!
//! Geometry lives on a **default** heap, filled once through an upload-heap staging
//! buffer. That matters for the stress tests: an upload-heap vertex buffer is CPU-visible
//! memory the GPU reads across PCIe on every draw, so at 100,000 triangles a benchmark
//! would be measuring bus bandwidth rather than the renderer. A default-heap buffer lives
//! in device-local memory.
//!
//! Uploading blocks until the copy completes. That is acceptable because it happens when
//! the scene changes, never per frame. Several streams uploaded together share one wait —
//! see `upload_many` and `crate::upload::StagingUploader`.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER, D3D12_VERTEX_BUFFER_VIEW, ID3D12Resource,
};

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::pipeline::Vertex;
use crate::resource::{GpuBuffer, TrackedResource};
use crate::upload::StagingUploader;

/// A vertex buffer in device-local memory.
pub struct GpuMesh {
    /// Kept alive because the view points into it.
    buffer: GpuBuffer,
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
        Self::upload_raw(device, vertices)
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
        let destination = GpuBuffer::new_default(device, byte_size)?;

        // One mesh, so one flush — but through the batching uploader, so a caller with
        // several meshes can share a single synchronization instead of paying one per
        // mesh. `GpuMesh::upload_many` is that caller.
        let mut uploader = StagingUploader::new(device)?;
        uploader.queue_buffer(
            device,
            &destination,
            data,
            D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
        )?;
        uploader.flush(device)?;

        Ok(Self::from_buffer(destination, size_of::<T>(), data.len()))
    }

    /// Uploads several vertex streams with a single GPU synchronization.
    ///
    /// The reason the batching uploader exists: `n` separate uploads cost `n` full waits,
    /// while this costs one however many streams are queued.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for an empty stream, or
    /// [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload_many<T: Copy>(
        device: &RenderDevice,
        streams: &[&[T]],
    ) -> Result<Vec<Self>, RendererError> {
        if streams.iter().any(|stream| stream.is_empty()) {
            return Err(RendererError::InvalidConfig {
                reason: "a vertex stream needs at least one element".to_owned(),
            });
        }

        let mut uploader = StagingUploader::new(device)?;
        let mut meshes = Vec::with_capacity(streams.len());

        for stream in streams {
            let buffer = GpuBuffer::new_default(device, std::mem::size_of_val(*stream))?;
            uploader.queue_buffer(
                device,
                &buffer,
                stream,
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            )?;
            meshes.push(Self::from_buffer(buffer, size_of::<T>(), stream.len()));
        }

        uploader.flush(device)?;
        Ok(meshes)
    }

    /// Builds a mesh around an already-filled buffer.
    fn from_buffer(buffer: GpuBuffer, stride: usize, count: usize) -> Self {
        let view = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: buffer.gpu_address(),
            SizeInBytes: (stride * count) as u32,
            StrideInBytes: stride as u32,
        };
        Self {
            buffer,
            view,
            vertex_count: count as u32,
        }
    }

    /// The underlying resource, for diagnostics.
    #[must_use]
    pub fn resource(&self) -> &ID3D12Resource {
        self.buffer.resource()
    }
}

// `GpuMesh` needs a device, so its behaviour is covered by the GPU integration tests in
// `tests/stress_scene.rs` rather than by unit tests here.
