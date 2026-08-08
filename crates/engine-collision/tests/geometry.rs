//! Geometry: the exact tests, from outside the crate.
//!
//! These use only the public API, so they are also a check that the public API is enough to
//! ask the questions a level author would ask. The unit tests inside the crate cover the
//! same shapes from the inside; this file covers the vocabulary a caller actually has.
#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_collision::narrow::{
    capsule_vs_aabb, capsule_vs_obb, capsule_vs_plane, capsule_vs_shape,
    closest_points_segment_aabb,
};
use engine_collision::prelude::*;
use engine_collision::shape::{Segment, aabb_overlaps, closest_point_on_aabb};
use engine_core::math::{Aabb, Plane, Quat, Vec3};

/// The character's shape, standing at `(x, y, z)`.
fn standing(x: f32, y: f32, z: f32) -> Capsule {
    Capsule::upright(Vec3::new(x, y, z), 0.4, 1.8)
}

/// A box from two corners.
fn boxed(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
    Aabb::new(
        Vec3::new(min.0, min.1, min.2),
        Vec3::new(max.0, max.1, max.2),
    )
}

// --- AABB against AABB ------------------------------------------------------------------

#[test]
fn two_boxes_overlap_exactly_when_they_share_space() {
    let a = boxed((0.0, 0.0, 0.0), (2.0, 2.0, 2.0));

    assert!(aabb_overlaps(&a, &boxed((1.0, 1.0, 1.0), (3.0, 3.0, 3.0))));
    assert!(aabb_overlaps(&a, &a), "a box overlaps itself");
    assert!(
        aabb_overlaps(&a, &boxed((-5.0, -5.0, -5.0), (5.0, 5.0, 5.0))),
        "containment is overlap"
    );
    assert!(!aabb_overlaps(&a, &boxed((2.1, 0.0, 0.0), (3.0, 2.0, 2.0))));
}

