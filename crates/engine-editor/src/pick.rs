//! Deciding which object the author clicked on.
//!
//! ```text
//!  cursor ──► cursor_ray ──► for each object: ray vs its collision box ──► nearest hit
//! ```
//!
//! # Why the collision box and not the mesh
//!
//! The editor has no meshes. It edits an [`EnvironmentScene`], which says *what* is placed and
//! not what it looks like — and that is deliberate, because it is what lets the whole editor be
//! tested without a device. So the thing a ray is tested against is the box the object already
//! carries for the character to run into.
//!
//! An object that blocks nothing has no box, and gets a small one around its origin so it can
//! still be picked: a fern is decoration to the character controller and a placement like any
//! other to the person arranging the level.

use engine_core::math::Vec3;
use engine_environment::{EnvironmentObject, EnvironmentScene};

/// Half-extents given to an object with no collision box, in metres.
///
/// Roughly a fern: large enough to hit at a normal working distance, small enough not to
/// swallow the clicks meant for what is behind it.
const DECORATION_HALF_EXTENTS: Vec3 = Vec3::new(0.6, 0.9, 0.6);

/// The pickable box of an object, in world space: centre, half-extents and rotation.
fn pick_box(object: &EnvironmentObject) -> (Vec3, Vec3, engine_core::math::Quat) {
    match object.world_collision() {
        Some(world) => (world.center, world.half_extents, world.rotation),
        None => {
            let scale = object.transform.scale;
            (
                object.position()
                    + Vec3::new(0.0, DECORATION_HALF_EXTENTS.y * scale.y, 0.0),
                Vec3::new(
                    DECORATION_HALF_EXTENTS.x * scale.x,
                    DECORATION_HALF_EXTENTS.y * scale.y,
                    DECORATION_HALF_EXTENTS.z * scale.z,
                ),
                object.transform.rotation,
            )
        }
    }
}

/// Where a ray enters a box, as a multiple of `direction`, if it does.
///
/// The ray is taken into the box's own space by the box's inverse rotation, which turns an
/// oriented box back into the slab test every ray tracer uses. `direction` need not be
/// normalized — the returned parameter is in whatever units it carries, which is what
/// [`cursor_ray`](engine_presentation::ui::targeting::cursor_ray) hands out.
#[must_use]
pub fn ray_box_distance(
    origin: Vec3,
    direction: Vec3,
    center: Vec3,
    half_extents: Vec3,
    rotation: engine_core::math::Quat,
) -> Option<f32> {
    let inverse = rotation.conjugate();
    let local_origin = inverse.rotate(origin - center);
    let local_direction = inverse.rotate(direction);

    let mut enter = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;
    for axis in 0..3 {
        let (o, d, half) = match axis {
            0 => (local_origin.x, local_direction.x, half_extents.x),
            1 => (local_origin.y, local_direction.y, half_extents.y),
            _ => (local_origin.z, local_direction.z, half_extents.z),
        };
        if d.abs() < 1e-9 {
            // Parallel to this pair of slabs: a miss unless the ray already lies between
            // them, and no constraint on the parameter if it does.
            if o.abs() > half {
                return None;
            }
            continue;
        }
        let first = (-half - o) / d;
        let second = (half - o) / d;
        let (near, far) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        enter = enter.max(near);
        exit = exit.min(far);
        if enter > exit {
            return None;
        }
    }

    if exit < 0.0 {
        // Entirely behind the eye.
        return None;
    }
    // A ray that starts inside the box enters it at zero, which is what makes a click from
    // inside a room still select the wall the author is standing in.
    Some(enter.max(0.0))
}

/// Which object a ray hits first, if any.
///
/// Nearest wins, and ties go to the earlier placement — which is stable, because a scene's
/// object order is stable.
#[must_use]
pub fn object_under_ray(
    scene: &EnvironmentScene,
    origin: Vec3,
    direction: Vec3,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, object) in scene.objects.iter().enumerate() {
        let (center, half_extents, rotation) = pick_box(object);
        let Some(distance) = ray_box_distance(origin, direction, center, half_extents, rotation)
        else {
            continue;
        };
        if best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_environment::asset::{FERN, PILLAR};
    use engine_environment::object::{EnvironmentCollision, default_collision};

    fn scene() -> EnvironmentScene {
        let mut scene = EnvironmentScene::with_capacity("pick", 3);
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::new(0.0, 0.0, -10.0))
                .with_collision(default_collision(PILLAR)),
        );
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::new(0.0, 0.0, -30.0))
                .with_collision(default_collision(PILLAR)),
        );
        scene.push(EnvironmentObject::decoration(FERN, Vec3::new(6.0, 0.0, -10.0)));
        scene
    }

    #[test]
    fn the_nearest_object_along_the_ray_wins() {
        // Two pillars in a line, one behind the other. Clicking picks the one in front, or
        // an author can never select anything in a corridor.
        let hit = object_under_ray(
            &scene(),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
        );
        assert_eq!(hit, Some(0));
    }

    #[test]
    fn a_decoration_with_no_collision_box_is_still_pickable() {
        let hit = object_under_ray(
            &scene(),
            Vec3::new(6.0, 0.9, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
        );
        assert_eq!(hit, Some(2), "a fern the character walks through is still an object");
    }

    #[test]
    fn a_ray_that_misses_everything_selects_nothing() {
        let hit = object_under_ray(
            &scene(),
            Vec3::new(100.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
        );
        assert_eq!(hit, None);
    }

    #[test]
    fn nothing_behind_the_eye_is_ever_picked() {
        let hit = object_under_ray(
            &scene(),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        );
        assert_eq!(hit, None, "the pillars are the other way");
    }

    #[test]
    fn a_yawed_box_is_tested_in_its_own_space() {
        // A wall turned a quarter turn is hit along the axis it now spans, and missed along
        // the one it used to. Testing an oriented box as if it were axis-aligned passes the
        // first and fails the second.
        let long = EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
            .with_collision(EnvironmentCollision::Box {
                center: Vec3::new(0.0, 1.0, 0.0),
                half_extents: Vec3::new(6.0, 1.0, 0.4),
            })
            .with_yaw(std::f32::consts::FRAC_PI_2);
        let (center, half_extents, rotation) = pick_box(&long);
        assert!(
            rotation.rotate(Vec3::new(1.0, 0.0, 0.0)).z.abs() > 0.9,
            "the box's own rotation travels with it"
        );

        let along_z = ray_box_distance(
            Vec3::new(0.0, 1.0, 20.0),
            Vec3::new(0.0, 0.0, -1.0),
            center,
            half_extents,
            rotation,
        );
        assert!(along_z.is_some(), "the turned wall spans Z now");

        // Twelve metres along +Z is past the end of a wall that reaches six, so a ray at
        // that offset must miss. An axis-aligned test would put the wall's twelve-metre
        // span on X and hit here, which is the bug this asserts against.
        let past_the_end = ray_box_distance(
            Vec3::new(20.0, 1.0, 12.0),
            Vec3::new(-1.0, 0.0, 0.0),
            center,
            half_extents,
            rotation,
        );
        assert!(past_the_end.is_none(), "the wall does not reach twelve metres");
    }
}
