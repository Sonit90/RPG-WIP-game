//! The narrow phase: exact tests between the character's capsule and one static shape.
//!
//! ```text
//! broad phase -> candidate -> narrow phase -> Option<Contact>
//!                             ^^^^^^^^^^^^
//!                             this module
//! ```
//!
//! Everything here is a free function over plain geometry. No world, no ids, no filtering,
//! no allocation, no state — which is what makes each one testable on its own and is why
//! the geometry test suite can be written without building a level.
//!
//! ## The one idea
//!
//! A capsule is a segment with a radius. So every capsule test is the same two steps:
//!
//! 1. find the closest points between the capsule's spine and the static shape;
//! 2. if they are closer than the radius, that gap is the penetration and the direction
//!    between them is the normal.
//!
//! The only complication is the degenerate case where the spine is *inside* the shape and
//! there is no direction between the two points. That is handled per shape by falling back
//! to the smallest translation that gets out, which is the only answer that does not push
//! a character through the far side of a wall it is stuck in.

use engine_core::math::{Aabb, Plane, Vec3};

use crate::contact::Contact;
use crate::shape::{Capsule, Obb, Segment, Shape, closest_point_on_aabb};

/// Below this, two points are the same point and the direction between them is noise.
const EPSILON: f32 = 1e-6;

/// Iterations the general segment-to-box solve is allowed.
///
/// Alternating projection between two convex sets, which converges monotonically; twelve is
/// far past the point where the result stops moving in `f32`, and the loop exits early when
/// it does. Fixed rather than "until converged" so the cost of a test cannot depend on the
/// geometry, which is what would make a benchmark unrepeatable.
const CLOSEST_POINT_ITERATIONS: u32 = 12;

/// Tests a capsule against any static shape.
#[must_use]
pub fn capsule_vs_shape(capsule: &Capsule, shape: &Shape) -> Option<Contact> {
    match shape {
        Shape::Plane(plane) => capsule_vs_plane(capsule, plane),
        Shape::Aabb(aabb) => capsule_vs_aabb(capsule, aabb),
        Shape::Obb(obb) => capsule_vs_obb(capsule, obb),
    }
}

/// Tests a capsule against an infinite plane.
///
/// The contact is taken at whichever cap is deeper, so a capsule resting on a floor
/// produces a contact under its feet rather than at its centre.
#[must_use]
pub fn capsule_vs_plane(capsule: &Capsule, plane: &Plane) -> Option<Contact> {
    let start = plane.signed_distance(capsule.segment.start);
    let end = plane.signed_distance(capsule.segment.end);
    let (distance, deepest) = if start <= end {
        (start, capsule.segment.start)
    } else {
        (end, capsule.segment.end)
    };

    let penetration = capsule.radius - distance;
    if penetration <= 0.0 {
        return None;
    }
    // The surface point below the deepest cap, not the cap itself: a contact point inside
    // the plane would draw a debug marker underground.
    let point = deepest - plane.normal * distance;
    Some(Contact::new(point, plane.normal, penetration))
}

/// Tests a capsule against an axis-aligned box.
#[must_use]
pub fn capsule_vs_aabb(capsule: &Capsule, bounds: &Aabb) -> Option<Contact> {
    if bounds.is_empty() {
        return None;
    }
    let (on_segment, on_box) = closest_points_segment_aabb(&capsule.segment, bounds);
    let offset = on_segment - on_box;
    let distance_squared = offset.length_squared();

    if distance_squared > capsule.radius * capsule.radius {
        return None;
    }

    if distance_squared > EPSILON * EPSILON {
        let distance = distance_squared.sqrt();
        let penetration = capsule.radius - distance;
        // Resting exactly on a surface is not a collision. Reporting a zero-penetration
        // contact would make a character permanently in contact with every wall it walks
        // past, and every one of those contacts would clip its velocity.
        if penetration <= 0.0 {
            return None;
        }
        return Some(Contact::new(on_box, offset / distance, penetration));
    }

    // The spine is inside the box. There is no direction between the two points, so the
    // answer is the shortest way out through a face — anything else pushes the character
    // through the wall it is embedded in.
    let (normal, depth) = shallowest_exit(bounds, on_segment);
    Some(Contact::new(
        on_segment + normal * depth,
        normal,
        capsule.radius + depth,
    ))
}