#[test]
fn boxes_that_share_only_a_face_an_edge_or_a_corner_count_as_touching() {
    // The boundary case a broad phase must not get wrong in the *rejecting* direction: a
    // candidate wrongly dropped is a wall walked through.
    let a = boxed((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    assert!(
        aabb_overlaps(&a, &boxed((1.0, 0.0, 0.0), (2.0, 1.0, 1.0))),
        "face"
    );
    assert!(
        aabb_overlaps(&a, &boxed((1.0, 1.0, 0.0), (2.0, 2.0, 1.0))),
        "edge"
    );
    assert!(
        aabb_overlaps(&a, &boxed((1.0, 1.0, 1.0), (2.0, 2.0, 2.0))),
        "corner"
    );
}

#[test]
fn separation_on_any_single_axis_is_enough_to_miss() {
    let a = boxed((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    for far in [
        boxed((2.0, 0.0, 0.0), (3.0, 1.0, 1.0)),
        boxed((0.0, 2.0, 0.0), (1.0, 3.0, 1.0)),
        boxed((0.0, 0.0, 2.0), (1.0, 1.0, 3.0)),
    ] {
        assert!(!aabb_overlaps(&a, &far), "{far:?} should be clear");
    }
}

// --- Capsule against plane --------------------------------------------------------------

#[test]
fn a_capsule_meets_a_floor_when_its_lower_cap_reaches_it() {
    let floor = Plane::new(Vec3::Y, 0.0);

    assert!(capsule_vs_plane(&standing(0.0, 1.0, 0.0), &floor).is_none());
    let contact = capsule_vs_plane(&standing(0.0, -0.25, 0.0), &floor).expect("it is inside");
    assert_eq!(contact.normal, Vec3::Y);
    assert!((contact.penetration - 0.25).abs() < 1e-5);
    assert!(
        contact.point.y.abs() < 1e-5,
        "the contact is on the surface"
    );
    assert!(contact.levelness() > 0.99, "a floor is level");
}

#[test]
fn a_tilted_plane_reports_its_own_normal_and_its_own_angle() {
    let angle = 30.0_f32.to_radians();
    let normal = Vec3::new(-angle.sin(), angle.cos(), 0.0);
    let plane = Plane::new(normal, 0.0);

    let contact = capsule_vs_plane(&standing(0.0, 0.0, 0.0), &plane).expect("a contact");
    assert!((contact.slope_angle() - angle).abs() < 1e-4);
    assert!((contact.normal - normal).length() < 1e-5);
}

#[test]
fn a_ceiling_plane_pushes_downwards() {
    // A plane whose normal points down: the underside of something.
    let ceiling = Plane::new(-Vec3::Y, 3.0);
    let contact = capsule_vs_plane(&standing(0.0, 1.5, 0.0), &ceiling).expect("a contact");
    assert!(contact.normal.y < 0.0, "{contact:?}");
    assert!(contact.levelness() < -0.99);
}

// --- Capsule against AABB ---------------------------------------------------------------

#[test]
fn a_capsule_is_pushed_out_of_the_nearest_face_of_a_wall() {
    let wall = boxed((2.0, 0.0, -5.0), (3.0, 4.0, 5.0));

    assert!(
        capsule_vs_aabb(&standing(1.0, 0.0, 0.0), &wall).is_none(),
        "clear"
    );

    let near = capsule_vs_aabb(&standing(1.8, 0.0, 0.0), &wall).expect("0.2 < radius");
    assert!((near.normal - -Vec3::X).length() < 1e-5, "{near:?}");
    assert!((near.penetration - 0.2).abs() < 1e-5, "{near:?}");
    assert!((near.point.x - 2.0).abs() < 1e-5, "on the face");

    let far = capsule_vs_aabb(&standing(3.2, 0.0, 0.0), &wall).expect("the other side");
    assert!((far.normal - Vec3::X).length() < 1e-5, "{far:?}");
}

#[test]
fn a_capsule_over_a_platform_is_pushed_up_and_one_under_it_is_pushed_down() {
    let platform = boxed((-4.0, 1.0, -4.0), (4.0, 2.0, 4.0));

    let above = capsule_vs_aabb(&standing(0.0, 1.8, 0.0), &platform).expect("standing on it");
    assert!((above.normal - Vec3::Y).length() < 1e-5, "{above:?}");
    assert!((above.penetration - 0.2).abs() < 1e-5, "{above:?}");

    let below = capsule_vs_aabb(&standing(0.0, -0.5, 0.0), &platform).expect("head into it");
    assert!((below.normal + Vec3::Y).length() < 1e-5, "{below:?}");
}

#[test]
fn the_normal_at_a_vertical_edge_is_diagonal_and_unit_length() {
    // The case independent X and Z clamping cannot express, and the reason movement
    // resolution works from normals.
    let block = boxed((0.0, 0.0, 0.0), (4.0, 3.0, 4.0));
    let contact = capsule_vs_aabb(&standing(-0.2, 0.0, -0.2), &block).expect("inside the radius");

    assert!(
        contact.normal.x < -0.5 && contact.normal.z < -0.5,
        "{contact:?}"
    );
    assert!(
        contact.normal.y.abs() < 1e-5,
        "a vertical edge is level in Y"
    );
    assert!((contact.normal.length() - 1.0).abs() < 1e-5);
    assert!(contact.penetration > 0.0 && contact.penetration < 0.4);
}

#[test]
fn penetration_grows_as_the_capsule_moves_in_and_the_normal_does_not_flip() {
    let wall = boxed((2.0, 0.0, -5.0), (3.0, 4.0, 5.0));
    let mut previous = 0.0;
    for step in 0..20 {
        let x = 1.65 + step as f32 * 0.01;
        let contact = capsule_vs_aabb(&standing(x, 0.0, 0.0), &wall).expect("inside");
        assert!(
            contact.penetration > previous,
            "penetration went backwards at x = {x}"
        );
        assert!(contact.normal.x < 0.0, "the normal flipped at x = {x}");
        previous = contact.penetration;
    }
}

#[test]
fn a_capsule_whose_spine_is_inside_a_box_is_pushed_out_the_shortest_way() {
    let block = boxed((-5.0, 0.0, -5.0), (5.0, 4.0, 5.0));
    for (feet, expected) in [
        (Vec3::new(4.6, 1.0, 0.0), Vec3::X),
        (Vec3::new(-4.6, 1.0, 0.0), -Vec3::X),
        (Vec3::new(0.0, 1.0, 4.6), Vec3::Z),
        (Vec3::new(0.0, 1.0, -4.6), -Vec3::Z),
    ] {
        let capsule = Capsule::upright(feet, 0.4, 1.8);
        let contact = capsule_vs_aabb(&capsule, &block).expect("buried");
        assert!(
            (contact.normal - expected).length() < 1e-5,
            "from {feet:?} it was pushed {:?}",
            contact.normal
        );
    }
}

#[test]
fn degenerate_input_is_answered_rather_than_crashed() {
    let wall = boxed((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));

    // An empty box collides with nothing.
    assert!(capsule_vs_aabb(&standing(0.5, 0.5, 0.5), &Aabb::EMPTY).is_none());
    // A zero-radius capsule is a segment, and a segment beside a box misses it.
    let thin = Capsule::new(
        Segment::new(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.0)),
        0.0,
    );
    assert!(capsule_vs_aabb(&thin, &wall).is_none());
    // A flat box is still a surface.
    let flat = boxed((0.0, 0.0, 0.0), (4.0, 0.0, 4.0));
    let contact = capsule_vs_aabb(&standing(1.0, -0.2, 1.0), &flat).expect("a floor with no depth");
    assert!(contact.penetration.is_finite());
}

// --- Capsule against OBB ----------------------------------------------------------------

#[test]
fn an_unrotated_oriented_box_behaves_exactly_like_an_axis_aligned_one() {
    let bounds = boxed((2.0, 0.0, -5.0), (3.0, 4.0, 5.0));
    let obb = Obb::new(bounds.center(), bounds.half_extents(), Quat::IDENTITY);

    for x in [1.7_f32, 1.9, 2.5, 3.3] {
        let capsule = standing(x, 0.0, 0.0);
        let flat = capsule_vs_aabb(&capsule, &bounds);
        let oriented = capsule_vs_obb(&capsule, &obb);
        match (flat, oriented) {
            (Some(a), Some(b)) => {
                assert!((a.normal - b.normal).length() < 1e-4, "at x = {x}");
                assert!((a.penetration - b.penetration).abs() < 1e-4, "at x = {x}");
            }
            (None, None) => {}
            (a, b) => panic!("the two paths disagreed at x = {x}: {a:?} vs {b:?}"),
        }
    }
}

#[test]
fn a_ramps_contact_normal_is_the_angle_it_was_built_at() {
    for degrees in [10.0_f32, 25.0, 45.0, 60.0] {
        let radians = degrees.to_radians();
        let ramp = Obb::ramp(
            Vec3::new(5.0 * radians.cos(), 5.0 * radians.sin(), 0.0),
            Vec3::new(5.0, 0.25, 3.0),
            Vec3::Z,
            radians,
        );
        // Standing on the ramp's surface, half way up.
        let x = 5.0 * radians.cos() * 0.5;
        let capsule = standing(x, x * radians.tan() - 0.1, 0.0);
        let contact = capsule_vs_obb(&capsule, &ramp).expect("standing on it");
        assert!(
            (contact.slope_angle().to_degrees() - degrees).abs() < 1.0,
            "{degrees} degrees reported as {}",
            contact.slope_angle().to_degrees()
        );
    }
}

// --- The dispatcher ---------------------------------------------------------------------

#[test]
fn one_entry_point_reaches_every_shape() {
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
            &Shape::box_at(Vec3::new(100.0, 0.0, 0.0), Vec3::ONE)
        )
        .is_none()
    );
}

// --- Supporting geometry ----------------------------------------------------------------

#[test]
fn the_closest_point_on_a_box_never_leaves_the_box() {
    let bounds = boxed((-1.0, -2.0, -3.0), (1.0, 2.0, 3.0));
    for point in [
        Vec3::new(50.0, 0.0, 0.0),
        Vec3::new(-50.0, -50.0, -50.0),
        Vec3::new(0.5, 0.5, 0.5),
        Vec3::new(0.0, 9.0, -9.0),
    ] {
        let closest = closest_point_on_aabb(&bounds, point);
        assert!(
            bounds.contains(closest),
            "{closest:?} is outside {bounds:?}"
        );
    }
}

#[test]
fn the_closest_pair_between_a_segment_and_a_box_is_the_shortest_gap_there_is() {
    let bounds = boxed((2.0, 0.0, -1.0), (3.0, 2.0, 1.0));
    let segment = Segment::new(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.5, 0.0));
    let (on_segment, on_box) = closest_points_segment_aabb(&segment, &bounds);
    let gap = (on_segment - on_box).length();

    // Sampled densely along the segment: nothing on it is closer than the answer.
    for step in 0..=100 {
        let sample = segment.at(step as f32 / 100.0);
        let closest = closest_point_on_aabb(&bounds, sample);
        assert!(
            (sample - closest).length() >= gap - 1e-4,
            "a closer pair exists at t = {}",
            step as f32 / 100.0
        );
    }
}

#[test]
fn a_capsules_bounds_contain_the_capsule() {
    let capsule = standing(1.0, 2.0, -3.0);
    let bounds = capsule.bounds();
    for point in [
        capsule.feet(),
        capsule.segment.start,
        capsule.segment.end,
        capsule.segment.end + Vec3::Y * capsule.radius,
        capsule.segment.start + Vec3::X * capsule.radius,
    ] {
        assert!(bounds.contains(point), "{point:?} escaped {bounds:?}");
    }
}
