//! Geometry: the two vertex layouts, and the submesh that carries one of them.
//!
//! A model is a list of submeshes rather than one mesh, because one draw can bind one
//! material. A mage whose robe and skin shade differently is two submeshes, and that is the
//! *only* reason to split one — a submesh boundary is a material boundary, never an artistic
//! one.

use engine_core::math::{Sphere, Vec2, Vec3, Vec4};

/// The vertex colour a mesh gets when its source has none: opaque white.
///
/// White rather than black because vertex colour multiplies: white is the identity, so a
/// mesh with no authored colours shades exactly as it did before the attribute existed.
pub const DEFAULT_VERTEX_COLOR: [u8; 4] = [255, 255, 255, 255];

/// A tangent whose XYZ is zero, meaning "not set".
///
/// The sentinel [`SubMesh::new`] looks for when it decides whether to generate tangents. A
/// zero vector cannot be a real tangent — it has no direction — so it is unambiguous, and it
/// is what `Default` produces, which is what makes every existing construction site keep
/// compiling and still get correct tangents.
pub const UNSET_TANGENT: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.0);

/// A vertex of a rigid mesh: position, normal, texture coordinate, tangent, colour.
///
/// 52 bytes, deliberately identical in layout to `engine_renderer::MeshVertex`. The two are
/// separate types because this crate must not depend on a renderer and the renderer's copy
/// is `repr(C)` against an input-layout description; keeping the fields the same makes the
/// conversion at upload a straight field copy rather than a reinterpretation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    /// Position in the model's local space, metres.
    pub position: Vec3,
    /// Unit normal in the model's local space.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
    /// Tangent direction in XYZ, **handedness** in W.
    ///
    /// W is +1 or -1 and says which way the bitangent runs:
    /// `bitangent = cross(normal, tangent.xyz) * tangent.w`. Storing the sign rather than
    /// the bitangent itself is four bytes against twelve, and it is the convention glTF
    /// already uses, so an imported tangent needs no conversion.
    ///
    /// [`UNSET_TANGENT`] until [`SubMesh::new`] fills it in.
    pub tangent: Vec4,
    /// Per-vertex colour, RGBA, straight 8-bit.
    ///
    /// [`DEFAULT_VERTEX_COLOR`] when the source has none. Nothing consumes it yet — this
    /// milestone establishes the data path and deliberately does not change shading.
    pub color: [u8; 4],
}

impl Default for MeshVertex {
    /// A degenerate vertex with an unset tangent and opaque white colour.
    ///
    /// Written out rather than derived because `[u8; 4]`'s derived default is transparent
    /// black, and a vertex colour that multiplies to zero would black out any mesh built
    /// through `Default`.
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            normal: Vec3::ZERO,
            uv: Vec2::ZERO,
            tangent: UNSET_TANGENT,
            color: DEFAULT_VERTEX_COLOR,
        }
    }
}

