//! A grid of heights as a collision surface: the ground the terrain describes.
//!
//! ```text
//!  capsule ──► the cells its footprint covers ──► two triangles each ──► deepest contact
//! ```
//!
//! # Why this is a shape rather than a thousand boxes
//!
//! A 129×129 terrain is 32 768 triangles. As boxes that is 32 768 static shapes in the world,
//! every one of them in the broad phase, and a hill made of boxes is a staircase a character
//! catches on. As one shape it is a single entry that answers a query by looking at the four
//! or nine cells under the capsule — the cost of a test depends on the capsule's size, not on
//! the terrain's — and the surface is the same triangles the mesh is drawn from, so what a
//! player sees and what they stand on cannot disagree.
//!
//! # What it deliberately cannot do
//!
//! One height per grid point. There is no way to express a cave mouth, an arch or a ledge
//! here, and no attempt is made: those are placed boxes standing *on* the field, and the
//! character meets them through the ordinary [`Shape::Aabb`](crate::shape::Shape) path. The
//! field is the broad shape of the ground and nothing else.
//!
//! # Cost model
//!
//! No allocation, no state. One test walks the cells the capsule's radius covers — four for a
//! character on a 4 m grid — and each cell is two triangle tests. Nothing here is per frame
//! except through the character controller, which already budgets for a handful of contacts.

use engine_core::math::{Aabb, Vec3};

use crate::contact::Contact;
use crate::shape::{Capsule, Segment};

/// A grid of heights, in world space.
///
/// The same numbers `engine_environment::Terrain` holds, copied into the collision world when
/// the level is built. Copied rather than borrowed because the collision world outlives any
/// one edit of the terrain and must not hold a reference into the scene it was built from.
#[derive(Debug, Clone, PartialEq)]
pub struct Heightfield {
    /// The corner of the grid: the position of height 0, with `y` the base height.
    pub origin: Vec3,
    /// Metres between grid points.
    pub cell_size: f32,
    /// Grid points a side.
    pub resolution: usize,
    /// Height above `origin.y`, row-major in `z`.
    pub heights: Vec<f32>,
}

impl Heightfield {
    /// A field over `heights`, or `None` if the numbers do not describe a surface.
    ///
    /// The one place the invariants are checked, so every method below can index without
    /// asking. A field with fewer than two points a side has no cell and no surface.
    #[must_use]
    pub fn new(origin: Vec3, cell_size: f32, resolution: usize, heights: Vec<f32>) -> Option<Self> {
        if resolution < 2 || cell_size <= 0.0 || !cell_size.is_finite() {
            return None;
        }
        if heights.len() != resolution * resolution {
            return None;
        }
        if !heights.iter().all(|height| height.is_finite()) {
            return None;
        }
        Some(Self {
            origin,
            cell_size,
            resolution,
            heights,
        })
    }

    /// How wide the field is, in metres.
    #[must_use]
    pub fn span(&self) -> f32 {
        (self.resolution - 1) as f32 * self.cell_size
    }

    /// The world position of grid point `(x, z)`.
    #[must_use]
    pub fn point(&self, x: usize, z: usize) -> Vec3 {
        let x = x.min(self.resolution - 1);
        let z = z.min(self.resolution - 1);
        Vec3::new(
            self.origin.x + x as f32 * self.cell_size,
            self.origin.y + self.heights[z * self.resolution + x],
            self.origin.z + z as f32 * self.cell_size,
        )
    }

    /// A box containing the whole surface.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let mut low = f32::MAX;
        let mut high = f32::MIN;
        for height in &self.heights {
            low = low.min(*height);
            high = high.max(*height);
        }
        let span = self.span();
        Aabb::new(
            Vec3::new(self.origin.x, self.origin.y + low, self.origin.z),
            Vec3::new(
                self.origin.x + span,
                self.origin.y + high,
                self.origin.z + span,
            ),
        )
    }

    /// The two triangles of cell `(x, z)`, wound so their normals point up.
    ///
    /// Split along the same diagonal the mesh uses. A field whose collision split ran the
    /// other way would put a ridge where the picture has a valley, half a cell wide, over the
    /// whole terrain — invisible, and impossible to explain from the screenshot.
    #[must_use]
    pub fn triangles(&self, x: usize, z: usize) -> [[Vec3; 3]; 2] {
        let a = self.point(x, z);
        let b = self.point(x + 1, z);
        let c = self.point(x, z + 1);
        let d = self.point(x + 1, z + 1);
        [[a, c, b], [b, c, d]]
    }
}

