//! Geometry: CPU-side mesh data, GPU-side indexed meshes, and raw vertex streams.
//!
//! Everything here lives on a **default** heap, filled once through an upload-heap staging
//! buffer. That matters for the stress tests: an upload-heap vertex buffer is CPU-visible
//! memory the GPU reads across PCIe on every draw, so at 100,000 triangles a benchmark
//! would be measuring bus bandwidth rather than the renderer.
//!
//! Uploading blocks until the copy completes. That is acceptable because it happens when
//! the scene changes, never per frame, and a mesh's vertex and index buffers share one
//! wait — see [`crate::upload::StagingUploader`].

use engine_core::math::{Sphere, Vec2, Vec3};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_INDEX_BUFFER_VIEW, D3D12_RESOURCE_STATE_INDEX_BUFFER,
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER, D3D12_VERTEX_BUFFER_VIEW, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT;

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::{GpuBuffer, TrackedResource};
use crate::upload::StagingUploader;

/// One vertex of a mesh: position, normal and texture coordinate.
///
/// `repr(C)` is required — the input layout describes this exact memory layout to the GPU,
/// so Rust must not be free to reorder the fields.
///
/// 32 bytes, which is two per cache line. Packing the normal into 4 bytes and the UV into 4
/// would halve that; it is not done because nothing yet is vertex-fetch bound, and an
/// unmeasured packing is a decoding cost in every shader for a saving nobody has seen.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeshVertex {
    /// Position in the mesh's local space.
    pub position: Vec3,
    /// Unit normal in the mesh's local space.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
}

impl MeshVertex {
    /// Builds a vertex.
    #[must_use]
    pub const fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position,
            normal,
            uv,
        }
    }
}

/// The index type. 32-bit throughout.
///
/// 16-bit indices halve index bandwidth and cap a mesh at 65,536 vertices. The cap is the
/// problem: a mesh that crosses it needs splitting, which is a build-time system that does
/// not exist. When index bandwidth shows up in a measurement, the format becomes a field on
/// the mesh rather than a constant.
pub type Index = u32;

/// A mesh on the CPU: vertices, indices and the bounds culling will use.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshData {
    vertices: Vec<MeshVertex>,
    indices: Vec<Index>,
    bounds: Sphere,
}

