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

use engine_core::math::{Sphere, Vec2, Vec3, Vec4};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_INDEX_BUFFER_VIEW, D3D12_RESOURCE_STATE_INDEX_BUFFER,
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER, D3D12_RESOURCE_STATES,
    D3D12_VERTEX_BUFFER_VIEW, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT;

use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::resource::{BufferSlice, BufferSource, GpuBuffer, TrackedResource};
use crate::upload::StagingUploader;

/// The state static geometry is left in after its upload.
///
/// Vertices, indices and material constants share the renderer's arena chunks, and D3D12
/// tracks a buffer's state whole — so the state has to cover every way a chunk is read. All
/// three are read-only states and D3D12 allows them to be combined.
const GEOMETRY_STATE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATES(
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER.0 | D3D12_RESOURCE_STATE_INDEX_BUFFER.0,
);

/// The alignment a suballocated geometry buffer starts on, in bytes.
///
/// Sixteen: enough for any vertex attribute this engine has, and a multiple of the index
/// element, so neither a vertex nor an index ever straddles the alignment the hardware wants.
const GEOMETRY_ALIGNMENT: usize = 16;

/// The vertex colour a mesh gets when nothing authored one: opaque white.
///
/// The renderer's copy of `engine_assets::DEFAULT_VERTEX_COLOR`, for the same reason the
/// vertex types are duplicated — this crate is the one the input layout describes, and it
/// must not depend on the asset crate to say what a default vertex is.
pub const DEFAULT_VERTEX_COLOR: [u8; 4] = [255, 255, 255, 255];

/// One vertex of a mesh: position, normal, texture coordinate, tangent and colour.
///
/// `repr(C)` is required — the input layout describes this exact memory layout to the GPU,
/// so Rust must not be free to reorder the fields.
///
/// **52 bytes**, and the field order is the offset table `mesh_vertex_elements` describes:
///
/// ```text
///  0  position  float3
/// 12  normal    float3
/// 24  uv        float2
/// 32  tangent   float4     xyz direction, w handedness
/// 48  color     unorm8x4
/// ```
///
/// Colour is four bytes rather than four floats deliberately: it is a 0..1 quantity the
/// hardware can unpack for free through `R8G8B8A8_UNORM`, and sixteen bytes for a tint would
/// have cost more than the tangent does. The tangent is *not* packed, because a normal map's
/// frame is what the packing error would show up in first, and nothing has measured that
/// this vertex is worth compressing yet.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    /// Position in the mesh's local space.
    pub position: Vec3,
    /// Unit normal in the mesh's local space.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
    /// Tangent direction in XYZ, handedness in W.
    ///
    /// Read by every mesh pixel shader: it is the frame a tangent-space normal map is applied
    /// in, and W says which way the bitangent runs. See `shaders/pbr.hlsli`.
    pub tangent: Vec4,
    /// Per-vertex colour, RGBA, read by the GPU as `R8G8B8A8_UNORM`.
    ///
    /// Uploaded and described to the input assembler; not read by any shader yet. How
    /// authored vertex colour combines with a material is its own decision, and nothing in
    /// this engine authors it yet.
    pub color: [u8; 4],
}

impl Default for MeshVertex {
    /// Written out rather than derived: `[u8; 4]`'s derived default is transparent black,
    /// and a default vertex colour has to be the multiplicative identity.
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            normal: Vec3::ZERO,
            uv: Vec2::ZERO,
            tangent: engine_assets::tangent::fallback(Vec3::ZERO),
            color: DEFAULT_VERTEX_COLOR,
        }
    }
}

