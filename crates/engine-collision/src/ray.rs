//! Firing a ray at a static shape.
//!
//! ```text
//!  origin o------------->  direction (unit)
//!         |<-- distance -->|
//!                          x  hit point, and the normal out of the surface
//! ```
//!
//! The second query this crate answers, after "is this capsule touching anything". It exists
//! because a targeted ability has to know whether a wall stands between two points, and
//! because answering that with a swept capsule would be a different question with a
//! different answer at the corners.
//!
//! Nothing here knows what a ray is *for*. The gameplay rule — which abilities need line of
//! sight, and what happens when they do not have it — lives in `engine-game`; see
//! `line_of_sight.md`.

use engine_core::math::{Aabb, Plane, Vec3};

use crate::shape::{Obb, Shape};

/// Below this the ray is treated as parallel to a slab, rather than dividing by it.
const PARALLEL_EPSILON: f32 = 1e-6;

/// A half-line: somewhere to start and a direction to go.
///
/// The direction is normalised on construction, so a hit's `distance` is always in metres
/// and can be compared against a range without a second thought.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Where the ray starts, in world space.
    pub origin: Vec3,
    /// Unit direction. Never zero for a ray built by [`Ray::new`] or [`Ray::between`].
    pub direction: Vec3,
}

impl Ray {
    /// A ray from `origin` along `direction`, which is normalised.
    ///
    /// `None` for a direction with no length: a ray that points nowhere has no hits to
    /// report, and inventing an axis for it would make one up.
    #[must_use]
    pub fn new(origin: Vec3, direction: Vec3) -> Option<Self> {
        let length = direction.length();
        if length <= PARALLEL_EPSILON {
            return None;
        }
        Some(Self {
            origin,
            direction: direction / length,
        })
    }

    /// A ray from `from` towards `to`, with the distance between them.
    ///
    /// The form a segment query is actually written in: the caller has two points and wants
    /// to know what is between them, not a direction and a length it worked out itself.
    #[must_use]
    pub fn between(from: Vec3, to: Vec3) -> Option<(Self, f32)> {
        let offset = to - from;
        let distance = offset.length();
        Self::new(from, offset).map(|ray| (ray, distance))
    }

    /// The point `distance` metres along the ray.
    #[must_use]
    pub fn at(&self, distance: f32) -> Vec3 {
        self.origin + self.direction * distance
    }
}

/// Where a ray met a shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// How far along the ray the hit is, in metres. Never negative.
    pub distance: f32,
    /// Where the ray met the surface, in world space.
    pub point: Vec3,
    /// Unit normal out of the surface, facing back along the ray.
    ///
    /// The same convention [`crate::contact::Contact`] uses: out of the static shape,
    /// towards whatever asked.
    pub normal: Vec3,
}

/// The nearest point at which `ray` meets `shape` within `max_distance`.
///
/// A ray that starts *inside* a solid shape hits it at distance zero: a caster standing in
/// a wall has no sight of anything, and reporting the far side instead would say the
/// opposite.
#[must_use]
pub fn ray_vs_shape(ray: &Ray, shape: &Shape, max_distance: f32) -> Option<RayHit> {
    match shape {
        Shape::Plane(plane) => ray_vs_plane(ray, plane, max_distance),
        Shape::Aabb(bounds) => ray_vs_aabb(ray, bounds, max_distance),
        Shape::Obb(obb) => ray_vs_obb(ray, obb, max_distance),
    }
}

/// The nearest point at which `ray` meets `plane` within `max_distance`.
#[must_use]
pub fn ray_vs_plane(ray: &Ray, plane: &Plane, max_distance: f32) -> Option<RayHit> {
    let along = plane.normal.dot(ray.direction);
    let height = plane.signed_distance(ray.origin);
    if along.abs() <= PARALLEL_EPSILON {
        // Parallel. Lying exactly in the plane counts as being in it, which is the same
        // rule the box uses for a ray that starts inside one.
        if height.abs() > PARALLEL_EPSILON {
            return None;
        }
        return Some(RayHit {
            distance: 0.0,
            point: ray.origin,
            normal: plane.normal,
        });
    }
    let distance = -height / along;
    if distance < 0.0 || distance > max_distance {
        return None;
    }
    Some(RayHit {
        distance,
        point: ray.at(distance),
        // Facing back along the ray: a floor hit from underneath reports −Y.
        normal: if along > 0.0 {
            -plane.normal
        } else {
            plane.normal
        },
    })
}

