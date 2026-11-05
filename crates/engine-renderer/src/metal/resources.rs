//! Buffers, meshes and textures.
//!
//! Apple silicon has one memory the CPU and GPU share, so none of this stages: a buffer is
//! created in shared storage with its contents, a texture is created and filled with
//! `replaceRegion`, and both are ready for the GPU the moment the call returns. That is why
//! the renderer's upload batch has nothing to defer.

use std::ptr::NonNull;

use engine_core::math::{Sphere, Vec3};
use objc2_metal::{
    MTLBuffer, MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLResourceOptions, MTLSize,
    MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use super::{Buffer, Device, Texture, api_error, slice_bytes};
use crate::dynamic_mesh::CopyRotation;
use crate::error::RendererError;
use crate::mesh::{Index, MeshVertex};
use crate::mipmap::Compression;

/// Creates a shared buffer holding `data`.
///
/// # Errors
///
/// [`RendererError::Api`] if the device cannot allocate it.
pub(crate) fn buffer_with_data<T: Copy>(
    device: &Device,
    data: &[T],
    what: &'static str,
) -> Result<Buffer, RendererError> {
    let bytes = slice_bytes(data);
    // A zero-length buffer is an error in Metal; an empty mesh still needs something to bind.
    let length = bytes.len().max(16);
    let options = MTLResourceOptions::StorageModeShared;
    let buffer = if bytes.is_empty() {
        device.newBufferWithLength_options(length, options)
    } else {
        // SAFETY: `bytes` is a live slice of `bytes.len()` readable bytes, and its pointer is
        // non-null because slices never are; Metal copies it before returning.
        unsafe {
            device.newBufferWithBytes_length_options(
                NonNull::from(bytes).cast::<std::ffi::c_void>(),
                bytes.len(),
                options,
            )
        }
    };
    buffer.ok_or_else(|| {
        api_error(
            "MTLDevice::newBuffer",
            format!("could not allocate {length} bytes for {what}"),
        )
    })
}

/// Creates an empty shared buffer of `length` bytes, mapped for the CPU.
///
/// # Errors
///
/// [`RendererError::Api`] if the device cannot allocate it.
pub(crate) fn empty_buffer(
    device: &Device,
    length: usize,
    what: &'static str,
) -> Result<Buffer, RendererError> {
    device
        .newBufferWithLength_options(length.max(16), MTLResourceOptions::StorageModeShared)
        .ok_or_else(|| {
            api_error(
                "MTLDevice::newBufferWithLength",
                format!("could not allocate {length} bytes for {what}"),
            )
        })
}

/// An indexed mesh: a vertex buffer, an `u32` index buffer and what to draw from them.
#[derive(Clone)]
pub(crate) struct GpuMesh {
    vertices: Buffer,
    indices: Buffer,
    vertex_count: u32,
    index_count: u32,
    bounds: Sphere,
}

impl GpuMesh {
    /// Uploads `vertices` and `indices` as a mesh.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if a buffer cannot be allocated.
    pub(crate) fn new<V: Copy>(
        device: &Device,
        vertices: &[V],
        indices: &[Index],
        bounds: Sphere,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            vertices: buffer_with_data(device, vertices, "a vertex buffer")?,
            indices: buffer_with_data(device, indices, "an index buffer")?,
            vertex_count: vertices.len() as u32,
            index_count: indices.len() as u32,
            bounds,
        })
    }

    /// The vertex buffer, drawn from at offset zero with vertex pulling.
    pub(crate) fn vertex_buffer(&self) -> &Buffer {
        &self.vertices
    }

    /// The index buffer.
    pub(crate) fn index_buffer(&self) -> &Buffer {
        &self.indices
    }

    /// Indices to draw; zero draws nothing.
    pub(crate) fn index_count(&self) -> u32 {
        self.index_count
    }

    /// Local-space bounding sphere.
    pub(crate) fn bounds(&self) -> Sphere {
        self.bounds
    }
}

/// The CPU side of a dynamic mesh: its buffer copies and which one is live.
///
/// The same scheme as the D3D12 one (`crate::dynamic_mesh`): `frames_in_flight + 1` copies,
/// advanced by each write that lands in a new frame, so a write never touches a copy a frame
/// still in flight reads. Metal's command buffers also retain what they bind, which makes the
/// rotation belt and braces rather than the only safety.
pub(crate) struct DynamicMesh {
    copies: Vec<(Buffer, Buffer)>,
    rotation: CopyRotation,
    max_vertices: usize,
    max_indices: usize,
}