impl MeshVertex {
    /// Builds a vertex with a normal-derived tangent and opaque white colour.
    ///
    /// Kept at three arguments so the offscreen helpers, the stress scene and the showcase
    /// geometry that build meshes by hand read the same as they did. Geometry that comes
    /// from an asset goes through [`MeshVertex::with_tangent`] and
    /// [`MeshVertex::with_color`] instead, which is what `Renderer::create_model` uses: an
    /// imported mesh's tangent comes from its UVs, and this constructor has none to consult.
    ///
    /// The tangent is `engine_assets::tangent::fallback` — the same deterministic
    /// perpendicular frame the asset crate gives a vertex whose UVs imply no direction. Not
    /// the zero vector, deliberately: the vertex format is frozen at this milestone, and
    /// every vertex buffer the GPU sees should already satisfy "unit XYZ, handedness +-1"
    /// before anything samples a normal map. A hand-authored mesh full of zeros would be the
    /// one that renders black on the day something does.
    #[must_use]
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position,
            normal,
            uv,
            tangent: engine_assets::tangent::fallback(normal),
            color: DEFAULT_VERTEX_COLOR,
        }
    }

    /// The same vertex with an explicit tangent.
    #[must_use]
    pub const fn with_tangent(mut self, tangent: Vec4) -> Self {
        self.tangent = tangent;
        self
    }

    /// The same vertex with an explicit colour.
    #[must_use]
    pub const fn with_color(mut self, color: [u8; 4]) -> Self {
        self.color = color;
        self
    }
}

/// One vertex of a skinned mesh: a rigid vertex plus what moves it.
///
/// **76 bytes**, `repr(C)`, and the exact memory the skinned input layout describes. The
/// field order matches [`MeshVertex`] for its first five fields, so the two layouts share
/// their first five input elements:
///
/// ```text
///  0  position  float3
/// 12  normal    float3
/// 24  uv        float2
/// 32  tangent   float4
/// 48  color     unorm8x4
/// 52  joints    uint16x4     4-byte aligned, which D3D12 requires of an element offset
/// 60  weights   float4
/// ```
///
/// Joints are `R16G16B16A16_UINT` and weights are `R32G32B32A32_FLOAT`. Packing them to
/// `R8G8B8A8_UINT` and `R8G8B8A8_UNORM` would make the vertex 60 bytes and is the obvious
/// later optimization; at one character of a few hundred vertices there is nothing yet to
/// measure, and `MAX_BONES` would have to come down to 256 first.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinnedVertex {
    /// Position in the mesh's bind pose, local space.
    pub position: Vec3,
    /// Unit normal in the bind pose.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
    /// Tangent direction in XYZ, handedness in W. See [`MeshVertex::tangent`].
    pub tangent: Vec4,
    /// Per-vertex colour. See [`MeshVertex::color`].
    pub color: [u8; 4],
    /// Indices into the bone palette.
    pub joints: [u16; 4],
    /// How much each joint contributes. Sums to one.
    pub weights: [f32; 4],
}

impl Default for SkinnedVertex {
    /// See [`MeshVertex::default`].
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            normal: Vec3::ZERO,
            uv: Vec2::ZERO,
            tangent: engine_assets::tangent::fallback(Vec3::ZERO),
            color: DEFAULT_VERTEX_COLOR,
            joints: [0; 4],
            weights: [0.0; 4],
        }
    }
}

impl SkinnedVertex {
    /// Builds a skinned vertex with a normal-derived tangent and opaque white colour.
    ///
    /// The tangent is the same fallback frame [`MeshVertex::new`] uses, for the same reason.
    #[must_use]
    pub fn new(
        position: Vec3,
        normal: Vec3,
        uv: Vec2,
        joints: [u16; 4],
        weights: [f32; 4],
    ) -> Self {
        Self {
            position,
            normal,
            uv,
            tangent: engine_assets::tangent::fallback(normal),
            color: DEFAULT_VERTEX_COLOR,
            joints,
            weights,
        }
    }

    /// The same vertex with an explicit tangent.
    #[must_use]
    pub const fn with_tangent(mut self, tangent: Vec4) -> Self {
        self.tangent = tangent;
        self
    }