/// The nearest point at which `ray` meets `bounds` within `max_distance`.
///
/// The slab method: clip the ray against each pair of parallel faces in turn and keep the
/// interval that survives all three.
#[must_use]
pub fn ray_vs_aabb(ray: &Ray, bounds: &Aabb, max_distance: f32) -> Option<RayHit> {
    if bounds.is_empty() {
        return None;
    }
    let origin = [ray.origin.x, ray.origin.y, ray.origin.z];
    let direction = [ray.direction.x, ray.direction.y, ray.direction.z];
    let min = [bounds.min.x, bounds.min.y, bounds.min.z];
    let max = [bounds.max.x, bounds.max.y, bounds.max.z];

    let mut near = 0.0_f32;
    let mut far = max_distance;
    // Which axis the entry face belongs to, and from which side it was entered.
    let mut axis = 0_usize;
    let mut sign = -1.0_f32;

    for index in 0..3 {
        if direction[index].abs() <= PARALLEL_EPSILON {
            // Parallel to this pair of faces: either always between them or never.
            if origin[index] < min[index] || origin[index] > max[index] {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / direction[index];
        let mut entry = (min[index] - origin[index]) * inverse;
        let mut exit = (max[index] - origin[index]) * inverse;
        let mut entry_sign = -1.0;
        if entry > exit {
            std::mem::swap(&mut entry, &mut exit);
            entry_sign = 1.0;
        }
        if entry > near {
            near = entry;
            axis = index;
            sign = entry_sign;
        }
        if exit < far {
            far = exit;
        }
        if near > far {
            return None;
        }
    }

    if near > max_distance {
        return None;
    }
    // `near` stays at zero for a ray that started inside, and the normal it reports is the
    // one it would have entered through — arbitrary, but never used by a sight query, which
    // only asks whether anything was there at all.
    let mut normal = Vec3::ZERO;
    match axis {
        0 => normal.x = sign,
        1 => normal.y = sign,
        _ => normal.z = sign,
    }
    Some(RayHit {
        distance: near,
        point: ray.at(near),
        normal,
    })
}

/// The nearest point at which `ray` meets `obb` within `max_distance`.
///
/// The ray is brought into the box's own space, where the box is axis-aligned and the slab
/// test above applies unchanged. Rotation preserves length, so the distance comes back
/// without a scale factor.
#[must_use]
pub fn ray_vs_obb(ray: &Ray, obb: &Obb, max_distance: f32) -> Option<RayHit> {
    let local = Ray {
        origin: obb.to_local(ray.origin),
        direction: obb.rotation.conjugate().rotate(ray.direction),
    };
    let hit = ray_vs_aabb(&local, &obb.local_bounds(), max_distance)?;
    Some(RayHit {
        distance: hit.distance,
        point: obb.to_world(hit.point),
        normal: obb.direction_to_world(hit.normal),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::math::Quat;

    fn ray(origin: Vec3, direction: Vec3) -> Ray {
        Ray::new(origin, direction).expect("a direction with length")
    }

    fn wall() -> Aabb {
        Aabb::new(Vec3::new(-1.0, 0.0, -5.0), Vec3::new(1.0, 4.0, 5.0))
    }

    #[test]
    fn a_ray_normalises_its_direction_and_refuses_to_point_nowhere() {
        let along = ray(Vec3::ZERO, Vec3::new(0.0, 0.0, -8.0));
        assert!((along.direction.length() - 1.0).abs() < 1e-6);
        assert_eq!(along.at(3.0), Vec3::new(0.0, 0.0, -3.0));
        assert_eq!(Ray::new(Vec3::ZERO, Vec3::ZERO), None);
    }

    #[test]
    fn two_points_give_a_ray_and_the_distance_between_them() {
        let (line, distance) =
            Ray::between(Vec3::new(-4.0, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)).expect("a ray");
        assert!((distance - 8.0).abs() < 1e-6);
        assert_eq!(line.direction, Vec3::X);
        assert_eq!(Ray::between(Vec3::ZERO, Vec3::ZERO), None);
    }

    #[test]
    fn a_ray_hits_the_near_face_of_a_box() {
        let hit = ray_vs_aabb(&ray(Vec3::new(-5.0, 1.0, 0.0), Vec3::X), &wall(), 100.0)
            .expect("the wall is in the way");
        assert!((hit.distance - 4.0).abs() < 1e-5);
        assert!((hit.point.x + 1.0).abs() < 1e-5);
        assert_eq!(hit.normal, Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn a_ray_that_stops_short_of_a_box_reports_nothing() {
        // The rule the target endpoint depends on: geometry beyond the far point is not
        // between the two points.
        assert_eq!(
            ray_vs_aabb(&ray(Vec3::new(-5.0, 1.0, 0.0), Vec3::X), &wall(), 3.0),
            None
        );
        assert!(ray_vs_aabb(&ray(Vec3::new(-5.0, 1.0, 0.0), Vec3::X), &wall(), 4.5).is_some());
    }

    #[test]
    fn a_ray_beside_or_behind_a_box_misses_it() {
        // Past the top.
        assert_eq!(
            ray_vs_aabb(&ray(Vec3::new(-5.0, 9.0, 0.0), Vec3::X), &wall(), 100.0),
            None
        );
        // Pointing away.
        assert_eq!(
            ray_vs_aabb(
                &ray(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),
                &wall(),
                100.0
            ),
            None
        );
        // Parallel to the slab and outside it.
        assert_eq!(
            ray_vs_aabb(&ray(Vec3::new(-5.0, 9.0, 0.0), Vec3::Z), &wall(), 100.0),
            None
        );
    }

    #[test]
    fn a_ray_starting_inside_a_box_hits_it_at_once() {
        let hit = ray_vs_aabb(&ray(Vec3::new(0.0, 1.0, 0.0), Vec3::X), &wall(), 100.0)
            .expect("inside counts as touching");
        assert_eq!(hit.distance, 0.0);
    }

    #[test]
    fn a_ray_meets_the_floor_and_reports_which_side_it_came_from() {
        let floor = Plane::new(Vec3::Y, 0.0);
        let down = ray_vs_plane(&ray(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y), &floor, 100.0)
            .expect("the floor is below");
        assert!((down.distance - 5.0).abs() < 1e-5);
        assert_eq!(down.normal, Vec3::Y);

        let up = ray_vs_plane(&ray(Vec3::new(0.0, -5.0, 0.0), Vec3::Y), &floor, 100.0)
            .expect("the floor is above");
        assert_eq!(up.normal, -Vec3::Y);

        // Parallel and clear of it.
        assert_eq!(
            ray_vs_plane(&ray(Vec3::new(0.0, 5.0, 0.0), Vec3::X), &floor, 100.0),
            None
        );
    }

    #[test]
    fn a_rotated_box_is_hit_on_its_own_faces() {
        // A slab turned a quarter turn about +Y: what was 2 m thick along X is now 2 m
        // thick along Z, and a ray along X passes through its 10 m length.
        let obb = Obb::new(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 2.0, 5.0),
            Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2),
        );
        let hit = ray_vs_obb(&ray(Vec3::new(-8.0, 1.0, 0.0), Vec3::X), &obb, 100.0)
            .expect("the slab is in the way");
        assert!((hit.distance - 3.0).abs() < 1e-4, "{}", hit.distance);
        assert!((hit.normal.x + 1.0).abs() < 1e-4);
        assert_eq!(
            ray_vs_obb(&ray(Vec3::new(0.0, 1.0, -8.0), Vec3::Z), &obb, 100.0)
                .map(|hit| (hit.distance * 100.0).round()),
            Some(700.0)
        );
    }

    #[test]
    fn the_shape_dispatch_agrees_with_each_test_it_dispatches_to() {
        let line = ray(Vec3::new(-5.0, 1.0, 0.0), Vec3::X);
        assert_eq!(
            ray_vs_shape(&line, &Shape::Aabb(wall()), 100.0),
            ray_vs_aabb(&line, &wall(), 100.0)
        );
        let floor = Plane::new(Vec3::Y, 0.0);
        let down = ray(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y);
        assert_eq!(
            ray_vs_shape(&down, &Shape::Plane(floor), 100.0),
            ray_vs_plane(&down, &floor, 100.0)
        );
    }
}