/// Tests a capsule against an oriented box.
///
/// Done entirely in the box's own space, where it is an [`Aabb`] again: two rotations of the
/// spine in, one rotation of the normal and one of the point out. That is cheaper than any
/// scheme that keeps the box oriented, and it means ramps and walls share one test.
#[must_use]
pub fn capsule_vs_obb(capsule: &Capsule, obb: &Obb) -> Option<Contact> {
    let local = Capsule::new(
        Segment::new(
            obb.to_local(capsule.segment.start),
            obb.to_local(capsule.segment.end),
        ),
        capsule.radius,
    );
    let contact = capsule_vs_aabb(&local, &obb.local_bounds())?;
    Some(Contact::new(
        obb.to_world(contact.point),
        obb.direction_to_world(contact.normal),
        contact.penetration,
    ))
}

/// The shortest translation that moves `point` — which is inside `bounds` — out of it.
///
/// Returns the direction and the distance. Used only for the deep case, where the capsule's
/// spine is already inside a box; the ordinary case never reaches it.
fn shallowest_exit(bounds: &Aabb, point: Vec3) -> (Vec3, f32) {
    let to_min = point - bounds.min;
    let to_max = bounds.max - point;

    let mut normal = Vec3::Y;
    let mut depth = f32::INFINITY;
    for (distance, direction) in [
        (to_min.x, -Vec3::X),
        (to_max.x, Vec3::X),
        (to_min.y, -Vec3::Y),
        (to_max.y, Vec3::Y),
        (to_min.z, -Vec3::Z),
        (to_max.z, Vec3::Z),
    ] {
        if distance < depth {
            depth = distance;
            normal = direction;
        }
    }
    (normal, depth.max(0.0))
}

