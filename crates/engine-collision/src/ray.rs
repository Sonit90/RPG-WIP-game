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
        // The surface itself, not the box around it: a box answers "blocked" for every ray
        // that passes over a valley below the highest hill, which made line of sight across
        // any terrain a wall, and a downward ray found the hilltop's height everywhere.
        Shape::Heightfield(field) => ray_vs_heightfield(ray, field, max_distance),
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

/// The nearest point at which `ray` meets the surface of `field` within `max_distance`.
///
/// The ray is clipped to the field's box, and the cells under it are walked in order along
/// the ray (a 2D grid traversal in `x`/`z`), testing the two triangles of each. Cells are
/// visited nearest first, so the first cell with a hit holds the nearest hit. Either face
/// counts: a ray from under the surface is stopped by it as surely as one from above.
///
/// Cost is the number of cells the ray crosses, not the size of the field.
#[must_use]
pub fn ray_vs_heightfield(
    ray: &Ray,
    field: &crate::heightfield::Heightfield,
    max_distance: f32,
) -> Option<RayHit> {
    let bounds = field.bounds();
    // A flat field has a box with no height; give it a sliver so the slab test can enter it.
    let bounds = Aabb::new(
        bounds.min - Vec3::new(0.0, 1e-3, 0.0),
        bounds.max + Vec3::new(0.0, 1e-3, 0.0),
    );
    let (near, far) = slab_interval(ray, &bounds, max_distance)?;
    let cell = field.cell_size;
    let last = field.resolution.saturating_sub(2) as i64;
    let start = ray.at(near);
    let index = |value: f32, origin: f32| -> i64 {
        (((value - origin) / cell).floor() as i64).clamp(0, last)
    };
    let mut x = index(start.x, field.origin.x);
    let mut z = index(start.z, field.origin.z);

    // Distance along the ray to the next cell boundary on each axis, and between boundaries.
    let axis = |direction: f32, position: f32, origin: f32, current: i64| -> (i64, f32, f32) {
        if direction.abs() <= PARALLEL_EPSILON {
            return (0, f32::INFINITY, f32::INFINITY);
        }
        let step = if direction > 0.0 { 1 } else { -1 };
        let boundary = origin + (current + i64::from(direction > 0.0)) as f32 * cell;
        let next = near + (boundary - position) / direction;
        (step, next.max(near), cell / direction.abs())
    };
    let (step_x, mut next_x, delta_x) = axis(ray.direction.x, start.x, field.origin.x, x);
    let (step_z, mut next_z, delta_z) = axis(ray.direction.z, start.z, field.origin.z, z);

    // Bounded: a ray can cross at most every column and every row once.
    let limit = 2 * field.resolution + 2;
    for _ in 0..limit {
        let mut best: Option<RayHit> = None;
        for triangle in field.triangles(x as usize, z as usize) {
            if let Some(hit) = ray_vs_triangle(ray, &triangle, max_distance)
                && best.is_none_or(|current| hit.distance < current.distance)
            {
                best = Some(hit);
            }
        }
        if best.is_some() {
            return best;
        }
        let exit = next_x.min(next_z);
        if exit > far {
            return None;
        }
        if next_x < next_z {
            x += step_x;
            next_x += delta_x;
        } else {
            z += step_z;
            next_z += delta_z;
        }
        if x < 0 || z < 0 || x > last || z > last {
            return None;
        }
    }
    None
}

