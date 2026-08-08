//! The collision vocabulary: a capsule, a box, an oriented box, a plane and a segment.
//!
//! Deliberately small. Every shape here earns its place by being something the current game
//! actually needs:
//!
//! ```text
//! Capsule  the character. Never the character's mesh.
//! Aabb     ordinary static geometry: walls, steps, platforms.
//! Obb      geometry that has to be tilted: ramps.
//! Plane    the infinite floor, so a player who leaves the arena still lands on something.
//! Segment  the interior of a capsule, and what a downward probe traces.
//! ```
//!
//! [`engine_core::math::Aabb`] and [`engine_core::math::Plane`] are reused rather than
//! redefined — the renderer's culler already speaks them, and a second `Aabb` in the
//! workspace would be a conversion function waiting to be written wrongly.
//!
//! ## The capsule is upright, always
//!
//! The character controller never tilts its capsule, so the interior segment is parallel to
//! +Y by construction. That is not a limitation of the narrow phase — [`Obb`] tests handle
//! an arbitrarily oriented segment — it is a deliberate property of a humanoid controller,
//! and it is what makes the AABB path two clamps instead of an iterative solve.

use engine_core::math::{Aabb, Plane, Quat, Vec3};

/// A line segment, as two endpoints.
///
/// The capsule's spine and the ground probe are both one of these, so the narrow phase has
/// a single primitive to test against a box rather than two nearly identical ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// One end.
    pub start: Vec3,
    /// The other end.
    pub end: Vec3,
}

impl Segment {
    /// A segment between two points.
    #[must_use]
    pub const fn new(start: Vec3, end: Vec3) -> Self {
        Self { start, end }
    }

    /// `start` to `end`.
    #[must_use]
    pub fn direction(&self) -> Vec3 {
        self.end - self.start
    }

    /// The point a fraction `t` of the way along, clamped to the segment.
    #[must_use]
    pub fn at(&self, t: f32) -> Vec3 {
        self.start + self.direction() * t.clamp(0.0, 1.0)
    }

    /// The point on the segment nearest `point`.
    ///
    /// A degenerate segment — both endpoints equal — returns its start rather than dividing
    /// by zero.
    #[must_use]
    pub fn closest_point(&self, point: Vec3) -> Vec3 {
        let direction = self.direction();
        let length_squared = direction.length_squared();
        if length_squared <= f32::EPSILON {
            return self.start;
        }
        let t = ((point - self.start).dot(direction) / length_squared).clamp(0.0, 1.0);
        self.start + direction * t
    }

    /// The box containing the segment.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        Aabb::new(self.start.min(self.end), self.start.max(self.end))
    }
}

/// The character's collision shape: a swept sphere, standing upright.
///
/// ```text
///        ___          <- top cap, centred at `segment.end`
///      /     \
///     |       |       <- the cylinder between the caps
///     |       |
///      \_____/        <- bottom cap, centred at `segment.start`
///          |
///          v
///        feet         <- `position`: segment.start.y - radius
/// ```
///
/// Stored as a segment plus a radius rather than as a position plus a height, because that
/// is the form every test wants: the distance from a point to a capsule is the distance to
/// its segment minus its radius, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    /// The spine: the centres of the two caps.
    pub segment: Segment,
    /// Radius of the caps and of the cylinder between them.
    pub radius: f32,
}

impl Capsule {
    /// An upright capsule standing with its feet at `feet`.
    ///
    /// `height` is the total height, feet to crown. A height below `2 * radius` cannot be a
    /// capsule, so it is clamped: the shape degenerates to a sphere rather than inverting
    /// its spine, which would put the bottom cap above the top one and turn every normal
    /// inside out.
    #[must_use]
    pub fn upright(feet: Vec3, radius: f32, height: f32) -> Self {
        let radius = radius.max(0.0);
        let height = height.max(2.0 * radius);
        Self {
            segment: Segment::new(
                feet + Vec3::new(0.0, radius, 0.0),
                feet + Vec3::new(0.0, height - radius, 0.0),
            ),
            radius,
        }
    }

    /// A capsule from an explicit spine.
    #[must_use]
    pub const fn new(segment: Segment, radius: f32) -> Self {
        Self { segment, radius }
    }

    /// Where the feet are: the bottom of the lower cap.
    #[must_use]
    pub fn feet(&self) -> Vec3 {
        self.segment.start - Vec3::new(0.0, self.radius, 0.0)
    }

    /// Total height, feet to crown.
    #[must_use]
    pub fn height(&self) -> f32 {
        self.segment.direction().length() + 2.0 * self.radius
    }