    /// The same vertex with an explicit colour.
    #[must_use]
    pub const fn with_color(mut self, color: [u8; 4]) -> Self {
        self.color = color;
        self
    }
}

/// A skinned mesh on the CPU.
///
/// Separate from [`MeshData`] rather than a variant of it, because the two feed different
/// pipeline states with different input layouts, and a single type would push the branch
/// into every consumer.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinnedMeshData {
    vertices: Vec<SkinnedVertex>,
    indices: Vec<Index>,
    bounds: Sphere,
}

impl SkinnedMeshData {
    /// Builds skinned mesh data, computing its bind-pose bounding sphere.
    ///
    /// # Errors
    ///
    /// The same checks as [`MeshData::new`], for the same reason: D3D12 does not
    /// bounds-check an index buffer.
    pub fn new(vertices: Vec<SkinnedVertex>, indices: Vec<Index>) -> Result<Self, RendererError> {
        if vertices.is_empty() || indices.is_empty() {
            return Err(RendererError::InvalidConfig {
                reason: "a skinned mesh needs at least one vertex and one index".to_owned(),
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
    pub fn vertices(&self) -> &[SkinnedVertex] {
        &self.vertices
    }

    /// The indices.
    #[must_use]
    pub fn indices(&self) -> &[Index] {
        &self.indices
    }

    /// The bind-pose bounding sphere.
    ///
    /// **Bind pose, not animated pose.** An animated character reaches outside it — an arm
    /// raised overhead is half a metre past the bounds the T-pose implies — so a caller that
    /// culls with this must pad it. `engine-presentation` does, and says why.
    #[must_use]
    pub fn bounds(&self) -> Sphere {
        self.bounds
    }

    /// Triangles, assuming a triangle list.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
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

    /// A flat unit rectangle lying face up, one metre across and one metre long, running
    /// from the origin along -Z.
    ///
    /// The footprint of a *linear* zone - Firestorm's line of fire. Anchored at one end
    /// rather than centred, because a line ability starts at the caster and runs forwards,
    /// and yaw zero faces -Z everywhere else in the project. The transform scales it to the
    /// ability's own width and length, so the strip drawn is the strip that burns.
    #[must_use]
    pub fn strip() -> Self {
        let vertices = vec![
            MeshVertex::new(Vec3::new(-0.5, 0.0, 0.0), Vec3::Y, Vec2::new(0.0, 0.0)),
            MeshVertex::new(Vec3::new(0.5, 0.0, 0.0), Vec3::Y, Vec2::new(1.0, 0.0)),
            MeshVertex::new(Vec3::new(0.5, 0.0, -1.0), Vec3::Y, Vec2::new(1.0, 1.0)),
            MeshVertex::new(Vec3::new(-0.5, 0.0, -1.0), Vec3::Y, Vec2::new(0.0, 1.0)),
        ];
        // Wound to face up, like the disc, the ring and the sector: the first triangle's
        // cross product has to point along +Y or the world pass backface-culls the whole
        // strip and the indicator is simply not there.
        let indices = vec![0, 1, 2, 0, 2, 3];

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
    /// Kept alive because the view points into it. A range of the renderer's geometry arena
    /// when the mesh came from there, and a whole resource when it did not — see
    /// [`BufferSlice`].
    vertex_buffer: BufferSlice,
    /// Kept alive because the view points into it.
    index_buffer: BufferSlice,
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
        let mesh = Self::queue(device, &mut uploader, &mut BufferSource::Dedicated, data)?;
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
            uploaded.push(Self::queue(
                device,
                &mut uploader,
                &mut BufferSource::Dedicated,
                data,
            )?);
        }
        uploader.flush(device)?;
        Ok(uploaded)
    }

    /// Uploads a skinned mesh and waits for the copy to finish.
    ///
    /// The result is an ordinary [`GpuMesh`] with a 56-byte stride: a vertex buffer is a
    /// vertex buffer, and only the input layout the pipeline was created with decides how it
    /// is read.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any D3D12 call fails.
    pub fn upload_skinned(
        device: &RenderDevice,
        data: &SkinnedMeshData,
    ) -> Result<Self, RendererError> {
        let mut uploader = StagingUploader::new(device)?;
        let mesh = Self::queue_typed(
            device,
            &mut uploader,
            &mut BufferSource::Dedicated,
            data.vertices(),
            data.indices(),
            data.bounds(),
        )?;
        uploader.flush(device)?;
        Ok(mesh)
    }

    /// Creates both buffers and queues their copies into a caller's uploader.
    ///
    /// No flush and therefore no wait: the caller decides when the copies are submitted, so
    /// a hundred meshes can share one synchronization instead of paying one each. That is
    /// the whole of [`Renderer::begin_upload_batch`](crate::Renderer::begin_upload_batch).
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any D3D12 call fails.
    pub fn queue_into(
        device: &RenderDevice,
        uploader: &mut StagingUploader,
        buffers: &mut BufferSource<'_>,
        data: &MeshData,
    ) -> Result<Self, RendererError> {
        Self::queue(device, uploader, buffers, data)
    }

    /// [`GpuMesh::queue_into`] for a skinned mesh.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if any D3D12 call fails.
    pub fn queue_skinned_into(
        device: &RenderDevice,
        uploader: &mut StagingUploader,
        buffers: &mut BufferSource<'_>,
        data: &SkinnedMeshData,
    ) -> Result<Self, RendererError> {
        Self::queue_typed(
            device,
            uploader,
            buffers,
            data.vertices(),
            data.indices(),
            data.bounds(),
        )
    }

    /// Creates both buffers and queues their copies, without flushing.
    fn queue(
        device: &RenderDevice,
        uploader: &mut StagingUploader,
        buffers: &mut BufferSource<'_>,
        data: &MeshData,
    ) -> Result<Self, RendererError> {
        Self::queue_typed(
            device,
            uploader,
            buffers,
            data.vertices(),
            data.indices(),
            data.bounds(),
        )
    }

    /// The upload itself, over whichever vertex type the caller has.
    ///
    /// Generic rather than duplicated: the rigid and skinned paths differ in the stride
    /// recorded in the view and in nothing else, and two copies of a buffer upload would
    /// eventually differ in something that matters.
    fn queue_typed<V: Copy>(
        device: &RenderDevice,
        uploader: &mut StagingUploader,
        buffers: &mut BufferSource<'_>,
        vertices: &[V],
        indices: &[Index],
        bounds: Sphere,
    ) -> Result<Self, RendererError> {
        // Aligned to the vertex stride, so an address is a whole number of vertices into the
        // buffer whatever else shares the chunk. The index buffer is aligned to its own
        // element for the same reason.
        let vertex_buffer = buffers.allocate(
            device,
            std::mem::size_of_val(vertices),
            size_of::<V>().max(GEOMETRY_ALIGNMENT),
        )?;
        uploader.queue_slice(device, &vertex_buffer, vertices, GEOMETRY_STATE)?;

        let index_buffer = buffers.allocate(
            device,
            std::mem::size_of_val(indices),
            size_of::<Index>().max(GEOMETRY_ALIGNMENT),
        )?;
        uploader.queue_slice(device, &index_buffer, indices, GEOMETRY_STATE)?;

        let vertex_view = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: vertex_buffer.gpu_address(),
            SizeInBytes: std::mem::size_of_val(vertices) as u32,
            StrideInBytes: size_of::<V>() as u32,
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
            bounds,
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

    /// The resource the vertices live in, for diagnostics and for tests that read it back.
    ///
    /// The whole resource, which is the mesh's own only when it was uploaded on its own:
    /// a mesh out of the renderer's arena shares its chunk, and
    /// [`GpuMesh::vertex_slice`] is the range that belongs to this mesh.
    #[must_use]
    pub fn vertex_buffer(&self) -> &GpuBuffer {
        self.vertex_buffer.buffer()
    }

    /// The range of [`GpuMesh::vertex_buffer`] this mesh's vertices occupy.
    #[must_use]
    pub fn vertex_slice(&self) -> &BufferSlice {
        &self.vertex_buffer
    }

    /// The resource the indices live in. See [`GpuMesh::vertex_buffer`].
    #[must_use]
    pub fn index_buffer(&self) -> &GpuBuffer {
        self.index_buffer.buffer()
    }

    /// The range of [`GpuMesh::index_buffer`] this mesh's indices occupy.
    #[must_use]
    pub fn index_slice(&self) -> &BufferSlice {
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
    fn every_ground_shape_is_wound_the_same_way_up() {
        // The bug this test exists for: a strip wound the other way is silently
        // backface-culled, and a targeting indicator that is simply not drawn looks like a
        // targeting system that never opened.
        fn faces_up(mesh: &MeshData) -> bool {
            mesh.indices().chunks_exact(3).all(|triangle| {
                let a = mesh.vertices()[triangle[0] as usize].position;
                let b = mesh.vertices()[triangle[1] as usize].position;
                let c = mesh.vertices()[triangle[2] as usize].position;
                let (u, v) = (b - a, c - a);
                // The +Y component of u x v. Positive means the triangle faces the sky.
                u.z * v.x - u.x * v.z > 0.0
            })
        }

        assert!(faces_up(&MeshData::disc(0.5, 16)), "disc");
        assert!(faces_up(&MeshData::ring(0.5, 0.05, 16)), "ring");
        assert!(faces_up(&MeshData::sector(1.0, 0.5, 8)), "sector");
        assert!(faces_up(&MeshData::strip()), "strip");
    }

    #[test]
    fn a_strip_is_a_unit_rectangle_anchored_at_one_end() {
        // Anchored rather than centred: a linear zone starts at the caster and runs
        // forwards, and yaw zero faces -Z everywhere else in the project.
        let strip = MeshData::strip();
        assert_eq!(strip.triangle_count(), 2);
        assert!(
            strip
                .vertices()
                .iter()
                .all(|vertex| vertex.position.x.abs() <= 0.5 + EPSILON
                    && (-1.0 - EPSILON..=EPSILON).contains(&vertex.position.z))
        );
        assert!(
            strip
                .vertices()
                .iter()
                .any(|vertex| (vertex.position.z + 1.0).abs() < EPSILON),
            "the strip does not reach its far end"
        );
    }

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
        assert_eq!(size_of::<MeshVertex>(), 52);
        assert_eq!(std::mem::offset_of!(MeshVertex, position), 0);
        assert_eq!(std::mem::offset_of!(MeshVertex, normal), 12);
        assert_eq!(std::mem::offset_of!(MeshVertex, uv), 24);
        assert_eq!(std::mem::offset_of!(MeshVertex, tangent), 32);
        assert_eq!(std::mem::offset_of!(MeshVertex, color), 48);
    }

    #[test]
    fn a_vertex_built_without_one_is_opaque_white() {
        // Vertex colour multiplies, so the default has to be the identity. A derived
        // `Default` would give transparent black and black out every mesh built by hand.
        assert_eq!(MeshVertex::default().color, DEFAULT_VERTEX_COLOR);
        assert_eq!(
            MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO).color,
            DEFAULT_VERTEX_COLOR
        );
        assert_eq!(SkinnedVertex::default().color, DEFAULT_VERTEX_COLOR);
        assert_eq!(
            SkinnedVertex::new(
                Vec3::ZERO,
                Vec3::Y,
                Vec2::ZERO,
                [0; 4],
                [1.0, 0.0, 0.0, 0.0]
            )
            .color,
            DEFAULT_VERTEX_COLOR
        );
    }
}
