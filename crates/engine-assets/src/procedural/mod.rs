//! Models authored in code: a technical baseline, not the final art direction.
//!
//! # Why a character is built from tapered prisms
//!
//! This project has an engine and no art pipeline yet. Something has to stand in the arena
//! so that skinning, animation, materials, LOD and the whole visual controller can be built,
//! measured and tested against a real character rather than against a cube — and it has to
//! be original work with no licence attached to it.
//!
//! A tapered prism is the smallest primitive that gives a limb a direction, a taper and a
//! flat readable silhouette. Fourteen of them make a humanoid at 300-odd triangles, which is
//! *cheaper* than most of the geometry already in the arena. The result reads as a hooded
//! figure with a staff; it is not sculpted, and it is not meant to be.
//!
//! # What this exists to prove
//!
//! Every model here goes through [`crate::validate_model`] and comes out as a
//! [`crate::ModelAsset`] — the same type [`crate::gltf_import`] produces. When an artist
//! delivers `fire_mage.glb`, the change is which function fills the library slot. Nothing
//! downstream — not the renderer, not the animation controller, not gameplay — can tell the
//! difference, and that is the property this module exists to demonstrate.

pub mod druid;
pub mod dummy;
pub mod environment;
pub mod firemage;
pub mod humanoid;
pub mod starter;
pub mod warrior;

use engine_core::math::{Vec2, Vec3};

use crate::mesh::{Geometry, MeshVertex, SubMesh};

pub use druid::{druid, druid_character};
pub use dummy::{DummyVariant, training_dummy};
pub use environment::{environment_library, environment_model};
pub use firemage::{fire_mage, fire_mage_character};
pub use starter::{STARTER_LIBRARY, STARTER_MATERIALS, StarterAsset, StarterMaterial, starter_model};
pub use warrior::{warrior, warrior_character};

/// One corner of a generated face.
pub type Corner = (Vec3, Vec3, Vec2);

/// The four corners of a prism's cross-section, in the order that makes every face wind
/// counter-clockwise seen from outside.
///
/// The order is load-bearing and is not obvious. With a right-handed basis `(x, y, z)` where
/// `z = x × y` and `y` runs along the prism, traversing `(-1,+1) → (+1,+1) → (+1,-1) → (-1,-1)`
/// is counter-clockwise seen from `+y`. Traversing it the other way produces a prism whose
/// outward faces are culled and whose *inward* faces are drawn — which looks almost right
/// until something reads the normal, and then every surface is lit from the wrong side.
const CROSS_SECTION: [(f32, f32); 4] = [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];

/// A four-sided prism between two points, with an independent cross-section at each end.
///
/// The one primitive every model in this module is built from. A limb is a prism, a torso is
/// a prism, a hood is a prism that tapers to almost nothing, a training dummy's post is a
/// prism. Six faces, 24 vertices, 12 triangles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prism {
    /// Centre of the lower cap.
    pub lower: Vec3,
    /// Centre of the upper cap.
    pub upper: Vec3,
    /// Half-width and half-depth at the lower cap.
    pub lower_half: Vec2,
    /// Half-width and half-depth at the upper cap.
    pub upper_half: Vec2,
    /// Which way "width" points. Normalized and re-orthogonalized against the axis.
    pub width_axis: Vec3,
    /// How far along `v` this prism's texture starts, and how much of it it uses.
    pub uv_span: (f32, f32),
}

impl Prism {
    /// A prism from `lower` to `upper`, square in cross-section at each end.
    #[must_use]
    pub fn new(lower: Vec3, upper: Vec3, lower_half: f32, upper_half: f32) -> Self {
        Self {
            lower,
            upper,
            lower_half: Vec2::new(lower_half, lower_half),
            upper_half: Vec2::new(upper_half, upper_half),
            width_axis: Vec3::X,
            uv_span: (0.0, 1.0),
        }
    }

    /// The same prism with rectangular cross-sections.
    #[must_use]
    pub fn with_halves(mut self, lower: Vec2, upper: Vec2) -> Self {
        self.lower_half = lower;
        self.upper_half = upper;
        self
    }