    /// The same capsule moved by `offset`.
    #[must_use]
    pub fn translated(&self, offset: Vec3) -> Self {
        Self {
            segment: Segment::new(self.segment.start + offset, self.segment.end + offset),
            radius: self.radius,
        }
    }

    /// The box containing the capsule.
    ///
    /// What the broad phase is asked with. Tight rather than conservative-by-a-factor: an
    /// inflated query box is candidate pairs that the narrow phase then rejects.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let radius = Vec3::splat(self.radius);
        let segment = self.segment.bounds();
        Aabb::new(segment.min - radius, segment.max + radius)
    }

    /// Whether `point` is inside the capsule.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        (self.segment.closest_point(point) - point).length_squared() <= self.radius * self.radius
    }
}

/// A box with a rotation: everything an [`Aabb`] cannot express, which in this game means
/// ramps.
///
/// Kept separate from `Aabb` rather than replacing it. An axis-aligned box is the common
/// case by a wide margin, its test is two clamps, and paying for a quaternion round trip on
/// every wall in the level to make ramps possible would be the wrong trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Obb {
    /// Centre in world space.
    pub center: Vec3,
    /// Half the size along each of its own axes, before rotation.
    pub half_extents: Vec3,
    /// Rotation from box space to world space.
    pub rotation: Quat,
}

impl Obb {
    /// An oriented box.
    #[must_use]
    pub fn new(center: Vec3, half_extents: Vec3, rotation: Quat) -> Self {
        Self {
            center,
            half_extents: half_extents.max(Vec3::ZERO),
            rotation,
        }
    }

    /// A ramp: a slab of `size`, tilted by `radians` about the given horizontal axis.
    ///
    /// The tilt is applied to a flat slab, so the surface the player walks on is the slab's
    /// top face and its normal is `rotation * +Y`. Rotating about +X tilts along Z; about
    /// +Z, along X.
    #[must_use]
    pub fn ramp(center: Vec3, half_extents: Vec3, axis: Vec3, radians: f32) -> Self {
        Self::new(center, half_extents, Quat::from_axis_angle(axis, radians))
    }

    /// The normal of the box's own +Y face, in world space.
    ///
    /// For a ramp this is the surface a character stands on, and comparing it against world
    /// up is what the demo scene's "walkable / not walkable" labels are derived from.
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.rotation.rotate(Vec3::Y)
    }

    /// Brings a world-space point into the box's own space.
    #[must_use]
    pub fn to_local(&self, point: Vec3) -> Vec3 {
        self.rotation.conjugate().rotate(point - self.center)
    }

    /// Takes a box-space point back to world space.
    #[must_use]
    pub fn to_world(&self, point: Vec3) -> Vec3 {
        self.rotation.rotate(point) + self.center
    }

    /// Takes a box-space direction back to world space.
    #[must_use]
    pub fn direction_to_world(&self, direction: Vec3) -> Vec3 {
        self.rotation.rotate(direction)
    }

    /// The box in its own space: `-half_extents` to `+half_extents`.
    #[must_use]
    pub fn local_bounds(&self) -> Aabb {
        Aabb::new(-self.half_extents, self.half_extents)
    }

    /// The axis-aligned box containing this one.
    ///
    /// Computed from the absolute rotation rather than by transforming eight corners: three
    /// rotations instead of eight, and it is exact for a box.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let x = self
            .rotation
            .rotate(Vec3::new(self.half_extents.x, 0.0, 0.0));
        let y = self
            .rotation
            .rotate(Vec3::new(0.0, self.half_extents.y, 0.0));
        let z = self
            .rotation
            .rotate(Vec3::new(0.0, 0.0, self.half_extents.z));
        let extent = Vec3::new(
            x.x.abs() + y.x.abs() + z.x.abs(),
            x.y.abs() + y.y.abs() + z.y.abs(),
            x.z.abs() + y.z.abs() + z.z.abs(),
        );
        Aabb::new(self.center - extent, self.center + extent)
    }
}

/// A static collision shape.
///
/// An enum rather than a trait object: the narrow phase is a `match` over three variants in
/// the hottest loop the character controller has, and a v-table call per candidate would
/// cost more than every test in this crate put together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// An infinite plane. The floor, and anything else a character must never get under.
    Plane(Plane),
    /// An axis-aligned box. Walls, steps, platforms.
    Aabb(Aabb),
    /// An oriented box. Ramps.
    Obb(Obb),
}

impl Shape {
    /// A box between two corners.
    #[must_use]
    pub fn box_between(min: Vec3, max: Vec3) -> Self {
        Self::Aabb(Aabb::new(min.min(max), min.max(max)))
    }

    /// A box of `size` centred at `center`.
    #[must_use]
    pub fn box_at(center: Vec3, size: Vec3) -> Self {
        let half = size.max(Vec3::ZERO) * 0.5;
        Self::Aabb(Aabb::new(center - half, center + half))
    }