/// Tests a capsule against a height field.
///
/// Returns the deepest contact of the cells under it, which is the one a character resolving
/// a single move should answer: a capsule standing on a ridge touches two triangles, and
/// pushing out of the deeper one takes it out of both.
#[must_use]
pub fn capsule_vs_heightfield(capsule: &Capsule, field: &Heightfield) -> Option<Contact> {
    let low = capsule.segment.start.min(capsule.segment.end) - Vec3::splat(capsule.radius);
    let high = capsule.segment.start.max(capsule.segment.end) + Vec3::splat(capsule.radius);

    // Which cells the capsule's footprint covers. Clamped, so a capsule half over the edge
    // tests the cells it is actually over and nothing off the end of the array.
    let last_cell = field.resolution - 2;
    let to_cell = |value: f32, origin: f32| ((value - origin) / field.cell_size).floor();
    let x0 = to_cell(low.x, field.origin.x).max(0.0) as usize;
    let z0 = to_cell(low.z, field.origin.z).max(0.0) as usize;
    let x1 = (to_cell(high.x, field.origin.x).max(0.0) as usize).min(last_cell);
    let z1 = (to_cell(high.z, field.origin.z).max(0.0) as usize).min(last_cell);
    if x0 > last_cell || z0 > last_cell {
        return None;
    }

    let mut deepest: Option<Contact> = None;
    for z in z0..=z1 {
        for x in x0..=x1 {
            for triangle in field.triangles(x, z) {
                let Some(contact) = capsule_vs_triangle(capsule, &triangle) else {
                    continue;
                };
                if deepest
                    .as_ref()
                    .is_none_or(|best| contact.penetration > best.penetration)
                {
                    deepest = Some(contact);
                }
            }
        }
    }
    deepest
}

/// Tests a capsule against one triangle.
///
/// The same two steps every test in [`crate::narrow`] takes: find the closest points between
/// the capsule's spine and the shape, and if they are closer than the radius, that gap is the
/// penetration. The normal is the *triangle's* rather than the direction between the points,
/// so a character standing on a slope is pushed along the surface normal instead of towards
/// whichever vertex happened to be nearest — which is what stops a capsule from being nudged
/// sideways every time it crosses a triangle edge.
#[must_use]
pub fn capsule_vs_triangle(capsule: &Capsule, triangle: &[Vec3; 3]) -> Option<Contact> {
    let edge1 = triangle[1] - triangle[0];
    let edge2 = triangle[2] - triangle[0];
    let normal = edge1.cross(edge2);
    let length = normal.length();
    if length < 1e-9 {
        // A degenerate triangle: two points of the cell are the same, which happens only for
        // a malformed field. It has no surface to stand on.
        return None;
    }
    let normal = normal * (1.0 / length);

    let (on_segment, on_triangle) = closest_points_segment_triangle(&capsule.segment, triangle);
    let offset = on_segment - on_triangle;
    let distance = offset.length();
    if distance >= capsule.radius {
        return None;
    }

    // Which side of the triangle the capsule is on. A character under a terrain — which
    // happens when they are pushed through it, or spawn below it — is lifted out through the
    // face rather than pulled further down.
    let side = if offset.dot(normal) < 0.0 { -1.0 } else { 1.0 };
    Some(Contact::new(
        on_triangle,
        normal * side,
        capsule.radius - distance,
    ))
}

/// The closest pair of points between a segment and a triangle.
///
/// Returns `(point on the segment, point on the triangle)`. Alternating projection, the same
/// approach [`crate::narrow`] uses for a segment against an oriented box: project each point
/// onto the other shape until neither moves. It converges monotonically, and the loop stops
/// early when the answer stops changing.
#[must_use]
pub fn closest_points_segment_triangle(segment: &Segment, triangle: &[Vec3; 3]) -> (Vec3, Vec3) {
    let mut on_segment = segment.start;
    let mut on_triangle = closest_point_on_triangle(triangle, on_segment);
    for _ in 0..8 {
        let next_segment = closest_point_on_segment(segment, on_triangle);
        let next_triangle = closest_point_on_triangle(triangle, next_segment);
        if (next_segment - on_segment).length_squared() < 1e-12
            && (next_triangle - on_triangle).length_squared() < 1e-12
        {
            return (next_segment, next_triangle);
        }
        on_segment = next_segment;
        on_triangle = next_triangle;
    }
    (on_segment, on_triangle)
}