    /// The same prism with its width running along `axis`.
    #[must_use]
    pub fn with_width_axis(mut self, axis: Vec3) -> Self {
        self.width_axis = axis;
        self
    }

    /// The same prism mapped to `start..start + length` of its texture's V axis.
    ///
    /// What puts a robe's hem at the bottom of the robe texture and its collar at the top,
    /// across three separate prisms, without a UV unwrap.
    #[must_use]
    pub fn with_uv_span(mut self, start: f32, length: f32) -> Self {
        self.uv_span = (start, length);
        self
    }

    /// The axis from the lower cap to the upper one, and its length.
    fn axis(&self) -> (Vec3, f32) {
        let delta = self.upper - self.lower;
        let length = delta.length();
        if length > 1e-6 {
            (delta * (1.0 / length), length)
        } else {
            (Vec3::Y, 0.0)
        }
    }

    /// Generates the prism's 24 corners and 36 indices, based at index `base`.
    ///
    /// Twenty-four rather than eight, for the reason `MeshData::cube` gives: a corner shared
    /// between three faces has three different normals, and averaging them turns a hard
    /// stylized edge into a soft ball.
    #[must_use]
    pub fn build(&self, base: u32) -> (Vec<Corner>, Vec<u32>) {
        let (up, _length) = self.axis();
        // Re-orthogonalize: a caller passing a width axis that is not perpendicular to the
        // limb — which is most of them, since limbs point in arbitrary directions — would
        // otherwise get a sheared cross-section.
        let width = {
            let projected = self.width_axis - up * self.width_axis.dot(up);
            if projected.length_squared() > 1e-6 {
                projected.normalize()
            } else {
                // The width axis was parallel to the limb. Any perpendicular will do.
                let fallback = if up.dot(Vec3::X).abs() < 0.9 {
                    Vec3::X
                } else {
                    Vec3::Z
                };
                (fallback - up * fallback.dot(up)).normalize()
            }
        };
        let depth = width.cross(up);

        let corner = |centre: Vec3, half: Vec2, (u, v): (f32, f32)| {
            centre + width * (u * half.x) + depth * (v * half.y)
        };
        let lower: Vec<Vec3> = CROSS_SECTION
            .iter()
            .map(|offset| corner(self.lower, self.lower_half, *offset))
            .collect();
        let upper: Vec<Vec3> = CROSS_SECTION
            .iter()
            .map(|offset| corner(self.upper, self.upper_half, *offset))
            .collect();

        let (v_start, v_length) = self.uv_span;
        let mut corners: Vec<Corner> = Vec::with_capacity(24);
        let mut indices: Vec<u32> = Vec::with_capacity(36);

        let mut quad = |points: [Vec3; 4], uvs: [Vec2; 4], corners: &mut Vec<Corner>| {
            // The face's own normal, from the quad it actually is rather than from the axis
            // it was meant to follow — which is what keeps a tapered side lit correctly.
            let normal = (points[1] - points[0])
                .cross(points[2] - points[1])
                .normalize();
            let first = base + corners.len() as u32;
            for (point, uv) in points.into_iter().zip(uvs) {
                corners.push((point, normal, uv));
            }
            indices.extend_from_slice(&[first, first + 1, first + 2, first, first + 2, first + 3]);
        };

        for side in 0..4 {
            let next = (side + 1) % 4;
            let u0 = side as f32 * 0.25;
            let u1 = u0 + 0.25;
            let v0 = v_start;
            let v1 = v_start + v_length;
            quad(
                [lower[side], lower[next], upper[next], upper[side]],
                [
                    Vec2::new(u0, v0),
                    Vec2::new(u1, v0),
                    Vec2::new(u1, v1),
                    Vec2::new(u0, v1),
                ],
                &mut corners,
            );
        }

        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        quad(
            [upper[0], upper[1], upper[2], upper[3]],
            square,
            &mut corners,
        );
        quad(
            [lower[3], lower[2], lower[1], lower[0]],
            square,
            &mut corners,
        );

        (corners, indices)
    }
}