impl DynamicMesh {
    /// Allocates the copies and returns them with the empty mesh that draws from them.
    ///
    /// # Errors
    ///
    /// [`RendererError::InvalidConfig`] for a zero capacity, [`RendererError::Api`] if a buffer
    /// cannot be allocated.
    pub(crate) fn new(
        device: &Device,
        frames_in_flight: usize,
        max_vertices: usize,
        max_indices: usize,
    ) -> Result<(Self, GpuMesh), RendererError> {
        if max_vertices == 0 || max_indices == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a dynamic mesh needs room for geometry, got {max_vertices} vertices and \
                     {max_indices} indices"
                ),
            });
        }
        let copy_count = frames_in_flight + 1;
        let mut copies = Vec::with_capacity(copy_count);
        for _ in 0..copy_count {
            copies.push((
                empty_buffer(
                    device,
                    max_vertices * size_of::<MeshVertex>(),
                    "a dynamic vertex buffer",
                )?,
                empty_buffer(
                    device,
                    max_indices * size_of::<Index>(),
                    "a dynamic index buffer",
                )?,
            ));
        }
        let mesh = GpuMesh {
            vertices: copies[0].0.clone(),
            indices: copies[0].1.clone(),
            vertex_count: 0,
            index_count: 0,
            bounds: Sphere::new(Vec3::ZERO, 0.0),
        };
        Ok((
            Self {
                copies,
                rotation: CopyRotation::new(copy_count),
                max_vertices,
                max_indices,
            },
            mesh,
        ))
    }

    /// Writes the geometry into the copy for `frame` and re-aims `mesh` at it.
    ///
    /// # Errors
    ///
    /// [`RendererError::InvalidConfig`] if the geometry exceeds the capacity given at creation.
    pub(crate) fn write(
        &mut self,
        mesh: &mut GpuMesh,
        frame: u64,
        vertices: &[MeshVertex],
        indices: &[Index],
        bounds: Sphere,
    ) -> Result<(), RendererError> {
        if vertices.len() > self.max_vertices || indices.len() > self.max_indices {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "dynamic mesh write of {} vertices and {} indices exceeds its capacity of \
                     {} and {}",
                    vertices.len(),
                    indices.len(),
                    self.max_vertices,
                    self.max_indices
                ),
            });
        }
        let index = self.rotation.next_for_write(frame);
        let (vertex_buffer, index_buffer) = &self.copies[index];
        copy_into(vertex_buffer, slice_bytes(vertices));
        copy_into(index_buffer, slice_bytes(indices));
        mesh.vertices = vertex_buffer.clone();
        mesh.indices = index_buffer.clone();
        mesh.vertex_count = vertices.len() as u32;
        // An empty index list means "draw nothing", whatever the vertices say.
        mesh.index_count = indices.len() as u32;
        mesh.bounds = bounds;
        Ok(())
    }
}

/// Copies `bytes` to the start of a shared buffer; `bytes` must fit.
fn copy_into(buffer: &Buffer, bytes: &[u8]) {
    let length = bytes.len().min(buffer.length());
    if length == 0 {
        return;
    }
    // SAFETY: the buffer is in shared storage, so `contents` is a CPU mapping of
    // `buffer.length()` bytes, and `length` is clamped to it. The source is caller memory, so
    // the ranges are distinct. The GPU is not reading this copy: see `DynamicMesh`.
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            buffer.contents().as_ptr().cast::<u8>(),
            length,
        );
    }
}

/// How a texture's levels are stored on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextureFormat {
    /// The Metal pixel format.
    pub(crate) pixel_format: MTLPixelFormat,
    /// Block compression of the stored levels, if any.
    pub(crate) compression: Compression,
}

impl TextureFormat {
    /// The format for `compression` and a colour space.
    pub(crate) fn new(compression: Compression, srgb: bool) -> Self {
        let pixel_format = match (compression, srgb) {
            (Compression::Bc1, true) => MTLPixelFormat::BC1_RGBA_sRGB,
            (Compression::Bc1, false) => MTLPixelFormat::BC1_RGBA,
            (Compression::Bc5, _) => MTLPixelFormat::BC5_RGUnorm,
            (Compression::None, true) => MTLPixelFormat::RGBA8Unorm_sRGB,
            (Compression::None, false) => MTLPixelFormat::RGBA8Unorm,
        };
        Self {
            pixel_format,
            compression,
        }
    }