impl MeshData {
    /// Builds mesh data, computing its bounding sphere.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for empty input, an index count that is not
    /// a multiple of three, or an index past the end of the vertex array. The last one is
    /// the important check: D3D12 does not bounds-check indices, and an out-of-range one
    /// reads whatever follows the buffer — usually rendering a spike to the horizon rather
    /// than failing.
    pub fn new(vertices: Vec<MeshVertex>, indices: Vec<Index>) -> Result<Self, RendererError> {
        if vertices.is_empty() || indices.is_empty() {
            return Err(RendererError::InvalidConfig {
                reason: "a mesh needs at least one vertex and one index".to_owned(),
            });
        }
        if indices.len() % 3 != 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a triangle list needs a multiple of three indices, got {}",
                    indices.len()
                ),
            });
        }
        if let Some(bad) = indices
            .iter()
            .find(|index| **index as usize >= vertices.len())
        {
            return Err(RendererError::InvalidConfig {
                reason: format!("index {bad} is past the end of {} vertices", vertices.len()),
            });
        }

        let bounds = Sphere::around(vertices.iter().map(|vertex| vertex.position));
        Ok(Self {
            vertices,
            indices,
            bounds,
        })
    }

    /// The vertices.
    #[must_use]
    pub fn vertices(&self) -> &[MeshVertex] {
        &self.vertices
    }

    /// The indices.
    #[must_use]
    pub fn indices(&self) -> &[Index] {
        &self.indices
    }

    /// The local-space bounding sphere.
    #[must_use]
    pub fn bounds(&self) -> Sphere {
        self.bounds
    }

    /// Triangles, assuming a triangle list.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// A cube centred on the origin, with flat per-face normals.
    ///
    /// Twenty-four vertices rather than eight: a cube's corners have three different
    /// normals each, and sharing them would average the normals into a sphere-like shading
    /// that makes a cube look like a bad ball.
    #[must_use]
    pub fn cube(size: f32) -> Self {
        let half = size * 0.5;
        let mut vertices = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);

        // Each face as (normal, tangent, bitangent), chosen so that `tangent × bitangent`
        // equals the normal. That is what makes the quad below wind counter-clockwise seen
        // from outside, which is what the rasterizer calls front-facing.
        //
        // Getting this wrong does not produce a missing cube: the outward faces are culled
        // and the *inward* ones are drawn instead, which looks identical until something
        // reads the normal — at which point every surface is lit from the wrong side.
        let faces = [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (-Vec3::X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (-Vec3::Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (-Vec3::Z, Vec3::Y, Vec3::X),
        ];
        debug_assert!(
            faces.iter().all(
                |(normal, tangent, bitangent)| (tangent.cross(*bitangent) - *normal)
                    .length_squared()
                    < 1e-6
            ),
            "each face basis must be right-handed about its own normal"
        );

        for (normal, tangent, bitangent) in faces {
            let base = vertices.len() as Index;
            let center = normal * half;
            for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                vertices.push(MeshVertex::new(
                    center + tangent * (u * half) + bitangent * (v * half),
                    normal,
                    Vec2::new((u + 1.0) * 0.5, (1.0 - v) * 0.5),
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        Self {
            bounds: Sphere::around(vertices.iter().map(|vertex| vertex.position)),
            vertices,
            indices,
        }
    }

    /// A flat quad in the XZ plane, facing +Y.
    #[must_use]
    pub fn plane(size: f32) -> Self {
        let half = size * 0.5;
        let vertices = vec![
            MeshVertex::new(Vec3::new(-half, 0.0, half), Vec3::Y, Vec2::new(0.0, 1.0)),
            MeshVertex::new(Vec3::new(half, 0.0, half), Vec3::Y, Vec2::new(1.0, 1.0)),
            MeshVertex::new(Vec3::new(half, 0.0, -half), Vec3::Y, Vec2::new(1.0, 0.0)),
            MeshVertex::new(Vec3::new(-half, 0.0, -half), Vec3::Y, Vec2::new(0.0, 0.0)),
        ];
        Self {
            bounds: Sphere::around(vertices.iter().map(|vertex| vertex.position)),
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    /// A flat disc of `radius`, facing up, as a fan of `segments` triangles.
    ///
    /// What an area-of-effect indicator is: a circle painted on the floor. A quad with a
    /// round texture would do the same job once there are textures; until then the shape
    /// has to be in the geometry, because a square decal says the ability hits a square.
    #[must_use]
    pub fn disc(radius: f32, segments: u32) -> Self {
        let segments = segments.max(3);
        let mut vertices = Vec::with_capacity(segments as usize + 1);
        vertices.push(MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::new(0.5, 0.5)));
        for segment in 0..segments {
            let angle = segment as f32 / segments as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            vertices.push(MeshVertex::new(
                Vec3::new(cos * radius, 0.0, sin * radius),
                Vec3::Y,
                Vec2::new(cos * 0.5 + 0.5, sin * 0.5 + 0.5),
            ));
        }

        let mut indices = Vec::with_capacity(segments as usize * 3);
        for segment in 0..segments {
            let next = segment % segments + 1;
            let following = (segment + 1) % segments + 1;
            // Wound so the disc faces up, which is the way the floor is lit from.
            indices.extend_from_slice(&[0, following, next]);
        }

        Self {
            bounds: Sphere::around(vertices.iter().map(|vertex| vertex.position)),
            vertices,
            indices,
        }
    }

    /// A flat ring lying face up: the outline of a disc, `thickness` wide.
    ///
    /// The bright edge of an area indicator. Drawn as geometry rather than as a line so it
    /// has a real width on the ground and stays readable at any distance and any angle.
    #[must_use]
    pub fn ring(radius: f32, thickness: f32, segments: u32) -> Self {
        let segments = segments.max(3);
        let inner = (radius - thickness).max(0.0);
        let mut vertices = Vec::with_capacity(segments as usize * 2);
        for segment in 0..segments {
            let angle = segment as f32 / segments as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            vertices.push(MeshVertex::new(
                Vec3::new(cos * inner, 0.0, sin * inner),
                Vec3::Y,
                Vec2::new(0.0, 0.0),
            ));
            vertices.push(MeshVertex::new(
                Vec3::new(cos * radius, 0.0, sin * radius),
                Vec3::Y,
                Vec2::new(1.0, 0.0),
            ));
        }

        let mut indices = Vec::with_capacity(segments as usize * 6);
        for segment in 0..segments {
            let a = segment * 2;
            let b = a + 1;
            let c = (segment + 1) % segments * 2;
            let d = c + 1;
            // Wound to face up, like the disc it outlines.
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }

        Self {
            bounds: Sphere::around(vertices.iter().map(|vertex| vertex.position)),
            vertices,
            indices,
        }
    }

    /// A flat sector lying face up: a slice of a disc of `radius`, `half_angle` either side
    /// of -Z, which is the direction a character with zero yaw faces.
    ///
    /// The footprint of a cone ability, so the player can see what a cone will actually
    /// catch before committing to it.
    #[must_use]
    pub fn sector(radius: f32, half_angle: f32, segments: u32) -> Self {
        let segments = segments.max(3);
        let mut vertices = Vec::with_capacity(segments as usize + 2);
        vertices.push(MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::new(0.5, 0.5)));
        for segment in 0..=segments {
            // Zero faces -Z; the sweep runs from one edge of the cone to the other.
            let angle = -std::f32::consts::FRAC_PI_2 - half_angle
                + (segment as f32 / segments as f32) * half_angle * 2.0;
            let (sin, cos) = angle.sin_cos();
            vertices.push(MeshVertex::new(
                Vec3::new(cos * radius, 0.0, sin * radius),
                Vec3::Y,
                Vec2::new(cos * 0.5 + 0.5, sin * 0.5 + 0.5),
            ));
        }

        let mut indices = Vec::with_capacity(segments as usize * 3);
        for segment in 0..segments {
            // Wound to face up, like the disc and the ring.
            indices.extend_from_slice(&[0, segment + 2, segment + 1]);
        }

        Self {
            bounds: Sphere::around(vertices.iter().map(|vertex| vertex.position)),
            vertices,
            indices,
        }
    }

    /// A UV sphere with `segments` around and `rings` from pole to pole.
    ///
    /// Exists because a lit sphere is the only shape whose shading is obviously right or
    /// obviously wrong at a glance — a cube's flat faces hide a normal that is subtly off.
    /// Both counts are clamped to a sane minimum rather than rejected, because this is a
    /// generator for test scenes, not an asset importer.
    #[must_use]
    pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let segments = segments.max(3);
        let rings = rings.max(2);

        let mut vertices = Vec::with_capacity(((segments + 1) * (rings + 1)) as usize);
        for ring in 0..=rings {
            let v = ring as f32 / rings as f32;
            let phi = v * std::f32::consts::PI;
            let (sin_phi, cos_phi) = phi.sin_cos();
            for segment in 0..=segments {
                let u = segment as f32 / segments as f32;
                let theta = u * std::f32::consts::TAU;
                let (sin_theta, cos_theta) = theta.sin_cos();
                let normal = Vec3::new(sin_phi * cos_theta, cos_phi, sin_phi * sin_theta);
                vertices.push(MeshVertex::new(normal * radius, normal, Vec2::new(u, v)));
            }
        }

        let stride = segments + 1;
        let mut indices = Vec::with_capacity((segments * rings * 6) as usize);
        for ring in 0..rings {
            for segment in 0..segments {
                let a = ring * stride + segment;
                let b = a + stride;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }

        Self {
            // The bound is exact here, so it is stated rather than fitted: a sphere of
            // radius r about the origin bounds itself.
            bounds: Sphere::new(Vec3::ZERO, radius),
            vertices,
            indices,
        }
    }
}

/// An indexed mesh in device-local memory.
pub struct GpuMesh {
    /// Kept alive because the view points into it.
    vertex_buffer: GpuBuffer,
    /// Kept alive because the view points into it.
    index_buffer: GpuBuffer,
    vertex_view: D3D12_VERTEX_BUFFER_VIEW,
    index_view: D3D12_INDEX_BUFFER_VIEW,
    vertex_count: u32,
    index_count: u32,
    bounds: Sphere,
}

impl GpuMesh {
    /// Uploads a mesh and waits for the copy to finish.
    ///
    /// Both buffers go through one [`StagingUploader`], so a mesh costs one
    /// synchronization rather than two.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload(device: &RenderDevice, data: &MeshData) -> Result<Self, RendererError> {
        let mut uploader = StagingUploader::new(device)?;
        let mesh = Self::queue(device, &mut uploader, data)?;
        uploader.flush(device)?;
        Ok(mesh)
    }

    /// Uploads several meshes with a single GPU synchronization.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload_many(
        device: &RenderDevice,
        meshes: &[&MeshData],
    ) -> Result<Vec<Self>, RendererError> {
        let mut uploader = StagingUploader::new(device)?;
        let mut uploaded = Vec::with_capacity(meshes.len());
        for data in meshes {
            uploaded.push(Self::queue(device, &mut uploader, data)?);
        }
        uploader.flush(device)?;
        Ok(uploaded)
    }

    /// Creates both buffers and queues their copies, without flushing.
    fn queue(
        device: &RenderDevice,
        uploader: &mut StagingUploader,
        data: &MeshData,
    ) -> Result<Self, RendererError> {
        let vertices = data.vertices();
        let indices = data.indices();

        let vertex_buffer = GpuBuffer::new_default(device, std::mem::size_of_val(vertices))?;
        uploader.queue_buffer(
            device,
            &vertex_buffer,
            vertices,
            D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
        )?;

        let index_buffer = GpuBuffer::new_default(device, std::mem::size_of_val(indices))?;
        uploader.queue_buffer(
            device,
            &index_buffer,
            indices,
            D3D12_RESOURCE_STATE_INDEX_BUFFER,
        )?;

        let vertex_view = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: vertex_buffer.gpu_address(),
            SizeInBytes: std::mem::size_of_val(vertices) as u32,
            StrideInBytes: size_of::<MeshVertex>() as u32,
        };
        let index_view = D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: index_buffer.gpu_address(),
            SizeInBytes: std::mem::size_of_val(indices) as u32,
            Format: DXGI_FORMAT_R32_UINT,
        };

        Ok(Self {
            vertex_buffer,
            index_buffer,
            vertex_view,
            index_view,
            vertex_count: vertices.len() as u32,
            index_count: indices.len() as u32,
            bounds: data.bounds(),
        })
    }

    /// The vertex-buffer view to bind.
    #[must_use]
    pub fn vertex_view(&self) -> &D3D12_VERTEX_BUFFER_VIEW {
        &self.vertex_view
    }

    /// The index-buffer view to bind.
    #[must_use]
    pub fn index_view(&self) -> &D3D12_INDEX_BUFFER_VIEW {
        &self.index_view
    }

    /// Vertices in the buffer.
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    /// Indices in the buffer — the count to pass to a draw.
    #[must_use]
    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    /// Triangles, assuming a triangle list.
    #[must_use]
    pub fn triangle_count(&self) -> u32 {
        self.index_count / 3
    }

    /// The local-space bounding sphere, for culling.
    #[must_use]
    pub fn bounds(&self) -> Sphere {
        self.bounds
    }

    /// The vertex buffer, for diagnostics and for tests that read it back.
    #[must_use]
    pub fn vertex_buffer(&self) -> &GpuBuffer {
        &self.vertex_buffer
    }

    /// The index buffer, for diagnostics and for tests that read it back.
    #[must_use]
    pub fn index_buffer(&self) -> &GpuBuffer {
        &self.index_buffer
    }
}