impl MeshVertex {
    /// Builds a rigid vertex with an unset tangent and opaque white colour.
    ///
    /// The tangent is filled in by [`SubMesh::new`], which is where every producer of
    /// geometry in this crate — the glTF importer and every procedural author — ends up. A
    /// three-argument constructor is kept deliberately: an author placing a box has nothing
    /// useful to say about tangents, and making it say something would be noise at three
    /// hundred call sites.
    #[must_use]
    pub const fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position,
            normal,
            uv,
            tangent: UNSET_TANGENT,
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

/// How many bones may influence one vertex.
///
/// Four is the number every real-time skinning pipeline settled on, and the reason is the
/// vertex shader rather than the art: four influences fit two vector registers and unroll
/// into four multiply-adds with no loop. A fifth influence on a stylized character is
/// almost always a weight below 0.02 that the normalization in [`SkinVertex::normalized`]
/// would have redistributed anyway.
pub const MAX_INFLUENCES: usize = 4;

/// A vertex of a skinned mesh: a rigid vertex plus what moves it.
///
/// 76 bytes, and deliberately identical in layout to `engine_renderer::SkinnedVertex`: the
/// rigid five fields first, then what moves the vertex. Joints are `u16` and weights are
/// `f32`, which is the uncompressed form. Packing joints to `u8` and weights to `unorm8`
/// would make it 60 and is the obvious later optimization; it is not done now because a
/// 76-byte vertex on one character at a few thousand vertices is under 300 KB of bandwidth
/// per frame, and an unmeasured packing buys a decode in every vertex shader for a saving
/// nobody has seen. `MAX_BONES` is the thing to check before choosing `u8`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinVertex {
    /// Position in the model's **bind pose**, local space, metres.
    pub position: Vec3,
    /// Unit normal in the bind pose.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
    /// Tangent direction in XYZ, handedness in W. See [`MeshVertex::tangent`].
    pub tangent: Vec4,
    /// Per-vertex colour, RGBA. See [`MeshVertex::color`].
    pub color: [u8; 4],
    /// Which bones move this vertex, as indices into [`crate::Skeleton::bones`].
    ///
    /// A zero weight's joint index is not read by the shader, but it still has to be in
    /// range: an out-of-range index reads past the palette even when its weight is zero,
    /// because the shader multiplies before it adds.
    pub joints: [u16; MAX_INFLUENCES],
    /// How much each of [`SkinVertex::joints`] contributes. Must sum to 1.
    pub weights: [f32; MAX_INFLUENCES],
}

impl Default for SkinVertex {
    /// See [`MeshVertex::default`] — the colour must be white, not transparent black.
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            normal: Vec3::ZERO,
            uv: Vec2::ZERO,
            tangent: UNSET_TANGENT,
            color: DEFAULT_VERTEX_COLOR,
            joints: [0; MAX_INFLUENCES],
            weights: [0.0; MAX_INFLUENCES],
        }
    }
}

impl SkinVertex {
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

    /// A vertex bound rigidly to one bone.
    ///
    /// Most of a stylized character is this: a forearm belongs to the forearm. Blending only
    /// matters across a joint, which is a small fraction of the vertices and the only place
    /// worth spending weights on.
    #[must_use]
    pub fn rigid(position: Vec3, normal: Vec3, uv: Vec2, joint: u16) -> Self {
        Self {
            position,
            normal,
            uv,
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
            ..Self::default()
        }
    }

    /// A vertex blended between two bones, `blend` of the way from `first` to `second`.
    #[must_use]
    pub fn blended(
        position: Vec3,
        normal: Vec3,
        uv: Vec2,
        first: u16,
        second: u16,
        blend: f32,
    ) -> Self {
        let blend = blend.clamp(0.0, 1.0);
        Self {
            position,
            normal,
            uv,
            joints: [first, second, 0, 0],
            weights: [1.0 - blend, blend, 0.0, 0.0],
            ..Self::default()
        }
    }

    /// The sum of the weights.
    #[must_use]
    pub fn weight_sum(&self) -> f32 {
        self.weights.iter().sum()
    }

    /// The same vertex with its weights scaled to sum to one.
    ///
    /// Weights that sum to 0.98 shrink the character by two percent wherever they apply —
    /// a skinning matrix blend is an affine combination, and an affine combination whose
    /// coefficients do not sum to one drags the vertex towards the origin. It is a subtle,
    /// asymmetric deflation that reads as "the model is slightly wrong" rather than as a bug,
    /// so every producer in this crate normalizes rather than hoping.
    ///
    /// A vertex with no weight at all is bound rigidly to bone 0, which is the root: it ends
    /// up somewhere deliberate instead of collapsing to the origin.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        let sum = self.weight_sum();
        if sum > 1e-6 {
            for weight in &mut self.weights {
                *weight /= sum;
            }
        } else {
            self.joints = [0; MAX_INFLUENCES];
            self.weights = [1.0, 0.0, 0.0, 0.0];
        }
        self
    }
}