    /// Bytes in one row of blocks (or of texels, uncompressed) of a `width`-texel level.
    fn row_bytes(self, width: u32) -> usize {
        match self.compression {
            Compression::None => width as usize * 4,
            Compression::Bc1 => (width as usize).div_ceil(4) * engine_assets::bc::BC1_BLOCK_BYTES,
            Compression::Bc5 => (width as usize).div_ceil(4) * engine_assets::bc::BC5_BLOCK_BYTES,
        }
    }
}

/// Creates a shared 2D texture and uploads every mip level into it.
///
/// `levels[0]` is the full-size image and each following entry the next smaller level, as
/// `mipmap` produces them; the texture has exactly `levels.len()` levels.
///
/// # Errors
///
/// [`RendererError::InvalidConfig`] for no levels, [`RendererError::Api`] if the texture cannot
/// be created.
pub(crate) fn texture_with_levels(
    device: &Device,
    width: u32,
    height: u32,
    format: TextureFormat,
    levels: &[&[u8]],
) -> Result<Texture, RendererError> {
    if levels.is_empty() {
        return Err(RendererError::InvalidConfig {
            reason: "a texture needs at least one level".to_owned(),
        });
    }
    // SAFETY: a plain constructor call; the class method has no preconditions.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format.pixel_format,
            width as usize,
            height as usize,
            false,
        )
    };
    // SAFETY: the level count is at least one and is the number of levels uploaded below.
    unsafe { descriptor.setMipmapLevelCount(levels.len()) };
    descriptor.setStorageMode(MTLStorageMode::Shared);
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    let Some(texture) = device.newTextureWithDescriptor(&descriptor) else {
        return Err(api_error(
            "MTLDevice::newTextureWithDescriptor",
            format!(
                "could not create a {width}x{height} texture with {} levels",
                levels.len()
            ),
        ));
    };
    for (level, texels) in levels.iter().enumerate() {
        let (level_width, level_height) = crate::mipmap::level_size(width, height, level as u32);
        let Some(pointer) = NonNull::new(texels.as_ptr().cast_mut()).map(NonNull::cast) else {
            continue;
        };
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: level_width as usize,
                height: level_height as usize,
                depth: 1,
            },
        };
        // SAFETY: `texels` holds one level of `format`, so it covers the `bytes_per_row`
        // rows the region reads; Metal copies from it before returning and never writes to it.
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                level,
                pointer,
                format.row_bytes(level_width),
            );
        }
    }
    Ok(texture)
}

/// Creates a depth texture of `width` x `height`: the camera's depth buffer, and the shadow
/// atlas.
///
/// `Depth32Float`, private storage, and sampleable so the contact-shadow march and the
/// lighting pass can read it.
///
/// # Errors
///
/// [`RendererError::Api`] if the texture cannot be created.
pub(crate) fn depth_texture(
    device: &Device,
    width: u32,
    height: u32,
) -> Result<Texture, RendererError> {
    // SAFETY: a plain constructor call; the class method has no preconditions.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::Depth32Float,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setStorageMode(MTLStorageMode::Private);
    descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    device.newTextureWithDescriptor(&descriptor).ok_or_else(|| {
        api_error(
            "MTLDevice::newTextureWithDescriptor",
            format!("could not create a {width}x{height} depth buffer"),
        )
    })
}

/// Creates a private `format` texture that passes render into and later passes sample: the
/// contact-shadow target.
///
/// # Errors
///
/// [`RendererError::Api`] if the texture cannot be created.
pub(crate) fn color_target_texture(
    device: &Device,
    width: u32,
    height: u32,
    format: MTLPixelFormat,
) -> Result<Texture, RendererError> {
    // SAFETY: a plain constructor call; the class method has no preconditions.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setStorageMode(MTLStorageMode::Private);
    descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    device.newTextureWithDescriptor(&descriptor).ok_or_else(|| {
        api_error(
            "MTLDevice::newTextureWithDescriptor",
            format!("could not create a {width}x{height} render target"),
        )
    })
}