/// Accumulates primitives into the vertices and indices of one submesh.
///
/// Every procedural model in this crate builds its geometry the same way: one builder per
/// *material*, primitives pushed into whichever builder that surface belongs to, and each
/// builder turned into a [`SubMesh`] at the end. A submesh boundary is a material boundary
/// and never an artistic one, so the builder is the thing that enforces the mapping.
///
/// Positions are absolute in the model's local space — there is no transform stack. A model
/// whose parts needed to be placed relative to one another would want one; none does, and a
/// stack that is always empty is a stack that is wrong the first time it is not.
#[derive(Debug, Default)]
pub struct MeshBuilder {
    vertices: Vec<MeshVertex>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    /// An empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a prism's six faces.
    pub fn prism(&mut self, prism: &Prism) {
        let base = self.vertices.len() as u32;
        let (corners, indices) = prism.build(base);
        for (position, normal, uv) in corners {
            self.vertices.push(MeshVertex::new(position, normal, uv));
        }
        self.indices.extend(indices);
    }

    /// Appends one quad, wound so its normal points where `normal` says.
    ///
    /// The caller supplies the normal rather than it being derived, because a quad is also
    /// how a flat ground plane and a hanging cloth are built, and those want a normal that
    /// is *not* the one their (possibly degenerate, possibly non-planar) corners imply.
    pub fn quad(&mut self, corners: [Vec3; 4], normal: Vec3, uvs: [Vec2; 4]) {
        let first = self.vertices.len() as u32;
        for (position, uv) in corners.into_iter().zip(uvs) {
            self.vertices.push(MeshVertex::new(position, normal, uv));
        }
        self.indices
            .extend_from_slice(&[first, first + 1, first + 2, first, first + 2, first + 3]);
    }

    /// Appends a flat horizontal N-gon centred on `(0, y, 0)`, facing up.
    ///
    /// A triangle fan from the centre: `segments` triangles, `segments + 1` vertices. UVs are
    /// planar, scaled so the texture tiles `uv_scale` times across the diameter.
    pub fn disc(&mut self, radius: f32, segments: u32, y: f32, uv_scale: f32) {
        let segments = segments.max(3);
        let center = self.vertices.len() as u32;
        self.vertices.push(MeshVertex::new(
            Vec3::new(0.0, y, 0.0),
            Vec3::Y,
            Vec2::new(uv_scale * 0.5, uv_scale * 0.5),
        ));
        for step in 0..segments {
            let angle = step as f32 / segments as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            self.vertices.push(MeshVertex::new(
                Vec3::new(cos * radius, y, sin * radius),
                Vec3::Y,
                Vec2::new(uv_scale * (cos * 0.5 + 0.5), uv_scale * (sin * 0.5 + 0.5)),
            ));
        }
        for step in 0..segments {
            let current = center + 1 + step;
            let next = center + 1 + (step + 1) % segments;
            // Counter-clockwise seen from +Y, which is what makes the up-facing side the
            // front face under the renderer's back-face culling.
            self.indices.extend_from_slice(&[center, next, current]);
        }
    }

    /// How many triangles have been pushed.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Whether nothing has been pushed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Turns the accumulated geometry into a submesh drawn with material `material`.
    #[must_use]
    pub fn into_submesh(self, name: &str, material: usize) -> SubMesh {
        SubMesh::new(
            name,
            Geometry::Static(self.vertices),
            self.indices,
            material,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faces_point_outwards(prism: &Prism) {
        let (corners, indices) = prism.build(0);
        assert_eq!(corners.len(), 24);
        assert_eq!(indices.len(), 36);

        let centre = (prism.lower + prism.upper) * 0.5;
        for (position, normal, _) in &corners {
            let outwards = *position - centre;
            assert!(
                outwards.dot(*normal) > 0.0,
                "a face at {position:?} with normal {normal:?} points into the prism"
            );
        }
    }

    #[test]
    fn every_face_of_an_upright_prism_points_away_from_its_centre() {
        // Getting the winding backwards draws the inside of the character and lights every
        // surface from the wrong side, which is much harder to spot than a missing model.
        faces_point_outwards(&Prism::new(Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0), 0.3, 0.3));
    }

    #[test]
    fn a_tapered_prism_is_still_wound_outwards() {
        faces_point_outwards(&Prism::new(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), 0.4, 0.05));
    }