    /// A box of `size` standing on the floor at `footprint`, growing upwards.
    ///
    /// The form level geometry is actually written in: a step is "0.4 m tall, here", not "a
    /// box whose centre is 0.2 m above the floor".
    #[must_use]
    pub fn box_standing(footprint: Vec3, size: Vec3) -> Self {
        let size = size.max(Vec3::ZERO);
        let half = Vec3::new(size.x * 0.5, 0.0, size.z * 0.5);
        Self::Aabb(Aabb::new(
            footprint - half,
            footprint + half + Vec3::new(0.0, size.y, 0.0),
        ))
    }

    /// A horizontal ground plane at height `y`.
    #[must_use]
    pub fn ground(y: f32) -> Self {
        Self::Plane(Plane::new(Vec3::Y, -y))
    }

    /// The axis-aligned box containing the shape.
    ///
    /// A plane has no finite bounds, so it reports `None` and is kept out of every spatial
    /// structure: a plane that had to be binned into a grid would land in every cell.
    #[must_use]
    pub fn bounds(&self) -> Option<Aabb> {
        match self {
            Self::Plane(_) => None,
            Self::Aabb(aabb) => Some(*aabb),
            Self::Obb(obb) => Some(obb.bounds()),
        }
    }
}

/// Whether two boxes overlap, touching included.
#[must_use]
pub fn aabb_overlaps(a: &Aabb, b: &Aabb) -> bool {
    a.min.x <= b.max.x
        && a.max.x >= b.min.x
        && a.min.y <= b.max.y
        && a.max.y >= b.min.y
        && a.min.z <= b.max.z
        && a.max.z >= b.min.z
}