/// A raw, non-indexed vertex buffer in device-local memory.
///
/// This is what the Phase 0 stress scene and the per-instance streams use: geometry that is
/// generated rather than authored, has no indices worth sharing, and whose element type is
/// not [`MeshVertex`]. Kept separate from [`GpuMesh`] because giving it bounds and an index
/// buffer it does not have would be fields that exist only to be ignored.
pub struct VertexStream {
    /// Kept alive because the view points into it.
    buffer: GpuBuffer,
    view: D3D12_VERTEX_BUFFER_VIEW,
    vertex_count: u32,
}

impl VertexStream {
    /// Uploads `vertices` to a default-heap buffer and waits for the copy to finish.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for empty input, or
    /// [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload(
        device: &RenderDevice,
        vertices: &[crate::pipeline::Vertex],
    ) -> Result<Self, RendererError> {
        Self::upload_raw(device, vertices)
    }

    /// Uploads any tightly packed `repr(C)` data as a vertex stream.
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

        // One stream, so one flush — but through the batching uploader, so a caller with
        // several streams can share a single synchronization instead of paying one per
        // stream. `upload_many` is that caller.
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
        let mut uploaded = Vec::with_capacity(streams.len());

        for stream in streams {
            let buffer = GpuBuffer::new_default(device, std::mem::size_of_val(*stream))?;
            uploader.queue_buffer(
                device,
                &buffer,
                stream,
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            )?;
            uploaded.push(Self::from_buffer(buffer, size_of::<T>(), stream.len()));
        }

        uploader.flush(device)?;
        Ok(uploaded)
    }

    /// Builds a stream around an already-filled buffer.
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
        self.buffer.resource()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    #[test]
    fn a_mesh_rejects_an_index_past_its_vertices() {
        // D3D12 does not bounds-check indices; an out-of-range one reads whatever follows
        // the buffer and usually draws a spike to the horizon.
        let vertices = vec![MeshVertex::default(); 3];
        assert!(MeshData::new(vertices, vec![0, 1, 3]).is_err());
    }

    #[test]
    fn a_mesh_rejects_an_incomplete_triangle() {
        let vertices = vec![MeshVertex::default(); 3];
        assert!(MeshData::new(vertices, vec![0, 1]).is_err());
    }

    #[test]
    fn a_mesh_rejects_emptiness() {
        assert!(MeshData::new(Vec::new(), vec![0]).is_err());
        assert!(MeshData::new(vec![MeshVertex::default()], Vec::new()).is_err());
    }

    #[test]
    fn a_cube_has_flat_faces_and_the_right_counts() {
        let cube = MeshData::cube(2.0);
        // Twenty-four vertices, not eight: shared corners would average three face normals
        // into one and shade a cube like a ball.
        assert_eq!(cube.vertices().len(), 24);
        assert_eq!(cube.triangle_count(), 12);

        for vertex in cube.vertices() {
            assert!((vertex.normal.length() - 1.0).abs() < EPSILON);
            // Axis-aligned: exactly one component is +-1 and the other two are zero.
            let normal = vertex.normal;
            let axes = [normal.x, normal.y, normal.z];
            assert_eq!(
                axes.iter()
                    .filter(|component| (component.abs() - 1.0).abs() < EPSILON)
                    .count(),
                1,
                "a cube face normal must lie on one axis, got {normal:?}"
            );
        }
    }

    #[test]
    fn a_cube_is_bounded_by_the_sphere_through_its_corners() {
        let cube = MeshData::cube(2.0);
        // Half-diagonal of a 2-unit cube is sqrt(3).
        assert!((cube.bounds().radius - 3.0_f32.sqrt()).abs() < EPSILON);
        assert!(
            cube.vertices()
                .iter()
                .all(|vertex| cube.bounds().contains(vertex.position))
        );
    }

    #[test]
    fn a_sphere_stays_on_its_radius() {
        let sphere = MeshData::uv_sphere(2.5, 16, 8);
        for vertex in sphere.vertices() {
            assert!((vertex.position.length() - 2.5).abs() < EPSILON);
            assert!((vertex.normal.length() - 1.0).abs() < EPSILON);
        }
        assert!((sphere.bounds().radius - 2.5).abs() < EPSILON);
    }

    #[test]
    fn a_sphere_indexes_every_quad_of_its_grid() {
        let (segments, rings) = (12, 6);
        let sphere = MeshData::uv_sphere(1.0, segments, rings);
        assert_eq!(sphere.triangle_count() as u32, segments * rings * 2);
        assert!(
            sphere
                .indices()
                .iter()
                .all(|index| (*index as usize) < sphere.vertices().len())
        );
    }

    #[test]
    fn a_degenerate_sphere_request_is_clamped_rather_than_rejected() {
        // This is a generator for test scenes, not an asset importer: a caller asking for
        // one segment gets the smallest sphere that exists, not an error to handle.
        let sphere = MeshData::uv_sphere(1.0, 0, 0);
        assert!(sphere.triangle_count() > 0);
    }

    #[test]
    fn a_plane_faces_up_and_is_flat() {
        let plane = MeshData::plane(4.0);
        assert_eq!(plane.triangle_count(), 2);
        assert!(
            plane
                .vertices()
                .iter()
                .all(|vertex| vertex.position.y == 0.0 && vertex.normal == Vec3::Y)
        );
    }

    #[test]
    fn a_vertex_is_the_size_the_input_layout_describes() {
        // The input layout hard-codes these offsets; a change here without a change there
        // reads the normal as part of the position.
        assert_eq!(size_of::<MeshVertex>(), 32);
        assert_eq!(std::mem::offset_of!(MeshVertex, position), 0);
        assert_eq!(std::mem::offset_of!(MeshVertex, normal), 12);
        assert_eq!(std::mem::offset_of!(MeshVertex, uv), 24);
    }
}