/// A submesh's vertices, in whichever layout it uses.
///
/// An enum rather than "every vertex is skinned with one bone": a training dummy has 100
/// instances on screen and no skeleton, and giving it 24 bytes of joint data per vertex
/// plus a skinning shader would be paying a character's cost for a post.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// Rigid geometry, transformed by the entity's world matrix alone.
    Static(Vec<MeshVertex>),
    /// Skinned geometry, transformed by a bone palette.
    Skinned(Vec<SkinVertex>),
}

impl Geometry {
    /// How many vertices there are.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Static(vertices) => vertices.len(),
            Self::Skinned(vertices) => vertices.len(),
        }
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether this geometry needs a skeleton to be drawn.
    #[must_use]
    pub fn is_skinned(&self) -> bool {
        matches!(self, Self::Skinned(_))
    }

    /// The normals, whichever layout this is.
    pub fn normals(&self) -> impl Iterator<Item = Vec3> + '_ {
        (0..self.len()).map(move |index| match self {
            Self::Static(vertices) => vertices[index].normal,
            Self::Skinned(vertices) => vertices[index].normal,
        })
    }

    /// The texture coordinates, whichever layout this is.
    pub fn uvs(&self) -> impl Iterator<Item = Vec2> + '_ {
        (0..self.len()).map(move |index| match self {
            Self::Static(vertices) => vertices[index].uv,
            Self::Skinned(vertices) => vertices[index].uv,
        })
    }

    /// The tangents, whichever layout this is.
    pub fn tangents(&self) -> impl Iterator<Item = Vec4> + '_ {
        (0..self.len()).map(move |index| match self {
            Self::Static(vertices) => vertices[index].tangent,
            Self::Skinned(vertices) => vertices[index].tangent,
        })
    }

    /// The colours, whichever layout this is.
    pub fn colors(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        (0..self.len()).map(move |index| match self {
            Self::Static(vertices) => vertices[index].color,
            Self::Skinned(vertices) => vertices[index].color,
        })
    }

    /// Replaces the tangent of every vertex, in order.
    ///
    /// The write half of [`Geometry::tangents`], used by [`crate::tangent::generate`]. A
    /// slice shorter than the geometry leaves the rest alone rather than panicking: the
    /// generator always produces exactly one tangent per vertex, and a length mismatch would
    /// be a bug in this crate rather than in an asset.
    pub fn set_tangents(&mut self, tangents: &[Vec4]) {
        match self {
            Self::Static(vertices) => {
                for (vertex, tangent) in vertices.iter_mut().zip(tangents) {
                    vertex.tangent = *tangent;
                }
            }
            Self::Skinned(vertices) => {
                for (vertex, tangent) in vertices.iter_mut().zip(tangents) {
                    vertex.tangent = *tangent;
                }
            }
        }
    }

    /// Whether every vertex already carries a usable tangent.
    ///
    /// "Usable" is deliberately strict: finite, non-degenerate in XYZ, and a handedness of
    /// ±1. An exporter that writes a zero tangent for a vertex it could not solve, or a NaN
    /// for a degenerate UV triangle, is common enough that trusting the count alone would
    /// import poison. See `crate::tangent` for what happens when this is false.
    #[must_use]
    pub fn has_tangents(&self) -> bool {
        self.tangents().all(|tangent| {
            let finite = tangent.x.is_finite()
                && tangent.y.is_finite()
                && tangent.z.is_finite()
                && tangent.w.is_finite();
            let directed =
                tangent.x * tangent.x + tangent.y * tangent.y + tangent.z * tangent.z > 1e-12;
            finite && directed && (tangent.w.abs() - 1.0).abs() < 1e-3
        })
    }

    /// The positions, whichever layout this is.
    pub fn positions(&self) -> impl Iterator<Item = Vec3> + '_ {
        // Two boxed-free branches joined by an enum of iterators would need a named type;
        // the allocation-free way to say this without one is to map over indices.
        (0..self.len()).map(move |index| match self {
            Self::Static(vertices) => vertices[index].position,
            Self::Skinned(vertices) => vertices[index].position,
        })
    }
}