/// The point of `box` nearest `point`, which is `point` itself when it is inside.
#[must_use]
pub fn closest_point_on_aabb(bounds: &Aabb, point: Vec3) -> Vec3 {
    point.max(bounds.min).min(bounds.max)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn an_upright_capsule_stands_on_its_feet() {
        let capsule = Capsule::upright(Vec3::new(1.0, 0.0, -2.0), 0.4, 1.8);
        assert_eq!(capsule.feet(), Vec3::new(1.0, 0.0, -2.0));
        assert_eq!(capsule.segment.start, Vec3::new(1.0, 0.4, -2.0));
        assert_eq!(capsule.segment.end, Vec3::new(1.0, 1.4, -2.0));
        assert!((capsule.height() - 1.8).abs() < 1e-5);
    }

    #[test]
    fn a_capsule_shorter_than_its_diameter_becomes_a_sphere_rather_than_inverting() {
        // The degenerate input: without the clamp the spine would point downwards and every
        // contact normal computed from it would be backwards.
        let capsule = Capsule::upright(Vec3::ZERO, 0.5, 0.2);
        assert_eq!(capsule.segment.start, capsule.segment.end);
        assert!((capsule.height() - 1.0).abs() < 1e-5);
        assert_eq!(
            capsule.feet(),
            Vec3::ZERO,
            "the clamp grows the capsule upwards; the feet stay where they were put"
        );
    }

    #[test]
    fn a_capsule_bounds_its_caps_and_not_just_its_spine() {
        let capsule = Capsule::upright(Vec3::ZERO, 0.5, 2.0);
        let bounds = capsule.bounds();
        assert_eq!(bounds.min, Vec3::new(-0.5, 0.0, -0.5));
        assert_eq!(bounds.max, Vec3::new(0.5, 2.0, 0.5));
    }

    #[test]
    fn a_capsule_contains_the_points_inside_it_and_no_others() {
        let capsule = Capsule::upright(Vec3::ZERO, 0.5, 2.0);
        assert!(capsule.contains(Vec3::new(0.0, 1.0, 0.0)));
        assert!(capsule.contains(Vec3::new(0.49, 1.0, 0.0)));
        assert!(!capsule.contains(Vec3::new(0.51, 1.0, 0.0)));
        // Inside the bounding box, outside the cap: the corner a sphere-in-a-box test gets
        // wrong.
        assert!(!capsule.contains(Vec3::new(0.45, 0.05, 0.45)));
    }

    #[test]
    fn the_closest_point_on_a_segment_is_clamped_to_its_ends() {
        let segment = Segment::new(Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(segment.closest_point(Vec3::new(1.0, 1.0, 0.0)).y, 1.0);
        assert_eq!(segment.closest_point(Vec3::new(0.0, -5.0, 0.0)), Vec3::ZERO);
        assert_eq!(
            segment.closest_point(Vec3::new(0.0, 9.0, 0.0)),
            Vec3::new(0.0, 2.0, 0.0)
        );
    }

    #[test]
    fn a_degenerate_segment_answers_with_its_start_rather_than_dividing_by_zero() {
        let point = Vec3::new(3.0, 1.0, 0.0);
        let segment = Segment::new(point, point);
        let closest = segment.closest_point(Vec3::ZERO);
        assert_eq!(closest, point);
        assert!(closest.x.is_finite() && closest.y.is_finite() && closest.z.is_finite());
    }

    #[test]
    fn a_ramps_up_vector_tilts_by_the_angle_it_was_built_with() {
        let angle = 30.0_f32.to_radians();
        let ramp = Obb::ramp(Vec3::ZERO, Vec3::new(2.0, 0.1, 2.0), Vec3::X, angle);
        let up = ramp.up();
        assert!(
            (up.dot(Vec3::Y) - angle.cos()).abs() < 1e-5,
            "{up:?} is not tilted by 30 degrees"
        );
    }

    #[test]
    fn an_oriented_box_bounds_itself_exactly_when_it_is_not_rotated() {
        let obb = Obb::new(Vec3::new(1.0, 2.0, 3.0), Vec3::splat(0.5), Quat::IDENTITY);
        let bounds = obb.bounds();
        assert!((bounds.min - Vec3::new(0.5, 1.5, 2.5)).length() < 1e-5);
        assert!((bounds.max - Vec3::new(1.5, 2.5, 3.5)).length() < 1e-5);
    }

    #[test]
    fn a_rotated_box_grows_its_axis_aligned_bounds() {
        let obb = Obb::ramp(
            Vec3::ZERO,
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
        );
        let bounds = obb.bounds();
        assert!(bounds.max.x > 1.4 && bounds.max.x < 1.5, "{bounds:?}");
        assert!((bounds.max.y - 1.0).abs() < 1e-5, "the tilt was about +Y");
    }

    #[test]
    fn local_and_world_space_round_trip() {
        let obb = Obb::ramp(Vec3::new(4.0, 1.0, -3.0), Vec3::splat(1.0), Vec3::Z, 0.6);
        let point = Vec3::new(4.4, 1.7, -2.5);
        let round_trip = obb.to_world(obb.to_local(point));
        assert!((round_trip - point).length() < 1e-4, "{round_trip:?}");
    }

    #[test]
    fn a_standing_box_grows_upwards_from_its_footprint() {
        let Shape::Aabb(bounds) = Shape::box_standing(Vec3::new(2.0, 0.0, 2.0), Vec3::splat(1.0))
        else {
            panic!("box_standing builds an Aabb");
        };
        assert_eq!(bounds.min, Vec3::new(1.5, 0.0, 1.5));
        assert_eq!(bounds.max, Vec3::new(2.5, 1.0, 2.5));
    }

    #[test]
    fn a_ground_plane_puts_its_surface_where_it_was_asked_to() {
        let Shape::Plane(plane) = Shape::ground(3.0) else {
            panic!("ground builds a Plane");
        };
        assert_eq!(plane.signed_distance(Vec3::new(0.0, 3.0, 0.0)), 0.0);
        assert!(plane.signed_distance(Vec3::new(0.0, 4.0, 0.0)) > 0.0);
        assert!(plane.signed_distance(Vec3::new(0.0, 2.0, 0.0)) < 0.0);
    }

    #[test]
    fn a_plane_has_no_finite_bounds_and_a_box_does() {
        assert_eq!(Shape::ground(0.0).bounds(), None);
        assert!(Shape::box_at(Vec3::ZERO, Vec3::ONE).bounds().is_some());
    }

    #[test]
    fn boxes_overlap_when_they_touch_and_not_when_they_are_apart() {
        let a = Aabb::new(Vec3::ZERO, Vec3::ONE);
        assert!(aabb_overlaps(
            &a,
            &Aabb::new(Vec3::splat(0.5), Vec3::splat(2.0))
        ));
        assert!(
            aabb_overlaps(&a, &Aabb::new(Vec3::ONE, Vec3::splat(2.0))),
            "a shared face counts as touching"
        );
        assert!(!aabb_overlaps(
            &a,
            &Aabb::new(Vec3::splat(1.01), Vec3::splat(2.0))
        ));
    }

    #[test]
    fn the_closest_point_on_a_box_is_the_point_itself_when_it_is_inside() {
        let bounds = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(
            closest_point_on_aabb(&bounds, Vec3::new(0.2, -0.3, 0.4)),
            Vec3::new(0.2, -0.3, 0.4)
        );
        assert_eq!(
            closest_point_on_aabb(&bounds, Vec3::new(5.0, 0.0, -5.0)),
            Vec3::new(1.0, 0.0, -1.0)
        );
    }
}
