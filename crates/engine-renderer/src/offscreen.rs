//! Offscreen rendering with CPU readback.
//!
//! Renders into a texture the swap chain never sees, copies it into a readback buffer and
//! returns the pixels. This is how the engine proves it actually draws something: a
//! recorded draw call and a silent validation layer are not evidence — a culled triangle
//! produces both. It needs no window, so it also runs where no desktop is available.
//!
//! Milestone 9 builds its automated GPU test on this; Milestone 4 uses it to verify the
//! triangle is visible.

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK, D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    D3D12_RANGE, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandAllocator, ID3D12DescriptorHeap,
    ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use crate::command::{scissor_for, transition_barrier, viewport_for};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::mesh::GpuMesh;
use crate::pipeline::{ScenePipeline, TRIANGLE_VERTICES, Vertex};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// Bytes per pixel of [`BACK_BUFFER_FORMAT`].
pub const BYTES_PER_PIXEL: usize = 4;

/// A rendered image read back to system memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA8 rows — the readback buffer's row padding is removed.
    pub pixels: Vec<u8>,
}

impl CapturedImage {
    /// Returns the RGBA value at `(x, y)`, or `None` if it is outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL;
        self.pixels
            .get(offset..offset + BYTES_PER_PIXEL)
            .map(|slice| [slice[0], slice[1], slice[2], slice[3]])
    }

    /// Counts pixels for which `predicate` holds.
    pub fn count_pixels(&self, predicate: impl Fn([u8; 4]) -> bool) -> usize {
        self.pixels
            .chunks_exact(BYTES_PER_PIXEL)
            .filter(|chunk| predicate([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .count()
    }
}

/// Renders [`TRIANGLE_VERTICES`] offscreen and reads the result back.
///
/// # Errors
///
/// Returns [`RendererError`] if the device, any resource, the pipeline or the readback
/// fails.
pub fn render_triangle(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
) -> Result<CapturedImage, RendererError> {
    render_vertices(width, height, clear_color, &TRIANGLE_VERTICES)
}

/// Renders arbitrary vertices offscreen and reads the result back.
///
/// Creates its own device, so it is independent of any swap chain or window.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
pub fn render_vertices(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    vertices: &[Vertex],
) -> Result<CapturedImage, RendererError> {
    if width == 0 || height == 0 {
        return Err(RendererError::InvalidConfig {
            reason: format!("offscreen size {width}x{height} has a zero dimension"),
        });
    }

    let context = RenderDevice::new(false, false)?;
    let device = context.device();

    let target_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: BACK_BUFFER_FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        ..Default::default()
    };
    let default_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };

    let mut target: Option<ID3D12Resource> = None;
    // SAFETY: FFI call with fully initialized descriptors that outlive it, and a valid
    // out-parameter. RENDER_TARGET is a legal initial state for this resource.
    unsafe {
        device.CreateCommittedResource(
            &default_heap,
            D3D12_HEAP_FLAG_NONE,
            &target_desc,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            None,
            &mut target,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;
    let Some(target) = target else {
        return Err(RendererError::InvalidConfig {
            reason: "offscreen render target creation produced no resource".to_owned(),
        });
    };

    // Descriptor heap with a single RTV for the offscreen texture.
    let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        NodeMask: 0,
    };
    // SAFETY: FFI call with an initialized descriptor that outlives the call.
    let rtv_heap: ID3D12DescriptorHeap = unsafe { device.CreateDescriptorHeap(&heap_desc) }
        .map_err(|error| RendererError::api("ID3D12Device::CreateDescriptorHeap", error))?;
    // SAFETY: FFI calls creating a view of a live resource in our own heap.
    let rtv = unsafe {
        let handle = rtv_heap.GetCPUDescriptorHandleForHeapStart();
        device.CreateRenderTargetView(&target, None, handle);
        handle
    };

    // Readback buffer. Texture rows are padded to D3D12_TEXTURE_DATA_PITCH_ALIGNMENT, so
    // the buffer is larger than width * height * 4 and the padding is stripped later.
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes = 0u64;
    // SAFETY: FFI call. `target_desc` describes the texture being copied and every
    // out-parameter is a valid, owned value.
    unsafe {
        device.GetCopyableFootprints(
            &target_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        );
    }

    let readback_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total_bytes,
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
    let readback_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        ..Default::default()
    };
    let mut readback: Option<ID3D12Resource> = None;
    // SAFETY: FFI call as above. COPY_DEST is the required initial state for a readback
    // buffer that a copy writes into.
    unsafe {
        device.CreateCommittedResource(
            &readback_heap,
            D3D12_HEAP_FLAG_NONE,
            &readback_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut readback,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;
    let Some(readback) = readback else {
        return Err(RendererError::InvalidConfig {
            reason: "readback buffer creation produced no resource".to_owned(),
        });
    };

    let pipeline = ScenePipeline::new(device)?;
    let mesh = GpuMesh::upload(&context, vertices)?;

    // SAFETY: FFI call on a live device with a valid command list type.
    let allocator: ID3D12CommandAllocator =
        unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandAllocator", error))?;
    // SAFETY: FFI call. The allocator is live and no initial pipeline state is required.
    let list: ID3D12GraphicsCommandList =
        unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandList", error))?;

    let source = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: non-owning copy of the interface pointer, kept in a ManuallyDrop so it
        // is never released. `target` outlives this structure.
        pResource: unsafe { std::mem::transmute_copy(&target) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let destination = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above; `readback` outlives this structure.
        pResource: unsafe { std::mem::transmute_copy(&readback) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };

    // SAFETY: FFI calls recording into the open list. Every referenced object is owned by
    // this function and outlives the recording, and the resource states match the
    // transitions requested.
    unsafe {
        list.RSSetViewports(&[viewport_for(width, height)]);
        list.RSSetScissorRects(&[scissor_for(width, height)]);
        list.OMSetRenderTargets(1, Some(&rtv), false, None);
        list.ClearRenderTargetView(rtv, &clear_color, None);
        list.SetGraphicsRootSignature(pipeline.root_signature());
        list.SetPipelineState(pipeline.pipeline_state());
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.IASetVertexBuffers(0, Some(&[*mesh.view()]));
        list.DrawInstanced(mesh.vertex_count(), 1, 0, 0);

        list.ResourceBarrier(&[transition_barrier(
            &target,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
    }

    // SAFETY: FFI call closing the list recorded above.
    unsafe { list.Close() }
        .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;

    let submission = [Some(list.cast().map_err(|error| {
        RendererError::api("ID3D12GraphicsCommandList::cast", error)
    })?)];
    // SAFETY: FFI call submitting the closed list, which outlives the call.
    unsafe { context.queue().ExecuteCommandLists(&submission) };

    let mut fence = crate::sync::GpuFence::new(&context)?;
    fence.wait_for_idle(context.queue())?;

    let pixels = read_pixels(&readback, &footprint, width, height, total_bytes)?;

    Ok(CapturedImage {
        width,
        height,
        pixels,
    })
}

/// Maps the readback buffer and copies its rows into a tightly packed image.
fn read_pixels(
    readback: &ID3D12Resource,
    footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    width: u32,
    height: u32,
    total_bytes: u64,
) -> Result<Vec<u8>, RendererError> {
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let row_bytes = width as usize * BYTES_PER_PIXEL;

    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    let read_range = D3D12_RANGE {
        Begin: 0,
        End: total_bytes as usize,
    };
    // SAFETY: FFI call mapping subresource 0 of a readback buffer, declaring that the
    // whole range will be read.
    unsafe { readback.Map(0, Some(&read_range), Some(&mut mapped)) }
        .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

    let mut pixels = vec![0u8; row_bytes * height as usize];
    // SAFETY: the mapping covers `total_bytes`, which GetCopyableFootprints sized to hold
    // `height` rows of `row_pitch` bytes; each copied row stays inside that range, and
    // source and destination are distinct allocations.
    unsafe {
        let base = mapped.cast::<u8>();
        for row in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                base.add(row * row_pitch),
                pixels.as_mut_ptr().add(row * row_bytes),
                row_bytes,
            );
        }
        // A written range of 0..0 tells the driver the CPU changed nothing.
        readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
    }

    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_lookup_is_row_major_and_bounds_checked() {
        let image = CapturedImage {
            width: 2,
            height: 2,
            pixels: vec![
                1, 2, 3, 4, // (0, 0)
                5, 6, 7, 8, // (1, 0)
                9, 10, 11, 12, // (0, 1)
                13, 14, 15, 16, // (1, 1)
            ],
        };
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
        assert_eq!(image.pixel(1, 0), Some([5, 6, 7, 8]));
        assert_eq!(image.pixel(0, 1), Some([9, 10, 11, 12]));
        assert_eq!(image.pixel(1, 1), Some([13, 14, 15, 16]));
        assert_eq!(image.pixel(2, 0), None);
        assert_eq!(image.pixel(0, 2), None);
    }

    #[test]
    fn counts_matching_pixels() {
        let image = CapturedImage {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 0, 0, 255],
        };
        assert_eq!(image.count_pixels(|p| p[0] > 128), 1);
        assert_eq!(image.count_pixels(|p| p[3] == 255), 2);
    }

    #[test]
    fn rejects_a_zero_sized_target() {
        assert!(render_triangle(0, 16, [0.0; 4]).is_err());
        assert!(render_triangle(16, 0, [0.0; 4]).is_err());
    }
}