/// One material's worth of geometry inside a model.
#[derive(Debug, Clone, PartialEq)]
pub struct SubMesh {
    /// What it is, for diagnostics: `"robe"`, `"staff"`, `"post"`.
    pub name: String,
    /// The vertices.
    pub geometry: Geometry,
    /// Triangle list indices into [`SubMesh::geometry`].
    pub indices: Vec<u32>,
    /// Index into [`crate::ModelAsset::materials`].
    pub material: usize,
    /// Local-space bounds, in the bind pose for skinned geometry.
    pub bounds: Sphere,
}

impl SubMesh {
    /// Builds a submesh and computes its bounds.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        geometry: Geometry,
        indices: Vec<u32>,
        material: usize,
    ) -> Self {
        let mut geometry = geometry;
        // The one place tangents are generated, because it is the one place every producer
        // of geometry in this crate passes through: the glTF importer and all six procedural
        // authors call this constructor. Doing it here rather than in each of them is what
        // makes "a submesh always has usable tangents" an invariant of the type instead of a
        // rule somebody has to remember.
        //
        // Import-time, never frame-time, and never in the renderer — a tangent is a property
        // of the mesh, not of how it is drawn.
        if !geometry.has_tangents() {
            crate::tangent::generate(&mut geometry, &indices);
        }
        let bounds = Sphere::around(geometry.positions().collect::<Vec<_>>());
        Self {
            name: name.into(),
            geometry,
            indices,
            material,
            bounds,
        }
    }

    /// Triangles, assuming a triangle list.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_that_do_not_sum_to_one_are_scaled_rather_than_accepted() {
        // Weights summing to 0.5 would place the vertex halfway to the origin, which reads
        // as a deflated model rather than as a bug.
        let vertex = SkinVertex {
            weights: [0.25, 0.25, 0.0, 0.0],
            ..SkinVertex::rigid(Vec3::X, Vec3::Y, Vec2::ZERO, 3)
        }
        .normalized();
        assert!((vertex.weight_sum() - 1.0).abs() < 1e-6);
        assert_eq!(vertex.weights[0], 0.5);
    }

    #[test]
    fn a_vertex_with_no_weight_falls_back_to_the_root_rather_than_the_origin() {
        let vertex = SkinVertex {
            weights: [0.0; MAX_INFLUENCES],
            joints: [7, 8, 9, 10],
            ..SkinVertex::rigid(Vec3::X, Vec3::Y, Vec2::ZERO, 7)
        }
        .normalized();
        assert_eq!(vertex.joints, [0; MAX_INFLUENCES]);
        assert_eq!(vertex.weights, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_rigid_vertex_is_already_normalized() {
        let vertex = SkinVertex::rigid(Vec3::ZERO, Vec3::Y, Vec2::ZERO, 2);
        assert_eq!(vertex.weight_sum(), 1.0);
        assert_eq!(vertex.normalized(), vertex);
    }

    #[test]
    fn a_blend_is_clamped_to_the_two_bones_it_names() {
        let vertex = SkinVertex::blended(Vec3::ZERO, Vec3::Y, Vec2::ZERO, 1, 2, 1.5);
        assert_eq!(vertex.weights, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(vertex.joints[..2], [1, 2]);
    }

    #[test]
    fn a_submesh_measures_its_own_bounds() {
        let vertices = vec![
            MeshVertex::new(Vec3::new(-1.0, 0.0, 0.0), Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::new(1.0, 0.0, 0.0), Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::new(0.0, 1.0, 0.0), Vec3::Y, Vec2::ZERO),
        ];
        let submesh = SubMesh::new("test", Geometry::Static(vertices), vec![0, 1, 2], 0);
        assert_eq!(submesh.triangle_count(), 1);
        assert!(submesh.bounds.radius >= 1.0);
        assert!(!submesh.geometry.is_skinned());
    }
}