    #[test]
    fn a_prism_along_an_arbitrary_axis_is_still_wound_outwards() {
        // Limbs point wherever the skeleton puts them, so no axis is special.
        faces_point_outwards(&Prism::new(
            Vec3::new(-0.3, 1.2, 0.1),
            Vec3::new(0.6, 0.7, -0.4),
            0.08,
            0.05,
        ));
    }

    #[test]
    fn a_width_axis_parallel_to_the_limb_still_produces_a_prism() {
        // A caller that says "width runs along +Y" for a limb that also runs along +Y has
        // given a degenerate basis. Any perpendicular is a correct answer; a NaN is not.
        let prism =
            Prism::new(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), 0.2, 0.2).with_width_axis(Vec3::Y);
        let (corners, _) = prism.build(0);
        for (position, normal, _) in corners {
            assert!(position.length_squared().is_finite(), "{position:?}");
            assert!((normal.length() - 1.0).abs() < 1e-3, "{normal:?}");
        }
    }

    #[test]
    fn a_zero_length_prism_does_not_produce_nans() {
        let (corners, _) = Prism::new(Vec3::Y, Vec3::Y, 0.2, 0.2).build(0);
        for (position, normal, _) in corners {
            assert!(position.length_squared().is_finite());
            assert!(normal.length_squared().is_finite());
        }
    }

    #[test]
    fn indices_are_based_where_the_caller_says() {
        let (_, indices) = Prism::new(Vec3::ZERO, Vec3::Y, 0.1, 0.1).build(100);
        assert!(indices.iter().all(|index| (100..124).contains(index)));
    }

    #[test]
    fn a_builder_accumulates_primitives_into_one_submesh() {
        let mut builder = MeshBuilder::new();
        assert!(builder.is_empty());
        builder.prism(&Prism::new(Vec3::ZERO, Vec3::Y, 0.2, 0.2));
        builder.disc(1.0, 8, 0.0, 1.0);
        assert_eq!(builder.triangle_count(), 12 + 8);

        let submesh = builder.into_submesh("test", 3);
        assert_eq!(submesh.material, 3);
        assert_eq!(submesh.triangle_count(), 20);
    }

    #[test]
    fn a_disc_faces_upwards_and_every_index_names_a_vertex_it_has() {
        let mut builder = MeshBuilder::new();
        builder.disc(2.0, 16, 0.5, 4.0);
        let count = builder.vertices.len() as u32;
        assert_eq!(count, 17, "a centre and sixteen rim vertices");
        assert!(builder.indices.iter().all(|index| *index < count));
        for vertex in &builder.vertices {
            assert_eq!(vertex.normal, Vec3::Y);
            assert!((vertex.position.y - 0.5).abs() < 1e-6);
            assert!(vertex.position.x.hypot(vertex.position.z) <= 2.0 + 1e-4);
        }

        // Wound counter-clockwise seen from above: the cross product of two edges of the
        // first triangle has to point along +Y, or the floor is only visible from below.
        let triangle = [
            builder.vertices[builder.indices[0] as usize].position,
            builder.vertices[builder.indices[1] as usize].position,
            builder.vertices[builder.indices[2] as usize].position,
        ];
        let normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[1]);
        assert!(normal.y > 0.0, "the disc is wound face-down: {normal:?}");
    }

    #[test]
    fn a_quad_keeps_the_normal_it_was_given() {
        let mut builder = MeshBuilder::new();
        builder.quad(
            [
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, 1.0),
            ],
            Vec3::Y,
            [
                Vec2::ZERO,
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ],
        );
        assert_eq!(builder.triangle_count(), 2);
        assert!(
            builder
                .vertices
                .iter()
                .all(|vertex| vertex.normal == Vec3::Y)
        );
    }

    #[test]
    fn a_uv_span_places_the_prism_on_a_shared_texture() {
        let prism = Prism::new(Vec3::ZERO, Vec3::Y, 0.1, 0.1).with_uv_span(0.25, 0.5);
        let (corners, _) = prism.build(0);
        // The four side faces span v = 0.25..0.75; the caps are mapped square.
        let sides = &corners[..16];
        for (_, _, uv) in sides {
            assert!((0.25..=0.75).contains(&uv.y), "{uv:?}");
        }
    }
}