/// The closest pair of points between a segment and a box.
///
/// Returns `(point on the segment, point on the box)`. When the two intersect, both points
/// are the same point inside the box and the caller is expected to handle that — see
/// [`capsule_vs_aabb`].
#[must_use]
pub fn closest_points_segment_aabb(segment: &Segment, bounds: &Aabb) -> (Vec3, Vec3) {
    let direction = segment.direction();

    // The character's capsule is upright, always, and that is by far the most common test
    // in the game. For a vertical spine the answer is exact and immediate: X and Z do not
    // vary along the segment, so only the Y overlap has to be resolved.
    if direction.x.abs() <= EPSILON && direction.z.abs() <= EPSILON {
        let (low, high) = if segment.start.y <= segment.end.y {
            (segment.start.y, segment.end.y)
        } else {
            (segment.end.y, segment.start.y)
        };
        let (segment_y, box_y) = if low > bounds.max.y {
            (low, bounds.max.y)
        } else if high < bounds.min.y {
            (high, bounds.min.y)
        } else {
            // The ranges overlap: the closest pair is anywhere in the overlap and the gap is
            // zero in Y. The middle of the overlap is chosen so a capsule standing beside a
            // tall wall reports a contact at its own middle rather than at an arbitrary end.
            let overlap = (low.max(bounds.min.y) + high.min(bounds.max.y)) * 0.5;
            (overlap, overlap)
        };
        let on_segment = Vec3::new(segment.start.x, segment_y, segment.start.z);
        let on_box = Vec3::new(
            on_segment.x.clamp(bounds.min.x, bounds.max.x),
            box_y,
            on_segment.z.clamp(bounds.min.z, bounds.max.z),
        );
        return (on_segment, on_box);
    }

    // The general case: alternate between projecting onto the box and back onto the
    // segment. Both are convex, so this converges to the closest pair.
    let length_squared = direction.length_squared();
    let mut t = 0.5_f32;
    let mut on_box = closest_point_on_aabb(bounds, segment.at(t));
    for _ in 0..CLOSEST_POINT_ITERATIONS {
        let next = ((on_box - segment.start).dot(direction) / length_squared).clamp(0.0, 1.0);
        if (next - t).abs() <= EPSILON {
            t = next;
            break;
        }
        t = next;
        on_box = closest_point_on_aabb(bounds, segment.at(t));
    }
    let on_segment = segment.at(t);
    (on_segment, closest_point_on_aabb(bounds, on_segment))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::math::Quat;

    fn standing(x: f32, y: f32, z: f32) -> Capsule {
        Capsule::upright(Vec3::new(x, y, z), 0.4, 1.8)
    }

    #[test]
    fn a_capsule_above_the_floor_does_not_touch_it() {
        let plane = Plane::new(Vec3::Y, 0.0);
        assert!(capsule_vs_plane(&standing(0.0, 2.0, 0.0), &plane).is_none());
    }

    #[test]
    fn a_capsule_standing_on_the_floor_touches_it_with_no_penetration_to_spare() {
        let plane = Plane::new(Vec3::Y, 0.0);
        // Exactly touching is the boundary case: the lower cap centre is one radius up.
        let contact = capsule_vs_plane(&standing(0.0, 0.0, 0.0), &plane);
        assert!(
            contact.is_none(),
            "resting exactly on the surface is not penetration"
        );

        let sunk = capsule_vs_plane(&standing(0.0, -0.1, 0.0), &plane).expect("it is inside");
        assert!((sunk.penetration - 0.1).abs() < 1e-5, "{sunk:?}");
        assert_eq!(sunk.normal, Vec3::Y);
        assert!(sunk.point.y.abs() < 1e-5, "the contact is on the surface");
    }

    #[test]
    fn a_capsule_under_the_floor_is_pushed_up_not_down() {
        // The sign error that lets a character fall through the world.
        let plane = Plane::new(Vec3::Y, 0.0);
        let contact = capsule_vs_plane(&standing(0.0, -5.0, 0.0), &plane).expect("it is inside");
        assert_eq!(contact.normal, Vec3::Y);
        assert!(contact.penetration >= 5.0, "{contact:?}");
    }

    #[test]
    fn a_capsule_beside_a_wall_is_pushed_along_the_wall_normal() {
        let wall = Aabb::new(Vec3::new(1.0, 0.0, -5.0), Vec3::new(2.0, 3.0, 5.0));
        let contact = capsule_vs_aabb(&standing(0.7, 0.0, 0.0), &wall).expect("0.3 < radius 0.4");

        assert!((contact.normal - -Vec3::X).length() < 1e-5, "{contact:?}");
        assert!((contact.penetration - 0.1).abs() < 1e-5, "{contact:?}");
        assert!((contact.point.x - 1.0).abs() < 1e-5, "on the wall's face");
    }

    #[test]
    fn a_capsule_clear_of_a_wall_reports_nothing() {
        let wall = Aabb::new(Vec3::new(1.0, 0.0, -5.0), Vec3::new(2.0, 3.0, 5.0));
        assert!(capsule_vs_aabb(&standing(0.5, 0.0, 0.0), &wall).is_none());
    }

    #[test]
    fn touching_is_at_most_an_infinitesimal_collision() {
        // The boundary. `0.6 + 0.4` is not exactly `1.0` in `f32`, so "exactly touching" is
        // not a case the arithmetic can be asked for: what must hold is that a hair on
        // either side of it is either no contact at all or a contact too small to move
        // anything. A wall that reported a real penetration here would clip the velocity of
        // every character that walked past it.
        let wall = Aabb::new(Vec3::new(1.0, 0.0, -5.0), Vec3::new(2.0, 3.0, 5.0));
        let touching = capsule_vs_aabb(&standing(0.6, 0.0, 0.0), &wall);
        assert!(
            touching.is_none_or(|contact| contact.penetration < 1e-6),
            "{touching:?}"
        );
        assert!(
            capsule_vs_aabb(&standing(0.59, 0.0, 0.0), &wall).is_none(),
            "a hair clear of the wall is clear of it"
        );
    }

    #[test]
    fn a_capsule_standing_on_a_box_is_pushed_up() {
        let platform = Aabb::new(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 1.0, 2.0));
        let contact = capsule_vs_aabb(&standing(0.0, 0.8, 0.0), &platform).expect("sunk 0.2 in");
        assert!((contact.normal - Vec3::Y).length() < 1e-5, "{contact:?}");
        assert!((contact.penetration - 0.2).abs() < 1e-5, "{contact:?}");
    }

    #[test]
    fn a_capsule_at_a_corner_gets_a_diagonal_normal() {
        // The case that independent X/Z clamping gets wrong: the closest feature is an edge,
        // so the normal points out of the corner rather than along an axis.
        let block = Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 3.0, 4.0));
        let contact =
            capsule_vs_aabb(&standing(-0.2, 0.0, -0.2), &block).expect("the corner is inside");
        assert!(contact.normal.x < -0.5, "{contact:?}");
        assert!(contact.normal.z < -0.5, "{contact:?}");
        assert!(contact.normal.y.abs() < 1e-5, "a vertical edge is level");
        assert!((contact.normal.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_capsule_buried_in_a_box_is_pushed_out_the_nearest_face() {
        // The degenerate case. A wrong answer here teleports a stuck character out the far
        // side of the wall.
        let block = Aabb::new(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 4.0, 5.0));
        let capsule = Capsule::upright(Vec3::new(4.5, 1.0, 0.0), 0.4, 1.8);
        let contact = capsule_vs_aabb(&capsule, &block).expect("it is buried");

        assert!((contact.normal - Vec3::X).length() < 1e-5, "{contact:?}");
        assert!(
            contact.penetration > 0.4,
            "a buried capsule is at least a radius deep: {contact:?}"
        );
    }

    #[test]
    fn an_empty_box_collides_with_nothing() {
        assert!(capsule_vs_aabb(&standing(0.0, 0.0, 0.0), &Aabb::EMPTY).is_none());
    }

    #[test]
    fn a_capsule_on_a_ramp_gets_the_ramps_own_normal() {
        let angle = 30.0_f32.to_radians();
        let ramp = Obb::ramp(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(4.0, 0.25, 4.0),
            Vec3::X,
            angle,
        );
        // Directly above the ramp's centre and low enough to be inside it.
        let contact = capsule_vs_obb(&standing(0.0, 0.0, 0.0), &ramp).expect("it is inside");
        let levelness = contact.normal.dot(Vec3::Y);
        assert!(
            (levelness - angle.cos()).abs() < 1e-3,
            "expected the ramp's own tilt, got {levelness}"
        );
    }

    #[test]
    fn an_unrotated_oriented_box_agrees_with_the_axis_aligned_test() {
        // The two paths must not disagree, or a level built from both would behave
        // differently depending on which one an author happened to use.
        let bounds = Aabb::new(Vec3::new(1.0, 0.0, -5.0), Vec3::new(2.0, 3.0, 5.0));
        let obb = Obb::new(bounds.center(), bounds.half_extents(), Quat::IDENTITY);
        let capsule = standing(0.7, 0.0, 0.0);

        let flat = capsule_vs_aabb(&capsule, &bounds).expect("a contact");
        let oriented = capsule_vs_obb(&capsule, &obb).expect("a contact");
        assert!((flat.normal - oriented.normal).length() < 1e-4);
        assert!((flat.penetration - oriented.penetration).abs() < 1e-4);
        assert!((flat.point - oriented.point).length() < 1e-4);
    }

    #[test]
    fn the_closest_pair_for_a_vertical_spine_is_exact() {
        let bounds = Aabb::new(Vec3::new(2.0, 0.0, -1.0), Vec3::new(3.0, 1.0, 1.0));
        // Spine entirely above the box.
        let above = Segment::new(Vec3::new(2.5, 3.0, 0.0), Vec3::new(2.5, 5.0, 0.0));
        let (on_segment, on_box) = closest_points_segment_aabb(&above, &bounds);
        assert_eq!(on_segment, Vec3::new(2.5, 3.0, 0.0));
        assert_eq!(on_box, Vec3::new(2.5, 1.0, 0.0));

        // Spine entirely below.
        let below = Segment::new(Vec3::new(2.5, -3.0, 0.0), Vec3::new(2.5, -2.0, 0.0));
        let (on_segment, on_box) = closest_points_segment_aabb(&below, &bounds);
        assert_eq!(on_segment, Vec3::new(2.5, -2.0, 0.0));
        assert_eq!(on_box, Vec3::new(2.5, 0.0, 0.0));

        // Spine beside it: the vertical ranges overlap, so the gap is horizontal only.
        let beside = Segment::new(Vec3::new(0.0, 0.2, 0.0), Vec3::new(0.0, 0.8, 0.0));
        let (on_segment, on_box) = closest_points_segment_aabb(&beside, &bounds);
        assert!((on_segment.y - on_box.y).abs() < 1e-6);
        assert_eq!(on_box.x, 2.0);
    }

    #[test]
    fn the_general_solve_agrees_with_the_vertical_one_when_the_spine_is_nearly_vertical() {
        // The two paths meet here, and a disagreement would show up as a character that
        // behaves differently for no visible reason.
        let bounds = Aabb::new(Vec3::new(1.0, 0.0, -1.0), Vec3::new(2.0, 2.0, 1.0));
        let vertical = Segment::new(Vec3::new(0.0, 0.4, 0.0), Vec3::new(0.0, 1.4, 0.0));
        let tilted = Segment::new(Vec3::new(0.0, 0.4, 0.0), Vec3::new(0.001, 1.4, 0.001));

        // Compared as *distances*, not as points: when the spine runs alongside a face,
        // every point of the overlap is equally close, so the two paths are entitled to
        // pick different ones. What must agree is how far apart the shapes are, because
        // that is what becomes the penetration.
        let (from, to) = closest_points_segment_aabb(&vertical, &bounds);
        let exact = (from - to).length();
        let (from, to) = closest_points_segment_aabb(&tilted, &bounds);
        let general = (from - to).length();
        assert!((exact - general).abs() < 1e-3, "{exact} vs {general}");
    }

    #[test]
    fn a_horizontal_spine_finds_the_face_it_points_at() {
        let bounds = Aabb::new(Vec3::new(2.0, 0.0, -1.0), Vec3::new(3.0, 2.0, 1.0));
        let spine = Segment::new(Vec3::new(-2.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        let (on_segment, on_box) = closest_points_segment_aabb(&spine, &bounds);
        assert!((on_segment - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-4);
        assert!((on_box - Vec3::new(2.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn every_shape_variant_is_reachable_through_one_entry_point() {
        let capsule = standing(0.0, -0.2, 0.0);
        assert!(capsule_vs_shape(&capsule, &Shape::ground(0.0)).is_some());
        assert!(capsule_vs_shape(&capsule, &Shape::box_at(Vec3::ZERO, Vec3::splat(2.0))).is_some());
        assert!(
            capsule_vs_shape(
                &capsule,
                &Shape::Obb(Obb::ramp(Vec3::ZERO, Vec3::splat(1.0), Vec3::X, 0.3))
            )
            .is_some()
        );
        assert!(
            capsule_vs_shape(
                &capsule,
                &Shape::box_at(Vec3::new(50.0, 0.0, 0.0), Vec3::ONE)
            )
            .is_none()
        );
    }
}