/// The point of a segment nearest `point`.
fn closest_point_on_segment(segment: &Segment, point: Vec3) -> Vec3 {
    let direction = segment.end - segment.start;
    let length_squared = direction.length_squared();
    if length_squared < 1e-12 {
        return segment.start;
    }
    let t = ((point - segment.start).dot(direction) / length_squared).clamp(0.0, 1.0);
    segment.start + direction * t
}

/// The point of a triangle nearest `point`, inside it or on an edge.
///
/// The barycentric solve from Ericson's *Real-Time Collision Detection*, §5.1.5: seven cases,
/// three vertices, three edges and the face, each decided by a pair of dot products. Written
/// out rather than reduced, because every branch is a different closest feature and a clever
/// version of this is where sign errors hide.
#[must_use]
pub fn closest_point_on_triangle(triangle: &[Vec3; 3], point: Vec3) -> Vec3 {
    let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
    let ab = b - a;
    let ac = c - a;
    let ap = point - a;

    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }

    let bp = point - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let denominator = d1 - d3;
        let v = if denominator.abs() < 1e-12 {
            0.0
        } else {
            d1 / denominator
        };
        return a + ab * v;
    }

    let cp = point - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let denominator = d2 - d6;
        let w = if denominator.abs() < 1e-12 {
            0.0
        } else {
            d2 / denominator
        };
        return a + ac * w;
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let denominator = (d4 - d3) + (d5 - d6);
        let w = if denominator.abs() < 1e-12 {
            0.0
        } else {
            (d4 - d3) / denominator
        };
        return b + (c - b) * w;
    }

    let denominator = va + vb + vc;
    if denominator.abs() < 1e-12 {
        return a;
    }
    let v = vb / denominator;
    let w = vc / denominator;
    a + ab * v + ac * w
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// A flat field, `resolution` points a side, `cell` metres apart, at y = 0.
    fn flat(resolution: usize, cell: f32) -> Heightfield {
        let span = (resolution - 1) as f32 * cell;
        Heightfield::new(
            Vec3::new(-span * 0.5, 0.0, -span * 0.5),
            cell,
            resolution,
            vec![0.0; resolution * resolution],
        )
        .expect("a valid field")
    }

    /// A capsule standing with its feet at `feet`.
    fn standing(feet: Vec3) -> Capsule {
        Capsule::new(
            Segment::new(
                feet + Vec3::new(0.0, 0.4, 0.0),
                feet + Vec3::new(0.0, 1.4, 0.0),
            ),
            0.4,
        )
    }

    #[test]
    fn a_field_needs_enough_points_and_finite_numbers_to_be_a_surface() {
        assert!(Heightfield::new(Vec3::ZERO, 1.0, 1, vec![0.0]).is_none());
        assert!(Heightfield::new(Vec3::ZERO, 0.0, 4, vec![0.0; 16]).is_none());
        assert!(Heightfield::new(Vec3::ZERO, 1.0, 4, vec![0.0; 9]).is_none());
        assert!(Heightfield::new(Vec3::ZERO, 1.0, 2, vec![0.0, f32::NAN, 0.0, 0.0]).is_none());
        assert!(Heightfield::new(Vec3::ZERO, 1.0, 2, vec![0.0; 4]).is_some());
    }

    #[test]
    fn a_capsule_standing_on_flat_ground_is_held_up_by_it() {
        let field = flat(5, 4.0);
        // Feet ten centimetres under the surface. The capsule's lowest point is its bottom
        // cap, so that is how far it has sunk in, and the push is straight up.
        let capsule = standing(Vec3::new(0.0, -0.1, 0.0));
        let contact = capsule_vs_heightfield(&capsule, &field).expect("the ground is there");
        assert!(contact.normal.y > 0.99, "{:?}", contact.normal);
        assert!(
            (contact.penetration - 0.1).abs() < 1e-3,
            "{}",
            contact.penetration
        );
    }

    #[test]
    fn a_capsule_well_above_the_ground_touches_nothing() {
        let field = flat(5, 4.0);
        assert!(capsule_vs_heightfield(&standing(Vec3::new(0.0, 5.0, 0.0)), &field).is_none());
    }

    #[test]
    fn a_capsule_off_the_edge_of_the_field_touches_nothing() {
        let field = flat(5, 4.0);
        assert!(capsule_vs_heightfield(&standing(Vec3::new(60.0, 0.0, 0.0)), &field).is_none());
        assert!(capsule_vs_heightfield(&standing(Vec3::new(0.0, 0.0, -60.0)), &field).is_none());
    }

    #[test]
    fn a_slope_pushes_out_along_its_own_normal() {
        // A field that rises one metre per metre along +X: a 45 degree slope.
        let resolution = 5;
        let cell = 2.0;
        let span = (resolution - 1) as f32 * cell;
        let mut heights = vec![0.0; resolution * resolution];
        for z in 0..resolution {
            for x in 0..resolution {
                heights[z * resolution + x] = x as f32 * cell;
            }
        }
        let field = Heightfield::new(
            Vec3::new(-span * 0.5, 0.0, -span * 0.5),
            cell,
            resolution,
            heights,
        )
        .expect("a valid field");

        // Standing in the middle, where the surface is at y = span/2 above the origin corner.
        let feet = Vec3::new(0.0, field.point(2, 2).y - 0.1, 0.0);
        let contact = capsule_vs_heightfield(&standing(feet), &field).expect("a contact");
        assert!(contact.normal.y > 0.6, "{:?}", contact.normal);
        assert!(
            contact.normal.x < -0.6,
            "the normal does not lean down the slope: {:?}",
            contact.normal
        );
        assert!((contact.normal.length() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_capsule_under_the_surface_is_pushed_out_downwards() {
        let field = flat(5, 4.0);
        // Entirely below the ground and within a radius of it: the way out is down. A contact
        // that pointed up would drag somebody who fell through back up through the terrain.
        let capsule = Capsule::new(
            Segment::new(Vec3::new(0.0, -0.3, 0.0), Vec3::new(0.0, -1.3, 0.0)),
            0.4,
        );
        let contact = capsule_vs_heightfield(&capsule, &field).expect("a contact");
        assert!(contact.normal.y < -0.9, "{:?}", contact.normal);
        assert!(
            (contact.penetration - 0.1).abs() < 1e-3,
            "{}",
            contact.penetration
        );
    }

    #[test]
    fn a_capsule_far_below_the_surface_is_left_alone() {
        // The field is a *surface*, not a solid: something a long way under it is falling
        // through empty space, and the level's own floor plane is what catches it. A field
        // that reached down forever would be a field nothing could ever be beneath.
        let field = flat(5, 4.0);
        assert!(capsule_vs_heightfield(&standing(Vec3::new(0.0, -30.0, 0.0)), &field).is_none());
    }

    #[test]
    fn the_deepest_cell_wins_when_several_are_touched() {
        // A step in the middle of the field: the capsule spans both cells, and the contact
        // reported is the deeper one.
        let resolution = 5;
        let cell = 1.0;
        let mut heights = vec![0.0; resolution * resolution];
        for z in 0..resolution {
            for x in 2..resolution {
                heights[z * resolution + x] = 1.0;
            }
        }
        let field = Heightfield::new(Vec3::new(-2.0, 0.0, -2.0), cell, resolution, heights)
            .expect("a valid field");
        let contact =
            capsule_vs_heightfield(&standing(Vec3::new(0.0, 0.5, 0.0)), &field).expect("a contact");
        assert!(contact.penetration > 0.0);
    }

    #[test]
    fn the_bounds_cover_the_highest_and_lowest_point() {
        let mut field = flat(5, 2.0);
        field.heights[0] = -3.0;
        field.heights[12] = 7.0;
        let bounds = field.bounds();
        assert_eq!(bounds.min.y, -3.0);
        assert_eq!(bounds.max.y, 7.0);
        assert_eq!(bounds.min.x, -4.0);
        assert_eq!(bounds.max.x, 4.0);
    }

    #[test]
    fn the_closest_point_on_a_triangle_lands_on_the_right_feature() {
        let triangle = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
        ];
        // Over the face.
        let inside = closest_point_on_triangle(&triangle, Vec3::new(0.5, 3.0, 0.5));
        assert!(
            (inside - Vec3::new(0.5, 0.0, 0.5)).length() < 1e-5,
            "{inside:?}"
        );
        // Past a vertex.
        let vertex = closest_point_on_triangle(&triangle, Vec3::new(-4.0, 1.0, -4.0));
        assert!((vertex - triangle[0]).length() < 1e-5, "{vertex:?}");
        // Beside an edge.
        let edge = closest_point_on_triangle(&triangle, Vec3::new(1.0, 1.0, -4.0));
        assert!(
            (edge - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "{edge:?}"
        );
    }
}