/// The part of `ray`, within `max_distance`, that is inside `bounds`: `(entry, exit)`.
fn slab_interval(ray: &Ray, bounds: &Aabb, max_distance: f32) -> Option<(f32, f32)> {
    let origin = [ray.origin.x, ray.origin.y, ray.origin.z];
    let direction = [ray.direction.x, ray.direction.y, ray.direction.z];
    let min = [bounds.min.x, bounds.min.y, bounds.min.z];
    let max = [bounds.max.x, bounds.max.y, bounds.max.z];
    let mut near = 0.0_f32;
    let mut far = max_distance;
    for index in 0..3 {
        if direction[index].abs() <= PARALLEL_EPSILON {
            if origin[index] < min[index] || origin[index] > max[index] {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / direction[index];
        let mut entry = (min[index] - origin[index]) * inverse;
        let mut exit = (max[index] - origin[index]) * inverse;
        if entry > exit {
            std::mem::swap(&mut entry, &mut exit);
        }
        near = near.max(entry);
        far = far.min(exit);
        if near > far {
            return None;
        }
    }
    Some((near, far))
}

/// Where `ray` meets `triangle` within `max_distance`, from either side (Möller–Trumbore).
fn ray_vs_triangle(ray: &Ray, triangle: &[Vec3; 3], max_distance: f32) -> Option<RayHit> {
    let [a, b, c] = *triangle;
    let edge_one = b - a;
    let edge_two = c - a;
    let p = ray.direction.cross(edge_two);
    let determinant = edge_one.dot(p);
    if determinant.abs() <= 1e-9 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let offset = ray.origin - a;
    let u = offset.dot(p) * inverse;
    if !(-1e-5..=1.0 + 1e-5).contains(&u) {
        return None;
    }
    let q = offset.cross(edge_one);
    let v = ray.direction.dot(q) * inverse;
    if v < -1e-5 || u + v > 1.0 + 1e-5 {
        return None;
    }
    let distance = edge_two.dot(q) * inverse;
    if distance < 0.0 || distance > max_distance {
        return None;
    }
    let mut normal = edge_one.cross(edge_two).normalize();
    if normal.dot(ray.direction) > 0.0 {
        normal *= -1.0;
    }
    Some(RayHit {
        distance,
        point: ray.at(distance),
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

    /// A 5x5-point field on a 2 m grid: flat at zero, with a 4 m hill at its centre point.
    fn hill() -> crate::heightfield::Heightfield {
        let mut heights = vec![0.0; 25];
        heights[2 * 5 + 2] = 4.0;
        crate::heightfield::Heightfield::new(Vec3::ZERO, 2.0, 5, heights).expect("a field")
    }

    #[test]
    fn a_ray_over_a_valley_is_not_stopped_by_the_hill_beside_it() {
        let field = hill();
        // One metre up, across the flat first row: the hill's box is 4 m tall, the ground
        // under the ray is flat, and nothing is in the way.
        let across = ray(Vec3::new(0.5, 1.0, 0.5), Vec3::X);
        assert!(ray_vs_heightfield(&across, &field, 7.0).is_none());
        // Straight through the hill, it is stopped.
        let through = ray(Vec3::new(0.5, 1.0, 4.0), Vec3::X);
        let hit = ray_vs_heightfield(&through, &field, 7.0).expect("the hill is in the way");
        assert!(hit.distance > 1.0 && hit.distance < 3.5, "{hit:?}");
    }

    #[test]
    fn a_ray_down_finds_the_surface_under_it() {
        let field = hill();
        let down = ray(Vec3::new(4.0, 10.0, 4.0), Vec3::new(0.0, -1.0, 0.0));
        let hit = ray_vs_heightfield(&down, &field, 20.0).expect("the ground");
        assert!((hit.point.y - 4.0).abs() < 1e-3, "{hit:?}");
        assert!(hit.normal.y > 0.0, "the normal faces back up the ray");
        let flat = ray(Vec3::new(0.7, 10.0, 7.3), Vec3::new(0.0, -1.0, 0.0));
        let hit = ray_vs_heightfield(&flat, &field, 20.0).expect("the ground");
        assert!(hit.point.y.abs() < 1e-3, "{hit:?}");
        // Off the edge of the field there is nothing.
        let outside = ray(Vec3::new(-3.0, 10.0, 4.0), Vec3::new(0.0, -1.0, 0.0));
        assert!(ray_vs_heightfield(&outside, &field, 20.0).is_none());
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
